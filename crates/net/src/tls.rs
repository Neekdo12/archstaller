//! TLS 1.2/1.3 client over any [`Stream`], using rustls' unbuffered API and the RustCrypto provider.
use crate::{Error, Result, Stream};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::{ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::unbuffered::{ConnectionState, EncodeError, EncryptError, UnbufferedStatus};
use rustls::{ClientConfig, RootCertStore};

/// TSC cycles spent decrypting/processing TLS records, and waiting for bytes from the transport.
/// Diagnostics for the hardware test's speed report.
pub static CRYPTO_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static IO_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn add(counter: &core::sync::atomic::AtomicU64, since: u64) {
    counter.fetch_add(tsc().wrapping_sub(since), core::sync::atomic::Ordering::Relaxed);
}

#[derive(Debug)]
struct WallClock(fn() -> u64);

impl TimeProvider for WallClock {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs((self.0)())))
    }
}

/// Builds a client config trusting the bundled Mozilla roots. `unix_time` supplies the wall clock
/// for certificate validity checks.
pub fn client_config(unix_time: fn() -> u64) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = ClientConfig::builder_with_details(Arc::new(rustls_rustcrypto::provider()), Arc::new(WallClock(unix_time)))
        .with_safe_default_protocol_versions()
        .expect("provider supports default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(cfg)
}

pub struct TlsStream<S: Stream> {
    inner: S,
    conn: UnbufferedClientConnection,
    /// Received TLS bytes not yet consumed by rustls.
    incoming: Vec<u8>,
    /// Scratch for records to send.
    outgoing: Vec<u8>,
    /// Decrypted data not yet handed to the caller.
    plaintext: Vec<u8>,
    plain_pos: usize,
    eof: bool,
}

enum Step {
    Continue,
    NeedData,
    HandshakeDone,
    Eof,
}

const MAX_RECORD: usize = 16 * 1024;

impl<S: Stream> TlsStream<S> {
    /// Connects and completes the handshake.
    pub fn connect(inner: S, config: Arc<ClientConfig>, server_name: &str) -> Result<Self> {
        let name = ServerName::try_from(String::from(server_name)).map_err(|_| Error::Http("bad server name"))?;
        let conn = UnbufferedClientConnection::new(config, name).map_err(|_| Error::Http("tls setup"))?;
        let mut t = TlsStream {
            inner,
            conn,
            incoming: Vec::new(),
            outgoing: alloc::vec![0; MAX_RECORD + 512],
            plaintext: Vec::new(),
            plain_pos: 0,
            eof: false,
        };
        loop {
            match t.step(true)? {
                Step::HandshakeDone => return Ok(t),
                Step::Eof => return Err(Error::Closed),
                Step::NeedData => t.fill()?,
                Step::Continue => {}
            }
        }
    }

    fn fill(&mut self) -> Result<()> {
        let mut tmp = [0u8; 16384];
        let t0 = tsc();
        let n = self.inner.read(&mut tmp);
        add(&IO_TSC, t0);
        let n = n?;
        if n == 0 {
            self.eof = true;
        }
        self.incoming.extend_from_slice(&tmp[..n]);
        Ok(())
    }

    /// Runs the state machine once.
    fn step(&mut self, handshaking: bool) -> Result<Step> {
        let t0 = tsc();
        let UnbufferedStatus { discard, state } = self.conn.process_tls_records(&mut self.incoming);
        let state = state.map_err(|_| Error::Http("tls error"))?;
        let mut to_send = 0usize;
        let mut plain: Vec<u8> = Vec::new();
        let mut extra_discard = 0usize;
        let step = match state {
            ConnectionState::EncodeTlsData(mut e) => {
                match e.encode(&mut self.outgoing) {
                    Ok(n) => to_send = n,
                    Err(EncodeError::InsufficientSize(s)) => {
                        self.outgoing.resize(s.required_size, 0);
                        // Retry after resizing; the chunk is kept by rustls on this error.
                        match e.encode(&mut self.outgoing) {
                            Ok(n) => to_send = n,
                            Err(_) => return Err(Error::Http("tls encode")),
                        }
                    }
                    Err(_) => return Err(Error::Http("tls encode")),
                }
                Step::Continue
            }
            ConnectionState::TransmitTlsData(t) => {
                t.done();
                Step::Continue
            }
            ConnectionState::BlockedHandshake => Step::NeedData,
            ConnectionState::WriteTraffic(_) => {
                if handshaking {
                    Step::HandshakeDone
                } else {
                    Step::NeedData
                }
            }
            ConnectionState::ReadTraffic(mut r) => {
                while let Some(rec) = r.next_record() {
                    let rec = rec.map_err(|_| Error::Http("tls record"))?;
                    plain.extend_from_slice(rec.payload);
                    extra_discard += rec.discard;
                }
                Step::Continue
            }
            ConnectionState::PeerClosed | ConnectionState::Closed => Step::Eof,
            _ => Step::Continue,
        };
        add(&CRYPTO_TSC, t0);
        let total_discard = discard + extra_discard;
        self.incoming.drain(..total_discard.min(self.incoming.len()));
        if to_send > 0 {
            self.inner.write_all(&self.outgoing[..to_send])?;
        }
        if !plain.is_empty() {
            self.plaintext.extend_from_slice(&plain);
        }
        Ok(step)
    }
}

impl<S: Stream> Stream for TlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        loop {
            if self.plain_pos < self.plaintext.len() {
                let n = (self.plaintext.len() - self.plain_pos).min(buf.len());
                buf[..n].copy_from_slice(&self.plaintext[self.plain_pos..self.plain_pos + n]);
                self.plain_pos += n;
                if self.plain_pos == self.plaintext.len() {
                    self.plaintext.clear();
                    self.plain_pos = 0;
                }
                return Ok(n);
            }
            match self.step(false)? {
                Step::Eof => return Ok(0),
                Step::NeedData => {
                    if self.eof {
                        return Ok(0); // peer closed without close_notify
                    }
                    self.fill()?;
                }
                Step::Continue | Step::HandshakeDone => {}
            }
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        for chunk in buf.chunks(MAX_RECORD) {
            let n = loop {
                let UnbufferedStatus { state, .. } = self.conn.process_tls_records(&mut []);
                match state.map_err(|_| Error::Http("tls error"))? {
                    ConnectionState::WriteTraffic(mut w) => match w.encrypt(chunk, &mut self.outgoing) {
                        Ok(n) => break n,
                        Err(EncryptError::InsufficientSize(s)) => self.outgoing.resize(s.required_size, 0),
                        Err(_) => return Err(Error::Http("tls encrypt")),
                    },
                    ConnectionState::EncodeTlsData(mut e) => {
                        let n = e.encode(&mut self.outgoing).map_err(|_| Error::Http("tls encode"))?;
                        self.inner.write_all(&self.outgoing[..n])?;
                    }
                    ConnectionState::TransmitTlsData(t) => t.done(),
                    _ => return Err(Error::Closed),
                }
            };
            self.inner.write_all(&self.outgoing[..n])?;
        }
        Ok(())
    }
}
