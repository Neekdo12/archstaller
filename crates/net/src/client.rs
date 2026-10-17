//! `http://` and `https://` GET with redirect following.
use crate::http::{self, Response};
use crate::tls::{self, TlsStream};
use crate::{Error, Result, Stack, Stream};
use alloc::string::String;
use alloc::sync::Arc;
use rustls::ClientConfig;

const MAX_REDIRECTS: usize = 5;

pub struct Client {
    tls: Arc<ClientConfig>,
}

struct Url<'a> {
    https: bool,
    host: &'a str,
    port: u16,
    path: &'a str,
}

fn parse(url: &str) -> Result<Url<'_>> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(Error::Http("unsupported URL scheme"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at_checked(i).unwrap_or((rest, "/")),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_e| Error::Http("bad port"))?),
        None => (authority, if https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(Error::Http("empty host"));
    }
    Ok(Url {
        https,
        host,
        port,
        path,
    })
}

/// The URL a `Location` header points to, seen from `path` on `host` (which includes a non-default port).
fn redirect_target(https: bool, host: &str, path: &str, loc: String) -> String {
    let scheme = if https { "https" } else { "http" };
    if loc.contains("://") {
        loc
    } else if let Some(rest) = loc.strip_prefix("//") {
        alloc::format!("{scheme}://{rest}")
    } else if loc.starts_with('/') {
        alloc::format!("{scheme}://{host}{loc}")
    } else {
        // Relative to the directory of the current path (RFC 3986 reference resolution, simplified).
        let dir = path
            .get(..path.rfind('/').map_or(0, |i| i + 1))
            .unwrap_or("");
        alloc::format!("{scheme}://{host}{dir}{loc}")
    }
}

impl Client {
    pub fn new(unix_time: fn() -> u64) -> Client {
        Client {
            tls: tls::client_config(unix_time),
        }
    }

    /// GET `url`, following redirects, delivering the body of the final 2xx response to `sink`.
    /// Returns the final response (status may be an error status, in which case no body was
    /// delivered).
    pub fn get(
        &self,
        stack: &mut Stack,
        url: &str,
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<Response> {
        let mut current = String::from(url);
        for _ in 0..=MAX_REDIRECTS {
            let u = parse(&current)?;
            let now = stack.clock();
            let t0 = now();
            let ip = stack.resolve(u.host, 8000).inspect_err(|e| {
                hal::info!(
                    "net: DNS lookup of {} failed after {} ms: {e:?}",
                    u.host,
                    now() - t0
                );
            })?;
            hal::log!(
                "net: {} is {}.{}.{}.{} ({} ms)",
                u.host,
                ip[0],
                ip[1],
                ip[2],
                ip[3],
                now() - t0
            );
            let host_header = if (u.https && u.port == 443) || (!u.https && u.port == 80) {
                String::from(u.host)
            } else {
                alloc::format!("{}:{}", u.host, u.port)
            };
            let t1 = now();
            let conn = stack.connect(ip, u.port, 8000).inspect_err(|e| {
                hal::info!(
                    "net: TCP connect to {}:{} failed after {} ms: {e:?}",
                    u.host,
                    u.port,
                    now() - t1
                );
            })?;
            let resp = if u.https {
                let t2 = now();
                let mut t =
                    TlsStream::connect(conn, self.tls.clone(), u.host).inspect_err(|e| {
                        hal::info!("net: TLS handshake with {} failed: {e:?}", u.host);
                    })?;
                hal::log!(
                    "net: TCP connected in {} ms, TLS handshake {} ms",
                    t2 - t1,
                    now() - t2
                );
                let t3 = now();
                let mut got = 0usize;
                let r = http::get(&mut t, &host_header, u.path, &mut |d| {
                    got += d.len();
                    sink(d)
                });
                match &r {
                    Ok(resp) => hal::log!(
                        "net: {} HTTP {}, {} body bytes in {} ms",
                        u.path,
                        resp.status,
                        got,
                        now() - t3
                    ),
                    Err(e) => hal::info!(
                        "net: {} failed after {} body bytes, {} ms: {e:?}",
                        u.path,
                        got,
                        now() - t3
                    ),
                }
                r?
            } else {
                let mut c = conn;
                let r = http::get(&mut c, &host_header, u.path, sink)?;
                let _res = c.read(&mut []);
                r
            };
            if matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
                let loc = resp
                    .location
                    .ok_or(Error::Http("redirect without Location"))?;
                current = redirect_target(u.https, &host_header, u.path, loc);
                continue;
            }
            return Ok(resp);
        }
        Err(Error::Http("too many redirects"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets() {
        let t = |https, loc: &str| {
            redirect_target(
                https,
                "mirror.example:8443",
                "/core/os/x86_64/core.db",
                loc.into(),
            )
        };
        assert_eq!(t(true, "http://other.example/x"), "http://other.example/x");
        assert_eq!(
            t(true, "//cdn.example/core.db"),
            "https://cdn.example/core.db"
        );
        assert_eq!(
            t(false, "/archlinux/core.db"),
            "http://mirror.example:8443/archlinux/core.db"
        );
        assert_eq!(
            t(true, "core.db.tar.gz"),
            "https://mirror.example:8443/core/os/x86_64/core.db.tar.gz"
        );
    }
}
