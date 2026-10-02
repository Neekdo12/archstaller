use core::sync::atomic::{AtomicU64, Ordering};
use spinning_top::RawSpinlock;
use talc::{source::Manual, TalcLock};

#[global_allocator]
static TALC: TalcLock<RawSpinlock, Manual> = TalcLock::new(Manual);

/// Heap size cap; the rest of usable RAM stays untouched.
const MAX_HEAP: u64 = 512 << 20;

static HEAP_BYTES: AtomicU64 = AtomicU64::new(0);

/// The boot loader's higher-half direct map always covers the first 4 GiB; above that only the memory ranges.
const HHDM_SAFE: u64 = 4 << 30;
/// A heap smaller than this below 4 GiB is not worth it; fall back to the biggest region anywhere.
const MIN_LOW_HEAP: u64 = 32 << 20;

/// Claims a usable region (through the HHDM) as the heap: the largest one that lies below 4 GiB,
/// where the direct map is guaranteed to exist. Machines with lots of RAM often have their
/// biggest region above 4 GiB, and writing there faulted on real hardware. Only if the low
/// memory is tiny is the biggest region anywhere used.
pub fn init(hhdm: u64, memmap: &[bootinfo::MemEntry]) {
    let usable = || memmap.iter().filter(|e| e.kind == bootinfo::MEM_USABLE);
    let largest = usable().max_by_key(|e| e.len).expect("no usable memory");
    let low = usable()
        .filter(|e| e.base < HHDM_SAFE)
        .map(|e| (e.base, e.len.min(HHDM_SAFE - e.base)))
        .max_by_key(|&(_, len)| len);
    let (base, len) = match low {
        Some((b, l)) if l >= MIN_LOW_HEAP => (b, l),
        _ => (largest.base, largest.len),
    };
    let size = len.min(MAX_HEAP);
    // Printed before the first write into the region, so a fault there still leaves the layout on screen.
    crate::println!(
        "memory: heap {} MiB at {:#x} (largest usable region {} MiB at {:#x})",
        size >> 20,
        base,
        largest.len >> 20,
        largest.base
    );
    unsafe { TALC.lock().claim((hhdm + base) as *mut u8, size as usize) }.expect("heap claim failed");
    HEAP_BYTES.store(size, Ordering::Relaxed);
}

pub fn size() -> u64 {
    HEAP_BYTES.load(Ordering::Relaxed)
}
