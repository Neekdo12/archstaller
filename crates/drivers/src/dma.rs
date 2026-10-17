//! DMA buffers. RAM is linearly mapped by the HHDM, so a heap allocation is physically
//! contiguous and `phys = virt - hhdm`.
use crate::platform;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};

pub struct Dma {
    ptr: *mut u8,
    layout: Layout,
}

// SAFETY: Dma owns a physically contiguous heap buffer with unique ownership that can safely be moved across threads.
unsafe impl Send for Dma {}

impl Dma {
    pub fn new(size: usize, align: usize) -> Dma {
        let layout = Layout::from_size_align(size.max(1), align.max(1)).expect("valid DMA layout");
        // SAFETY: The layout has non-zero size, and alloc_zeroed returns a valid block or null.
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "DMA allocation failed");
        Dma { ptr, layout }
    }

    /// A buffer for one USB transfer. An xHCI transfer TRB must not cross a 64 KiB boundary, so the
    /// buffer is aligned to its own size rounded up to a power of two (at most 64 KiB), which keeps
    /// any transfer of up to 64 KiB inside one boundary-safe region.
    pub fn for_transfer(len: usize) -> Dma {
        Dma::new(len, len.max(1).next_power_of_two().clamp(64, 65536))
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn phys(&self) -> u64 {
        self.ptr as u64 - platform::hhdm()
    }

    pub fn phys_at(&self, off: usize) -> u64 {
        self.phys() + off as u64
    }

    pub fn len(&self) -> usize {
        self.layout.size()
    }

    pub fn is_empty(&self) -> bool {
        self.layout.size() == 0
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `self.ptr` is valid for reads of `self.layout.size()` bytes as allocated.
        unsafe { core::slice::from_raw_parts(self.ptr, self.layout.size()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `self.ptr` is valid for reads and writes of `self.layout.size()` bytes and uniquely owned.
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was allocated with `self.layout` using `alloc_zeroed`.
        unsafe { dealloc(self.ptr, self.layout) };
    }
}
