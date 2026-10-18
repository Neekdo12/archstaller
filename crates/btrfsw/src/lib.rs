#![no_std]
#![expect(clippy::indexing_slicing, reason = "on-disk serialization uses known fixed offsets")]

extern crate alloc;

pub mod crc;
pub mod ondisk;
pub mod writer;

pub use disk::{FileKind, FsWriter, Meta, Storage};
pub use writer::{BtrfsWriter, Error, Options, Result, SubvolConfig};
