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
/// How long a leased address is ARP-probed for conflicts, and how many conflicting offers are declined.
const PROBE_MS: u64 = 1500;
const MAX_DECLINES: u32 = 4;
/// Resolvers tried after the ones DHCP provides (Cloudflare, Google).
const FALLBACK_DNS: [[u8; 4]; 2] = [[1, 1, 1, 1], [8, 8, 8, 8]];
/// TCP receive window. Large enough for a long, fast path (window scaling is on), small enough that a
/// full window fits the NIC receive rings (the largest is 512 KiB), so a burst is not dropped while
/// the CPU is busy decrypting.
const TCP_RX: usize = 256 * 1024;
const TCP_TX: usize = 16 * 1024;

/// Frame counters by kind, to tell "nothing received" from "received but not answered".
#[derive(Default, Clone, Copy)]
struct Counts {
    arp: u32,
    udp: u32,
    tcp: u32,
    other: u32,
    /// Frames addressed to us alone (not broadcast or multicast).
    unicast: u32,
    /// Ethertypes of the frames counted as "other" (type, count), first few kinds only.
    types: [(u16, u32); 6],
    arp_logged: u32,
}

impl Counts {
    fn note(&mut self, dir: &str, f: &[u8]) {
        if f.len() < 14 {
            return;
        }
        if f[0] & 1 == 0 {
            self.unicast += 1;
        }
        let ethertype = u16::from_be_bytes([f[12], f[13]]);
        match (ethertype, f.get(23)) {
            (0x0806, _) => {
                self.arp += 1;
                if self.arp_logged < 12 && f.len() >= 42 {
                    self.arp_logged += 1;
                    hal::log!(
                        "net: {dir} ARP {} from {:02x?} {}.{}.{}.{} to {}.{}.{}.{} (frame {:02x?} -> {:02x?})",
                        if f[21] == 1 { "request" } else { "reply" },
                        &f[22..28],
                        f[28], f[29], f[30], f[31],
                        f[38], f[39], f[40], f[41],
                        &f[6..12],
                        &f[0..6]
                    );
                }
            }
            (0x0800, Some(17)) => self.udp += 1,
            (0x0800, Some(6)) => self.tcp += 1,
            _ => {
                self.other += 1;
                if let Some(slot) = self.types.iter_mut().find(|(t, _)| *t == ethertype || *t == 0) {
                    slot.0 = ethertype;
                    slot.1 += 1;
                }
            }
        }
    }
}

struct Adapter {
    dev: Box<dyn NetDevice>,
    rx: Counts,
    tx: Counts,
    /// While probing a leased address: the address being watched, and the MAC of anyone else
    /// seen using it (an ARP reply or probe naming the same IP).
    watch_ip: Option<[u8; 4]>,
    conflict: Option<[u8; 6]>,
}

struct RxTok(Vec<u8>);
struct TxTok<'a>(&'a mut dyn NetDevice, &'a mut Counts);

