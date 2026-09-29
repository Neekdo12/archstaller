//! Uncached MMIO mappings added to the page tables Limine set up.
use alloc::alloc::{alloc_zeroed, Layout};
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// PML4 slot 510; Limine only uses the HHDM (lower half of the upper half) and slot 511.
const MMIO_BASE: u64 = 0xffff_ff00_0000_0000;
const MMIO_END: u64 = 0xffff_ff80_0000_0000;

const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

static NEXT_VA: AtomicU64 = AtomicU64::new(MMIO_BASE);

fn table_at(hhdm: u64, phys: u64) -> *mut u64 {
    (hhdm + phys) as *mut u64
}

/// Returns the next-level table for `entry`, allocating it if absent.
unsafe fn next_table(hhdm: u64, entry: *mut u64) -> *mut u64 {
    let e = entry.read_volatile();
    if e & PRESENT != 0 {
        return table_at(hhdm, e & ADDR_MASK);
    }
    let page = alloc_zeroed(Layout::from_size_align(4096, 4096).unwrap());
    assert!(!page.is_null(), "page table allocation failed");
    entry.write_volatile((page as u64 - hhdm) | PRESENT | WRITE);
    page as *mut u64
}

/// Maps `len` bytes at physical `phys` as uncached device memory.
pub fn map_mmio(hhdm: u64, phys: u64, len: usize) -> *mut u8 {
    let start = phys & !0xfff;
    let end = (phys + len as u64 + 0xfff) & !0xfff;
    let va = NEXT_VA.fetch_add(end - start, Ordering::Relaxed);
    assert!(va + (end - start) <= MMIO_END, "MMIO window exhausted");

    let cr3: u64;
    unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack)) };
    let pml4 = table_at(hhdm, cr3 & ADDR_MASK);
    let mut off = 0;
    while off < end - start {
        let v = va + off;
        unsafe {
            let pdpt = next_table(hhdm, pml4.add(((v >> 39) & 0x1ff) as usize));
            let pd = next_table(hhdm, pdpt.add(((v >> 30) & 0x1ff) as usize));
            let pt = next_table(hhdm, pd.add(((v >> 21) & 0x1ff) as usize));
            pt.add(((v >> 12) & 0x1ff) as usize)
                .write_volatile((start + off) | PRESENT | WRITE | PWT | PCD);
        }
        off += 4096;
    }
    (va + (phys & 0xfff)) as *mut u8
}
