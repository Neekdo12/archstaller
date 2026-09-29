use core::sync::atomic::{AtomicU64, Ordering};
use spinning_top::RawSpinlock;
use talc::{source::Manual, TalcLock};

#[global_allocator]
static TALC: TalcLock<RawSpinlock, Manual> = TalcLock::new(Manual);

/// Heap size cap; the rest of usable RAM stays untouched.
const MAX_HEAP: u64 = 512 << 20;

static HEAP_BYTES: AtomicU64 = AtomicU64::new(0);

/// Claims the largest usable region (through the HHDM) as the heap.
pub fn init(hhdm: u64, memmap: &[&limine::memmap::Entry]) {
    let best = memmap
        .iter()
        .filter(|e| e.type_ == limine::memmap::MEMMAP_USABLE)
        .max_by_key(|e| e.length)
        .expect("no usable memory");
    let size = best.length.min(MAX_HEAP);
    let base = (hhdm + best.base) as *mut u8;
    unsafe { TALC.lock().claim(base, size as usize) }.expect("heap claim failed");
    HEAP_BYTES.store(size, Ordering::Relaxed);
}

pub fn size() -> u64 {
    HEAP_BYTES.load(Ordering::Relaxed)
}