impl phy::RxToken for RxTok {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl phy::TxToken for TxTok<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        self.1.note("sent", &buf);
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
        self.rx.note("received", &buf);
        if let Some(ip) = self.watch_ip {
            // ARP: [14..22] fixed fields, sender MAC [22..28], sender IP [28..32], target IP [38..42].
            if buf.len() >= 42 && buf[12..14] == [0x08, 0x06] {
                let (sender_ip, target_ip) = (&buf[28..32], &buf[38..42]);
                let probe = sender_ip == [0u8; 4] && target_ip == ip;
                if (sender_ip == ip || probe) && buf[22..28] != self.dev.mac() {
                    self.conflict = Some(buf[22..28].try_into().unwrap());
                }
            }
        }
        Some((RxTok(buf), TxTok(self.dev.as_mut(), &mut self.tx)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<TxTok<'_>> {
        Some(TxTok(self.dev.as_mut(), &mut self.tx))
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
    gateway: Option<[u8; 4]>,
    dns_servers: Vec<[u8; 4]>,
}

impl Stack {
    /// `now_ms` must be monotonic; `seed` should differ per boot.
    pub fn new(dev: Box<dyn NetDevice>, now_ms: fn() -> u64, seed: u64) -> Stack {
        let mac = dev.mac();
        let mut dev = Adapter { dev, rx: Counts::default(), tx: Counts::default(), watch_ip: None, conflict: None };
        let mut cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        cfg.random_seed = seed;
        let iface = Interface::new(cfg, &mut dev, Instant::from_millis(now_ms() as i64));
        let mut sockets = SocketSet::new(Vec::new());
        let dhcp = sockets.add(dhcpv4::Socket::new());
        let dns = sockets.add(dns::Socket::new(&[], [None, None]));
        Stack { dev, iface, sockets, dhcp, dns, now_ms, next_port: 49152 + (seed % 10000) as u16, gateway: None, dns_servers: Vec::new() }
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

    /// The stack's millisecond clock, for diagnostics (a plain function, so it can be kept while a
    /// connection borrows the stack).
    pub fn clock(&self) -> fn() -> u64 {
        self.now_ms
    }

    fn deadline(&self, timeout_ms: u64) -> u64 {
        (self.now_ms)() + timeout_ms
    }

    /// Runs DHCP until a lease is acquired and applies it to the interface.
    pub fn dhcp(&mut self, timeout_ms: u64) -> Result<Lease> {
        let mut end = self.deadline(timeout_ms);
        let mut declined = 0;
        while (self.now_ms)() < end {
            self.poll();
            let ev = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll();
            if let Some(dhcpv4::Event::Configured(c)) = ev {
                // Copy what is needed so the socket borrow ends here.
                let (cidr, router, server) = (c.address, c.router, c.server);
                let dhcp_dns: Vec<Ipv4Address> = c.dns_servers.iter().copied().collect();
                let ip = cidr.address().octets();

                // Before using the address, check that nobody else does (RFC 5227), as Linux's DHCP
                // clients do. A static host inside the DHCP pool made every reply to us go to it
                // (seen on a school network); the right response is to decline and ask for another.
                if declined >= MAX_DECLINES {
                    hal::info!("net: the DHCP server keeps offering addresses that are in use; using {}.{}.{}.{} anyway", ip[0], ip[1], ip[2], ip[3]);
                } else {
                    if let Some(mac) = self.probe_address(ip) {
                        hal::info!(
                            "net: {}.{}.{}.{} is already in use by {:02x?}: declining it and asking the DHCP server again",
                            ip[0], ip[1], ip[2], ip[3], mac
                        );
                        self.dhcp_decline(ip, server.identifier.octets());
                        declined += 1;
                        self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).reset();
                        // The server needs a moment to mark the address bad before it offers again.
                        let wait = (self.now_ms)() + 3000;
                        while (self.now_ms)() < wait {
                            self.poll();
                            hal::idle();
                        }
                        end = self.deadline(timeout_ms);
                        continue;
                    }
                }
                let lease = Lease {
                    address: ip,
                    prefix: cidr.prefix_len(),
                    router: router.map(|r| r.octets()),
                    dns: dhcp_dns.first().map(|d| d.octets()),
                };
                let mut dns: Vec<IpAddress> = dhcp_dns.iter().map(|d| IpAddress::Ipv4(*d)).collect();
                // Some networks hand out a resolver that never answers us (found on a campus network);
                // fall back to public ones after the DHCP-provided servers.
                for fallback in FALLBACK_DNS {
                    let a = IpAddress::Ipv4(Ipv4Address::from(fallback));
                    if !dns.contains(&a) {
                        dns.push(a);
                    }
                }
                hal::log!("net: DNS servers {:?}", dns.iter().map(|d| alloc::format!("{d}")).collect::<Vec<_>>());
                hal::log!("net: DHCP server {} (identifier {}), declined {declined} address(es)", server.address, server.identifier);
                self.gateway = router.map(|r| r.octets());
                self.dns_servers = dhcp_dns.iter().map(|d| d.octets()).collect();
                self.iface.update_ip_addrs(|a| {
                    a.clear();
                    let _ = a.push(IpCidr::Ipv4(cidr));
                });
                if let Some(r) = router {
                    let _ = self.iface.routes_mut().add_default_ipv4_route(r);
                }
                self.sockets.get_mut::<dns::Socket>(self.dns).update_servers(&dns);
                // Announce the address with a gratuitous ARP, as Linux's DHCP clients do, so switches
                // and gateways learn this MAC/IP pairing before they see any other traffic from it.
                self.announce(lease.address);
                return Ok(lease);
            }
            core::hint::spin_loop();
        }
        Err(Error::Timeout)
    }

    /// ARP-probes `ip` (RFC 5227: three probes with sender address 0.0.0.0, then a short wait) and
    /// returns the MAC of any other host that answers for it or probes for it too.
    fn probe_address(&mut self, ip: [u8; 4]) -> Option<[u8; 6]> {
        let mac = self.dev.dev.mac();
        let mut f = [0u8; 42];
        f[0..6].fill(0xff);
        f[6..12].copy_from_slice(&mac);
        f[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
        f[22..28].copy_from_slice(&mac);
        // sender IP stays 0.0.0.0, target hardware address zero
        f[38..42].copy_from_slice(&ip);
        self.dev.watch_ip = Some(ip);
        self.dev.conflict = None;
        let start = (self.now_ms)();
        let mut sent = 0u64;
        while (self.now_ms)() - start < PROBE_MS && self.dev.conflict.is_none() {
            if sent < 3 && (self.now_ms)() - start >= sent * 300 {
                self.dev.tx.note("sent", &f);
                let _ = self.dev.dev.transmit(&f);
                sent += 1;
            }
            self.poll();
            hal::idle();
        }
        self.dev.watch_ip = None;
        self.dev.conflict.take()
    }

    /// DHCPDECLINE for `ip` to the server `server`: tells it the address is in use.
    fn dhcp_decline(&mut self, ip: [u8; 4], server: [u8; 4]) {
        let mac = self.dev.dev.mac();
        let mut bootp = vec![0u8; 236];
        bootp[0] = 1; // BOOTREQUEST
        bootp[1] = 1; // Ethernet
        bootp[2] = 6;
        bootp[4..8].copy_from_slice(&((self.now_ms)() as u32 ^ 0x5a5a_a5a5).to_be_bytes()); // xid
        bootp[28..34].copy_from_slice(&mac); // chaddr
        bootp.extend_from_slice(&[99, 130, 83, 99]); // magic cookie
        bootp.extend_from_slice(&[53, 1, 4]); // DHCPDECLINE
        bootp.extend_from_slice(&[50, 4]);
        bootp.extend_from_slice(&ip); // requested address
        bootp.extend_from_slice(&[54, 4]);
        bootp.extend_from_slice(&server); // server identifier
        bootp.extend_from_slice(&[61, 7, 1]);
        bootp.extend_from_slice(&mac); // client identifier
        bootp.push(255);
        let udp_len = 8 + bootp.len();
        let ip_len = 20 + udp_len;
        let mut f = Vec::with_capacity(14 + ip_len);
        f.extend_from_slice(&[0xff; 6]);
        f.extend_from_slice(&mac);
        f.extend_from_slice(&0x0800u16.to_be_bytes());
        let ip_start = f.len();
        f.extend_from_slice(&[0x45, 0, (ip_len >> 8) as u8, ip_len as u8, 0, 0, 0, 0, 64, 17, 0, 0]);
        f.extend_from_slice(&[0, 0, 0, 0]); // source 0.0.0.0
        f.extend_from_slice(&[255, 255, 255, 255]);
        let mut sum = 0u32;
        for w in f[ip_start..ip_start + 20].chunks(2) {
            sum += u16::from_be_bytes([w[0], w[1]]) as u32;
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        let ck = !(sum as u16);
        f[ip_start + 10..ip_start + 12].copy_from_slice(&ck.to_be_bytes());
        f.extend_from_slice(&68u16.to_be_bytes());
        f.extend_from_slice(&67u16.to_be_bytes());
        f.extend_from_slice(&(udp_len as u16).to_be_bytes());
        f.extend_from_slice(&[0, 0]); // UDP checksum omitted (allowed for IPv4)
        f.extend_from_slice(&bootp);
        self.dev.tx.note("sent", &f);
        let _ = self.dev.dev.transmit(&f);
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
        hal::info!("net: DNS query for {name} unanswered; {}", self.counters());
        Err(Error::Timeout)
    }

    /// Gratuitous ARP for `ip` (an ARP request for our own address).
    fn announce(&mut self, ip: [u8; 4]) {
        let mac = self.dev.dev.mac();
        let mut f = [0u8; 42];
        f[0..6].fill(0xff);
        f[6..12].copy_from_slice(&mac);
        f[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
        f[22..28].copy_from_slice(&mac);
        f[28..32].copy_from_slice(&ip);
        f[38..42].copy_from_slice(&ip);
        self.dev.tx.note("sent", &f);
        let _ = self.dev.dev.transmit(&f);
    }

    /// The default gateway from the DHCP lease.
    pub fn gateway(&self) -> Option<[u8; 4]> {
        self.gateway
    }

    /// The DHCP-provided DNS servers (without the built-in fallbacks).
    pub fn dhcp_dns(&self) -> &[[u8; 4]] {
        &self.dns_servers
    }

    /// ICMP echo to `ip`; returns the round-trip time in ms if a reply came back in time.
    pub fn ping(&mut self, ip: [u8; 4], timeout_ms: u64) -> Option<u64> {
        use smoltcp::phy::ChecksumCapabilities;
        use smoltcp::socket::icmp;
        use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr};
        let rx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 2], vec![0; 256]);
        let tx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 2], vec![0; 256]);
        let h = self.sockets.add(icmp::Socket::new(rx, tx));
        let ident = 0x4152;
        let dst = IpAddress::Ipv4(Ipv4Address::from(ip));
        let caps = ChecksumCapabilities::default();
        let result = (|| {
            let sock = self.sockets.get_mut::<icmp::Socket>(h);
            sock.bind(icmp::Endpoint::Ident(ident)).ok()?;
            let repr = Icmpv4Repr::EchoRequest { ident, seq_no: 1, data: b"archstaller" };
            let buf = sock.send(repr.buffer_len(), dst).ok()?;
            repr.emit(&mut Icmpv4Packet::new_unchecked(buf), &caps);
            let start = (self.now_ms)();
            while (self.now_ms)() - start < timeout_ms {
                self.poll();
                let sock = self.sockets.get_mut::<icmp::Socket>(h);
                if sock.can_recv() {
                    let (payload, _) = sock.recv().ok()?;
                    let pkt = Icmpv4Packet::new_checked(payload).ok()?;
                    if matches!(Icmpv4Repr::parse(&pkt, &caps), Ok(Icmpv4Repr::EchoReply { ident: i, .. }) if i == ident) {
                        return Some((self.now_ms)() - start);
                    }
                }
                hal::idle();
            }
            None
        })();
        self.sockets.remove(h);
        result
    }

