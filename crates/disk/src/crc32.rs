/// CRC-32 (IEEE 802.3) as used by GPT.
const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
            k += 1;
        }
        let ptr = t.as_mut_ptr();
        // SAFETY: i is bounded by 0..256 which is within the 256-element array.
        let slot = unsafe { ptr.add(i) };
        // SAFETY: slot is a valid non-null pointer within t.
        unsafe {
            *slot = c;
        }
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        let entry = TABLE.get(((crc ^ b as u32) & 0xff) as usize).copied().unwrap_or(0);
        crc = entry ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}
