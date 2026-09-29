//! "pacman-lite": sync database parsing, version comparison, dependency resolution and
//! streaming tar/compression readers.
#![no_std]

extern crate alloc;

pub mod compress;
pub mod db;
pub mod desc;
pub mod io;
pub mod resolve;
pub mod tar;
pub mod vercmp;

pub use desc::{Dep, Op, Package};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Read from the underlying source failed.
    Io,
    /// Unexpected end of input.
    Eof,
    /// Corrupt archive, compressed stream or database entry.
    Format(&'static str),
    Unsupported(&'static str),
    /// Dependency cannot be satisfied.
    Unsatisfied(alloc::string::String),
    Conflict(alloc::string::String, alloc::string::String),
    UnknownPackage(alloc::string::String),
}

pub type Result<T> = core::result::Result<T, Error>;
