//! Disk layout: GPT with protective MBR, a write-once FAT32 filesystem for the ESP, byte-level
//! partition access.
#![no_std]

extern crate alloc;

pub mod crc32;
pub mod fat32;
pub mod gpt;
pub mod region;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io,
    /// Disk or partition too small or too large for the requested layout.
    Size(&'static str),
    /// Existing on-disk structure is not what we expect.
    Invalid(&'static str),
    NoSpace,
    NotFound,
    Exists,
}

pub type Result<T> = core::result::Result<T, Error>;

impl From<hal::Error> for Error {
    fn from(_: hal::Error) -> Error {
        Error::Io
    }
}
