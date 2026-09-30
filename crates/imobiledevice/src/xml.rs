//! Minimal XML plist reader/writer: the object types Apple's usbmuxd exchanges on the
//! control channel (dict, array, string, integer, real, bool, data). Not a general XML
//! parser — it scans for the plist tags and ignores everything else (headers, DOCTYPE).
use crate::plist::{Error, Result, Value};
use alloc::string::String;
use alloc::vec::Vec;

pub fn parse(buf: &[u8]) -> Result<Value> {
    let s = core::str::from_utf8(buf).map_err(|_| Error::Malformed)?;
    let mut p = Parser { s, pos: 0 };
    // Skip the prolog: <?xml ...?> processing instructions and the DOCTYPE.
    loop {
        p.skip_junk();
        if p.rest().starts_with("<?") {
            let n = p.rest().find("?>").ok_or(Error::Malformed)?;
            p.pos += n + 2;
        } else if p.rest().starts_with("<!DOCTYPE") {
            let n = p.rest().find('>').ok_or(Error::Malformed)?;
            p.pos += n + 1;
        } else {
            break;
        }
    }
    p.expect("<plist")?;
    p.skip_attrs();
    p.expect(">")?;
    p.skip_junk();
    let v = p.value()?;
    Ok(v)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn skip_junk(&mut self) {
        loop {
            let trimmed = self.rest().trim_start();
            self.pos = self.s.len() - trimmed.len();
            if self.rest().starts_with("<!--") {
                match self.rest().find("-->") {
                    Some(n) => self.pos += n + 3,
                    None => {
                        self.pos = self.s.len();
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, what: &str) -> Result<()> {
        if self.rest().starts_with(what) {
            self.pos += what.len();
            Ok(())
        } else {
            Err(Error::Malformed)
        }
    }

    fn skip_attrs(&mut self) {
        // after "<plist": skip until '>'
        if let Some(n) = self.rest().find('>') {
            self.pos += n;
        }
    }

    fn text_until(&mut self, tag_end: &str) -> Result<String> {
        let n = self.rest().find(tag_end).ok_or(Error::Malformed)?;
        let text = &self.rest()[..n];
        self.pos += n + tag_end.len();
        Ok(unescape(text.trim()))
    }

    fn value(&mut self) -> Result<Value> {
        self.skip_junk();
        if self.rest().starts_with("<dict>") {
            self.pos += 6;
            let mut kv = Vec::new();
            loop {
                self.skip_junk();
                if self.rest().starts_with("</dict>") {
                    self.pos += 7;
                    return Ok(Value::Dict(kv));
                }
                self.expect("<key>")?;
                let k = self.text_until("</key>")?;
                let v = self.value()?;
                kv.push((k, v));
            }
        } else if self.rest().starts_with("<array>") {
            self.pos += 7;
            let mut items = Vec::new();
            loop {
                self.skip_junk();
                if self.rest().starts_with("</array>") {
                    self.pos += 8;
                    return Ok(Value::Array(items));
                }
                items.push(self.value()?);
            }
        } else if self.rest().starts_with("<string") {
            self.expect("<string")?;
            self.expect(">")?;
            Ok(Value::String(self.text_until("</string>")?))
        } else if self.rest().starts_with("<integer>") {
            self.pos += 9;
            let t = self.text_until("</integer>")?;
            t.trim().parse::<u64>().map(Value::Int).map_err(|_| Error::Malformed)
        } else if self.rest().starts_with("<real>") {
            self.pos += 6;
            let t = self.text_until("</real>")?;
            t.trim().parse::<f64>().map(Value::Real).map_err(|_| Error::Malformed)
        } else if self.rest().starts_with("<true/>") || self.rest().starts_with("<true />") {
            self.pos = self.rest().find("/>").ok_or(Error::Malformed)? + self.pos + 2;
            Ok(Value::Bool(true))
        } else if self.rest().starts_with("<false/>") || self.rest().starts_with("<false />") {
            self.pos = self.rest().find("/>").ok_or(Error::Malformed)? + self.pos + 2;
            Ok(Value::Bool(false))
        } else if self.rest().starts_with("<data>") {
            self.pos += 6;
            let t = self.text_until("</data>")?;
            let raw = pgp_lite::base64_decode(&t).map_err(|_| Error::Malformed)?;
            Ok(Value::Data(raw))
        } else if self.rest().starts_with("<data/>") || self.rest().starts_with("<data />") {
            self.pos = self.rest().find("/>").ok_or(Error::Malformed)? + self.pos + 2;
            Ok(Value::Data(Vec::new()))
        } else {
            Err(Error::Malformed)
        }
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
}

pub fn write(v: &Value) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n",
    );
    write_value(&mut s, v, 1);
    s.push_str("</plist>\n");
    s
}

fn indent(s: &mut String, n: usize) {
    for _ in 0..n {
        s.push('\t');
    }
}

fn write_value(s: &mut String, v: &Value, depth: usize) {
    indent(s, depth);
    match v {
        Value::Bool(b) => s.push_str(if *b { "<true/>\n" } else { "<false/>\n" }),
        Value::Int(i) => s.push_str(&alloc::format!("<integer>{i}</integer>\n")),
        Value::Real(f) => s.push_str(&alloc::format!("<real>{f}</real>\n")),
        Value::String(t) => s.push_str(&alloc::format!("<string>{}</string>\n", escape(t))),
        Value::Data(d) => s.push_str(&alloc::format!("<data>{}</data>\n", base64_encode(d))),
        Value::Array(items) => {
            s.push_str("<array>\n");
            for i in items {
                write_value(s, i, depth + 1);
            }
            indent(s, depth);
            s.push_str("</array>\n");
        }
        Value::Dict(kv) => {
            s.push_str("<dict>\n");
            for (k, v) in kv {
                indent(s, depth + 1);
                s.push_str(&alloc::format!("<key>{}</key>\n", escape(k)));
                write_value(s, v, depth + 1);
            }
            indent(s, depth);
            s.push_str("</dict>\n");
        }
    }
}

/// Standard base64 (RFC 4648) encoder.
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
