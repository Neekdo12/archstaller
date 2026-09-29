/// Volatile accessors for a mapped register window.
#[derive(Clone, Copy)]
pub struct Mmio {
    base: *mut u8,
}

unsafe impl Send for Mmio {}
unsafe impl Sync for Mmio {}

impl Mmio {
    pub fn new(base: *mut u8) -> Mmio {
        Mmio { base }
    }

    pub fn ptr(&self) -> *mut u8 {
        self.base
    }

    pub fn offset(&self, off: usize) -> Mmio {
        Mmio { base: unsafe { self.base.add(off) } }
    }

    pub fn read8(&self, off: usize) -> u8 {
        unsafe { self.base.add(off).read_volatile() }
    }
    pub fn read16(&self, off: usize) -> u16 {
        unsafe { (self.base.add(off) as *const u16).read_volatile() }
    }
    pub fn read32(&self, off: usize) -> u32 {
        unsafe { (self.base.add(off) as *const u32).read_volatile() }
    }
    pub fn read64(&self, off: usize) -> u64 {
        self.read32(off) as u64 | (self.read32(off + 4) as u64) << 32
    }
    pub fn write8(&self, off: usize, v: u8) {
        unsafe { self.base.add(off).write_volatile(v) }
    }
    pub fn write16(&self, off: usize, v: u16) {
        unsafe { (self.base.add(off) as *mut u16).write_volatile(v) }
    }
    pub fn write32(&self, off: usize, v: u32) {
        unsafe { (self.base.add(off) as *mut u32).write_volatile(v) }
    }
    pub fn write64(&self, off: usize, v: u64) {
        self.write32(off, v as u32);
        self.write32(off + 4, (v >> 32) as u32);
    }
}
