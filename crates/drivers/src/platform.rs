//! Services the kernel provides to drivers.
use hal::{Error, Result};
use spin::Once;

pub trait Platform: Sync {
    /// Maps `len` bytes of device memory at `phys` uncached and returns the virtual address.
    fn map_mmio(&self, phys: u64, len: usize) -> *mut u8;
    /// Offset of the higher-half direct map (phys + offset = virt for RAM).
    fn hhdm(&self) -> u64;
    fn now_ns(&self) -> u64;
}

static PLATFORM: Once<&'static dyn Platform> = Once::new();

pub fn init(p: &'static dyn Platform) {
    PLATFORM.call_once(|| p);
}

fn get() -> &'static dyn Platform {
    *PLATFORM.get().expect("drivers::platform::init not called")
}

pub fn map_mmio(phys: u64, len: usize) -> *mut u8 {
    get().map_mmio(phys, len)
}

pub fn hhdm() -> u64 {
    get().hhdm()
}

pub fn now_ns() -> u64 {
    get().now_ns()
}

/// Polls `cond` until it is true or `timeout_ms` passes.
pub fn wait_until(timeout_ms: u64, mut cond: impl FnMut() -> bool) -> Result<()> {
    let deadline = now_ns() + timeout_ms * 1_000_000;
    loop {
        if cond() {
            return Ok(());
        }
        if now_ns() > deadline {
            return Err(Error::Timeout);
        }
        core::hint::spin_loop();
    }
}

/// Busy-waits for `us` microseconds.
pub fn delay_us(us: u64) {
    let deadline = now_ns() + us * 1000;
    while now_ns() < deadline {
        core::hint::spin_loop();
    }
}
