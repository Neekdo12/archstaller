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
    req.push_str("\r\nUser-Agent: archstaller\r\nAccept: */*\r\nConnection: close\r\n\r\n");
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
    // Only ASCII headers are read; a stray non-UTF-8 byte in another one (a Latin-1 file name) must not fail the download.
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
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

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    /// A canned response, read back `step` bytes at a time.
    struct Canned {
        data: Vec<u8>,
        pos: usize,
        step: usize,
        sent: Vec<u8>,
    }

    impl Stream for Canned {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.sent.extend_from_slice(buf);
            Ok(())
        }
    }

    fn fetch(response: &[u8], step: usize) -> (Result<Response>, Vec<u8>) {
        let mut s = Canned { data: response.to_vec(), pos: 0, step, sent: Vec::new() };
        let mut body = Vec::new();
        let r = get(&mut s, "mirror.example", "/core.db", &mut |d| {
            body.extend_from_slice(d);
            Ok(())
        });
        (r, body)
    }

    #[test]
    fn content_length_body() {
        for step in [1, 3, 4096] {
            let (r, body) = fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello and more", step);
            assert_eq!(r.unwrap().content_length, Some(5));
            assert_eq!(body, b"hello", "step {step}");
        }
    }

    #[test]
    fn chunked_body_with_extensions_and_trailer() {
        let resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\nWiki\r\n6\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\nX-Trailer: y\r\n\r\n";
        for step in [1, 7, 4096] {
            let (r, body) = fetch(resp, step);
            assert_eq!(r.unwrap().status, 200);
            assert_eq!(body, b"Wikipedia in \r\n\r\nchunks.", "step {step}");
        }
    }

    #[test]
    fn a_non_utf8_header_does_not_fail_the_download() {
        let mut resp = b"HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"".to_vec();
        resp.extend_from_slice(&[0xe9, b'.', b'd', b'b']); // Latin-1
        resp.extend_from_slice(b"\"\r\nContent-Length: 2\r\n\r\nok");
        let (r, body) = fetch(&resp, 4096);
        assert_eq!(r.unwrap().status, 200);
        assert_eq!(body, b"ok");
    }

    #[test]
    fn redirects_and_errors_deliver_no_body() {
        let (r, body) = fetch(b"HTTP/1.1 302 Found\r\nLocation: https://other.example/core.db\r\nContent-Length: 3\r\n\r\nabc", 4096);
        let r = r.unwrap();
        assert_eq!((r.status, r.location.as_deref()), (302, Some("https://other.example/core.db")));
        assert!(body.is_empty());
        let (r, body) = fetch(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found", 4096);
        assert_eq!(r.unwrap().status, 404);
        assert!(body.is_empty());
    }

    #[test]
    fn a_truncated_body_is_an_error() {
        let (r, _) = fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort", 4096);
        assert_eq!(r.err(), Some(Error::Closed));
    }
}
