//! The kernel's capture of everything it prints, so the whole boot and install log can be written out
//! at the end (see `logdump.rs`). The ring itself lives in the `logring` crate (so it can be unit
//! tested on the host); this module only owns the one kernel-wide instance and adapts the console.
//!
//! The console tees into it. `logring` never allocates on push, so it is safe to feed from the panic
//! path and from before the heap exists; once full, the oldest bytes are dropped.
//!
//! Re-entrancy and faults: `write` must never block, because it is on the path taken by the panic and
//! exception reporters. If a fault happens while this CPU already holds the ring lock (for example a
//! fault inside the console's own formatting, or the `fault-test` arm that holds the lock on purpose),
//! a blocking `lock()` would spin forever and the reporter would never print. So `write` uses
//! `try_lock` and simply drops the fragment when the ring is busy: losing one line in a pathological
//! self-fault is always better than a dead reporter. `force_unlock` additionally clears the lock for
//! the reporters, so the message they print actually lands in the capture.
use core::sync::atomic::{AtomicBool, Ordering};
use spinning_top::Spinlock;

/// The capture size. The install log is a few tens of KiB; 128 KiB keeps a full run with room to spare.
const CAP: usize = 128 * 1024;

static RING: Spinlock<logring::Ring<CAP>> = Spinlock::new(logring::Ring::new());

/// Whether a fragment was ever dropped because the ring was busy (a fault while it was held). Kept so
/// the end-of-flow dump can say so instead of silently missing lines.
static DROPPED: AtomicBool = AtomicBool::new(false);

/// Called by the console for every printed fragment.
///
/// Never blocks: if the ring is locked (a fault while it is held), this fragment is dropped and
/// `DROPPED` is set rather than spinning on a lock this CPU already owns.
pub fn write(args: core::fmt::Arguments) {
    match RING.try_lock() {
        Some(mut ring) => {
            let _ = ring.write_fmt(args);
        }
        None => DROPPED.store(true, Ordering::Release),
    }
}

/// Whether any printed fragment was dropped because the ring was busy (see `write`).
pub fn dropped() -> bool {
    DROPPED.load(Ordering::Acquire)
}

/// Break the ring lock; for the exception and panic reporters (as `console::force_unlock` is for the
/// console). Safe to call even when the lock is not held.
pub unsafe fn force_unlock() {
    unsafe { RING.force_unlock() };
}

/// Takes and returns the ring lock, to let the fault self-test fault while it is held and so prove the
/// reporter breaks it (see the `fault-test` arm in `main.rs`). Only compiled for that feature.
#[cfg(feature = "fault-test")]
pub fn hold_for_fault_test() -> spinning_top::guard::SpinlockGuard<'static, logring::Ring<CAP>> {
    RING.lock()
}

/// The captured bytes in order, oldest first (with a truncation marker first when the ring wrapped).
/// Allocates the result on the heap; used by the log writer in `logdump.rs`, so it runs only after the
/// heap is up. A truncation marker is prefixed if the ring wrapped, and a note is added if any
/// fragment was dropped while the ring was busy.
pub fn snapshot() -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    if dropped() {
        out.extend_from_slice(
            b"[note: some printed lines were dropped while the log ring was busy]\n",
        );
    }
    RING.lock().write_snapshot(&mut out);
    out
}
