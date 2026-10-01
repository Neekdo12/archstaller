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
        while pos + 3 <= self.cs.len() {
            let len = self.cs[pos] as usize;
            if len < 3 || pos + len > self.cs.len() {
                return None;
            }
            if self.cs[pos + 2] == subtype {
                return Some(&self.cs[pos..pos + len]);
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
        let len = blob[pos] as usize;
        let ty = blob[pos + 1];
        if len < 2 || pos + len > blob.len() {
            break;
        }
        match (ty, len) {
            (DT_INTERFACE, 9..) => {
                let d = &blob[pos..pos + len];
                out.push(InterfaceDesc {
                    num: d[2],
                    alt: d[3],
                    class: d[5],
                    subclass: d[6],
                    protocol: d[7],
                    endpoints: Vec::new(),
                    cs: Vec::new(),
                });
                cur = Some(out.len() - 1);
            }
            (0x24, 3..) => {
                if let Some(i) = cur {
                    out[i].cs.extend_from_slice(&blob[pos..pos + len]);
                }
            }
            (DT_ENDPOINT, 7..) => {
                if let Some(i) = cur {
                    let d = &blob[pos..pos + len];
                    out[i].endpoints.push(EndpointDesc {
                        addr: d[2],
                        attrs: d[3],
                        max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x7ff,
                        interval: d[6],
                    });
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
    if blob.len() < 2 || blob[1] != DT_STRING {
        return None;
    }
    let n = (blob[0] as usize).min(blob.len());
    let units: Vec<u16> = (2..n & !1)
        .step_by(2)
        .map(|i| u16::from_le_bytes([blob[i], blob[i + 1]]))
        .collect();
    Some(alloc::string::String::from_utf16_lossy(&units))
}
