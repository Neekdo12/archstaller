use crate::{fb::FbConsole, serial::Serial};
use core::fmt::{self, Write};
use spinning_top::Spinlock;

struct Console {
    serial: Serial,
    fb: Option<FbConsole>,
}

static CONSOLE: Spinlock<Console> = Spinlock::new(Console {
    serial: Serial::new(),
    fb: None,
});

pub fn init_serial() {
    CONSOLE.lock().serial.init();
}

/// Whether the serial port stopped draining (see `serial::State::Stuck`).
pub fn serial_stuck() -> bool {
    CONSOLE.lock().serial.stuck()
}

pub fn init_fb(fb: FbConsole) {
    CONSOLE.lock().fb = Some(fb);
}

/// Break the console lock; only for exception and panic paths.
pub unsafe fn force_unlock() {
    unsafe { CONSOLE.force_unlock() };
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.serial.write_str(s) {
            Ok(()) | Err(_) => {}
        }
        if let Some(fb) = self.fb.as_mut() {
            match fb.write_str(s) {
                Ok(()) | Err(_) => {}
            }
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    // Tee into the bounded capture first (it never blocks on the console and never allocates), so the
    // log survives even if the console itself is wedged; `Arguments` is `Copy`, so formatting twice is
    // free of surprises.
    crate::logbuf::write(args);
    match CONSOLE.lock().write_fmt(args) {
        Ok(()) | Err(_) => {}
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}
