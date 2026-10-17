/// Volatile accessors for a mapped register window.
#[derive(Clone, Copy)]
pub struct Mmio {
    base: *mut u8,
}

// SAFETY: MMIO base pointers reference hardware memory-mapped I/O apertures that can be transferred across threads.
unsafe impl Send for Mmio {}
// SAFETY: Volatile operations on hardware MMIO registers are safe across shared references in polling drivers.
unsafe impl Sync for Mmio {}

impl Mmio {
    pub fn new(base: *mut u8) -> Mmio {
        Mmio { base }
    }

    pub fn ptr(&self) -> *mut u8 {
        self.base
    }

    pub fn offset(&self, off: usize) -> Mmio {
        Mmio {
            // SAFETY: Pointer arithmetic within the MMIO register aperture.
            base: unsafe { self.base.add(off) },
        }
    }

    pub fn read8(&self, off: usize) -> u8 {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) };
        // SAFETY: Volatile read from mapped MMIO register.
        unsafe { ptr.read_volatile() }
    }
    pub fn read16(&self, off: usize) -> u16 {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) } as *const u16;
        // SAFETY: Volatile read from mapped MMIO register.
        unsafe { ptr.read_volatile() }
    }
    pub fn read32(&self, off: usize) -> u32 {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) } as *const u32;
        // SAFETY: Volatile read from mapped MMIO register.
        unsafe { ptr.read_volatile() }
    }
    pub fn read64(&self, off: usize) -> u64 {
        self.read32(off) as u64 | (self.read32(off + 4) as u64) << 32
    }
    pub fn write8(&self, off: usize, v: u8) {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) };
        // SAFETY: Volatile write to mapped MMIO register.
        unsafe { ptr.write_volatile(v) }
    }
    pub fn write16(&self, off: usize, v: u16) {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) } as *mut u16;
        // SAFETY: Volatile write to mapped MMIO register.
        unsafe { ptr.write_volatile(v) }
    }
    pub fn write32(&self, off: usize, v: u32) {
        // SAFETY: Pointer arithmetic within the MMIO register aperture.
        let ptr = unsafe { self.base.add(off) } as *mut u32;
        // SAFETY: Volatile write to mapped MMIO register.
        unsafe { ptr.write_volatile(v) }
    }
    pub fn write64(&self, off: usize, v: u64) {
        self.write32(off, v as u32);
        self.write32(off + 4, (v >> 32) as u32);
    }
}
