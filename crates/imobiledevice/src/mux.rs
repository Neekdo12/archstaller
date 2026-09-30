//! usbmuxd protocol version 1 (plist flavor) over a USB device's bulk endpoints.
//!
//! Wire format (clean-room from the public libusbmuxd protocol description):
//! a 16-byte little-endian header `{ length, version, message, tag }` where `length`
//! covers header + payload. Control messages use `message = 8` (plist) with an XML
//! plist body. After a `Connect` succeeds, the bulk pipe carries raw connection bytes
//! with no further framing — one connection at a time, which is why the lockdownd
//! connection is closed before opening a service channel.
use crate::plist::{self, Value};
use alloc::vec::Vec;
use hal::{Error, Result};

const VERSION: u32 = 1;
const MSG_PLIST: u32 = 8;

pub const RESULT_OK: u64 = 0;
pub const RESULT_CONNREFUSED: u64 = 3;
pub const RESULT_BADVERSION: u64 = 6;

pub const LOCKDOWN_PORT: u16 = 62078;

/// Apple's USB vendor id; the mux interface is class ff / subclass fe / protocol 02.
pub const APPLE_VENDOR: u16 = 0x05ac;
pub const MUX_CLASS: (u8, u8, u8) = (0xff, 0xfe, 0x02);

pub struct Muxer {
    ctrl: usb::Controller,
    dev: usb::Device,
    ep_in: u8,
    ep_out: u8,
    tag: u32,
    /// Bytes read from the bulk pipe beyond the current frame/connection boundary.
    pending: Vec<u8>,
}

impl Muxer {
    /// Finds the first attached Apple device exposing the usbmux interface and takes
    /// ownership of its controller.
    pub fn find() -> Result<Muxer> {
        let (ctrl, dev) = usb::find_device(APPLE_VENDOR, 0, MUX_CLASS.0, MUX_CLASS.1, MUX_CLASS.2)?;
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
        Ok(Muxer { ctrl, dev, ep_in, ep_out, tag: 0, pending: Vec::new() })
    }

    pub fn device_serial(&self) -> &str {
        &self.dev.serial
    }

    fn send_frame(&mut self, message: u32, payload: &[u8]) -> Result<u32> {
        self.tag += 1;
        let tag = self.tag;
        let frame = encode_frame(message, tag, payload);
        self.ctrl.bulk_write(&self.dev, self.ep_out, &frame)?;
        Ok(tag)
    }

