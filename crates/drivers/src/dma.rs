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
