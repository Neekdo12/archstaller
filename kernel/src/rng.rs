//! RDRAND-backed randomness for the TLS stack (registered as the getrandom backend).
use core::arch::x86_64::_rdrand64_step;

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

fn has_rdrand() -> bool {
    // CPUID.01H:ECX bit 30
    core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0
}

pub fn fill(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    if !has_rdrand() {
        return Err(getrandom::Error::UNSUPPORTED);
    }
    for chunk in buf.chunks_mut(8) {
        let v = unsafe { rdrand() }.ok_or(getrandom::Error::UNEXPECTED)?;
        chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
    }
    Ok(())
}

getrandom::register_custom_getrandom!(fill);
