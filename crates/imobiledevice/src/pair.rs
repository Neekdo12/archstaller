//! Lockdownd pair record generation: RSA-2048 host/root keys, self-signed certificates,
//! HostID/SystemBUID. The flow and field names follow the public lockdownd protocol
//! documentation (libimobiledevice's `lockdownd_pair`, used as a reference only).
use crate::cert;
use alloc::string::String;
use alloc::vec::Vec;
use hal::{Error, Result};

/// A pair record as exchanged with lockdownd (private keys are never sent).
pub struct PairRecord {
    /// PEM of the host-generated device certificate (device pubkey, signed by root).
    pub device_certificate: Vec<u8>,
    pub host_certificate: Vec<u8>,
    pub root_certificate: Vec<u8>,
    pub host_id: String,
    pub system_buid: String,
    /// Filled from the Pair response, if the device sends one.
    pub escrow_bag: Option<Vec<u8>>,
    // Never serialized into the plist sent to the device:
    pub host_key_pkcs8: Vec<u8>,
    /// DER of the host + root certs, for the TLS client certificate chain.
    pub host_cert_der: Vec<u8>,
    pub root_cert_der: Vec<u8>,
}

/// RDRAND-backed RNG implementing rand_core 0.6's traits for the `rsa` crate.
struct KernelRng;

impl rand_core::RngCore for KernelRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        if getrandom::getrandom(dest).is_err() {
            // RDRAND unavailable; a weak fallback for key material is not acceptable.
            panic!("getrandom (RDRAND) unavailable for RSA key generation");
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> core::result::Result<(), rand_core::Error> {
        getrandom::getrandom(dest)
            .map_err(|_| rand_core::Error::from(core::num::NonZeroU32::new(1).unwrap()))
    }
}

impl rand_core::CryptoRng for KernelRng {}

/// Random RFC 4122 version 4 UUID in text form.
pub fn uuid4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    alloc::format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
    .to_uppercase()
}

fn rsa_keypair() -> Result<(rsa::RsaPrivateKey, rsa::RsaPublicKey)> {
    let privkey = rsa::RsaPrivateKey::new(&mut KernelRng, 2048).map_err(|_| Error::Io)?;
    let pubkey = rsa::RsaPublicKey::from(&privkey);
    Ok((privkey, pubkey))
}

/// Generates host identity keys and certs. `device_pubkey_pem` is the device's public
/// key as returned by lockdownd's `GetValue(DevicePublicKey)`; the host signs it into
/// the DeviceCertificate of the pair record (this is how the reference flow works: the
/// device never generates its own certificate).
pub fn generate_pair_record(
    device_pubkey_pem: &[u8],
    system_buid: &str,
    unix_time: u64,
) -> Result<PairRecord> {
    let (root_key, root_pub) = rsa_keypair()?;
    let (host_key, host_pub) = rsa_keypair()?;

    let now = cert::civil_from_unix(unix_time);
    let later = (now.0 + 10, now.1, now.2, now.3, now.4, now.5);
    let mut serial = [0u8; 8];
    let _ = getrandom::getrandom(&mut serial);

    let root_spki = cert::spki_from_rsa(&root_pub).map_err(|_| Error::Io)?;
    let root_der = cert::build_cert(
        "archstaler-root",
        &root_key,
        "archstaler-root",
        &root_spki,
        true,
        &serial,
        now,
        later,
    )
    .map_err(|_| Error::Io)?;

    let host_spki = cert::spki_from_rsa(&host_pub).map_err(|_| Error::Io)?;
    serial[7] = serial[7].wrapping_add(1);
    let host_der = cert::build_cert(
        "archstaler-root",
        &root_key,
        "archstaler-host",
        &host_spki,
        false,
        &serial,
        now,
        later,
    )
    .map_err(|_| Error::Io)?;

    let device_spki = cert::spki_from_pem(device_pubkey_pem).map_err(|_| Error::Io)?;
    serial[7] = serial[7].wrapping_add(1);
    let device_der = cert::build_cert(
        "archstaler-root",
        &root_key,
        "archstaler-device",
        &device_spki,
        false,
        &serial,
        now,
        later,
    )
    .map_err(|_| Error::Io)?;

    use rsa::pkcs8::EncodePrivateKey;
    let host_key_pkcs8 = host_key.to_pkcs8_der().map_err(|_| Error::Io)?.as_bytes().to_vec();

    Ok(PairRecord {
        device_certificate: cert::pem("CERTIFICATE", &device_der),
        host_certificate: cert::pem("CERTIFICATE", &host_der),
        root_certificate: cert::pem("CERTIFICATE", &root_der),
        host_id: uuid4(),
        system_buid: String::from(system_buid),
        escrow_bag: None,
        host_key_pkcs8,
        host_cert_der: host_der,
        root_cert_der: root_der,
    })
}

/// The pair record as a plist dict for a Pair/ValidatePair request: certificates and
/// IDs only, never private keys.
pub fn record_plist(r: &PairRecord) -> crate::plist::Value {
    use crate::plist::Value;
    let mut kv = alloc::vec![
        ("DeviceCertificate", Value::data(&r.device_certificate)),
        ("HostCertificate", Value::data(&r.host_certificate)),
        ("HostID", Value::str(&r.host_id)),
        ("RootCertificate", Value::data(&r.root_certificate)),
        ("SystemBUID", Value::str(&r.system_buid)),
    ];
    if let Some(bag) = &r.escrow_bag {
        kv.push(("EscrowBag", Value::data(bag)));
    }
    Value::dict(kv)
}
