//! The iPhone's own USB mux protocol, spoken directly over the mux interface's bulk
//! endpoints (the protocol a host `usbmuxd` daemon speaks to the device; there is no
//! daemon here, and none of the plist socket protocol that clients use to talk to the daemon).
//!
//! Clean-room from the public description of the wire format (libimobiledevice's usbmuxd
//! documents it in `device.c`); all multi-byte fields are big-endian:
//!
//! - Packet: `{ protocol u32, length u32 }` (8-byte "v1" header), or after the version
//!   exchange `{ protocol, length, magic 0xfeedface, tx_seq u16, rx_seq u16 }` (16 bytes,
//!   "v2"). One mux packet is one USB transfer.
//! - Handshake: the host sends a version packet (`major 2, minor 0, pad 0`, v1 header), the
//!   device answers with its version, and for version 2 the host sends a setup packet (`0x07`).
//! - Connections are a small TCP-like protocol carried in protocol 6 packets: a 20-byte
//!   TCP header (ports, seq, ack, flags, window scaled by 256) plus payload. A SYN does not
//!   consume sequence space here; after the device's SYN|ACK both counters start at 1.
use alloc::vec::Vec;
use hal::{Error, Result};

const PROTO_VERSION: u32 = 0;
const PROTO_SETUP: u32 = 2;
const PROTO_TCP: u32 = 6;
const MAGIC: u32 = 0xfeed_face;

const TH_SYN: u8 = 0x02;
const TH_RST: u8 = 0x04;
const TH_ACK: u8 = 0x10;
const TCP_HDR: usize = 20;

/// Largest payload per data packet.
const MAX_PAYLOAD: usize = 8192;
/// Receive window we advertise, as in the reference implementation.
const RX_WINDOW: u32 = 131_072;
/// One USB read; mux packets are far smaller in this flow.
const RX_BUF: usize = 16 * 1024;

pub const LOCKDOWN_PORT: u16 = 62078;

/// Apple's USB vendor id; the mux interface is class ff / subclass fe / protocol 02.
pub const APPLE_VENDOR: u16 = 0x05ac;
pub const MUX_CLASS: (u8, u8, u8) = (0xff, 0xfe, 0x02);

// ---------------------------------------------------------------------------
// Pure framing (host-testable)
// ---------------------------------------------------------------------------

/// Builds one mux packet. `version` selects the header size (0/1: 8 bytes, 2: 16 bytes).
#[doc(hidden)]
pub fn encode_packet(version: u32, protocol: u32, tx_seq: u16, rx_seq: u16, payload: &[u8]) -> Vec<u8> {
    let hdr = if version >= 2 { 16 } else { 8 };
    let mut p = Vec::with_capacity(hdr + payload.len());
    p.extend_from_slice(&protocol.to_be_bytes());
    p.extend_from_slice(&((hdr + payload.len()) as u32).to_be_bytes());
    if version >= 2 {
        p.extend_from_slice(&MAGIC.to_be_bytes());
        p.extend_from_slice(&tx_seq.to_be_bytes());
        p.extend_from_slice(&rx_seq.to_be_bytes());
    }
    p.extend_from_slice(payload);
    p
}

/// Splits a complete mux packet into `(protocol, rx_seq field, payload)`.
#[doc(hidden)]
pub fn parse_packet(version: u32, buf: &[u8]) -> Result<(u32, u16, &[u8])> {
    let hdr = if version >= 2 { 16 } else { 8 };
    if buf.len() < hdr {
        return Err(Error::Io);
    }
    let protocol = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    let length = u32::from_be_bytes(buf[4..8].try_into().unwrap()) as usize;
    if length != buf.len() {
        return Err(Error::Io);
    }
    let rx_seq = if version >= 2 { u16::from_be_bytes(buf[14..16].try_into().unwrap()) } else { 0 };
    Ok((protocol, rx_seq, &buf[hdr..]))
}

#[doc(hidden)]
#[derive(Debug, PartialEq, Eq)]
pub struct Tcp {
    pub sport: u16,
    pub dport: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    /// Window field as on the wire (the real window is this << 8).
    pub win: u16,
}

