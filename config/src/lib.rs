//! Config types shared by xtask (serializes) and kernel (deserializes).
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keymap: String,
    pub disk: Disk,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disk {
    /// Substring matched against the disk model; must select exactly one disk.
    pub model: Option<String>,
    /// Serial of the disk that will be wiped; installation aborts on mismatch.
    pub confirm_serial: String,
}
