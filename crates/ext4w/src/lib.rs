//! Write-once ext4 image writer: mkfs plus streaming population, no journal.
//!
//! Features: extents, filetype, sparse_super, large_file, dir_nlink, extra_isize, ext_attr,
//! dir_index (directories stay linear), metadata_csum. 4 KiB blocks, 256-byte inodes.
//! Metadata lives in RAM until [`Ext4Writer::finish`]; file data is streamed to storage.
#![no_std]

extern crate alloc;

mod crc;
mod writer;

pub use writer::{Ext4Writer, FileKind, Meta, Options};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io,
    NoSpace,
    NoInodes,
    NotFound,
    Exists,
    NotDir,
    IsDir,
    Invalid(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

/// Byte-addressed backing store (a partition).
pub trait Storage {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}
