//! Hardware abstraction traits shared by drivers and the installer logic.
#![no_std]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Device reported a failure.
    Io,
    /// Device did not answer in time.
    Timeout,
    /// Bad LBA, unaligned buffer or similar caller error.
    InvalidArgument,
    /// Device or feature not supported by the driver.
    Unsupported,
}

pub type Result<T> = core::result::Result<T, Error>;

static IDLE_HOOK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Installs the function `idle()` calls. The kernel points it at a halt-until-next-tick.
pub fn set_idle_hook(f: fn()) {
    IDLE_HOOK.store(f as usize, core::sync::atomic::Ordering::Relaxed);
}

/// Called by polling loops that found nothing to do. Returns after at most a short
/// pause (the kernel halts the CPU until its timer tick); a plain spin hint by default.
pub fn idle() {
    match IDLE_HOOK.load(core::sync::atomic::Ordering::Relaxed) {
        0 => core::hint::spin_loop(),
        f => unsafe { core::mem::transmute::<usize, fn()>(f)() },
    }
}

static LOG_HOOK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Installs the sink for `hal::log!` (the kernel points it at its console).
pub fn set_log_hook(f: fn(core::fmt::Arguments)) {
    LOG_HOOK.store(f as usize, core::sync::atomic::Ordering::Relaxed);
}

/// Diagnostic line from a driver crate that has no console of its own; dropped until a hook is set.
pub fn log(args: core::fmt::Arguments) {
    match LOG_HOOK.load(core::sync::atomic::Ordering::Relaxed) {
        0 => {}
        f => unsafe { core::mem::transmute::<usize, fn(core::fmt::Arguments)>(f)(args) },
    }
}

/// True when built with the `debug` feature (`cargo xtask build --debug`): verbose driver tracing is on.
pub const DEBUG: bool = cfg!(feature = "debug");

/// Verbose diagnostics (driver traces, timings): compiled out unless the `debug` feature is on,
/// and the arguments are not evaluated then.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { if $crate::DEBUG { $crate::log(format_args!($($t)*)) } };
}

/// Messages that matter in a normal build too (failures, things the user must act on).
#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::log(format_args!($($t)*)) };
}

pub trait BlockDevice {
    fn model(&self) -> &str;
    fn serial(&self) -> &str;
    /// Bytes per logical sector (512 or 4096).
    fn sector_size(&self) -> u32;
    fn sector_count(&self) -> u64;
    /// `buf.len()` must be a multiple of `sector_size()`.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()>;
    /// `buf.len()` must be a multiple of `sector_size()`.
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
}

pub trait NetDevice {
    fn name(&self) -> &str;
    fn mac(&self) -> [u8; 6];
    fn link_up(&mut self) -> bool;
    /// Sends one Ethernet frame (without FCS); blocks until queued.
    fn transmit(&mut self, frame: &[u8]) -> Result<()>;
    /// Copies the next received frame into `buf` and returns its length, or `None` if idle.
    fn receive(&mut self, buf: &mut [u8]) -> Option<usize>;
}

pub trait Clock {
    /// Monotonic nanoseconds.
    fn now_ns(&self) -> u64;
    /// Seconds since the Unix epoch.
    fn unix_time(&self) -> u64;
}

pub trait Rng {
    fn fill(&mut self, buf: &mut [u8]);
}
