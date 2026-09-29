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
