//! DMA buffers. RAM is linearly mapped by the HHDM, so a heap allocation is physically
//! contiguous and `phys = virt - hhdm`.
use crate::platform;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};

pub struct Dma {
    ptr: *mut u8,
    layout: Layout,
}

unsafe impl Send for Dma {}

impl Dma {
    pub fn new(size: usize, align: usize) -> Dma {
        let layout = Layout::from_size_align(size.max(1), align.max(1)).unwrap();
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

    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.layout.size()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) };
    }
}
