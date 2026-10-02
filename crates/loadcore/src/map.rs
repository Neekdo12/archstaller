//! Physical memory map handling: firmware maps are unsorted, overlap, and are not page aligned.
use bootinfo::*;

const MAX: usize = 512;

/// How much a kind wins when ranges overlap (the safest reading of a bad firmware map).
fn strength(kind: u32) -> u8 {
    match kind {
        MEM_USABLE => 0,
        MEM_ACPI_RECLAIMABLE => 1,
        MEM_RESERVED => 2,
        MEM_ACPI_NVS => 3,
        MEM_LOADER => 4,
        _ => 5,
    }
}

pub struct Map {
    e: [MemEntry; MAX],
    n: usize,
    scratch: [MemEntry; MAX],
    points: [u64; 2 * MAX],
}

const ZERO: MemEntry = MemEntry { base: 0, len: 0, kind: 0, _pad: 0 };

impl Map {
    pub const fn new() -> Self {
        Map { e: [ZERO; MAX], n: 0, scratch: [ZERO; MAX], points: [0; 2 * MAX] }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn entries(&self) -> &[MemEntry] {
        &self.e[..self.n]
    }

    /// Adds a raw range (call `normalize` after the last one). Contiguous neighbours of one kind merge,
    /// which keeps a sorted firmware map small.
    pub fn push(&mut self, base: u64, len: u64, kind: u32) {
        if len == 0 {
            return;
        }
        if let Some(last) = self.e[..self.n].last_mut() {
            if last.kind == kind && last.base + last.len == base {
                last.len += len;
                return;
            }
        }
        if self.n == MAX {
            panic!("memory map has too many entries");
        }
        self.e[self.n] = MemEntry { base, len, kind, _pad: 0 };
        self.n += 1;
    }

    /// Sorts, aligns (usable ranges shrink to whole pages, everything else grows), resolves overlaps
    /// (the strongest kind wins) and merges neighbours.
    pub fn normalize(&mut self) {
        let mut np = 0;
        for i in 0..self.n {
            let e = &mut self.e[i];
            let end = e.base.saturating_add(e.len);
            let (b, en) = if e.kind == MEM_USABLE { ((e.base + 4095) & !4095, end & !4095) } else { (e.base & !4095, end.saturating_add(4095) & !4095) };
            if en <= b {
                e.len = 0;
                continue;
            }
            e.base = b;
            e.len = en - b;
            self.points[np] = b;
            self.points[np + 1] = en;
            np += 2;
        }
        let pts = &mut self.points[..np];
        pts.sort_unstable();
        let mut out = 0;
        let mut k = 0;
        while k + 1 < np {
            let (lo, hi) = (pts[k], pts[k + 1]);
            k += 1;
            if lo == hi {
                continue;
            }
            let mut best: Option<u32> = None;
            for e in &self.e[..self.n] {
                if e.len != 0 && e.base <= lo && lo < e.base + e.len && best.map_or(true, |b| strength(e.kind) > strength(b)) {
                    best = Some(e.kind);
                }
            }
            let Some(kind) = best else { continue };
            if out > 0 && self.scratch[out - 1].kind == kind && self.scratch[out - 1].base + self.scratch[out - 1].len == lo {
                self.scratch[out - 1].len += hi - lo;
            } else {
                self.scratch[out] = MemEntry { base: lo, len: hi - lo, kind, _pad: 0 };
                out += 1;
            }
        }
        self.e[..out].copy_from_slice(&self.scratch[..out]);
        self.n = out;
    }

    /// Marks a range (usually carved out of usable memory) as `kind`.
    pub fn carve(&mut self, base: u64, len: u64, kind: u32) {
        self.push(base, len, kind);
        self.normalize();
    }

    /// The lowest usable range of `len` bytes at `align`, between `min` and `max` (exclusive).
    pub fn find_free(&self, len: u64, align: u64, min: u64, max: u64) -> Option<u64> {
        self.entries().iter().filter(|e| e.kind == MEM_USABLE).find_map(|e| {
            let start = (e.base.max(min) + align - 1) & !(align - 1);
            (start + len <= (e.base + e.len).min(max)).then_some(start)
        })
    }

    /// End of the highest range that is memory (not MMIO or holes).
    pub fn top(&self) -> u64 {
        self.entries().iter().filter(|e| matches!(e.kind, MEM_USABLE | MEM_ACPI_RECLAIMABLE | MEM_ACPI_NVS | MEM_LOADER)).map(|e| e.base + e.len).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::boxed::Box;
    use std::vec::Vec;

    fn kinds(m: &Map) -> Vec<(u64, u64, u32)> {
        m.entries().iter().map(|e| (e.base, e.len, e.kind)).collect()
    }

    #[test]
    fn sorts_and_merges() {
        let mut m = Box::new(Map::new());
        m.push(0x20_0000, 0x10_0000, MEM_USABLE);
        m.push(0x10_0000, 0x10_0000, MEM_USABLE);
        m.normalize();
        assert_eq!(kinds(&m), [(0x10_0000, 0x20_0000, MEM_USABLE)]);
    }

    #[test]
    fn reserved_wins_over_usable() {
        let mut m = Box::new(Map::new());
        m.push(0x10_0000, 0x40_0000, MEM_USABLE);
        m.push(0x20_0000, 0x1000, MEM_RESERVED);
        m.normalize();
        assert_eq!(
            kinds(&m),
            [(0x10_0000, 0x10_0000, MEM_USABLE), (0x20_0000, 0x1000, MEM_RESERVED), (0x20_1000, 0x2f_f000, MEM_USABLE)]
        );
    }

    #[test]
    fn usable_shrinks_and_reserved_grows_to_pages() {
        let mut m = Box::new(Map::new());
        m.push(0x1800, 0x3000, MEM_USABLE); // 0x1800..0x4800 -> 0x2000..0x4000
        m.push(0x10_0800, 0x100, MEM_RESERVED); // -> the whole page
        m.normalize();
        assert_eq!(kinds(&m), [(0x2000, 0x2000, MEM_USABLE), (0x10_0000, 0x1000, MEM_RESERVED)]);
    }

    #[test]
    fn carve_and_find_free() {
        let mut m = Box::new(Map::new());
        m.push(0x10_0000, 0x100_0000, MEM_USABLE);
        m.normalize();
        assert_eq!(m.find_free(0x1000, 0x1000, 0x10_0000, u64::MAX), Some(0x10_0000));
        m.carve(0x10_0000, 0x20_0000, MEM_LOADER);
        assert_eq!(m.find_free(0x1000, 0x1000, 0x10_0000, u64::MAX), Some(0x30_0000));
        assert_eq!(m.find_free(0x1000, 0x20_0000, 0x10_0000, u64::MAX), Some(0x40_0000));
        assert_eq!(m.find_free(0x1000, 0x1000, 0x10_0000, 0x30_0000), None);
        assert_eq!(m.top(), 0x110_0000);
    }
}
