use usbnet::rndis::*;

#[test]
fn init_message_layout() {
    let m = init_msg(1, 0x4000);
    assert_eq!(m.len(), 24);
    assert_eq!(m, [2, 0, 0, 0, 24, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]);
}

#[test]
fn query_and_set_layout() {
    let q = query_msg(2, OID_802_3_PERMANENT_ADDRESS, 48);
    assert_eq!(q.len(), 76, "header plus a 48-byte zeroed information buffer");
    assert_eq!(&q[0..8], &[4, 0, 0, 0, 76, 0, 0, 0]);
    assert_eq!(&q[12..16], &0x0101_0101u32.to_le_bytes());
    assert_eq!(&q[16..20], &48u32.to_le_bytes());
    assert!(q[28..].iter().all(|b| *b == 0));
    assert_eq!(&q[20..24], &20u32.to_le_bytes(), "buffer offset counts from the request id");
    let s = set_msg(3, OID_GEN_CURRENT_PACKET_FILTER, &PACKET_FILTER.to_le_bytes());
    assert_eq!(s.len(), 32);
    assert_eq!(&s[4..8], &32u32.to_le_bytes());
    assert_eq!(&s[28..], &0xfu32.to_le_bytes());
}

#[test]
fn parses_completions() {
    // INITIALIZE_CMPLT, 52 bytes: max transfer size at offset 36.
    let mut init = vec![0u8; 52];
    init[0..4].copy_from_slice(&(MSG_INIT | CMPLT).to_le_bytes());
    init[4..8].copy_from_slice(&52u32.to_le_bytes());
    init[8..12].copy_from_slice(&1u32.to_le_bytes());
    init[36..40].copy_from_slice(&0x4000u32.to_le_bytes());
    let r = parse_reply(&init).unwrap();
    assert_eq!((r.msg_type, r.request_id, r.status, r.max_transfer), (MSG_INIT | CMPLT, 1, 0, 0x4000));

    // QUERY_CMPLT with a 6-byte MAC; the offset is relative to the request id (byte 8).
    let mut q = vec![0u8; 24 + 6];
    q[0..4].copy_from_slice(&(MSG_QUERY | CMPLT).to_le_bytes());
    q[4..8].copy_from_slice(&30u32.to_le_bytes());
    q[8..12].copy_from_slice(&2u32.to_le_bytes());
    q[16..20].copy_from_slice(&6u32.to_le_bytes());
    q[20..24].copy_from_slice(&16u32.to_le_bytes());
    q[24..30].copy_from_slice(&[0x02, 0, 0x11, 0x22, 0x33, 0x44]);
    let r = parse_reply(&q).unwrap();
    assert_eq!(r.info, [0x02, 0, 0x11, 0x22, 0x33, 0x44]);

    assert!(parse_reply(&q[..20]).is_err(), "length field exceeds the buffer");
}

#[test]
fn packet_wrapping_round_trip() {
    let frame: Vec<u8> = (0..60u8).collect();
    let w = wrap_frame(&frame);
    assert_eq!(w.len(), PACKET_HDR + 60);
    assert_eq!(&w[0..4], &1u32.to_le_bytes());
    assert_eq!(&w[8..12], &36u32.to_le_bytes(), "data offset from byte 8");
    assert_eq!(&w[12..16], &60u32.to_le_bytes());
    assert_eq!(unwrap_frames(&w), vec![frame.clone()]);

    // Two packet messages in one bulk transfer.
    let mut two = w.clone();
    two.extend_from_slice(&wrap_frame(b"second frame"));
    let frames = unwrap_frames(&two);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1], b"second frame");

    // Garbage and truncation yield nothing rather than panicking.
    assert!(unwrap_frames(&w[..30]).is_empty());
    assert!(unwrap_frames(&[0xffu8; 40]).is_empty());
}
