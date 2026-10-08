//! Randomness for the TLS stack and key generation (registered as the getrandom backend).
//!
//! `RDRAND` when the CPU has it. Older CPUs (before Intel Ivy Bridge / AMD Excavator) do not, so
//! a fallback gathers timing jitter: the TSC deltas around reads of the PIT counter (a slow,
//! asynchronous I/O device whose phase relative to the CPU clock is unpredictable), hashed into a
//! pool and expanded with SHA-256 in counter mode. That is weaker than a hardware RNG and is
//! reported as such by the hardware test; it exists so an old machine can still install.
use crate::port::{inb, outb};
use core::arch::x86_64::{_rdrand64_step, _rdtsc};
use sha2::{Digest, Sha256};
use spinning_top::Spinlock;

#[target_feature(enable = "rdrand")]
unsafe fn rdrand() -> Option<u64> {
    for _ in 0..10 {
        let mut v = 0u64;
        if _rdrand64_step(&mut v) == 1 {
            return Some(v);
        }
    }
    None
}

pub fn has_rdrand() -> bool {
    // CPUID.01H:ECX bit 30
    core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0
}

/// Mixes `samples` TSC deltas around PIT counter reads into `h`.
fn gather_jitter(h: &mut Sha256, samples: usize) {
    unsafe {
        let mut prev = _rdtsc();
        for _ in 0..samples {
            outb(0x43, 0x00); // latch PIT channel 0's counter
            let lo = inb(0x40);
            let hi = inb(0x40);
            let now = _rdtsc();
            h.update((now.wrapping_sub(prev)).to_le_bytes());
            h.update([lo, hi]);
            prev = now;
        }
        h.update(_rdtsc().to_le_bytes());
    }
}

struct Drbg {
    state: [u8; 32],
    counter: u64,
}

static DRBG: Spinlock<Option<Drbg>> = Spinlock::new(None);

fn jitter_fill(buf: &mut [u8]) {
    let mut guard = DRBG.lock();
    let d = guard.get_or_insert_with(|| {
        let mut h = Sha256::new();
        h.update(b"archstaller jitter seed");
        gather_jitter(&mut h, 4096);
        Drbg { state: h.finalize().into(), counter: 0 }
    });
    for chunk in buf.chunks_mut(32) {
        // Fresh jitter on every block as well, so the output is not a function of the seed alone.
        let mut h = Sha256::new();
        h.update(d.state);
        h.update(d.counter.to_le_bytes());
        gather_jitter(&mut h, 64);
        d.counter = d.counter.wrapping_add(1);
        let block: [u8; 32] = h.finalize().into();
        let mut next = Sha256::new();
        next.update(b"ratchet");
        next.update(block);
        d.state = next.finalize().into();
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
}

pub fn fill(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    if !has_rdrand() {
        jitter_fill(buf);
        return Ok(());
    }
    for chunk in buf.chunks_mut(8) {
        let v = unsafe { rdrand() }.ok_or(getrandom::Error::UNEXPECTED)?;
        chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
    }
    Ok(())
}

/// Sanity check of the jitter generator: consecutive outputs must differ and the output must not
/// be constant. (Not a statistical test.)
pub fn jitter_selfcheck() -> bool {
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    jitter_fill(&mut a);
    jitter_fill(&mut b);
    a != b && a.iter().any(|x| *x != a[0])
}

getrandom::register_custom_getrandom!(fill);
