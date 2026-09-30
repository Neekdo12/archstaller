//! Host-side tests for xHCI ring bookkeeping and descriptor parsing, against MMIO windows
//! backed by plain memory (the pattern used by crates/disk's in-memory tests).
use std::sync::Once;

fn init_platform() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        struct TestPlatform;
        impl drivers::platform::Platform for TestPlatform {
            fn map_mmio(&self, _phys: u64, _len: usize) -> *mut u8 {
                unimplemented!("tests never map device memory")
            }
            fn hhdm(&self) -> u64 {
                0 // phys == virt
            }
            fn now_ns(&self) -> u64 {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64
            }
        }
        drivers::platform::init(Box::leak(Box::new(TestPlatform)));
    });
}

mod ring {
    use super::init_platform;
    use usb::xhci::Ring;

    fn trb(ring_mem: &[u8], i: usize) -> (u64, u32, u32) {
        let b = &ring_mem[i * 16..i * 16 + 16];
        (
            u64::from_le_bytes(b[0..8].try_into().unwrap()),
            u32::from_le_bytes(b[8..12].try_into().unwrap()),
            u32::from_le_bytes(b[12..16].try_into().unwrap()),
        )
    }

    #[test]
    fn cycle_bits_and_link_trb() {
        init_platform();
        let mut r = Ring::new(4); // 3 usable slots + link
        let p0 = r.push(0x1000, 0, 2 << 10);
        let p1 = r.push(0x2000, 0, 2 << 10);
        let p2 = r.push(0x3000, 0, 2 << 10);
        assert_eq!(p1 - p0, 16);
        assert_eq!(p2 - p1, 16);
        let mem = r.memory();
        assert_eq!(trb(mem, 0).2 & 1, 1, "cycle bit set in first pass");
        assert_eq!(trb(mem, 2).2 & 1, 1);
        // Slot 3 is the link TRB written when wrapping.
        let (param, _status, control) = trb(mem, 3);
        assert_eq!(param, p0, "link TRB points back at the ring base");
        assert_eq!((control >> 10) & 0x3f, 6, "link TRB type");
        assert_eq!(control & 1, 1, "link TRB cycle matches the pass it was written in");
        assert_ne!(control & 2, 0, "link TRB has the toggle-cycle bit");
        // Next pass runs with the flipped cycle state.
        r.push(0x4000, 0, 2 << 10);
        let mem = r.memory();
        assert_eq!(trb(mem, 0).2 & 1, 0, "cycle bit flipped after wrap");
        assert_eq!(trb(mem, 0).0, 0x4000);
    }
}

mod desc {
    use usb::desc;

    /// A minimal config blob: 9-byte config header + interface (class 0xff/0xfe/0x02)
    /// + bulk OUT ep1 + bulk IN ep1, like an iPhone's mux interface shape.
    const BLOB: &[u8] = &[
        9, 2, 32, 0, 1, 1, 0, 0x80, 250, // config
        9, 4, 0, 0, 2, 0xff, 0xfe, 0x02, 0, // interface
        7, 5, 0x01, 0x02, 0x00, 0x02, 0, // ep 1 OUT bulk, mps 512
        7, 5, 0x81, 0x02, 0x00, 0x02, 0, // ep 1 IN bulk, mps 512
    ];

    #[test]
    fn parses_interfaces_and_endpoints() {
        let ifs = desc::parse_config(BLOB);
        assert_eq!(ifs.len(), 1);
        let i = &ifs[0];
        assert_eq!((i.class, i.subclass, i.protocol), (0xff, 0xfe, 0x02));
        assert_eq!(i.endpoints.len(), 2);
        assert!(!i.endpoints[0].is_in());
        assert!(i.endpoints[1].is_in());
        assert_eq!(i.endpoints[0].dci(), 2);
        assert_eq!(i.endpoints[1].dci(), 3);
        assert!(i.endpoints.iter().all(|e| e.is_bulk()));
        assert_eq!(i.endpoints[0].max_packet, 512);
    }

    #[test]
    fn parses_utf16_string() {
        // "Ab" as a USB string descriptor.
        let s = [6u8, 3, b'A', 0, b'b', 0];
        assert_eq!(desc::parse_string(&s).as_deref(), Some("Ab"));
        assert_eq!(desc::parse_string(&[2, 3]), Some("".into()));
        assert_eq!(desc::parse_string(&[2, 9]), None);
    }
}
