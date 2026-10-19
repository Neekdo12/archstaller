//! CDC-NCM (Network Control Model) framing, 16-bit NTB format only. Ethernet frames travel
//! in NCM Transfer Blocks: a 12-byte header (`NCMH`), the datagrams, and a datagram pointer
//! table (`NCM0`) listing each frame's offset and length. Written from the public USB CDC
//! NCM 1.0 specification.
use alloc::vec::Vec;
use hal::{Error, Result};

const NTH16_SIG: u32 = 0x484d_434e; // "NCMH"
const NDP16_SIG: u32 = 0x304d_434e; // "NCM0" (no datagram CRC)
const NTH16_LEN: usize = 12;
const NDP16_LEN: usize = 16; // header + one datagram entry + the terminating zero entry

/// NTB limits the device reports in GET_NTB_PARAMETERS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    pub in_max: u32,
    pub out_max: u32,
    pub out_divisor: u16,
    pub out_remainder: u16,
    pub out_align: u16,
}

fn put_slice(buf: &mut [u8], offset: usize, data: &[u8]) -> Result<()> {
    let end = offset.checked_add(data.len()).ok_or(Error::InvalidArgument)?;
    let dst = buf.get_mut(offset..end).ok_or(Error::InvalidArgument)?;
    dst.copy_from_slice(data);
    Ok(())
}

fn le16(b: &[u8], o: usize) -> Result<u16> {
    match b.get(o..o + 2) {
        Some(&[b0, b1]) => Ok(u16::from_le_bytes([b0, b1])),
        _ => Err(Error::Io),
    }
}
fn le32(b: &[u8], o: usize) -> Result<u32> {
    match b.get(o..o + 4) {
        Some(&[b0, b1, b2, b3]) => Ok(u32::from_le_bytes([b0, b1, b2, b3])),
        _ => Err(Error::Io),
    }
}

/// Parses the 28-byte GET_NTB_PARAMETERS reply. The device must support the 16-bit format.
pub fn parse_params(b: &[u8]) -> Result<Params> {
    if le16(b, 0)? < 28 || le16(b, 2)? & 1 == 0 {
        return Err(Error::Unsupported);
    }
    Ok(Params {
        in_max: le32(b, 4)?,
        out_max: le32(b, 16)?,
        out_divisor: le16(b, 20)?,
        out_remainder: le16(b, 22)?,
        out_align: le16(b, 24)?,
    })
}

fn align_up(v: usize, a: usize) -> usize {
    let a = a.max(1);
    v.div_ceil(a) * a
}

/// Builds an NTB holding one Ethernet frame: header, the datagram at an offset that satisfies
/// the device's divisor/remainder rule, then the pointer table at the device's alignment.
pub fn build_ntb(frame: &[u8], seq: u16, p: &Params) -> Result<Vec<u8>> {
    let div = (p.out_divisor as usize).max(1);
    let rem = p.out_remainder as usize;
    let mut dg = NTH16_LEN;
    while dg % div != rem % div {
        dg += 1;
    }
    let ndp = align_up(dg + frame.len(), (p.out_align as usize).max(4));
    let total = ndp + NDP16_LEN;
    if total > p.out_max as usize || total > 0xffff {
        return Err(Error::InvalidArgument);
    }
    let mut b = alloc::vec![0u8; total];
    put_slice(&mut b, 0, &NTH16_SIG.to_le_bytes())?;
    put_slice(&mut b, 4, &(NTH16_LEN as u16).to_le_bytes())?;
    put_slice(&mut b, 6, &seq.to_le_bytes())?;
    put_slice(&mut b, 8, &(total as u16).to_le_bytes())?;
    put_slice(&mut b, 10, &(ndp as u16).to_le_bytes())?;
    put_slice(&mut b, dg, frame)?;
    put_slice(&mut b, ndp, &NDP16_SIG.to_le_bytes())?;
    put_slice(&mut b, ndp + 4, &(NDP16_LEN as u16).to_le_bytes())?;
    // wNextNdpIndex = 0
    put_slice(&mut b, ndp + 8, &(dg as u16).to_le_bytes())?;
    put_slice(&mut b, ndp + 10, &(frame.len() as u16).to_le_bytes())?;
    // The last entry (index 0, length 0) terminates the table.
    Ok(b)
}

/// Extracts every datagram from a received NTB (following chained pointer tables). Returns
/// what could be parsed; malformed blocks yield an empty list.
pub fn parse_ntb(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if le32(b, 0) != Ok(NTH16_SIG) || le16(b, 4) != Ok(NTH16_LEN as u16) {
        return out;
    }
    let mut ndp = le16(b, 10).unwrap_or(0) as usize;
    let mut guard = 0;
    while ndp != 0 && guard < 8 {
        guard += 1;
        // "NCM0" has no datagram CRC. "NCM1" appends one; its frames are still usable, the
        // length just includes 4 CRC bytes we do not strip, so only accept NCM0.
        if le32(b, ndp) != Ok(NDP16_SIG) {
            break;
        }
        let len = le16(b, ndp + 4).unwrap_or(0) as usize;
        let next = le16(b, ndp + 6).unwrap_or(0) as usize;
        let mut e = ndp + 8;
        while let Some(e_next) = e.checked_add(4) {
            let Some(ndp_end) = ndp.checked_add(len) else {
                break;
            };
            if e_next > ndp_end {
                break;
            }
            let (idx, dlen) = (
                le16(b, e).unwrap_or(0) as usize,
                le16(b, e + 2).unwrap_or(0) as usize,
            );
            if idx == 0 && dlen == 0 {
                break;
            }
            if idx < NTH16_LEN || dlen == 0 {
                e = e_next;
                continue;
            }
            if let Some(end) = idx.checked_add(dlen) {
                if let Some(d) = b.get(idx..end) {
                    out.push(d.to_vec());
                }
            }
            e = e_next;
        }
        ndp = next;
    }
    out
}
