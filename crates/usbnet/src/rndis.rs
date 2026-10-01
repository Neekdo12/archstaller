//! RNDIS (Remote NDIS over USB) message building and parsing, as used by Android's USB
//! tethering. All fields are little-endian. Control messages travel in the CDC encapsulated
//! command/response requests; data packets are wrapped in a 44-byte `RNDIS_PACKET_MSG`
//! header on the bulk endpoints. Written from Microsoft's public RNDIS specification.
use alloc::vec::Vec;
use hal::{Error, Result};

pub const MSG_PACKET: u32 = 0x0000_0001;
pub const MSG_INIT: u32 = 0x0000_0002;
pub const MSG_QUERY: u32 = 0x0000_0004;
pub const MSG_SET: u32 = 0x0000_0005;
pub const MSG_KEEPALIVE: u32 = 0x0000_0008;
/// Completion messages have the request type with this bit set.
pub const CMPLT: u32 = 0x8000_0000;

pub const OID_802_3_PERMANENT_ADDRESS: u32 = 0x0101_0101;
pub const OID_GEN_CURRENT_PACKET_FILTER: u32 = 0x0001_010e;
/// Directed | multicast | all multicast | broadcast.
pub const PACKET_FILTER: u32 = 0x0000_000f;

pub const STATUS_SUCCESS: u32 = 0;

/// Header in front of every data frame on the bulk endpoints.
pub const PACKET_HDR: usize = 44;

fn le(buf: &[u8], off: usize) -> Result<u32> {
    buf.get(off..off + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or(Error::Io)
}

fn header(msg_type: u32, len: usize, id: u32) -> Vec<u8> {
    let mut m = Vec::with_capacity(len);
    m.extend_from_slice(&msg_type.to_le_bytes());
    m.extend_from_slice(&(len as u32).to_le_bytes());
    m.extend_from_slice(&id.to_le_bytes());
    m
}

/// `REMOTE_NDIS_INITIALIZE_MSG`: version 1.0, `max_transfer` bytes per bulk transfer.
pub fn init_msg(id: u32, max_transfer: u32) -> Vec<u8> {
    let mut m = header(MSG_INIT, 24, id);
    m.extend_from_slice(&1u32.to_le_bytes()); // major
    m.extend_from_slice(&0u32.to_le_bytes()); // minor
    m.extend_from_slice(&max_transfer.to_le_bytes());
    m
}

/// `REMOTE_NDIS_QUERY_MSG` followed by `in_len` zero bytes of information buffer. Devices
/// (and QEMU) reject a query whose buffer is empty; Linux's `rndis_host` sends 48 bytes for
/// the MAC address query, so that is what callers should pass.
pub fn query_msg(id: u32, oid: u32, in_len: u32) -> Vec<u8> {
    let mut m = header(MSG_QUERY, 28 + in_len as usize, id);
    m.extend_from_slice(&oid.to_le_bytes());
    m.extend_from_slice(&in_len.to_le_bytes()); // information buffer length
    m.extend_from_slice(&20u32.to_le_bytes()); // information buffer offset (from the request id)
    m.extend_from_slice(&0u32.to_le_bytes()); // device VC handle
    m.resize(28 + in_len as usize, 0);
    m
}

/// `REMOTE_NDIS_SET_MSG`.
pub fn set_msg(id: u32, oid: u32, value: &[u8]) -> Vec<u8> {
    let mut m = header(MSG_SET, 28 + value.len(), id);
    m.extend_from_slice(&oid.to_le_bytes());
    m.extend_from_slice(&(value.len() as u32).to_le_bytes());
    m.extend_from_slice(&20u32.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(value);
    m
}

/// Answer to a keepalive from the device.
pub fn keepalive_cmplt(id: u32) -> Vec<u8> {
    let mut m = header(MSG_KEEPALIVE | CMPLT, 16, id);
    m.extend_from_slice(&STATUS_SUCCESS.to_le_bytes());
    m
}

/// A parsed completion (or other control) message.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub msg_type: u32,
    pub request_id: u32,
    pub status: u32,
    /// QUERY_CMPLT information buffer (empty for other types).
    pub info: Vec<u8>,
    /// INITIALIZE_CMPLT: the largest bulk transfer the device accepts.
    pub max_transfer: u32,
}

/// Parses a control message from the encapsulated-response channel. Messages without a
/// request id/status (e.g. indications) come back with zeros.
pub fn parse_reply(buf: &[u8]) -> Result<Reply> {
    let msg_type = le(buf, 0)?;
    let len = le(buf, 4)? as usize;
    if len < 12 || len > buf.len() {
        return Err(Error::Io);
    }
    let buf = &buf[..len];
    let request_id = le(buf, 8)?;
    let status = if len >= 16 { le(buf, 12)? } else { 0 };
    let mut r = Reply { msg_type, request_id, status, info: Vec::new(), max_transfer: 0 };
    if msg_type == MSG_INIT | CMPLT {
        r.max_transfer = le(buf, 36)?;
    } else if msg_type == MSG_QUERY | CMPLT && status == STATUS_SUCCESS {
        let info_len = le(buf, 16)? as usize;
        let info_off = 8 + le(buf, 20)? as usize;
        r.info = buf.get(info_off..info_off + info_len).ok_or(Error::Io)?.to_vec();
    }
    Ok(r)
}

/// Wraps one Ethernet frame in an `RNDIS_PACKET_MSG`.
pub fn wrap_frame(frame: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(PACKET_HDR + frame.len());
    m.extend_from_slice(&MSG_PACKET.to_le_bytes());
    m.extend_from_slice(&((PACKET_HDR + frame.len()) as u32).to_le_bytes());
    m.extend_from_slice(&((PACKET_HDR - 8) as u32).to_le_bytes()); // data offset from byte 8
    m.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    m.extend_from_slice(&[0u8; PACKET_HDR - 16]); // OOB and per-packet info (none), VC handle, reserved
    m.extend_from_slice(frame);
    m
}

/// Splits one bulk transfer into its Ethernet frames (a transfer may hold several packet
/// messages back to back). Stops at the first malformed message.
pub fn unwrap_frames(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 16 <= buf.len() {
        let msg = &buf[pos..];
        let (Ok(ty), Ok(len), Ok(off), Ok(dlen)) = (le(msg, 0), le(msg, 4), le(msg, 8), le(msg, 12)) else { break };
        let (len, start, dlen) = (len as usize, 8 + off as usize, dlen as usize);
        if ty != MSG_PACKET || len < 16 || len > msg.len() || start + dlen > len {
            break;
        }
        out.push(msg[start..start + dlen].to_vec());
        pos += len;
    }
    out
}
