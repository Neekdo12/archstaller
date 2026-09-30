//! lockdownd client over a usbmux channel: plist messages framed as
//! `[u32 big-endian length][bplist body]`. StartSession upgrades the channel to TLS
//! (client-authenticated with the pair record's host certificate), which is why the
//! client is generic over the byte transport.
use crate::cert;
use crate::mux::Channel;
use crate::pair::{self, PairRecord};
use crate::plist::{self, Value};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use hal::{Error, Result};

const LABEL: &str = "archstaler";
const PROTOCOL_VERSION: &str = "2";

/// The lockdownd service relaying Personal Hotspot traffic.
///
/// UNVERIFIED: per `docs/iphone-tethering.md` step 5, service names have changed across
/// iOS versions and this must be confirmed against a real device with USB captures
/// before being trusted. Update this constant once confirmed.
pub const HOTSPOT_SERVICE: &str = "com.apple.mobile.insecure_lockdown";

/// Byte transport for lockdownd messages: the plain mux channel, or TLS on top of it.
pub trait Io {
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;
    /// Fills `buf` completely or fails (timeout after `timeout_ms` at the latest).
    fn read_exact(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<()>;
}

impl Io for Channel {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        Channel::write_all(self, buf)
    }
    fn read_exact(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<()> {
        Channel::read_exact(self, buf, timeout_ms)
    }
}

impl<S: net::Stream> Io for net::tls::TlsStream<S> {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        net::Stream::write_all(self, buf).map_err(|_| Error::Io)
    }
    fn read_exact(&mut self, buf: &mut [u8], _timeout_ms: u64) -> Result<()> {
        let mut got = 0usize;
        while got < buf.len() {
            match net::Stream::read(self, &mut buf[got..]) {
                Ok(0) => return Err(Error::Io),
                Ok(n) => got += n,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
}

fn frame_send(io: &mut impl Io, v: &Value) -> Result<()> {
    let body = plist::to_binary(v);
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    io.write_all(&frame)
}

fn frame_recv(io: &mut impl Io, timeout_ms: u64) -> Result<Value> {
    let mut lenb = [0u8; 4];
    io.read_exact(&mut lenb, timeout_ms)?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len == 0 || len > 1 << 20 {
        return Err(Error::Io);
    }
    let mut body = alloc::vec![0u8; len];
    io.read_exact(&mut body, timeout_ms)?;
    plist::parse(&body).map_err(|_| Error::Io)
}

/// Checks a reply for lockdownd's `{"Request": ..., "Result": "Success"}` /
/// `{"Error": ...}` convention; returns the reply on success.
fn checked(reply: Value, request: &str) -> Result<Value> {
    if reply.dict_get_str("Request") == Some(request) && reply.dict_get_str("Result") == Some("Success") {
        return Ok(reply);
    }
    if let Some(e) = reply.dict_get_str("Error") {
        return Err(match e {
            // The Trust dialog is up; the caller retries until the user taps it.
            "PairingDialogResponsePending" => Error::Timeout,
            "UserDeniedPairing" | "PasswordProtected" => Error::Unsupported,
            _ => Error::Io,
        });
    }
    Err(Error::Io)
}

pub struct Lockdown<T: Io> {
    io: T,
}

impl Lockdown<Channel> {
    /// Connects and verifies the service type via QueryType.
    pub fn new(chan: Channel) -> Result<Lockdown<Channel>> {
        let mut ld = Lockdown { io: chan };
        let reply = ld.request(&Value::dict(alloc::vec![("Request", Value::str("QueryType"))]))?;
        match reply.dict_get_str("Type") {
            Some("com.apple.mobile.lockdown") => Ok(ld),
            _ => Err(Error::Unsupported),
        }
    }

    pub fn into_channel(self) -> Channel {
        self.io
    }
}

impl<T: Io> Lockdown<T> {
    /// Wraps an already-established transport without a QueryType check (used after
    /// the TLS upgrade, mid-conversation).
    pub fn from_io(io: T) -> Lockdown<T> {
        Lockdown { io }
    }

    /// One framed request/response round trip. Adds Label (and ProtocolVersion for
    /// pairing verbs), matching the reference flow.
    pub fn request(&mut self, v: &Value) -> Result<Value> {
        let req = match v.clone() {
            Value::Dict(mut kv) => {
                kv.push(("Label".into(), Value::str(LABEL)));
                let pairing = v
                    .dict_get_str("Request")
                    .is_some_and(|r| r == "Pair" || r == "ValidatePair" || r == "Unpair");
                if pairing && kv.iter().all(|(k, _)| k != "ProtocolVersion") {
                    kv.push(("ProtocolVersion".into(), Value::str(PROTOCOL_VERSION)));
                }
                Value::Dict(kv)
            }
            other => other,
        };
        frame_send(&mut self.io, &req)?;
        frame_recv(&mut self.io, 5000)
    }

    /// lockdownd GetValue; returns the raw value node.
    pub fn get_value(&mut self, domain: Option<&str>, key: &str) -> Result<Value> {
        let mut req = alloc::vec![
            ("Key", Value::str(key)),
            ("Request", Value::str("GetValue")),
        ];
        if let Some(d) = domain {
            req.push(("Domain", Value::str(d)));
        }
        let reply = self.request(&Value::dict(req))?;
        reply.get("Value").cloned().ok_or(Error::Io)
    }

    /// Full pairing flow: read the device public key, generate a pair record, send Pair.
    /// Surfaces `Err(Error::Timeout)` while the Trust dialog is pending — the caller
    /// retries until the user taps Trust (or gives up).
    pub fn pair(&mut self, system_buid: &str, unix_time: u64) -> Result<PairRecord> {
        let pubkey = self.get_value(None, "DevicePublicKey")?;
        let pubkey_pem = pubkey.as_data().ok_or(Error::Io)?.to_vec();
        let record = pair::generate_pair_record(&pubkey_pem, system_buid, unix_time)?;

        let req = Value::dict(alloc::vec![
            ("PairRecord", pair::record_plist(&record)),
            ("Request", Value::str("Pair")),
            ("ProtocolVersion", Value::str(PROTOCOL_VERSION)),
            ("PairingOptions", Value::dict(alloc::vec![("ExtendedPairingErrors", Value::Bool(true))])),
        ]);
        let reply = checked(self.request(&req)?, "Pair")?;
        let mut record = record;
        if let Some(bag) = reply.dict_get_data("EscrowBag") {
            record.escrow_bag = Some(bag.to_vec());
        }
        Ok(record)
    }

    /// Validates an existing pair record with the device.
    pub fn validate_pair(&mut self, record: &PairRecord) -> Result<()> {
        let req = Value::dict(alloc::vec![
            ("PairRecord", pair::record_plist(record)),
            ("Request", Value::str("ValidatePair")),
        ]);
        checked(self.request(&req)?, "ValidatePair")?;
        Ok(())
    }

    /// Starts a session; the response must have `EnableSessionSSL: true` (the caller
    /// then TLS-wraps the channel). Returns the SessionID.
    pub fn start_session(&mut self, record: &PairRecord) -> Result<String> {
        let req = Value::dict(alloc::vec![
            ("HostID", Value::str(&record.host_id)),
            ("Request", Value::str("StartSession")),
            ("SystemBUID", Value::str(&record.system_buid)),
        ]);
        let reply = checked(self.request(&req)?, "StartSession")?;
        match reply.dict_get_bool("EnableSessionSSL") {
            Some(true) => reply.dict_get_str("SessionID").map(String::from).ok_or(Error::Io),
            _ => Err(Error::Unsupported),
        }
    }

    pub fn stop_session(&mut self, session_id: &str) -> Result<()> {
        let req = Value::dict(alloc::vec![
            ("Request", Value::str("StopSession")),
            ("SessionID", Value::str(session_id)),
        ]);
        checked(self.request(&req)?, "StopSession")?;
        Ok(())
    }

    /// StartService within an open (TLS) session; returns the device port to Connect
    /// to and whether that service wants its own TLS layer.
    pub fn start_service(&mut self, service: &str) -> Result<(u16, bool)> {
        let req = Value::dict(alloc::vec![
            ("Request", Value::str("StartService")),
            ("Service", Value::str(service)),
        ]);
        let reply = checked(self.request(&req)?, "StartService")?;
        let port = reply.dict_get_int("Port").ok_or(Error::Io)? as u16;
        let ssl = reply.dict_get_bool("EnableServiceSSL").unwrap_or(false);
        Ok((port, ssl))
    }
}

// ---------------------------------------------------------------------------
// TLS session (StartSession wraps the lockdownd channel with mutual TLS)
// ---------------------------------------------------------------------------

/// Server verifier pinning the exact device certificate from the pair record.
/// The chain is built from our own self-signed root, so web PKI does not apply.
#[derive(Debug)]
struct DeviceCertVerifier {
    device_cert_der: Vec<u8>,
}

impl rustls::client::danger::ServerCertVerifier for DeviceCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> core::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.device_cert_der {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(String::from("unexpected device certificate")))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> core::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> core::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        alloc::vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[derive(Debug)]
struct WallClock(fn() -> u64);

impl rustls::time_provider::TimeProvider for WallClock {
    fn current_time(&self) -> Option<rustls::pki_types::UnixTime> {
        Some(rustls::pki_types::UnixTime::since_unix_epoch(core::time::Duration::from_secs((self.0)())))
    }
}

/// Extracts the DER from a PEM certificate.
fn cert_der_from_pem(pem: &[u8]) -> Result<Vec<u8>> {
    let (label, der) = cert::unpem(pem).map_err(|_| Error::Io)?;
    if label != "CERTIFICATE" {
        return Err(Error::Io);
    }
    Ok(der)
}

/// Builds the client config for a lockdownd session: no root store (the device cert is
/// pinned instead), client certificate = pair record host cert + our root.
pub fn session_config(record: &PairRecord, unix_time: fn() -> u64) -> Result<Arc<rustls::ClientConfig>> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let verifier = Arc::new(DeviceCertVerifier {
        device_cert_der: cert_der_from_pem(&record.device_certificate)?,
    });
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(record.host_key_pkcs8.clone()));
    let chain = alloc::vec![
        CertificateDer::from(record.host_cert_der.clone()),
        CertificateDer::from(record.root_cert_der.clone()),
    ];
    let cfg = rustls::ClientConfig::builder_with_details(
        Arc::new(rustls_rustcrypto::provider()),
        Arc::new(WallClock(unix_time)),
    )
    .with_safe_default_protocol_versions()
    .map_err(|_| Error::Unsupported)?
    .dangerous()
    .with_custom_certificate_verifier(verifier)
    .with_client_auth_cert(chain, key)
    .map_err(|_| Error::Io)?;
    Ok(Arc::new(cfg))
}
