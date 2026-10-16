#![no_std]
#![no_main]

use core::arch::global_asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};
use hal::aarch64::uart::Pl011;

global_asm!(include_str!("entry.s"));

static UART_BASE: AtomicUsize = AtomicUsize::new(Pl011::BASE_RPI3);

struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Pl011::new(UART_BASE.load(Ordering::Relaxed)).write_str(s);
        Ok(())
    }
}

pub fn print_args(args: fmt::Arguments) {
    let mut writer = ConsoleWriter;
    let _ = writer.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::print_args(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[no_mangle]
pub extern "C" fn kmain(dtb_ptr: u64) -> ! {
    // Determine the base UART based on DTB or CPU/platform
    let mut uart_base = Pl011::BASE_RPI3;

    // Check DTB magic (0xD00DFEED in big-endian)
    let valid_dtb = if dtb_ptr != 0 {
        let magic = u32::from_be(unsafe { core::ptr::read_volatile(dtb_ptr as *const u32) });
        magic == 0xD00D_FEED
    } else {
        false
    };

    // Check CPU part number from MIDR_EL1
    let midr: u64;
    unsafe {
        core::arch::asm!("mrs {}, midr_el1", out(reg) midr, options(nomem, nostack));
    }
    let part_num = (midr >> 4) & 0xFFF;

    // If running on QEMU virt with Cortex-A57 or similar, or if DTB suggests virt:
    // Cortex-A53 is 0xD03 (Pi 3), Cortex-A76 is 0xD0B (Pi 5), Cortex-A57 is 0xD07 (virt default)
    if part_num == 0xD07 {
        uart_base = Pl011::BASE_QEMU_VIRT;
    } else if part_num == 0xD0B {
        uart_base = Pl011::BASE_RPI5;
    }

    UART_BASE.store(uart_base, Ordering::Relaxed);

    println!();
    println!("============================================================");
    println!("  archstaller (aarch64 / bare-metal)");
    println!("  Target: Raspberry Pi 3B+ & Pi 5 installer kernel");
    println!("============================================================");
    println!();

    let el = hal::aarch64::cpu::current_el();
    println!("CPU: Exception Level = EL{}", el);
    println!("CPU: MIDR_EL1 = 0x{:016x} (PartNum: 0x{:03x})", midr, part_num);

    let freq = hal::aarch64::timer::cntfrq();
    let ticks = hal::aarch64::timer::cntpct();
    println!("Timer: CNTFRQ = {} Hz, CNTPCT = {}", freq, ticks);

    if valid_dtb {
        println!("Firmware: Device Tree Blob detected at 0x{:016x}", dtb_ptr);
    } else {
        println!("Firmware: No DTB detected at 0x{:016x}", dtb_ptr);
    }

    println!("Console: PL011 UART initialized at MMIO 0x{:016x}", uart_base);
    println!();
    println!("Archstaller AArch64 bare-metal kernel booted successfully!");
    println!("Halting CPU (wfe loop)...");

    loop {
        hal::aarch64::cpu::wfe();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!();
    println!("KERNEL PANIC: {}", info);
    loop {
        hal::aarch64::cpu::wfe();
    }
}
