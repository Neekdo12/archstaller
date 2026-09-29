use crate::{fb::FbConsole, serial::Serial};
use core::fmt::{self, Write};
use spinning_top::Spinlock;

struct Console {
    serial: Serial,
    fb: Option<FbConsole>,
}

static CONSOLE: Spinlock<Console> = Spinlock::new(Console { serial: Serial, fb: None });

pub fn init_serial() {
    Serial::init();
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
        let _ = self.serial.write_str(s);
        if let Some(fb) = self.fb.as_mut() {
            let _ = fb.write_str(s);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    let _ = CONSOLE.lock().write_fmt(args);
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
