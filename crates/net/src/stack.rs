use crate::{Error, Result, Stream};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use hal::NetDevice;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::socket::{dhcpv4, dns, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{DnsQueryType, EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

const MTU: usize = 1514;
const TCP_RX: usize = 64 * 1024;
const TCP_TX: usize = 16 * 1024;

struct Adapter {
    dev: Box<dyn NetDevice>,
}

struct RxTok(Vec<u8>);
struct TxTok<'a>(&'a mut dyn NetDevice);

impl phy::RxToken for RxTok {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl phy::TxToken for TxTok<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        let _ = self.0.transmit(&buf);
        r
    }
}

impl Device for Adapter {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(RxTok, TxTok<'_>)> {
        let mut buf = vec![0u8; MTU + 4];
        let n = self.dev.receive(&mut buf)?;
        buf.truncate(n);
        Some((RxTok(buf), TxTok(self.dev.as_mut())))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<TxTok<'_>> {
        Some(TxTok(self.dev.as_mut()))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = MTU;
        c
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Lease {
    pub address: [u8; 4],
    pub prefix: u8,
    pub router: Option<[u8; 4]>,
    pub dns: Option<[u8; 4]>,
}

pub struct Stack {
    dev: Adapter,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    dns: SocketHandle,
    now_ms: fn() -> u64,
    next_port: u16,
}

impl Stack {
    /// `now_ms` must be monotonic; `seed` should differ per boot.
    pub fn new(dev: Box<dyn NetDevice>, now_ms: fn() -> u64, seed: u64) -> Stack {
        let mac = dev.mac();
        let mut dev = Adapter { dev };
        let mut cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        cfg.random_seed = seed;
        let iface = Interface::new(cfg, &mut dev, Instant::from_millis(now_ms() as i64));
        let mut sockets = SocketSet::new(Vec::new());
        let dhcp = sockets.add(dhcpv4::Socket::new());
        let dns = sockets.add(dns::Socket::new(&[], [None, None]));
        Stack { dev, iface, sockets, dhcp, dns, now_ms, next_port: 49152 + (seed % 10000) as u16 }
    }

    fn now(&self) -> Instant {
        Instant::from_millis((self.now_ms)() as i64)
    }

    pub fn poll(&mut self) {
        let now = self.now();
        self.iface.poll(now, &mut self.dev, &mut self.sockets);
    }

    pub fn link_up(&mut self) -> bool {
        self.dev.dev.link_up()
    }

    fn deadline(&self, timeout_ms: u64) -> u64 {
        (self.now_ms)() + timeout_ms
    }

    /// Runs DHCP until a lease is acquired and applies it to the interface.
    pub fn dhcp(&mut self, timeout_ms: u64) -> Result<Lease> {
        let end = self.deadline(timeout_ms);
        while (self.now_ms)() < end {
            self.poll();
            let ev = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll();
            if let Some(dhcpv4::Event::Configured(c)) = ev {
                let lease = Lease {
                    address: c.address.address().octets(),
                    prefix: c.address.prefix_len(),
                    router: c.router.map(|r| r.octets()),
                    dns: c.dns_servers.first().map(|d| d.octets()),
                };
                let cidr = c.address;
                let router = c.router;
                let dns: Vec<IpAddress> = c.dns_servers.iter().map(|d| IpAddress::Ipv4(*d)).collect();
                self.iface.update_ip_addrs(|a| {
                    a.clear();
                    let _ = a.push(IpCidr::Ipv4(cidr));
                });
                if let Some(r) = router {
                    let _ = self.iface.routes_mut().add_default_ipv4_route(r);
                }
                self.sockets.get_mut::<dns::Socket>(self.dns).update_servers(&dns);
                return Ok(lease);
            }
            core::hint::spin_loop();
        }
        Err(Error::Timeout)
    }

    /// Resolves an IPv4 address (dotted quads are parsed directly).
    pub fn resolve(&mut self, name: &str, timeout_ms: u64) -> Result<[u8; 4]> {
        if let Some(ip) = parse_ipv4(name) {
            return Ok(ip);
        }
        let h = self
            .sockets
            .get_mut::<dns::Socket>(self.dns)
            .start_query(self.iface.context(), name, DnsQueryType::A)
            .map_err(|_| Error::Dns)?;
        let end = self.deadline(timeout_ms);
        while (self.now_ms)() < end {
            self.poll();
            match self.sockets.get_mut::<dns::Socket>(self.dns).get_query_result(h) {
                Ok(addrs) => {
                    return match addrs.first() {
                        Some(IpAddress::Ipv4(v4)) => Ok(v4.octets()),
                        None => Err(Error::Dns),
                    };
                }
                Err(dns::GetQueryResultError::Pending) => hal::idle(),
                Err(_) => return Err(Error::Dns),
            }
        }
        self.sockets.get_mut::<dns::Socket>(self.dns).cancel_query(h);
        Err(Error::Timeout)
    }

    pub fn connect(&mut self, ip: [u8; 4], port: u16, timeout_ms: u64) -> Result<TcpConn<'_>> {
        let sock = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_RX]), tcp::SocketBuffer::new(vec![0; TCP_TX]));
        let handle = self.sockets.add(sock);
        let local = self.next_port;
        self.next_port = if self.next_port >= 60000 { 49152 } else { self.next_port + 1 };
        let remote = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(ip)), port);
        let s = self.sockets.get_mut::<tcp::Socket>(handle);
        s.set_nagle_enabled(false);
        if s.connect(self.iface.context(), remote, local).is_err() {
            self.sockets.remove(handle);
            return Err(Error::Connect);
        }
        let mut conn = TcpConn { stack: self, handle, timeout_ms: 30_000 };
        let end = conn.stack.deadline(timeout_ms);
        loop {
            conn.stack.poll();
            match conn.sock().state() {
                tcp::State::Established => return Ok(conn),
                tcp::State::Closed => return Err(Error::Connect),
                _ => {}
            }
            if (conn.stack.now_ms)() >= end {
                return Err(Error::Timeout);
            }
            hal::idle();
        }
    }
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for o in out.iter_mut() {
        *o = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

pub struct TcpConn<'a> {
    stack: &'a mut Stack,
    handle: SocketHandle,
    /// Idle timeout for blocking reads and writes.
    pub timeout_ms: u64,
}