    /// Frames sent and received so far, by kind (diagnostics).
    pub fn counters(&self) -> alloc::string::String {
        let (r, t) = (&self.dev.rx, &self.dev.tx);
        let types: alloc::vec::Vec<alloc::string::String> =
            r.types.iter().filter(|(t, _)| *t != 0).map(|(t, n)| alloc::format!("{t:04x}x{n}")).collect();
        alloc::format!(
            "rx: {} arp, {} udp, {} tcp, {} other ({} unicast; other ethertypes {:?}); tx: {} arp, {} udp, {} tcp, {} other",
            r.arp, r.udp, r.tcp, r.other, r.unicast, types, t.arp, t.udp, t.tcp, t.other
        )
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

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};
    use smoltcp::wire::{DhcpMessageType, DhcpPacket, DhcpRepr, EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, UdpPacket};
    use std::cell::RefCell;
    use std::rc::Rc;

    static CLOCK: AtomicU64 = AtomicU64::new(0);
    /// A clock that moves 5 ms per reading, so loops that wait for time terminate.
    fn clock() -> u64 {
        CLOCK.fetch_add(5, Ordering::Relaxed)
    }

    #[derive(Default)]
    struct Wire {
        sent: std::vec::Vec<std::vec::Vec<u8>>,
        /// Frames to hand back from `receive`, one per call.
        inbox: std::collections::VecDeque<std::vec::Vec<u8>>,
    }

