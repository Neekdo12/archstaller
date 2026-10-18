#![no_std]

extern crate alloc;

pub mod crc;
pub mod writer;

pub use writer::{Options, XfsWriter};
