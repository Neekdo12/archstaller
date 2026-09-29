//! CRC32C as used by ext4 metadata checksums: raw (no pre/post inversion) with a caller seed.

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

pub fn crc32c(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = TABLE[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_check_value() {
        // Standard CRC-32C is the raw CRC with init and final xor of all ones.
        assert_eq!(crc32c(!0, b"123456789") ^ !0, 0xE306_9283);
    }
}
