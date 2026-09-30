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
    // CPUID first: no port I/O, cannot hang. The PIT is only the fallback (older or AMD CPUs).
    let hz = match cpuid_tsc_hz() {
        Some(hz) if (500_000_000..10_000_000_000).contains(&hz) => hz,
        _ => match pit_tsc_hz() {
            Some(hz) if (500_000_000..10_000_000_000).contains(&hz) => hz,
            _ => 2_000_000_000, // unknown: only timeouts and TLS time checks are off
        },
    };
    TSC_HZ.store(hz, Ordering::Relaxed);
    TSC_BASE.store(unsafe { _rdtsc() }, Ordering::Relaxed);
    BOOT_UNIX.store(boot_unix.max(0) as u64, Ordering::Relaxed);
}

/// Calibrates against PIT channel 2 (one-shot, polled). The wait is bounded in TSC cycles (about
/// a second at typical clocks) because some chipsets never raise the output bit, and port reads
/// on real hardware are slow, so an iteration cap alone can look like a hang.
fn pit_tsc_hz() -> Option<u64> {
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
        let mut end = start;
        let mut fired = false;
        while end - start < 3_000_000_000 {
            if inb(0x61) & 0x20 != 0 {
                fired = true;
                break;
            }
            end = _rdtsc();
        }
        if fired {
            end = _rdtsc();
        }
        outb(0x61, gate);
        fired.then(|| (end - start) * PIT_HZ / CAL_TICKS as u64)
    }
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
