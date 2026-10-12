use crate::port::{inb, outb};
use core::fmt;

const COM1: u16 = 0x3f8;
/// Status reads to wait for the transmitter (about 1 us each on real hardware); a byte takes about
/// 90 us at 115200 baud.
const WAIT: u32 = 10_000;

#[derive(Clone, Copy, PartialEq)]
enum State {
    /// No UART at COM1 (its scratch register does not keep a value): nothing is sent.
    Absent,
    Ready,
    /// A wait timed out: the UART does not drain. Bytes are then sent only when the transmitter is
    /// already empty, until it drains again. Waiting for every byte made the screen, which is written
    /// after the serial port, crawl at a few characters per second on such machines.
    Stuck,
}

pub struct Serial {
    state: State,
}

impl Serial {
    pub const fn new() -> Self {
        Serial { state: State::Absent }
    }

    pub fn init(&mut self) {
        unsafe {
            outb(COM1 + 7, 0x5a);
            let first = inb(COM1 + 7);
            outb(COM1 + 7, 0xa5);
            if first != 0x5a || inb(COM1 + 7) != 0xa5 {
                return;
            }
            outb(COM1 + 1, 0x00); // no interrupts
            outb(COM1 + 3, 0x80); // DLAB
            outb(COM1, 0x01); // 115200 baud
            outb(COM1 + 1, 0x00);
            outb(COM1 + 3, 0x03); // 8N1
            outb(COM1 + 2, 0xc7); // FIFO
            outb(COM1 + 4, 0x03); // DTR, RTS
        }
        self.state = State::Ready;
    }

    /// Whether the UART stopped draining, so the output only reaches the screen.
    pub fn stuck(&self) -> bool {
        self.state == State::Stuck
    }

    fn put(&mut self, b: u8) {
        let tries = match self.state {
            State::Absent => return,
            State::Ready => WAIT,
            State::Stuck => 1,
        };
        for _ in 0..tries {
            if unsafe { inb(COM1 + 5) } & 0x20 != 0 {
                unsafe { outb(COM1, b) };
                self.state = State::Ready;
                return;
            }
            core::hint::spin_loop();
        }
        self.state = State::Stuck;
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
