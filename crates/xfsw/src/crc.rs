//! CRC32C (Castagnoli) checksum implementation for XFS V5 metadata verification.
//!
//! Polynomial: 0x82F63B78 (reversed), initial value: 0xFFFFFFFF, final XOR: 0xFFFFFFFF.

#[expect(clippy::indexing_slicing, reason = "constant evaluation loop 0..256 into array of size 256")]
const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0x82F6_3B78;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC32C_TABLE: [u32; 256] = make_table();

/// Computes the CRC32C for `data` with standard Castagnoli parameters.
pub fn crc32c(data: &[u8]) -> u32 {
    crc32c_update(!0u32, data) ^ !0u32
}

/// Updates an ongoing CRC32C calculation.
pub fn crc32c_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        let idx = ((crc ^ (b as u32)) & 0xFF) as usize;
        if let Some(&entry) = CRC32C_TABLE.get(idx) {
            crc = entry ^ (crc >> 8);
        }
    }
    crc
}

/// Computes the XFS V5 buffer checksum.
///
/// In XFS V5, checksums are calculated over the entire metadata block / sector,
/// with the 4-byte CRC field treated as zero. The result is returned as a little-endian `u32`.
pub fn xfs_calc_cksum(buffer: &[u8], crc_offset: usize) -> u32 {
    let zero = [0u8; 4];
    let mut crc = !0u32;
    if crc_offset > 0 {
        if let Some(prefix) = buffer.get(..crc_offset) {
            crc = crc32c_update(crc, prefix);
        }
    }
    crc = crc32c_update(crc, &zero);
    if let Some(suffix) = buffer.get(crc_offset + 4..) {
        crc = crc32c_update(crc, suffix);
    }
    crc ^ !0u32
}

/// Writes the XFS V5 checksum into `buffer` at `crc_offset` as little-endian bytes.
pub fn xfs_write_cksum(buffer: &mut [u8], crc_offset: usize) {
    let sum = xfs_calc_cksum(buffer, crc_offset);
    if let Some(slot) = buffer.get_mut(crc_offset..crc_offset + 4) {
        slot.copy_from_slice(&sum.to_le_bytes());
    }
}

/// Verifies whether the XFS V5 checksum in `buffer` at `crc_offset` is correct.
pub fn xfs_verify_cksum(buffer: &[u8], crc_offset: usize) -> bool {
    let Some(slot) = buffer.get(crc_offset..crc_offset + 4) else {
        return false;
    };
    let Ok(bytes) = slot.try_into() else {
        return false;
    };
    let expected = u32::from_le_bytes(bytes);
    let computed = xfs_calc_cksum(buffer, crc_offset);
    expected == computed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32c_standard_vector() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn test_xfs_write_and_verify_cksum() {
        let mut buf = [0u8; 512];
        buf[0..4].copy_from_slice(b"XFSB");
        buf[100] = 0x42;
        let crc_off = 224;

        assert!(!xfs_verify_cksum(&buf, crc_off));
        xfs_write_cksum(&mut buf, crc_off);
        assert!(xfs_verify_cksum(&buf, crc_off));

        // Corrupting one byte fails verification
        buf[50] ^= 0x01;
        assert!(!xfs_verify_cksum(&buf, crc_off));
        // Restoring it passes again
        buf[50] ^= 0x01;
        assert!(xfs_verify_cksum(&buf, crc_off));
    }

    #[test]
    fn test_crc32c_update_incremental() {
        let part1 = b"1234";
        let part2 = b"56789";
        let mut crc = !0u32;
        crc = crc32c_update(crc, part1);
        crc = crc32c_update(crc, part2);
        assert_eq!(crc ^ !0u32, 0xE306_9283);
    }

    #[test]
    fn test_xfs_calc_cksum_direct() {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(b"TEST");
        let sum = xfs_calc_cksum(&buf, 8);
        assert_ne!(sum, 0);

        // Put the sum at offset 8
        buf[8..12].copy_from_slice(&sum.to_le_bytes());
        assert!(xfs_verify_cksum(&buf, 8));

        // Buffer too small for offset returns false
        assert!(!xfs_verify_cksum(&buf, 62));
    }
}