    /// Reads bytes into `buf`, first from the stash, then from the device.
    fn read_raw(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<usize> {
        if !self.pending.is_empty() {
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        self.ctrl.bulk_read(&self.dev, self.ep_in, buf, timeout_ms)
    }

    fn read_exact(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<()> {
        let deadline = drivers_now() + timeout_ms * 1_000_000;
        let mut got = 0usize;
        while got < buf.len() {
            let left = (deadline.saturating_sub(drivers_now())) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            got += self.read_raw(&mut buf[got..], left.max(1))?;
        }
        Ok(())
    }

    /// Receives one control frame; returns `(message_type, tag, payload)`.
    fn recv_frame(&mut self, timeout_ms: u64) -> Result<(u32, u32, Vec<u8>)> {
        let mut hdr = [0u8; 16];
        self.read_exact(&mut hdr, timeout_ms)?;
        let (length, message, tag) = parse_header(&hdr)?;
        if length < 16 || length > 1 << 20 {
            return Err(Error::Io);
        }
        let mut payload = alloc::vec![0u8; length - 16];
        self.read_exact(&mut payload, timeout_ms)?;
        Ok((message, tag, payload))
    }

    pub fn send_plist(&mut self, v: &Value) -> Result<u32> {
        self.send_frame(MSG_PLIST, &plist::to_xml(v))
    }

    /// Waits for the plist reply to a request (requests are serialized, so the first
    /// plist frame back is the answer; device-list events are skipped).
    pub fn recv_plist(&mut self, _tag: u32, timeout_ms: u64) -> Result<Value> {
        let deadline = drivers_now() + timeout_ms * 1_000_000;
        loop {
            let left = (deadline.saturating_sub(drivers_now())) / 1_000_000;
            if left == 0 {
                return Err(Error::Timeout);
            }
            let (message, _rtag, payload) = self.recv_frame(left.max(1))?;
            if message != MSG_PLIST {
                continue; // device add/remove etc.: not listened for, skip
            }
            return plist::parse(&payload).map_err(|_| Error::Io);
        }
    }

    /// Sends a request plist and waits for the reply.
    pub fn exchange(&mut self, v: &Value, timeout_ms: u64) -> Result<Value> {
        let tag = self.send_plist(v)?;
        self.recv_plist(tag, timeout_ms)
    }

    /// Result code of the last reply (`MessageType == "Result"`, `Number` field).
    pub fn result_code(v: &Value) -> Result<u64> {
        if v.dict_get_str("MessageType") == Some("Result") {
            v.dict_get_int("Number").ok_or(Error::Io)
        } else {
            Err(Error::Io)
        }
    }

    fn control_message(message_type: &str) -> Value {
        Value::dict(alloc::vec![
            ("ClientVersionString", Value::str("archstaler")),
            ("MessageType", Value::str(message_type)),
            ("ProgName", Value::str("archstaler")),
            ("kLibUSBMuxVersion", Value::Int(3)),
        ])
    }

    /// Reads the SystemBUID from the device (no session needed).
    pub fn read_buid(&mut self) -> Result<alloc::string::String> {
        let r = self.exchange(&Self::control_message("ReadBUID"), 5000)?;
        if let Some(buid) = r.dict_get_str("BUID") {
            return Ok(alloc::string::String::from(buid));
        }
        Err(Error::Io)
    }

    /// Opens a connection to a TCP port on the device. On success the bulk pipe becomes
    /// a raw byte stream; the returned channel owns the pipe until dropped.
    pub fn connect(mut self, device_id: u32, port: u16) -> Result<Channel> {
        let mut req = Self::control_message("Connect");
        if let Value::Dict(kv) = &mut req {
            kv.push(("DeviceID".into(), Value::Int(device_id as u64)));
            // The port goes on the wire in network byte order, as an integer.
            kv.push(("PortNumber".into(), Value::Int(port.to_be() as u64)));
        }
        let tag = self.send_plist(&req)?;
        let reply = self.recv_plist(tag, 5000)?;
        match Self::result_code(&reply) {
            Ok(RESULT_OK) => Ok(Channel { mux: self }),
            Ok(RESULT_CONNREFUSED) => Err(Error::Io),
            Ok(RESULT_BADVERSION) => Err(Error::Unsupported),
            _ => Err(Error::Io),
        }
    }
}

/// An established connection to a device port: raw bytes on the bulk pipe.
pub struct Channel {
    mux: Muxer,
}

impl Channel {
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.mux.ctrl.bulk_write(&self.mux.dev, self.mux.ep_out, buf)
    }

    /// Blocks up to `timeout_ms` for data; returns 0 on timeout.
    pub fn read(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<usize> {
        match self.mux.read_raw(buf, timeout_ms) {
            Err(Error::Timeout) => Ok(0),
            r => r,
        }
    }

    pub fn read_exact(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<()> {
        self.mux.read_exact(buf, timeout_ms)
    }

    /// Gives the underlying muxer back (e.g. to open another connection after this one
    /// is finished — the bulk pipe carries one connection at a time).
    pub fn into_muxer(self) -> Muxer {
        self.mux
    }
}

fn drivers_now() -> u64 {
    drivers::platform::now_ns()
}

/// Builds one usbmuxd frame (header + payload); pure, for host-side tests.
#[doc(hidden)]
pub fn encode_frame(message: u32, tag: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(16 + payload.len());
    frame.extend_from_slice(&(16 + payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&VERSION.to_le_bytes());
    frame.extend_from_slice(&message.to_le_bytes());
    frame.extend_from_slice(&tag.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Parses a usbmuxd frame header into `(total_length, message, tag)`; pure, for tests.
#[doc(hidden)]
pub fn parse_header(hdr: &[u8; 16]) -> Result<(usize, u32, u32)> {
    Ok((
        u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize,
        u32::from_le_bytes(hdr[8..12].try_into().unwrap()),
        u32::from_le_bytes(hdr[12..16].try_into().unwrap()),
    ))
}
