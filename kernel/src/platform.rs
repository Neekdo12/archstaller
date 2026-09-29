use crate::{paging, time};

pub struct KernelPlatform {
    pub hhdm: u64,
}

impl drivers::platform::Platform for KernelPlatform {
    fn map_mmio(&self, phys: u64, len: usize) -> *mut u8 {
        paging::map_mmio(self.hhdm, phys, len)
    }

    fn hhdm(&self) -> u64 {
        self.hhdm
    }

    fn now_ns(&self) -> u64 {
        time::uptime_ns()
    }
}
