//! Initramfs assembly: cpio (newc) writer and kernel module dependency resolution from the ELF
//! `.modinfo` section (Arch ships no `modules.dep`).
#![no_std]

extern crate alloc;

pub mod cpio;
pub mod modules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an ELF64 little-endian module or `.modinfo` missing.
    BadModule(alloc::string::String),
    /// A required module is neither present nor built into the kernel.
    Missing(alloc::string::String),
    Decompress(alloc::string::String),
}

pub type Result<T> = core::result::Result<T, Error>;
