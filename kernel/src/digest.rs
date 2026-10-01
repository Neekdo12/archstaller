//! Keeps the driver log lines (`hal::log!`) so the hardware test can print them again at the end of
//! its report: the report is long, and the lines that matter (NIC identification, link and
//! receive diagnostics) otherwise scroll off the screen before anyone can read them.
use crate::println;
use alloc::string::String;
use alloc::vec::Vec;
use spinning_top::Spinlock;

const MAX_LINES: usize = 300;

static LINES: Spinlock<Vec<String>> = Spinlock::new(Vec::new());

/// Adds a line (ignored until the heap exists).
pub fn push(line: String) {
    if crate::heap::size() == 0 {
        return;
    }
    let mut l = LINES.lock();
    if l.len() >= MAX_LINES {
        l.remove(0);
    }
    l.push(line);
}

/// Called by the log hook: prints the line and remembers it.
pub fn log(args: core::fmt::Arguments) {
    println!("{args}");
    if crate::heap::size() != 0 {
        push(alloc::format!("{args}"));
    }
}

pub fn print_all() {
    for l in LINES.lock().iter() {
        println!("{l}");
    }
}
