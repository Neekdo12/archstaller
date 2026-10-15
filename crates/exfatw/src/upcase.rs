//! exFAT up-case table: fold name units to their up-cased form for the stream NameHash.

#[derive(Debug)]
pub struct Upcase {
    pub units: alloc::vec::Vec<u16>,
}

impl Upcase {
    /// exFAT up-case table checksum (rotate-right-1 then add, over the raw bytes).
    pub fn checksum(bytes: &[u8]) -> u32 {
        let mut sum: u32 = 0;
        for &b in bytes {
            sum = sum.rotate_right(1).wrapping_add(b as u32);
        }
        sum
    }

    /// The up-cased form of one unit, via the table (identity when out of range).
    pub fn fold(&self, u: u16) -> u16 {
        match self.units.get(u as usize) {
            Some(&m) if m != 0 && m != 0xffff => m,
            _ => u,
        }
    }
}

/// The exFAT NameHash, over already-up-cased units.
pub fn upcase_hash(units: &[u16]) -> u16 {
    let mut hash: u16 = 0;
    for &u in units {
        let b = u.to_le_bytes();
        hash = hash.rotate_right(1).wrapping_add(b[0] as u16);
        hash = hash.rotate_right(1).wrapping_add(b[1] as u16);
    }
    hash
}
