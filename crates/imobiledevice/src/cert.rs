//! Minimal DER encoder and X.509 certificate generation for the lockdownd pairing
//! record: a self-signed root CA cert, and leaf certs (host, device) signed by it.
//! RSA-2048 keys, SHA-256 signatures. Hand-written per ITU-T X.509 — small and fixed
//! in shape, exactly what Apple's lockdownd expects in a pair record.
use crate::xml::base64_encode;
use alloc::vec::Vec;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Crypto,
    Malformed,
}

// ---------------------------------------------------------------------------
// DER building blocks
// ---------------------------------------------------------------------------

fn tlv(out: &mut Vec<u8>, tag: u8, content: &[u8]) {
    out.push(tag);
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else if content.len() <= 0xff {
        out.push(0x81);
        out.push(content.len() as u8);
    } else {
        out.push(0x82);
        out.push((content.len() >> 8) as u8);
        out.push(content.len() as u8);
    }
    out.extend_from_slice(content);
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    let mut c = Vec::new();
    for p in parts {
        c.extend_from_slice(p);
    }
    let mut out = Vec::new();
    tlv(&mut out, 0x30, &c);
    out
}

fn oid(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    tlv(&mut out, 0x06, bytes);
    out
}

fn null() -> Vec<u8> {
    alloc::vec![0x05, 0x00]
}

fn boolean(v: bool) -> Vec<u8> {
    alloc::vec![0x01, 0x01, if v { 0xff } else { 0x00 }]
}

fn integer(bytes: &[u8]) -> Vec<u8> {
    // strip leading zeros, keep a leading 0x00 if the top bit would be set
    let mut b = bytes;
    while b.len() > 1 && b[0] == 0 {
        b = &b[1..];
    }
    let mut c = Vec::with_capacity(b.len() + 1);
    if b[0] & 0x80 != 0 {
        c.push(0);
    }
    c.extend_from_slice(b);
    let mut out = Vec::new();
    tlv(&mut out, 0x02, &c);
    out
}

fn utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    tlv(&mut out, 0x0c, s.as_bytes());
    out
}

fn utc_time(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Vec<u8> {
    let text = alloc::format!("{:02}{:02}{:02}{:02}{:02}{:02}Z", y % 100, mo, d, h, mi, s);
    let mut out = Vec::new();
    tlv(&mut out, 0x17, text.as_bytes());
    out
}

fn bit_string(content: &[u8]) -> Vec<u8> {
    let mut c = alloc::vec![0u8]; // no unused bits
    c.extend_from_slice(content);
    let mut out = Vec::new();
    tlv(&mut out, 0x03, &c);
    out
}

fn explicit(n: u8, content: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    tlv(&mut out, 0xa0 | n, &content);
    out
}

fn octet_string(content: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    tlv(&mut out, 0x04, content);
    out
}

// ---------------------------------------------------------------------------
// OIDs
// ---------------------------------------------------------------------------

const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_SHA256_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];

fn rsa_algorithm() -> Vec<u8> {
    seq(&[&oid(OID_RSA_ENCRYPTION), &null()])
}

fn signature_algorithm() -> Vec<u8> {
    seq(&[&oid(OID_SHA256_WITH_RSA), &null()])
}

fn name(cn: &str) -> Vec<u8> {
    let rdn = seq(&[&oid(OID_CN), &utf8(cn)]);
    let mut set = Vec::new();
    tlv(&mut set, 0x31, &rdn);
    seq(&[&set])
}

/// SubjectPublicKeyInfo for an RSA key given as PKCS#1 (SEQUENCE { n, e }).
pub fn spki_from_pkcs1(pkcs1: &[u8]) -> Vec<u8> {
    seq(&[&rsa_algorithm(), &bit_string(pkcs1)])
}

