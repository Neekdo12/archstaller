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
                (
                    "List",
                    Value::Array(vec![Value::str("a"), Value::Int(0x1_0000)]),
                ),
            ]),
        ),
        ("Long", Value::str(&"x".repeat(40))), // forces the 0x0f long-form length
    ])
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
    assert_eq!(v.get("B").and_then(|b| b.as_bool()), Some(true));
    assert_eq!(v.dict_get_data("blob"), Some(&[1u8, 2, 3][..]));
    let d = v.get("D").unwrap();
    assert_eq!(d.dict_get_str("k"), Some("v"));
    let tags = v.get("Tags").unwrap();
    assert_eq!(
        tags,
        &Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(300)])
    );
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
fn mux_version_packet_uses_the_short_header() {
    use imobiledevice::mux::encode_packet;
    let mut vh = [0u8; 12];
    vh[3] = 2; // major 2, minor 0, padding 0
    let p = encode_packet(0, 0, 0, 0xffff, &vh);
    assert_eq!(
        p,
        [0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn mux_setup_packet_uses_the_long_header() {
    use imobiledevice::mux::{encode_packet, parse_packet};
    let p = encode_packet(2, 2, 0, 0xffff, &[7]);
    assert_eq!(
        p,
        [0, 0, 0, 2, 0, 0, 0, 17, 0xfe, 0xed, 0xfa, 0xce, 0, 0, 0xff, 0xff, 7]
    );
    let (proto, rx_seq, payload) = parse_packet(2, &p).unwrap();
    assert_eq!((proto, rx_seq, payload), (2, 0xffff, &[7u8][..]));
    // Length must match the transfer exactly.
    parse_packet(2, &p[..16]).unwrap_err();
}

#[test]
fn mux_tcp_header_round_trip() {
    use imobiledevice::mux::{encode_tcp, parse_tcp, Tcp};
    let t = Tcp {
        sport: 1,
        dport: 62078,
        seq: 1,
        ack: 1,
        flags: 0x10,
        win: 512,
    };
    let p = encode_tcp(&t, b"hello");
    assert_eq!(p.len(), 25);
    assert_eq!(p[12], 0x50, "data offset is 5 words");
    assert_eq!(&p[0..4], &[0, 1, 0xf2, 0x7e]);
    let (back, data) = parse_tcp(&p).unwrap();
    assert_eq!(back, t);
    assert_eq!(data, b"hello");
    parse_tcp(&p[..19]).unwrap_err();
}

#[test]
fn cert_chain_verified_by_openssl() {
    if std::process::Command::new("openssl")
        .arg("version")
        .output()
        .is_err()
    {
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
    let root = dir.join("archstaller-test-root.pem");
    let devc = dir.join("archstaller-test-dev.pem");
    let host = dir.join("archstaller-test-host.pem");
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

#[test]
fn binary_plist_malicious_trailer_does_not_overflow() {
    let mut b = vec![0u8; 40];
    b[0..8].copy_from_slice(b"bplist00");
    b[8 + 6] = 8;
    b[8 + 7] = 1;
    b[8 + 8..8 + 16].copy_from_slice(&u64::MAX.to_be_bytes());
    imobiledevice::plist::parse(&b).unwrap_err();
}

#[test]
fn xml_empty_self_closing_tags() {
    let xml = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <plist version=\"1.0\">\n\
                <dict>\n\
                    <key>EmptyDict</key>\n\
                    <dict/>\n\
                    <key>EmptyArray</key>\n\
                    <array/>\n\
                    <key>EmptyString</key>\n\
                    <string/>\n\
                </dict>\n\
                </plist>";
    let val = imobiledevice::plist::parse(xml).expect("parse empty self-closing tags");
    assert_eq!(val.get("EmptyDict"), Some(&Value::Dict(Vec::new())));
    assert_eq!(val.get("EmptyArray"), Some(&Value::Array(Vec::new())));
    assert_eq!(val.get("EmptyString"), Some(&Value::String(String::new())));
}

#[test]
fn xml_negative_integer() {
    let xml = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <plist version=\"1.0\">\n\
                <dict>\n\
                    <key>Number</key>\n\
                    <integer>-1</integer>\n\
                </dict>\n\
                </plist>";
    let val = imobiledevice::plist::parse(xml).expect("parse negative integer");
    assert_eq!(val.dict_get_int("Number"), Some(u64::MAX));
}

#[test]
fn binary_plist_shift_overflow_panic() {
    let mut b = vec![
        b'b', b'p', b'l', b'i', b's', b't', b'0', b'0',
        0x1f, 0x10, 64,
    ];
    while b.len() < 32 {
        b.push(0);
    }
    let off_start = b.len();
    b.push(8);
    let mut trailer = [0u8; 32];
    trailer[6] = 1;
    trailer[7] = 1;
    trailer[8..16].copy_from_slice(&1u64.to_be_bytes());
    trailer[16..24].copy_from_slice(&0u64.to_be_bytes());
    trailer[24..32].copy_from_slice(&(off_start as u64).to_be_bytes());
    b.extend_from_slice(&trailer);

    imobiledevice::plist::parse(&b).unwrap_err();
}

