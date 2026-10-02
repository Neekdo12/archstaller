//! The kernel's initial page tables.
use crate::{up, Arena, HUGE, PAGE};
use bootinfo::*;

const P: u64 = 1;
const W: u64 = 2;
const PS: u64 = 1 << 7;
const ADDR: u64 = 0x000f_ffff_ffff_f000;
/// The direct map covers at most this much (one PML4 slot).
const HHDM_MAX: u64 = 512 << 30;

unsafe fn new_table(arena: &mut Arena) -> *mut u64 {
    arena.take(PAGE, PAGE) as *mut u64
}

/// Entry `idx` of `table`, creating the next level when it is empty.
unsafe fn next(arena: &mut Arena, table: *mut u64, idx: u64) -> *mut u64 {
    let e = table.add(idx as usize);
    if *e & P == 0 {
        *e = new_table(arena) as u64 | P | W;
    }
    (*e & ADDR) as *mut u64
}

unsafe fn map_2m(arena: &mut Arena, pml4: *mut u64, virt: u64, phys: u64) {
    let pdpt = next(arena, pml4, (virt >> 39) & 511);
    let pd = next(arena, pdpt, (virt >> 30) & 511);
    *pd.add(((virt >> 21) & 511) as usize) = phys | P | W | PS;
}

unsafe fn map_4k(arena: &mut Arena, pml4: *mut u64, virt: u64, phys: u64) {
    let pdpt = next(arena, pml4, (virt >> 39) & 511);
    let pd = next(arena, pdpt, (virt >> 30) & 511);
    let pt = next(arena, pd, (virt >> 21) & 511);
    *pt.add(((virt >> 12) & 511) as usize) = phys | P | W;
}

/// Returns the physical address of the PML4: the direct map (the first 4 GiB, every memory range,
/// the framebuffer), the kernel at `KERNEL_BASE`, and the trampoline page identity mapped.
pub unsafe fn build(arena: &mut Arena, map: &crate::Map, kernel_phys: u64, kernel_mem: u64, tramp: u64, fb: &Framebuffer) -> u64 {
    let pml4 = new_table(arena);
    let hhdm = |arena: &mut Arena, base: u64, end: u64| {
        let mut a = base & !(HUGE - 1);
        while a < end.min(HHDM_MAX) {
            map_2m(arena, pml4, HHDM_BASE + a, a);
            a += HUGE;
        }
    };
    hhdm(arena, 0, 4 << 30);
    for e in map.entries() {
        if e.kind != MEM_BAD {
            hhdm(arena, e.base, e.base + e.len);
        }
    }
    if fb.addr != 0 {
        hhdm(arena, fb.addr, fb.addr + fb.pitch as u64 * fb.height as u64);
    }
    for i in 0..up(kernel_mem, HUGE) / HUGE {
        map_2m(arena, pml4, KERNEL_BASE + i * HUGE, kernel_phys + i * HUGE);
    }
    map_4k(arena, pml4, tramp, tramp);
    pml4 as u64
}
