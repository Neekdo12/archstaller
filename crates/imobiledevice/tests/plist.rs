//! Host-side tests: plist round-trips (binary and XML), PEM/DER cert generation
//! cross-checked with openssl, and mux frame encoding.
use imobiledevice::plist::{self, Value};

fn sample() -> Value {
    Value::dict(vec![
        ("MessageType", Value::str("Connect")),
        ("DeviceID", Value::Int(2)),
        ("PortNumber", Value::Int(62078u16.to_be() as u64)), // htons, like the reference
        ("TrustMe", Value::Bool(true)),
        ("NotThis", Value::Bool(false)),
        ("Payload", Value::data(&[0xde, 0xad, 0xbe, 0xef])),
        (
            "Nested",
            Value::dict(vec![
                ("Inner", Value::Int(42)),
                ("List", Value::Array(vec![Value::str("a"), Value::Int(0x1_0000)])),
            ]),
        ),
        ("Long", Value::str(&"x".repeat(40))), // forces the 0x0f long-form length
    ])
}

#[test]
fn binary_round_trip() {
    let v = sample();
    let bytes = plist::to_binary(&v);
    assert!(bytes.starts_with(b"bplist00"));
    let back = plist::parse(&bytes).expect("parse own bplist");
    assert_eq!(v, back);
}

#[test]
fn xml_round_trip() {
    let v = sample();
    let text = plist::to_xml(&v);
    let back = plist::parse(&text).expect("parse own xml");
    assert_eq!(v, back);
}

#[test]
fn parses_python_plistlib_bplist() {
    // Cross-check the reader against Apple's canonical writer (Python's plistlib).
    let bytes = std::process::Command::new("python3")
        .args([
            "-c",
            "import plistlib,sys; sys.stdout.buffer.write(plistlib.dumps({'MessageType':'Result','Number':0,'Tags':[1,2,300],'D':{'k':'v'},'B':True,'blob':b'\\x01\\x02\\x03'}, fmt=plistlib.FMT_BINARY))",
        ])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    if bytes.is_empty() {
        return; // no python3: skip, like other host-tool tests in this repo
    }
    let v = plist::parse(&bytes).expect("parse plistlib bplist");
    assert_eq!(v.dict_get_str("MessageType"), Some("Result"));
    assert_eq!(v.dict_get_int("Number"), Some(0));
    assert_eq!(v.dict_get_bool("B"), Some(true));
    assert_eq!(v.dict_get_data("blob"), Some(&[1u8, 2, 3][..]));
    let d = v.get("D").unwrap();
    assert_eq!(d.dict_get_str("k"), Some("v"));
    let tags = v.get("Tags").unwrap();
    assert_eq!(tags, &Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(300)]));
}

#[test]
fn parses_python_plistlib_xml() {
    let out = std::process::Command::new("python3")
        .args([
            "-c",
            "import plistlib,sys; sys.stdout.buffer.write(plistlib.dumps({'MessageType':'Result','Number':6}, fmt=plistlib.FMT_XML))",
        ])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    if out.is_empty() {
        return;
    }
    let v = plist::parse(&out).expect("parse plistlib xml");
    assert_eq!(v.dict_get_str("MessageType"), Some("Result"));
    assert_eq!(v.dict_get_int("Number"), Some(6));
}

#[test]
fn mux_frame_header_little_endian() {
    let f = imobiledevice::mux::encode_frame(8, 0xdead, b"xy");
    assert_eq!(f.len(), 18);
    let (len, msg, tag) = imobiledevice::mux::parse_header(&f[..16].try_into().unwrap()).unwrap();
    assert_eq!(len, 18);
    assert_eq!(msg, 8);
    assert_eq!(tag, 0xdead);
    assert_eq!(&f[16..], b"xy");
}

#[test]
fn cert_chain_verified_by_openssl() {
    if std::process::Command::new("openssl").arg("version").output().is_err() {
        return;
    }
    // A stand-in "device" key as PKCS#1 PEM, like lockdownd's DevicePublicKey value.
    let dev = std::process::Command::new("openssl")
        .args(["genrsa", "512"])
        .output()
        .expect("openssl genrsa");
    let pkcs1 = std::process::Command::new("openssl")
        .args(["rsa", "-RSAPublicKey_out"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut p| {
            use std::io::Write;
            p.stdin.as_mut().unwrap().write_all(&dev.stdout)?;
            let out = p.wait_with_output()?;
            Ok(out.stdout)
        })
        .expect("openssl rsa pubkey");
    let now = 1_759_000_000u64; // 2025-09-28
    let rec = imobiledevice::pair::generate_pair_record(&pkcs1, &imobiledevice::pair::uuid4(), now)
        .expect("pair record");

    let dir = std::env::temp_dir();
    let root = dir.join("archstaler-test-root.pem");
    let devc = dir.join("archstaler-test-dev.pem");
    let host = dir.join("archstaler-test-host.pem");
    std::fs::write(&root, &rec.root_certificate).unwrap();
    std::fs::write(&devc, &rec.device_certificate).unwrap();
    std::fs::write(&host, &rec.host_certificate).unwrap();

    // openssl parses the certs (structure check)...
    for f in [&root, &devc, &host] {
        let ok = std::process::Command::new("openssl")
            .args(["x509", "-in"])
            .arg(f)
            .args(["-noout", "-subject", "-issuer"])
            .output()
            .expect("openssl x509");
        assert!(ok.status.success(), "openssl could not parse {f:?}");
    }
    // ...and the signatures verify against the root.
    for f in [&devc, &host] {
        let ok = std::process::Command::new("openssl")
            .args(["verify", "-CAfile"])
            .arg(&root)
            .arg(f)
            .output()
            .expect("openssl verify");
        assert!(ok.status.success(), "openssl verify failed for {f:?}");
    }
    // The root is self-signed.
    let ok = std::process::Command::new("openssl")
        .args(["verify", "-CAfile"])
        .arg(&root)
        .arg(&root)
        .output()
        .expect("openssl verify root");
    assert!(ok.status.success(), "openssl verify failed for the root");
}

#[test]
fn civil_from_unix_spot_check() {
    // 2025-09-28 12:00:00 UTC = 1759060800.
    let (y, mo, d, h, mi, s) = imobiledevice::cert::civil_from_unix(1_759_060_800);
    assert_eq!((y, mo, d, h, mi, s), (2025, 9, 28, 12, 0, 0));
    // 2000-02-29 00:00:00 UTC = 951782400 (leap day).
    let (y, mo, d, ..) = imobiledevice::cert::civil_from_unix(951_782_400);
    assert_eq!((y, mo, d), (2000, 2, 29));
}