/// SubjectPublicKeyInfo for an `rsa` crate public key.
pub fn spki_from_rsa(key: &rsa::RsaPublicKey) -> Result<Vec<u8>> {
    use rsa::traits::PublicKeyParts;
    let n = integer(&key.n().to_bytes_be());
    let e = integer(&key.e().to_bytes_be());
    Ok(spki_from_pkcs1(&seq(&[&n, &e])))
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

/// Builds and signs a certificate: `subject_cn` with `subject_spki`, issued by
/// `issuer_cn`/`issuer_key`. `ca` sets the basicConstraints CA flag.
pub fn build_cert(
    issuer_cn: &str,
    issuer_key: &rsa::RsaPrivateKey,
    subject_cn: &str,
    subject_spki: &[u8],
    ca: bool,
    serial: &[u8; 8],
    not_before: (u32, u32, u32, u32, u32, u32),
    not_after: (u32, u32, u32, u32, u32, u32),
) -> Result<Vec<u8>> {
    let (y0, mo0, d0, h0, mi0, s0) = not_before;
    let (y1, mo1, d1, h1, mi1, s1) = not_after;
    let validity = seq(&[&utc_time(y0, mo0, d0, h0, mi0, s0), &utc_time(y1, mo1, d1, h1, mi1, s1)]);
    // basicConstraints { cA }
    let bc = seq(&[&boolean(ca)]);
    let ext = seq(&[&oid(OID_BASIC_CONSTRAINTS), &boolean(true), &octet_string(&bc)]);
    let exts = explicit(3, seq(&[&ext]));
    let tbs = seq(&[
        &explicit(0, integer(&[2])), // v3
        &integer(serial),
        &signature_algorithm(),
        &name(issuer_cn),
        &validity,
        &name(subject_cn),
        subject_spki,
        &exts,
    ]);

    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(&tbs);
    let padding = rsa::Pkcs1v15Sign::new::<sha2::Sha256>();
    let sig = issuer_key.sign(padding, &digest).map_err(|_| Error::Crypto)?;

    Ok(seq(&[&tbs, &signature_algorithm(), &bit_string(&sig)]))
}

/// PEM armor for a DER blob.
pub fn pem(label: &str, der: &[u8]) -> Vec<u8> {
    let b64 = base64_encode(der);
    let mut out = alloc::format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        out.push_str(core::str::from_utf8(line).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(&alloc::format!("-----END {label}-----\n"));
    out.into_bytes()
}

/// Strips PEM armor; returns `(label, der)`.
pub fn unpem(text: &[u8]) -> Result<(alloc::string::String, Vec<u8>)> {
    let s = core::str::from_utf8(text).map_err(|_| Error::Malformed)?;
    let s = s.trim_start();
    let rest = s.strip_prefix("-----BEGIN ").ok_or(Error::Malformed)?;
    let end = rest.find("-----").ok_or(Error::Malformed)?;
    let label = alloc::string::String::from(&rest[..end]);
    let body_start = end + 5 + 1; // skip "-----\n"
    let body_end = rest.find("-----END").ok_or(Error::Malformed)?;
    let der = pgp_lite::base64_decode(&rest[body_start..body_end]).map_err(|_| Error::Malformed)?;
    Ok((label, der))
}

/// Wraps a PEM public key from the device (PKCS#1 "RSA PUBLIC KEY" or SPKI "PUBLIC KEY")
/// into a SubjectPublicKeyInfo DER.
pub fn spki_from_pem(pem_data: &[u8]) -> Result<Vec<u8>> {
    let (label, der) = unpem(pem_data)?;
    if label == "RSA PUBLIC KEY" {
        Ok(spki_from_pkcs1(&der))
    } else if label == "PUBLIC KEY" {
        Ok(der)
    } else {
        Err(Error::Malformed)
    }
}

/// UTC broken-down time from a unix timestamp (days/civil algorithm, no leap seconds).
pub fn civil_from_unix(t: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (t / 86400) as i64;
    let secs = t % 86400;
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (y + if m <= 2 { 1 } else { 0 }) as u32;
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32, (secs % 60) as u32)
}
