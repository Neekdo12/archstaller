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

/// The real-time clock as Unix seconds (the CMOS clock is assumed to hold UTC and a year after 2000).
pub fn rtc_unix_time() -> i64 {
    fn read(reg: u8) -> u8 {
        unsafe {
            outb(0x70, reg);
            inb(0x71)
        }
    }
    // An update is under way when bit 7 of status A is set; read until two passes agree.
    let snapshot = || {
        for _ in 0..1_000_000 {
            if read(0x0a) & 0x80 == 0 {
                break;
            }
        }
        [read(0), read(2), read(4), read(7), read(8), read(9)]
    };
    let mut t = snapshot();
    for _ in 0..4 {
        let again = snapshot();
        if again == t {
            break;
        }
        t = again;
    }
    let b = read(0x0b);
    let bcd = |v: u8| if b & 4 == 0 { (v & 0x0f) + (v >> 4) * 10 } else { v };
    let pm = t[2] & 0x80 != 0;
    let mut hour = bcd(t[2] & 0x7f) as i64;
    if b & 2 == 0 {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let (sec, min, day, month, year) = (bcd(t[0]) as i64, bcd(t[1]) as i64, bcd(t[3]) as i64, bcd(t[4]) as i64, 2000 + bcd(t[5]) as i64);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + min * 60 + sec
}
