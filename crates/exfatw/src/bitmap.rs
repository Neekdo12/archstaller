//! exFAT allocation bitmap: the real source of truth for free clusters.

#[derive(Debug)]
pub struct Bitmap {
    pub bytes: alloc::vec::Vec<u8>,
    pub first_cluster: u32,
}

impl Bitmap {
    /// Whether cluster `c` (>= 2) is marked in use. Out of range counts as used.
    pub fn used(&self, c: u32) -> bool {
        let idx = (c as usize).saturating_sub(2);
        let byte = idx / 8;
        let Some(&b) = self.bytes.get(byte) else {
            return true;
        };
        (b >> (idx % 8)) & 1 != 0
    }

    /// Marks cluster `c` in use in memory. Returns the bitmap byte index that changed.
    pub fn set(&mut self, c: u32) -> Option<usize> {
        let idx = (c as usize).saturating_sub(2);
        let byte = idx / 8;
        let slot = self.bytes.get_mut(byte)?;
        *slot |= 1 << (idx % 8);
        Some(byte)
    }
}
