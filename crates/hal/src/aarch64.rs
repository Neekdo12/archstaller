//! AArch64 hardware primitives: CPU controls, system timer, and early PL011 UART.

pub mod cpu {
    use core::arch::asm;

    /// Wait For Interrupt: puts CPU into low-power state until an interrupt/event occurs.
    #[inline(always)]
    pub fn wfi() {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }

    /// Wait For Event.
    #[inline(always)]
    pub fn wfe() {
        unsafe { asm!("wfe", options(nomem, nostack)) };
    }

    /// Instruction Synchronization Barrier.
    #[inline(always)]
    pub fn isb() {
        unsafe { asm!("isb", options(nostack)) };
    }

    /// Data Synchronization Barrier.
    #[inline(always)]
    pub fn dsb() {
        unsafe { asm!("dsb sy", options(nostack)) };
    }

    /// Data Memory Barrier.
    #[inline(always)]
    pub fn dmb() {
        unsafe { asm!("dmb sy", options(nostack)) };
    }

    /// Returns the current Exception Level (1 for EL1, 2 for EL2, 3 for EL3).
    #[inline(always)]
    pub fn current_el() -> u8 {
        let el: u64;
        unsafe { asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack)) };
        ((el >> 2) & 0x3) as u8
    }
}

pub mod timer {
    use core::arch::asm;

    /// Reads the 64-bit physical counter (`CNTPCT_EL0`).
    #[inline(always)]
    pub fn cntpct() -> u64 {
        let val: u64;
        unsafe { asm!("mrs {}, cntpct_el0", out(reg) val, options(nomem, nostack)) };
        val
    }

    /// Reads the counter frequency in Hz (`CNTFRQ_EL0`).
    #[inline(always)]
    pub fn cntfrq() -> u64 {
        let val: u64;
        unsafe { asm!("mrs {}, cntfrq_el0", out(reg) val, options(nomem, nostack)) };
        val
    }

    /// Busy-wait delay in microseconds using the generic timer.
    pub fn delay_us(us: u64) {
        let freq = cntfrq();
        if freq == 0 {
            for _ in 0..(us * 100) {
                core::hint::spin_loop();
            }
            return;
        }
        let ticks = (us * freq) / 1_000_000;
        let start = cntpct();
        while cntpct().wrapping_sub(start) < ticks {
            core::hint::spin_loop();
        }
    }
}

pub mod uart {
    use core::fmt;

    /// Standard PL011 UART register offsets from base MMIO.
    pub const UART_DR: usize = 0x00;
    pub const UART_FR: usize = 0x18;
    pub const UART_IBRD: usize = 0x24;
    pub const UART_FBRD: usize = 0x28;
    pub const UART_LCR_H: usize = 0x2C;
    pub const UART_CR: usize = 0x30;
    pub const UART_IMSC: usize = 0x38;
    pub const UART_ICR: usize = 0x44;

    /// Flag register bits.
    pub const FR_TXFF: u32 = 1 << 5; // Transmit FIFO full
    pub const FR_RXFE: u32 = 1 << 4; // Receive FIFO empty

    /// Well-known PL011 base MMIO physical addresses.
    pub const BASE_QEMU_VIRT: usize = 0x0900_0000;
    pub const BASE_RPI3: usize = 0x3F20_1000;
    pub const BASE_RPI4: usize = 0xFE20_1000;
    pub const BASE_RPI5: usize = 0x10_7D00_1000;

    /// Direct polling PL011 UART driver.
    #[derive(Debug, Clone, Copy)]
    pub struct Pl011 {
        base: usize,
    }

    impl Pl011 {
        pub const BASE_QEMU_VIRT: usize = BASE_QEMU_VIRT;
        pub const BASE_RPI3: usize = BASE_RPI3;
        pub const BASE_RPI4: usize = BASE_RPI4;
        pub const BASE_RPI5: usize = BASE_RPI5;

        pub const fn new(base: usize) -> Self {
            Self { base }
        }

        #[inline(always)]
        unsafe fn read_reg(&self, offset: usize) -> u32 {
            core::ptr::read_volatile((self.base + offset) as *const u32)
        }

        #[inline(always)]
        unsafe fn write_reg(&self, offset: usize, val: u32) {
            core::ptr::write_volatile((self.base + offset) as *mut u32, val);
        }

        /// Waits until the transmit FIFO has room and writes one byte.
        pub fn write_byte(&self, b: u8) {
            unsafe {
                while (self.read_reg(UART_FR) & FR_TXFF) != 0 {
                    core::hint::spin_loop();
                }
                self.write_reg(UART_DR, b as u32);
            }
        }

        /// Writes a byte slice to the UART, translating `\n` to `\r\n`.
        pub fn write_bytes(&self, bytes: &[u8]) {
            for &b in bytes {
                if b == b'\n' {
                    self.write_byte(b'\r');
                }
                self.write_byte(b);
            }
        }

        /// Writes a string slice to the UART.
        pub fn write_str(&self, s: &str) {
            self.write_bytes(s.as_bytes());
        }

        /// Reads a byte if available in the receive FIFO.
        pub fn read_byte(&self) -> Option<u8> {
            unsafe {
                if (self.read_reg(UART_FR) & FR_RXFE) == 0 {
                    Some((self.read_reg(UART_DR) & 0xFF) as u8)
                } else {
                    None
                }
            }
        }
    }

    impl fmt::Write for Pl011 {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            Pl011::write_str(self, s);
            Ok(())
        }
    }
}
