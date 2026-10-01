//! Idle waits. The kernel is polling-only, so a loop with nothing to do used to spin a core at
//! 100%. This programs the PIT as a 1 kHz tick (PIC remapped, only IRQ0 unmasked) so such a loop
//! can `hlt` until the next tick. Interrupts stay disabled everywhere except inside `idle()`.
//! If no tick arrives during the self-check (no legacy PIC/PIT, routed elsewhere), `idle()` stays
//! a spin hint so nothing can hang.
use crate::port::outb;
use crate::time;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const VECTOR: usize = 0x20;
const PIT_HZ: u32 = 1_193_182;
const TICK_HZ: u32 = 1000;

static TICKS: AtomicU64 = AtomicU64::new(0);
static TICK_OK: AtomicBool = AtomicBool::new(false);

/// Called from the IRQ0 handler in `idt.rs`.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe { outb(0x20, 0x20) }; // EOI to the master PIC
}

pub fn init(hhdm: u64) {
    unsafe {
        unmask_extint(hhdm);
        // Remap the PICs so IRQ0-15 land on vectors 0x20-0x2f, clear of the CPU exceptions.
        outb(0x20, 0x11);
        outb(0xa0, 0x11);
        outb(0x21, VECTOR as u8);
        outb(0xa1, VECTOR as u8 + 8);
        outb(0x21, 4); // slave on IRQ2
        outb(0xa1, 2);
        outb(0x21, 1); // 8086 mode
        outb(0xa1, 1);
        outb(0x21, 0xfe); // only IRQ0 (PIT)
        outb(0xa1, 0xff);
        // PIT channel 0, rate generator.
        let div = PIT_HZ / TICK_HZ;
        outb(0x43, 0x34);
        outb(0x40, (div & 0xff) as u8);
        outb(0x40, (div >> 8) as u8);
        // Self-check: interrupts on, spin (never hlt) until a few ticks arrive or 100 ms pass.
        let deadline = time::uptime_ns() + 100_000_000;
        asm!("sti", options(nomem, nostack));
        while TICKS.load(Ordering::Relaxed) < 3 && time::uptime_ns() < deadline {
            core::hint::spin_loop();
        }
        asm!("cli", options(nomem, nostack));
        if TICKS.load(Ordering::Relaxed) >= 3 {
            TICK_OK.store(true, Ordering::Relaxed);
            hal::set_idle_hook(idle);
        } else {
            outb(0x21, 0xff); // no tick: mask everything, keep spinning
        }
    }
    if hal::DEBUG {
        crate::println!("idle: timer tick {}", if TICK_OK.load(Ordering::Relaxed) { "ok, halting when idle" } else { "unavailable, spinning when idle" });
    }
}

/// The bootloader leaves the local APIC's LINT0 (where the legacy PIC is wired in virtual-wire
/// mode) masked, so PIC interrupts never reach the CPU. Unmasks it as ExtINT. Does nothing
/// when the APIC is off or in x2APIC mode (no MMIO window); the self-check then falls back.
unsafe fn unmask_extint(hhdm: u64) {
    let (lo, hi): (u32, u32);
    asm!("rdmsr", in("ecx") 0x1bu32, out("eax") lo, out("edx") hi, options(nomem, nostack));
    let base = (hi as u64) << 32 | lo as u64;
    if base & (1 << 11) == 0 || base & (1 << 10) != 0 {
        return;
    }
    let regs = crate::paging::map_mmio(hhdm, base & 0x000f_ffff_ffff_f000, 0x1000) as *mut u32;
    let svr = regs.add(0xf0 / 4);
    svr.write_volatile(svr.read_volatile() | 0x1ff); // software-enable, spurious vector 0xff
    regs.add(0x350 / 4).write_volatile(0x0000_8700); // LINT0: ExtINT, level, unmasked
}

/// Halts until the next interrupt (at most 1 ms).
pub fn idle() {
    if TICK_OK.load(Ordering::Relaxed) {
        // `sti` delays interrupts until after the next instruction, so the tick cannot slip in
        // between it and `hlt` and be lost.
        unsafe { asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    } else {
        core::hint::spin_loop();
    }
}
