//! CRC32C (Castagnoli polynomial 0x82F63B78) for Btrfs metadata and data checksums.

#[expect(clippy::indexing_slicing, reason = "constant evaluation loop 0..256 into array of size 256")]
const fn make_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x82F6_3B78
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = make_table();

/// Raw CRC32C update with table lookup.
#[inline]
pub fn crc32c_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xff) as usize;
        if let Some(&entry) = TABLE.get(idx) {
            crc = entry ^ (crc >> 8);
        }
    }
    crc
}

/// Standard Btrfs on-disk checksum (used for superblocks, tree blocks, and data extents).
///
/// Uses Castagnoli polynomial with initial seed 0xFFFFFFFF and final XOR 0xFFFFFFFF.
#[inline]
pub fn btrfs_csum(data: &[u8]) -> u32 {
    crc32c_update(!0, data) ^ !0
}

/// Btrfs directory entry name hash for `DIR_ITEM` key offset.
///
/// Uses Castagnoli polynomial with seed `~1` (`0xFFFFFFFE`) and no final inversion,
/// matching the Linux kernel `btrfs_name_hash()` function.
#[inline]
pub fn btrfs_name_hash(name: &[u8]) -> u64 {
    crc32c_update(!1u32, name) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_check_value() {
        // Standard CRC-32C of "123456789" is 0xE3069283
        assert_eq!(btrfs_csum(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn test_empty_csum() {
        assert_eq!(btrfs_csum(b""), 0);
    }

    #[test]
    fn test_btrfs_name_hash_known_value() {
        // Verified against real btrfs dump-tree
        assert_eq!(btrfs_name_hash(b"hello.txt"), 0x415f_eb59);
    }

    #[test]
    fn test_btrfs_name_hash_distinct() {
        assert_ne!(btrfs_name_hash(b"hello.txt"), btrfs_name_hash(b"world.txt"));
        assert_ne!(btrfs_name_hash(b"file1"), btrfs_name_hash(b"file2"));
    }
}
