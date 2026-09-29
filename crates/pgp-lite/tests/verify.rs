use pgp_lite::{Error, Keyring, Verifier};
use std::path::PathBuf;

const NOW: u64 = 1_790_000_000; // 2026-09-21

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn keyring() -> Keyring {
    let blob = std::fs::read(dir().join("../../target/keyring.bin")).expect("run `cargo xtask keyring` first");
    Keyring::from_bytes(&blob).expect("keyring blob parses")
}

fn check(pkg: &str, mutate: impl FnOnce(&mut Vec<u8>), ring: &Keyring) -> Result<[u8; 20], Error> {
    let fx = dir().join("tests/fixtures");
    let mut data = std::fs::read(fx.join(pkg)).unwrap();
    let sig = std::fs::read(fx.join(format!("{pkg}.sig"))).unwrap();
    mutate(&mut data);
    let mut v = Verifier::from_packet(&sig)?;
    for chunk in data.chunks(1000) {
        v.update(chunk);
    }
    v.finish(ring, NOW)
}

const RSA_PKG: &str = "pambase-20260616-1-any.pkg.tar.zst";
const ED_PKG: &str = "systemd-sysvcompat-261.1-1-x86_64.pkg.tar.zst";

#[test]
fn rsa_signature_verifies() {
    check(RSA_PKG, |_| {}, &keyring()).unwrap();
}

#[test]
fn eddsa_signature_verifies() {
    check(ED_PKG, |_| {}, &keyring()).unwrap();
}

#[test]
fn tampered_data_is_rejected() {
    for pkg in [RSA_PKG, ED_PKG] {
        let r = check(pkg, |d| d[100] ^= 1, &keyring());
        assert_eq!(r, Err(Error::BadSignature), "{pkg}");
    }
}

#[test]
fn unknown_key_is_reported() {
    let r = check(RSA_PKG, |_| {}, &Keyring::default());
    assert_eq!(r, Err(Error::UnknownKey));
}

#[test]
fn expired_key_is_rejected() {
    let mut ring = keyring();
    for k in &mut ring.keys {
        k.expires = Some(1);
    }
    assert_eq!(check(ED_PKG, |_| {}, &ring), Err(Error::Expired));
}

#[test]
fn base64_roundtrip() {
    assert_eq!(pgp_lite::base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(pgp_lite::base64_decode("aGVs\nbG8h").unwrap(), b"hello!");
    assert!(pgp_lite::base64_decode("a$b").is_err());
}
