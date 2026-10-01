//! Property list values (dict, array, string, data, integer, bool, real). The binary format
//! (bplist00) is read only, because a phone may answer lockdownd in either format; everything we
//! send is XML (`to_xml`, implemented in `xml.rs`, which also reads XML).
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(u64),
    Real(f64),
    String(String),
    Data(Vec<u8>),
    Array(Vec<Value>),
    Dict(Vec<(String, Value)>),
}

impl Value {
    /// Dict lookup.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<u64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Value::Data(d) => Some(d),
            _ => None,
        }
    }
    pub fn dict_get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }
    pub fn dict_get_int(&self, key: &str) -> Option<u64> {
        self.get(key)?.as_int()
    }
    pub fn dict_get_data(&self, key: &str) -> Option<&[u8]> {
        self.get(key)?.as_data()
    }
    /// Convenience: builds a dict from (key, value) pairs.
    pub fn dict(kv: Vec<(&str, Value)>) -> Value {
        Value::Dict(kv.into_iter().map(|(k, v)| (String::from(k), v)).collect())
    }
    pub fn str(s: &str) -> Value {
        Value::String(String::from(s))
    }
    pub fn data(d: &[u8]) -> Value {
        Value::Data(d.to_vec())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Unsupported,
}

pub type Result<T> = core::result::Result<T, Error>;

/// Parses a plist in either format (auto-detected: `bplist00` magic, else XML).
pub fn parse(buf: &[u8]) -> Result<Value> {
    if buf.starts_with(b"bplist00") {
        parse_binary(buf)
    } else {
        crate::xml::parse(buf)
    }
}

/// Serializes in XML format (what every request to lockdownd uses).
pub fn to_xml(v: &Value) -> Vec<u8> {
    crate::xml::write(v).into_bytes()
}

// ---------------------------------------------------------------------------
// Binary plist reader
// ---------------------------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
}

impl<'a> Reader<'a> {
    fn obj(&self, idx: usize, depth: usize) -> Result<Value> {
        if depth > 64 || idx >= self.offsets.len() {
            return Err(Error::Malformed);
        }
        let mut p = self.offsets[idx];
        let marker = *self.buf.get(p).ok_or(Error::Malformed)?;
        p += 1;
        let (ty, mut info) = (marker >> 4, (marker & 0x0f) as usize);
        let mut len_or_count = info;
        if info == 0x0f {
            // Length follows as an inline int object.
            let m2 = *self.buf.get(p).ok_or(Error::Malformed)?;
            p += 1;
            if m2 >> 4 != 0x1 {
                return Err(Error::Malformed);
            }
            let n = 1usize << (m2 & 0x0f);
            let bytes = self.buf.get(p..p + n).ok_or(Error::Malformed)?;
            len_or_count = 0;
            for b in bytes {
                len_or_count = len_or_count << 8 | *b as usize;
            }
            info = len_or_count;
            p += n;
        }
        match ty {
            0x0 => match marker {
                0x08 => Ok(Value::Bool(false)),
                0x09 => Ok(Value::Bool(true)),
                _ => Err(Error::Unsupported), // null / fill
            },
            0x1 => {
                let n = 1usize << info;
                if n > 8 {
                    return Err(Error::Unsupported); // 128-bit ints not needed
                }
                let bytes = self.buf.get(p..p + n).ok_or(Error::Malformed)?;
                let mut v = 0u64;
                for b in bytes {
                    v = v << 8 | *b as u64;
                }
                Ok(Value::Int(v))
            }
            0x2 | 0x3 => {
                // real / date (date is seconds since 2001; we don't need the distinction)
                let v = match info {
                    2 => f64::from(f32::from_be_bytes(self.buf.get(p..p + 4).ok_or(Error::Malformed)?.try_into().map_err(|_| Error::Malformed)?)),
                    3 => f64::from_be_bytes(self.buf.get(p..p + 8).ok_or(Error::Malformed)?.try_into().map_err(|_| Error::Malformed)?),
                    _ => return Err(Error::Malformed),
                };
                Ok(Value::Real(v))
            }
            0x4 => {
                let d = self.buf.get(p..p + len_or_count).ok_or(Error::Malformed)?;
                Ok(Value::Data(d.to_vec()))
            }
            0x5 => {
                let d = self.buf.get(p..p + len_or_count).ok_or(Error::Malformed)?;
                Ok(Value::String(String::from_utf8_lossy(d).into_owned()))
            }
            0x6 => {
                let d = self.buf.get(p..p + 2 * len_or_count).ok_or(Error::Malformed)?;
                let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Ok(Value::String(String::from_utf16_lossy(&units)))
            }
            0xA => {
                let mut out = Vec::with_capacity(len_or_count);
                for i in 0..len_or_count {
                    let r = self.read_ref(p + i * self.ref_size)?;
                    out.push(self.obj(r, depth + 1)?);
                }
                Ok(Value::Array(out))
            }
            0xD => {
                let mut out = Vec::with_capacity(len_or_count);
                for i in 0..len_or_count {
                    let kr = self.read_ref(p + i * self.ref_size)?;
                    let vr = self.read_ref(p + (len_or_count + i) * self.ref_size)?;
                    let key = match self.obj(kr, depth + 1)? {
                        Value::String(s) => s,
                        _ => return Err(Error::Malformed),
                    };
                    out.push((key, self.obj(vr, depth + 1)?));
                }
                Ok(Value::Dict(out))
            }
            _ => Err(Error::Unsupported),
        }
    }

    fn read_ref(&self, p: usize) -> Result<usize> {
        let bytes = self.buf.get(p..p + self.ref_size).ok_or(Error::Malformed)?;
        let mut v = 0usize;
        for b in bytes {
            v = v << 8 | *b as usize;
        }
        Ok(v)
    }
}

fn parse_binary(buf: &[u8]) -> Result<Value> {
    if buf.len() < 8 + 32 {
        return Err(Error::Malformed);
    }
    let t = &buf[buf.len() - 32..];
    let off_size = t[6] as usize;
    let ref_size = t[7] as usize;
    let num = u64::from_be_bytes(t[8..16].try_into().map_err(|_| Error::Malformed)?) as usize;
    let top = u64::from_be_bytes(t[16..24].try_into().map_err(|_| Error::Malformed)?) as usize;
    let off_start = u64::from_be_bytes(t[24..32].try_into().map_err(|_| Error::Malformed)?) as usize;
    if off_size == 0 || off_size > 8 || ref_size == 0 || ref_size > 8 || off_start + num * off_size > buf.len() {
        return Err(Error::Malformed);
    }
    let mut offsets = Vec::with_capacity(num);
    for i in 0..num {
        let p = off_start + i * off_size;
        let mut v = 0usize;
        for b in &buf[p..p + off_size] {
            v = v << 8 | *b as usize;
        }
        if v >= buf.len() {
            return Err(Error::Malformed);
        }
        offsets.push(v);
    }
    Reader { buf, offsets, ref_size }.obj(top, 0)
}
