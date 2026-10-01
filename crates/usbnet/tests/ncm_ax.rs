use usbnet::{ax88179, ncm};

fn params(div: u16, rem: u16, align: u16) -> ncm::Params {
    ncm::Params { in_max: 16384, out_max: 16384, out_divisor: div, out_remainder: rem, out_align: align }
}

#[test]
fn ncm_parameters_parse() {
    let mut b = [0u8; 28];
    b[0..2].copy_from_slice(&28u16.to_le_bytes());
    b[2..4].copy_from_slice(&1u16.to_le_bytes()); // NTB16 supported
    b[4..8].copy_from_slice(&16384u32.to_le_bytes());
    b[16..20].copy_from_slice(&8192u32.to_le_bytes());
    b[20..22].copy_from_slice(&4u16.to_le_bytes());
    b[22..24].copy_from_slice(&0u16.to_le_bytes());
    b[24..26].copy_from_slice(&4u16.to_le_bytes());
    let p = ncm::parse_params(&b).unwrap();
    assert_eq!((p.in_max, p.out_max, p.out_divisor, p.out_remainder, p.out_align), (16384, 8192, 4, 0, 4));
    b[2] = 0; // no NTB16
    assert!(ncm::parse_params(&b).is_err());
    assert!(ncm::parse_params(&b[..10]).is_err());
}

#[test]
fn ncm_ntb_layout_and_round_trip() {
    let frame: Vec<u8> = (0..60u8).collect();
    let ntb = ncm::build_ntb(&frame, 7, &params(4, 0, 4)).unwrap();
    // NTH16: "NCMH", header length 12, sequence, block length, NDP index.
    assert_eq!(&ntb[0..4], b"NCMH");
    assert_eq!(&ntb[4..6], &12u16.to_le_bytes());
    assert_eq!(&ntb[6..8], &7u16.to_le_bytes());
    assert_eq!(&ntb[8..10], &(ntb.len() as u16).to_le_bytes());
    assert_eq!(&ntb[10..12], &72u16.to_le_bytes(), "NDP follows the datagram, 4-byte aligned");
    assert_eq!(&ntb[12..72], &frame[..]);
    // NDP16: "NCM0", length 16, no next NDP, one entry (offset 12, length 60), then zeros.
    assert_eq!(&ntb[72..76], b"NCM0");
    assert_eq!(&ntb[76..78], &16u16.to_le_bytes());
    assert_eq!(&ntb[80..84], &[12, 0, 60, 0]);
    assert_eq!(&ntb[84..88], &[0, 0, 0, 0]);
    assert_eq!(ncm::parse_ntb(&ntb), vec![frame]);
}

#[test]
fn ncm_respects_divisor_and_remainder() {
    let frame = vec![0xaau8; 100];
    let ntb = ncm::build_ntb(&frame, 1, &params(8, 2, 8)).unwrap();
    let ndp = u16::from_le_bytes([ntb[10], ntb[11]]) as usize;
    let dg = u16::from_le_bytes([ntb[ndp + 8], ntb[ndp + 9]]) as usize;
    assert_eq!(dg % 8, 2);
    assert_eq!(ndp % 8, 0);
    assert_eq!(ncm::parse_ntb(&ntb), vec![frame.clone()]);
    // Too big for the device's limit.
    let small = ncm::Params { out_max: 64, ..params(4, 0, 4) };
    assert!(ncm::build_ntb(&frame, 1, &small).is_err());
}

#[test]
fn ncm_parses_multiple_datagrams_and_rejects_garbage() {
    // Hand-built NTB with two datagrams in one NDP.
    let (a, b) = (vec![1u8; 20], vec![2u8; 30]);
    let mut n = vec![0u8; 12 + 20 + 30 + 2 + 20];
    n[0..4].copy_from_slice(b"NCMH");
    n[4..6].copy_from_slice(&12u16.to_le_bytes());
    let ndp = 64usize;
    n[10..12].copy_from_slice(&(ndp as u16).to_le_bytes());
    n[12..32].copy_from_slice(&a);
    n[32..62].copy_from_slice(&b);
    n[ndp..ndp + 4].copy_from_slice(b"NCM0");
    n[ndp + 4..ndp + 6].copy_from_slice(&20u16.to_le_bytes());
    n[ndp + 8..ndp + 12].copy_from_slice(&[12, 0, 20, 0]);
    n[ndp + 12..ndp + 16].copy_from_slice(&[32, 0, 30, 0]);
    assert_eq!(ncm::parse_ntb(&n), vec![a, b]);
    assert!(ncm::parse_ntb(&[0u8; 40]).is_empty());
    assert!(ncm::parse_ntb(&n[..20]).is_empty());
}

#[test]
fn ax88179_tx_header_pads_on_packet_boundaries() {
    let f = vec![0u8; 60];
    let t = ax88179::tx_frame(&f, 512);
    assert_eq!(&t[0..4], &60u32.to_le_bytes());
    assert_eq!(&t[4..8], &0u32.to_le_bytes());
    assert_eq!(t.len(), 68);
    let f = vec![0u8; 504]; // 504 + 8 == 512
    assert_eq!(&ax88179::tx_frame(&f, 512)[4..8], &0x8000_8000u32.to_le_bytes());
}

#[test]
fn ax88179_rx_splits_batched_packets() {
    let (f1, f2): (Vec<u8>, Vec<u8>) = ((0..60u8).collect(), (100..168u8).collect());
    let mut buf = Vec::new();
    // Packet 1: 2 bytes alignment padding + frame, padded to 8 bytes.
    buf.extend_from_slice(&[0, 0]);
    buf.extend_from_slice(&f1);
    while buf.len() % 8 != 0 {
        buf.push(0);
    }
    let end1 = buf.len();
    buf.extend_from_slice(&[0, 0]);
    buf.extend_from_slice(&f2);
    while buf.len() % 8 != 0 {
        buf.push(0);
    }
    let hdr_off = buf.len();
    buf.extend_from_slice(&(((f1.len() + 2) as u32) << 16).to_le_bytes());
    buf.extend_from_slice(&(((f2.len() + 2) as u32) << 16).to_le_bytes());
    buf.extend_from_slice(&(((hdr_off as u32) << 16) | 2).to_le_bytes());
    assert_eq!(end1, 64);
    assert_eq!(ax88179::parse_rx(&buf), vec![f1.clone(), f2.clone()]);

    // A CRC-error packet is dropped, the next one survives.
    let mut bad = buf.clone();
    bad[hdr_off + 3] |= 0x20; // bit 29 of the first packet header
    assert_eq!(ax88179::parse_rx(&bad), vec![f2]);
    assert!(ax88179::parse_rx(&[1, 2, 3]).is_empty());
}

#[test]
fn ax88179_link_settings_follow_speed() {
    use ax88179::*;
    let (m, q, rx) = link_settings(PHYSR_GIGA | PHYSR_FULL | PHYSR_LINK, USB_SS);
    assert_eq!(m, MEDIUM_RECEIVE_EN | MEDIUM_TXFLOW | MEDIUM_RXFLOW | MEDIUM_GIGA | MEDIUM_125MHZ | MEDIUM_FULL_DUPLEX);
    assert_eq!(q, [7, 0x4f, 0, 0x12, 0xff]);
    assert_eq!(rx, 1024 * (0x12 + 2));
    let (m, q, _) = link_settings(PHYSR_100 | PHYSR_FULL | PHYSR_LINK, USB_HS);
    assert_eq!(m & MEDIUM_PS, MEDIUM_PS);
    assert_eq!(m & MEDIUM_GIGA, 0);
    assert_eq!(q[3], 0x18);
}
