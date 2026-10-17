//! USB descriptor parsing (device, configuration, interface, endpoint, strings).
use alloc::string::String;
use alloc::vec::Vec;

pub const GET_DESCRIPTOR: u8 = 6;
pub const SET_CONFIGURATION: u8 = 9;

pub const DT_DEVICE: u8 = 1;
pub const DT_CONFIG: u8 = 2;
pub const DT_STRING: u8 = 3;
pub const DT_INTERFACE: u8 = 4;
pub const DT_ENDPOINT: u8 = 5;

pub const EP_BULK: u8 = 2;

#[derive(Debug, Clone)]
pub struct EndpointDesc {
    /// bEndpointAddress; bit 7 set = IN.
    pub addr: u8,
    pub attrs: u8,
    pub max_packet: u16,
    pub interval: u8,
}

impl EndpointDesc {
    pub fn is_in(&self) -> bool {
        self.addr & 0x80 != 0
    }
    pub fn is_bulk(&self) -> bool {
        self.attrs & 3 == EP_BULK
    }
    /// xHCI device-context index for this endpoint.
    pub fn dci(&self) -> u8 {
        (self.addr & 0x0f) * 2 + if self.is_in() { 1 } else { 0 }
    }
}

#[derive(Debug, Clone)]
pub struct InterfaceDesc {
    pub num: u8,
    /// bAlternateSetting; every alternate setting is its own entry (alt 0 first).
    pub alt: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<EndpointDesc>,
    /// Raw class-specific descriptors (type 0x24, e.g. CDC functional descriptors) that
    /// follow this interface descriptor, concatenated.
    pub cs: Vec<u8>,
}

impl InterfaceDesc {
    /// The CDC functional descriptor with the given subtype, if present (whole descriptor).
    pub fn cdc_functional(&self, subtype: u8) -> Option<&[u8]> {
        let mut pos = 0;
        while let Some(&len_byte) = self.cs.get(pos) {
            let len = len_byte as usize;
            if len < 3 || pos.checked_add(len)? > self.cs.len() {
                return None;
            }
            let desc = self.cs.get(pos..pos + len)?;
            if desc.get(2).copied() == Some(subtype) {
                return Some(desc);
            }
            pos += len;
        }
        None
    }
}

/// Parses a full configuration descriptor blob into its interface settings and endpoints.
/// Each alternate setting of an interface is returned as a separate entry.
pub fn parse_config(blob: &[u8]) -> Vec<InterfaceDesc> {
    let mut out: Vec<InterfaceDesc> = Vec::new();
    let mut pos = 0usize;
    let mut cur: Option<usize> = None;
    while pos + 2 <= blob.len() {
        let Some(&len_byte) = blob.get(pos) else { break };
        let Some(&ty) = blob.get(pos + 1) else { break };
        let len = len_byte as usize;
        if len < 2 || pos.checked_add(len).is_none_or(|end| end > blob.len()) {
            break;
        }
        let Some(d) = blob.get(pos..pos + len) else { break };
        match (ty, len) {
            (DT_INTERFACE, 9..) => {
                if let [_, _, num, alt, _, class, subclass, protocol, ..] = d {
                    out.push(InterfaceDesc {
                        num: *num,
                        alt: *alt,
                        class: *class,
                        subclass: *subclass,
                        protocol: *protocol,
                        endpoints: Vec::new(),
                        cs: Vec::new(),
                    });
                    cur = Some(out.len() - 1);
                }
            }
            (0x24, 3..) => {
                if let Some(i) = cur {
                    if let Some(last) = out.get_mut(i) {
                        last.cs.extend_from_slice(d);
                    }
                }
            }
            (DT_ENDPOINT, 7..) => {
                if let Some(i) = cur {
                    if let Some(last) = out.get_mut(i) {
                        if let [_, _, addr, attrs, p0, p1, interval, ..] = d {
                            last.endpoints.push(EndpointDesc {
                                addr: *addr,
                                attrs: *attrs,
                                max_packet: u16::from_le_bytes([*p0, *p1]) & 0x7ff,
                                interval: *interval,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        pos += len;
    }
    out
}

/// Decodes a USB string descriptor (UTF-16LE) into a String.
pub fn parse_string(blob: &[u8]) -> Option<String> {
    if blob.len() < 2 || blob.get(1).copied() != Some(DT_STRING) {
        return None;
    }
    let n = (*blob.first()? as usize).min(blob.len());
    let slice = blob.get(2..n & !1)?;
    let (pairs, _) = slice.as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    Some(alloc::string::String::from_utf16_lossy(&units))
}

