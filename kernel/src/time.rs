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
        // Bounded: some chipsets never raise PIT channel 2's output bit, which would hang here.
        let mut spins = 0u32;
        while inb(0x61) & 0x20 == 0 && spins < 20_000_000 {
            spins += 1;
        }
        let end = _rdtsc();
        outb(0x61, gate);
        let pit_hz = if spins < 20_000_000 { (end - start) * PIT_HZ / CAL_TICKS as u64 } else { 0 };
        // Trust the PIT only when the result is plausible (0.5 to 10 GHz); otherwise ask CPUID.
        let hz = if (500_000_000..10_000_000_000).contains(&pit_hz) { pit_hz } else { cpuid_tsc_hz().unwrap_or(2_000_000_000) };
        TSC_HZ.store(hz, Ordering::Relaxed);
        TSC_BASE.store(_rdtsc(), Ordering::Relaxed);
    }
    BOOT_UNIX.store(boot_unix.max(0) as u64, Ordering::Relaxed);
}

/// TSC frequency from CPUID leaf 0x15 (crystal ratio), else leaf 0x16 (base MHz).
fn cpuid_tsc_hz() -> Option<u64> {
    use core::arch::x86_64::__cpuid;
    let max = __cpuid(0).eax;
    if max >= 0x15 {
        let l = __cpuid(0x15);
        if l.eax != 0 && l.ebx != 0 && l.ecx != 0 {
            return Some(l.ecx as u64 * l.ebx as u64 / l.eax as u64);
        }
    }
    if max >= 0x16 {
        let mhz = __cpuid(0x16).eax as u64 & 0xffff;
        if mhz != 0 {
            return Some(mhz * 1_000_000);
        }
    }
    None
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
