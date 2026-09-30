//! `hal::NetDevice` over the tethered channel. `crates/net` is Ethernet-only
//! (`Medium::Ethernet` is hardwired in `stack.rs` and the spec says not to touch it),
//! so if the relay channel carries raw IP packets this adapter synthesizes/strips the
//! 14-byte Ethernet header around them. Which of the two the hotspot relay emits is
//! marked UNVERIFIED in `lockdown.rs` — flip `raw_ip` to match what a real capture shows.
use crate::mux::Channel;
use hal::{NetDevice, Result};

const ETH_HDR: usize = 14;

pub struct IphoneNet {
    chan: Channel,
    mac: [u8; 6],
    /// Peer MAC to put in synthesized headers (the phone never checks it on a
    /// point-to-point relay).
    peer: [u8; 6],
    raw_ip: bool,
    up: bool,
}

impl IphoneNet {
    pub fn new(chan: Channel, mac: [u8; 6], raw_ip: bool) -> IphoneNet {
        IphoneNet {
            chan,
            mac,
            peer: [0x02, 0x00, 0x70, 0x68, 0x6f, 0x6e], // locally administered, "phon"
            raw_ip,
            up: true,
        }
    }

    /// Derives a locally administered MAC from the device serial.
    pub fn mac_from_serial(serial: &str) -> [u8; 6] {
        let mut mac = [0x02u8, 0x61, 0x70, 0x70, 0x6c, 0x65]; // 02:"apple"
        let mut hash = 0u32;
        for b in serial.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(b as u32);
        }
        mac[3..6].copy_from_slice(&hash.to_le_bytes()[..3]);
        mac
    }

    fn ethertype(payload: &[u8]) -> u16 {
        match payload.first().map(|b| b >> 4) {
            Some(6) => 0x86dd,
            _ => 0x0800, // IPv4 (default)
        }
    }
}

impl NetDevice for IphoneNet {
    fn name(&self) -> &str {
        "iphone-usb"
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        self.up
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        if self.raw_ip {
            if frame.len() < ETH_HDR {
                return Err(hal::Error::InvalidArgument);
            }
            self.chan.write_all(&frame[ETH_HDR..])
        } else {
            self.chan.write_all(frame)
        }
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.raw_ip {
            if buf.len() <= ETH_HDR {
                return None;
            }
            let n = self.chan.read(&mut buf[ETH_HDR..], 1).ok()?;
            if n == 0 {
                return None;
            }
            let et = Self::ethertype(&buf[ETH_HDR..ETH_HDR + n]);
            buf[0..6].copy_from_slice(&self.mac);
            buf[6..12].copy_from_slice(&self.peer);
            buf[12..14].copy_from_slice(&et.to_be_bytes());
            Some(ETH_HDR + n)
        } else {
            let n = self.chan.read(buf, 1).ok()?;
            if n == 0 {
                return None;
            }
            Some(n)
        }
    }
}

impl crate::mux::Channel {
    /// Adapts the channel to `net::Stream` so `net::tls::TlsStream` can run on it
    /// (used by the lockdownd session TLS layer).
    pub fn as_stream(&mut self) -> ChannelStream<'_> {
        ChannelStream(self)
    }
}

pub struct ChannelStream<'a>(&'a mut Channel);

impl net::Stream for ChannelStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> net::Result<usize> {
        // TLS wants blocking reads; loop on empty polls until the overall timeout.
        for _ in 0..10_000 {
            match self.0.read(buf, 1) {
                Ok(0) => continue,
                Ok(n) => return Ok(n),
                Err(_) => return Err(net::Error::Closed),
            }
        }
        Err(net::Error::Timeout)
    }
    fn write_all(&mut self, buf: &[u8]) -> net::Result<()> {
        self.0.write_all(buf).map_err(|_| net::Error::Closed)
    }
}
