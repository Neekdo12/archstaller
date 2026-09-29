//! Minimal HTTP/1.1 GET client with streaming body delivery.
use crate::{Error, Result, Stream};
use alloc::string::String;
use alloc::vec::Vec;

pub struct Response {
    pub status: u16,
    pub content_length: Option<u64>,
    pub location: Option<String>,
}

/// Sends `GET path` and streams the body to `sink`. Returns the status line info; the body is
/// only delivered for 2xx responses.
pub fn get<S: Stream>(
    s: &mut S,
    host: &str,
    path: &str,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<Response> {
    let mut req = String::new();
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\nHost: ");
    req.push_str(host);
    req.push_str("\r\nUser-Agent: archstaler\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes())?;

    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 16 * 1024 {
            return Err(Error::Http("header too large"));
        }
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Err(Error::Closed);
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = core::str::from_utf8(&buf[..head_end]).map_err(|_| Error::Http("bad header"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(Error::Http("no status line"))?;
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or(Error::Http("bad status line"))?;
    let mut resp = Response { status, content_length: None, location: None };
    let mut chunked = false;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        if k.eq_ignore_ascii_case("content-length") {
            resp.content_length = v.parse().ok();
        } else if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
            chunked = true;
        } else if k.eq_ignore_ascii_case("location") {
            resp.location = Some(v.into());
        }
    }
    let body_start = head_end + 4;
    let leftover: Vec<u8> = buf[body_start..].to_vec();
    drop(buf);
    if !(200..300).contains(&status) {
        return Ok(resp);
    }

    if chunked {
        read_chunked(s, leftover, sink)?;
    } else {
        let mut remaining = resp.content_length;
        let mut deliver = |data: &[u8], remaining: &mut Option<u64>| -> Result<()> {
            let data = match remaining {
                Some(r) => &data[..data.len().min(*r as usize)],
                None => data,
            };
            if let Some(r) = remaining {
                *r -= data.len() as u64;
            }
            if data.is_empty() {
                return Ok(());
            }
            sink(data)
        };
        deliver(&leftover, &mut remaining)?;
        while remaining != Some(0) {
            let n = s.read(&mut tmp)?;
            if n == 0 {
                if remaining.is_some() {
                    return Err(Error::Closed);
                }
                break;
            }
            deliver(&tmp[..n], &mut remaining)?;
        }
    }
    Ok(resp)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_chunked<S: Stream>(
    s: &mut S,
    mut pending: Vec<u8>,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut tmp = [0u8; 4096];
    let mut fill = |pending: &mut Vec<u8>, s: &mut S| -> Result<()> {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Err(Error::Closed);
        }
        pending.extend_from_slice(&tmp[..n]);
        Ok(())
    };
    loop {
        // Chunk size line.
        let line_end = loop {
            if let Some(p) = find(&pending, b"\r\n") {
                break p;
            }
            fill(&mut pending, s)?;
        };
        let size_str = core::str::from_utf8(&pending[..line_end]).map_err(|_| Error::Http("bad chunk"))?;
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16).map_err(|_| Error::Http("bad chunk size"))?;
        pending.drain(..line_end + 2);
        if size == 0 {
            return Ok(());
        }
        let mut left = size;
        while left > 0 {
            if pending.is_empty() {
                fill(&mut pending, s)?;
            }
            let n = pending.len().min(left);
            sink(&pending[..n])?;
            pending.drain(..n);
            left -= n;
        }
        while pending.len() < 2 {
            fill(&mut pending, s)?;
        }
        pending.drain(..2); // CRLF after chunk data
    }
}
