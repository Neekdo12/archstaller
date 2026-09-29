use crate::port::{inb, outb};
use core::fmt;

const COM1: u16 = 0x3f8;

pub struct Serial;

impl Serial {
    pub fn init() {
        unsafe {
            outb(COM1 + 1, 0x00); // no interrupts
            outb(COM1 + 3, 0x80); // DLAB
            outb(COM1, 0x01); // 115200 baud
            outb(COM1 + 1, 0x00);
            outb(COM1 + 3, 0x03); // 8N1
            outb(COM1 + 2, 0xc7); // FIFO
            outb(COM1 + 4, 0x03); // DTR, RTS
        }
    }

    fn put(&self, b: u8) {
        unsafe {
            let mut spins = 0u32;
            while inb(COM1 + 5) & 0x20 == 0 {
                spins += 1;
                if spins > 100_000 {
                    return; // no UART behind the port
                }
                core::hint::spin_loop();
            }
            outb(COM1, b);
        }
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.put(b'\r');
            }
            self.put(b);
        }
        Ok(())
    }
}