    struct Mock(Rc<RefCell<Wire>>);

    impl NetDevice for Mock {
        fn name(&self) -> &str {
            "mock"
        }
        fn mac(&self) -> [u8; 6] {
            [2, 0, 0, 0, 0, 1]
        }
        fn link_up(&mut self) -> bool {
            true
        }
        fn transmit(&mut self, frame: &[u8]) -> hal::Result<()> {
            self.0.borrow_mut().sent.push(frame.to_vec());
            Ok(())
        }
        fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
            let f = self.0.borrow_mut().inbox.pop_front()?;
            buf[..f.len()].copy_from_slice(&f);
            Some(f.len())
        }
    }

    fn arp_frame(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> std::vec::Vec<u8> {
        let mut f = std::vec![0u8; 42];
        f[0..6].fill(0xff);
        f[6..12].copy_from_slice(&sender_mac);
        f[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
        f[22..28].copy_from_slice(&sender_mac);
        f[28..32].copy_from_slice(&sender_ip);
        f[38..42].copy_from_slice(&target_ip);
        f
    }

    fn stack() -> (Stack, Rc<RefCell<Wire>>) {
        let wire = Rc::new(RefCell::new(Wire::default()));
        (Stack::new(Box::new(Mock(wire.clone())), clock, 7), wire)
    }

    #[test]
    fn decline_frame_is_a_valid_dhcp_decline() {
        let (mut s, wire) = stack();
        s.dhcp_decline([10, 21, 14, 2], [10, 21, 14, 1]);
        let w = wire.borrow();
        let f = w.sent.last().expect("decline sent");
        let eth = EthernetFrame::new_checked(&f[..]).unwrap();
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv4);
        let ip = Ipv4Packet::new_checked(eth.payload()).unwrap();
        assert!(ip.verify_checksum(), "IP header checksum");
        assert_eq!(ip.next_header(), IpProtocol::Udp);
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!((udp.src_port(), udp.dst_port()), (68, 67));
        let dhcp = DhcpPacket::new_checked(udp.payload()).unwrap();
        let repr = DhcpRepr::parse(&dhcp).unwrap();
        assert_eq!(repr.message_type, DhcpMessageType::Decline);
        assert_eq!(repr.requested_ip, Some(Ipv4Address::new(10, 21, 14, 2)));
        assert_eq!(repr.server_identifier, Some(Ipv4Address::new(10, 21, 14, 1)));
        assert_eq!(repr.client_hardware_address, EthernetAddress([2, 0, 0, 0, 0, 1]));
    }

    #[test]
    fn probe_sees_another_host_using_the_address() {
        let (mut s, wire) = stack();
        // Someone else answers the probe for 10.21.14.2.
        wire.borrow_mut().inbox.push_back(arp_frame([0x5c, 0xf9, 0xdd, 0x55, 0x07, 0x0c], [10, 21, 14, 2], [10, 21, 14, 2]));
        assert_eq!(s.probe_address([10, 21, 14, 2]), Some([0x5c, 0xf9, 0xdd, 0x55, 0x07, 0x0c]));
        // The probe itself: ARP request, sender IP 0.0.0.0, target the probed address.
        let w = wire.borrow();
        let p = &w.sent[0];
        assert_eq!(&p[12..14], &[0x08, 0x06]);
        assert_eq!(&p[28..32], &[0, 0, 0, 0]);
        assert_eq!(&p[38..42], &[10, 21, 14, 2]);
    }

    #[test]
    fn probe_ignores_unrelated_and_own_arp() {
        let (mut s, wire) = stack();
        // An ARP about another address, and our own MAC echoed back.
        wire.borrow_mut().inbox.push_back(arp_frame([0x5c, 0, 0, 0, 0, 9], [10, 21, 14, 7], [10, 21, 14, 7]));
        wire.borrow_mut().inbox.push_back(arp_frame([2, 0, 0, 0, 0, 1], [10, 21, 14, 2], [10, 21, 14, 2]));
        assert_eq!(s.probe_address([10, 21, 14, 2]), None);
        // (The stack's own DHCP socket may also have sent a DISCOVER meanwhile; count only the probes.)
        let probes = wire.borrow().sent.iter().filter(|f| f[12..14] == [0x08, 0x06] && f[28..32] == [0, 0, 0, 0]).count();
        assert_eq!(probes, 3, "three ARP probes were sent");
    }
}
