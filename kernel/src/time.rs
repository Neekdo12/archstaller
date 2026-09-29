use crate::port::{inb, outb};
use core::arch::x86_64::_rdtsc;
use core::sync::atomic::{AtomicU64, Ordering};

const PIT_HZ: u64 = 1_193_182;
/// ~50 ms of PIT ticks.
const CAL_TICKS: u16 = 59_659;

static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static TSC_BASE: AtomicU64 = AtomicU64::new(0);
static BOOT_UNIX: AtomicU64 = AtomicU64::new(0);

/// Calibrates the TSC against PIT channel 2 (one-shot, polled).
pub fn init(boot_unix: i64) {
    unsafe {
        let gate = inb(0x61);
        outb(0x61, (gate & !0x02) | 0x01); // gate on, speaker off
        outb(0x43, 0xb0); // ch2, lobyte/hibyte, mode 0, binary
        outb(0x42, (CAL_TICKS & 0xff) as u8);
        outb(0x42, (CAL_TICKS >> 8) as u8);
        // Restart the count by toggling the gate.
        let g = inb(0x61) & !0x01;
        outb(0x61, g);
        outb(0x61, g | 0x01);
        let start = _rdtsc();
        while inb(0x61) & 0x20 == 0 {}
        let end = _rdtsc();
        outb(0x61, gate);
        TSC_HZ.store((end - start) * PIT_HZ / CAL_TICKS as u64, Ordering::Relaxed);
        TSC_BASE.store(end, Ordering::Relaxed);
    }
    BOOT_UNIX.store(boot_unix.max(0) as u64, Ordering::Relaxed);
}

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

/// Nanoseconds since `init`.
pub fn uptime_ns() -> u64 {
    let hz = tsc_hz();
    if hz == 0 {
        return 0;
    }
    let ticks = unsafe { _rdtsc() } - TSC_BASE.load(Ordering::Relaxed);
    (ticks as u128 * 1_000_000_000 / hz as u128) as u64
}

/// Wall clock seconds since the Unix epoch.
pub fn unix_time() -> u64 {
    BOOT_UNIX.load(Ordering::Relaxed) + uptime_ns() / 1_000_000_000
}