/// Builds the 20-byte TCP header followed by `data`.
#[doc(hidden)]
pub fn encode_tcp(t: &Tcp, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(TCP_HDR + data.len());
    p.extend_from_slice(&t.sport.to_be_bytes());
    p.extend_from_slice(&t.dport.to_be_bytes());
    p.extend_from_slice(&t.seq.to_be_bytes());
    p.extend_from_slice(&t.ack.to_be_bytes());
    p.push((TCP_HDR as u8 / 4) << 4);
    p.push(t.flags);
    p.extend_from_slice(&t.win.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // checksum, urgent pointer: unused
    p.extend_from_slice(data);
    p
}

#[doc(hidden)]
pub fn parse_tcp(buf: &[u8]) -> Result<(Tcp, &[u8])> {
    if buf.len() < TCP_HDR {
        return Err(Error::Io);
    }
    let off = ((buf[12] >> 4) as usize) * 4;
    if off < TCP_HDR || off > buf.len() {
        return Err(Error::Io);
    }
    let t = Tcp {
        sport: u16::from_be_bytes(buf[0..2].try_into().unwrap()),
        dport: u16::from_be_bytes(buf[2..4].try_into().unwrap()),
        seq: u32::from_be_bytes(buf[4..8].try_into().unwrap()),
        ack: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
        flags: buf[13],
        win: u16::from_be_bytes(buf[14..16].try_into().unwrap()),
    };
    Ok((t, &buf[off..]))
}

// ---------------------------------------------------------------------------
// The muxer
// ---------------------------------------------------------------------------

struct Conn {
    sport: u16,
    dport: u16,
    /// Next sequence number we send / bytes of the device's stream we acknowledge.
    tx_seq: u32,
    tx_ack: u32,
    /// What the device has acknowledged of ours, and its receive window in bytes.
    peer_ack: u32,
    peer_win: u32,
    /// Received payload not yet handed to the caller.
    rx: Vec<u8>,
    reset: bool,
}

pub struct Muxer {
    ctrl: usb::Controller,
    dev: usb::Device,
    ep_in: u8,
    ep_out: u8,
    mps_out: usize,
    version: u32,
    tx_seq: u16,
    rx_seq: u16,
    next_sport: u16,
    conn: Option<Conn>,
}

impl Muxer {
    /// Takes the first attached Apple device exposing the mux interface (preferring a
    /// configuration that also has the tethering interface) and its controller. Call
    /// `handshake` before `connect`.
    pub fn find(scan: &mut usb::Scan) -> Result<Muxer> {
        let (ctrl, dev) = scan.take(
            |d| {
                if d.vendor != APPLE_VENDOR {
                    return None;
                }
                d.interfaces.iter().find(|i| (i.class, i.subclass, i.protocol) == MUX_CLASS).cloned()
            },
            |d| {
                d.interfaces.iter().any(|i| {
                    (i.class, i.subclass, i.protocol) == crate::netdev::IPHETH_CLASS && d.bulk_in(i).is_some() && d.bulk_out(i).is_some()
                })
            },
        )?;
        Self::from_device(ctrl, dev)
    }

    pub fn from_device(ctrl: usb::Controller, dev: usb::Device) -> Result<Muxer> {
        let iface = dev
            .interfaces
            .iter()
            .find(|i| (i.class, i.subclass, i.protocol) == MUX_CLASS)
            .cloned()
            .ok_or(Error::Unsupported)?;
        let ep_in = dev.bulk_in(&iface).ok_or(Error::Unsupported)?;
        let ep_out = dev.bulk_out(&iface).ok_or(Error::Unsupported)?;
        let mps_out = iface.endpoints.iter().find(|e| e.addr == ep_out).map_or(512, |e| e.max_packet as usize);
        Ok(Muxer {
            ctrl,
            dev,
            ep_in,
            ep_out,
            mps_out,
            version: 0,
            tx_seq: 0,
            rx_seq: 0xffff,
            next_sport: 1,
            conn: None,
        })
    }

    /// Mux protocol version agreed with the device (0 before `handshake`).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Gives the USB controller and device back (the mux pipe is no longer needed).
    pub fn into_parts(self) -> (usb::Controller, usb::Device) {
        (self.ctrl, self.dev)
    }

    fn send_packet(&mut self, protocol: u32, payload: &[u8]) -> Result<()> {
        let p = encode_packet(self.version, protocol, self.tx_seq, self.rx_seq, payload);
        if self.version >= 2 {
            self.tx_seq = self.tx_seq.wrapping_add(1);
        }
        self.ctrl.bulk_write(&self.dev, self.ep_out, &p)?;
        if p.len() % self.mps_out == 0 {
            self.ctrl.bulk_write_zlp(&self.dev, self.ep_out)?;
        }
        Ok(())
    }

    /// Receives one mux packet (a packet larger than one transfer is reassembled).
    fn recv_packet(&mut self, timeout_ms: u64) -> Result<(u32, Vec<u8>)> {
        let deadline = drivers_now() + timeout_ms * 1_000_000;
        let mut data: Vec<u8> = Vec::new();
        loop {
            let left = deadline.saturating_sub(drivers_now()) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            let mut chunk = alloc::vec![0u8; RX_BUF];
            let n = self.ctrl.bulk_read(&self.dev, self.ep_in, &mut chunk, left.max(1))?;
            data.extend_from_slice(&chunk[..n]);
            if data.len() >= 8 {
                let total = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
                if data.len() >= total {
                    break;
                }
            }
        }
        let (protocol, rx_seq, payload) = parse_packet(self.version, &data)?;
        if self.version >= 2 {
            self.rx_seq = rx_seq;
        }
        Ok((protocol, payload.to_vec()))
    }

    /// Version exchange (and, for version 2, the setup packet). Must run once per
    /// device before any connection.
    pub fn handshake(&mut self) -> Result<()> {
        let mut vh = [0u8; 12];
        vh[0..4].copy_from_slice(&2u32.to_be_bytes()); // major 2, minor 0, padding 0
        self.send_packet(PROTO_VERSION, &vh)?;
        let deadline = drivers_now() + 5_000_000_000;
        loop {
            let left = deadline.saturating_sub(drivers_now()) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            let (protocol, payload) = self.recv_packet(left)?;
            if protocol != PROTO_VERSION || payload.len() < 12 {
                continue;
            }
            let major = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            if major != 1 && major != 2 {
                return Err(Error::Unsupported);
            }
            self.version = major;
            if major >= 2 {
                self.send_packet(PROTO_SETUP, &[7])?;
            }
            return Ok(());
        }
    }

    fn send_tcp(&mut self, flags: u8, data: &[u8]) -> Result<()> {
        let c = self.conn.as_ref().ok_or(Error::InvalidArgument)?;
        let t = Tcp {
            sport: c.sport,
            dport: c.dport,
            seq: c.tx_seq,
            ack: c.tx_ack,
            flags,
            win: (RX_WINDOW >> 8) as u16,
        };
        let p = encode_tcp(&t, data);
        self.send_packet(PROTO_TCP, &p)
    }

    /// Opens a connection to a TCP port on the device. The returned channel owns the
    /// muxer; `into_muxer` gives it back (the previous connection is dropped).
    pub fn connect(mut self, port: u16) -> Result<Channel> {
        if self.version == 0 {
            self.handshake()?;
        }
        let sport = self.next_sport;
        self.next_sport = self.next_sport.wrapping_add(1).max(1);
        self.conn = Some(Conn { sport, dport: port, tx_seq: 0, tx_ack: 0, peer_ack: 0, peer_win: RX_WINDOW, rx: Vec::new(), reset: false });
        self.send_tcp(TH_SYN, &[])?;
        let deadline = drivers_now() + 5_000_000_000;
        loop {
            let left = deadline.saturating_sub(drivers_now()) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            let (protocol, payload) = self.recv_packet(left)?;
            if protocol != PROTO_TCP {
                continue;
            }
            let Ok((t, _)) = parse_tcp(&payload) else { continue };
            if t.dport != sport || t.sport != port {
                continue;
            }
            if t.flags & TH_RST != 0 {
                hal::info!("usb: mux connection to port {port} refused");
                return Err(Error::Io);
            }
            if t.flags != (TH_SYN | TH_ACK) {
                continue;
            }
            let c = self.conn.as_mut().unwrap();
            c.tx_seq = 1;
            c.tx_ack = 1;
            c.peer_ack = t.ack;
            c.peer_win = (t.win as u32) << 8;
            self.send_tcp(TH_ACK, &[])?;
            return Ok(Channel { mux: self });
        }
    }

    /// Receives and processes one packet for the open connection. `Ok(false)` on timeout.
    fn pump(&mut self, timeout_ms: u64) -> Result<bool> {
        let (protocol, payload) = match self.recv_packet(timeout_ms) {
            Ok(p) => p,
            Err(Error::Timeout) => return Ok(false),
            Err(e) => return Err(e),
        };
        if protocol != PROTO_TCP {
            return Ok(true);
        }
        let Ok((t, data)) = parse_tcp(&payload) else { return Ok(true) };
        let Some(c) = self.conn.as_mut() else { return Ok(true) };
        if t.dport != c.sport || t.sport != c.dport {
            return Ok(true); // a packet for a connection we already dropped
        }
        if t.flags & TH_RST != 0 {
            c.reset = true;
            return Ok(true);
        }
        c.peer_ack = t.ack;
        c.peer_win = (t.win as u32) << 8;
        if !data.is_empty() {
            c.rx.extend_from_slice(data);
            c.tx_ack = c.tx_ack.wrapping_add(data.len() as u32);
            self.send_tcp(TH_ACK, &[])?;
        }
        Ok(true)
    }
}

/// An established connection to a device port.
pub struct Channel {
    mux: Muxer,
}

impl Channel {
    pub fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        let deadline = drivers_now() + 10_000_000_000;
        while !buf.is_empty() {
            let c = self.mux.conn.as_ref().ok_or(Error::Io)?;
            if c.reset {
                return Err(Error::Io);
            }
            let inflight = c.tx_seq.wrapping_sub(c.peer_ack);
            let window = c.peer_win.saturating_sub(inflight) as usize;
            let n = buf.len().min(MAX_PAYLOAD).min(window);
            if n == 0 {
                // Window full: wait for the device's ACKs.
                if drivers_now() > deadline {
                    return Err(Error::Timeout);
                }
                self.mux.pump(50)?;
                continue;
            }
            self.mux.send_tcp(TH_ACK, &buf[..n])?;
            let c = self.mux.conn.as_mut().unwrap();
            c.tx_seq = c.tx_seq.wrapping_add(n as u32);
            buf = &buf[n..];
        }
        Ok(())
    }

    /// Blocks up to `timeout_ms` for data; returns 0 on timeout.
    pub fn read(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<usize> {
        let deadline = drivers_now() + timeout_ms * 1_000_000;
        loop {
            let c = self.mux.conn.as_mut().ok_or(Error::Io)?;
            if !c.rx.is_empty() {
                let n = buf.len().min(c.rx.len());
                buf[..n].copy_from_slice(&c.rx[..n]);
                c.rx.drain(..n);
                return Ok(n);
            }
            if c.reset {
                return Err(Error::Io);
            }
            let left = deadline.saturating_sub(drivers_now()) / 1_000_000;
            if left == 0 {
                return Ok(0);
            }
            self.mux.pump(left)?;
        }
    }

    pub fn read_exact(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<()> {
        let deadline = drivers_now() + timeout_ms * 1_000_000;
        let mut got = 0usize;
        while got < buf.len() {
            let left = deadline.saturating_sub(drivers_now()) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            got += self.read(&mut buf[got..], left)?;
        }
        Ok(())
    }

    /// Gives the underlying muxer back (e.g. to open another connection).
    pub fn into_muxer(self) -> Muxer {
        self.mux
    }
}

fn drivers_now() -> u64 {
    drivers::platform::now_ns()
}