impl TcpConn<'_> {
    fn sock(&mut self) -> &mut tcp::Socket<'static> {
        self.stack.sockets.get_mut::<tcp::Socket>(self.handle)
    }

    pub fn close(&mut self) {
        self.sock().close();
        let end = self.stack.deadline(2000);
        while (self.stack.now_ms)() < end && self.sock().is_active() {
            self.stack.poll();
            hal::idle();
        }
    }
}

impl Stream for TcpConn<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let end = self.stack.deadline(self.timeout_ms);
        loop {
            self.stack.poll();
            let s = self.sock();
            if s.can_recv() {
                return s.recv_slice(buf).map_err(|_| Error::Closed);
            }
            if !s.may_recv() {
                return Ok(0);
            }
            if (self.stack.now_ms)() >= end {
                return Err(Error::Timeout);
            }
            hal::idle();
        }
    }

    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        let mut end = self.stack.deadline(self.timeout_ms);
        while !buf.is_empty() {
            self.stack.poll();
            let s = self.sock();
            if !s.may_send() {
                return Err(Error::Closed);
            }
            if s.can_send() {
                let n = s.send_slice(buf).map_err(|_| Error::Closed)?;
                buf = &buf[n..];
                end = self.stack.deadline(self.timeout_ms);
            } else if (self.stack.now_ms)() >= end {
                return Err(Error::Timeout);
            } else {
                hal::idle();
            }
        }
        // Push out what is queued.
        self.stack.poll();
        Ok(())
    }
}

impl Drop for TcpConn<'_> {
    fn drop(&mut self) {
        self.sock().abort();
        self.stack.poll();
        self.stack.sockets.remove(self.handle);
    }
}
