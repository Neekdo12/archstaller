use crate::{root, run, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;

const VERSION: &str = "v12.9.1";
const SHA256: &str = "ce972a05e9d1973dc9b725f9130bd15af01add0eb3a7f90c118ac5cfe21c17b8";

pub struct Limine {
    pub dir: PathBuf,
}

impl Limine {
    pub fn tool(&self) -> PathBuf {
        self.dir.join("limine")
    }
    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// Downloads the pinned Limine binary release (sha256-verified) and builds the host tool.
pub fn fetch() -> Result<Limine> {
    let dir = root().join("target/limine").join(VERSION);
    let l = Limine { dir: dir.clone() };
    if l.tool().exists() {
        return Ok(l);
    }
    std::fs::create_dir_all(&dir)?;
    let tarball = dir.join("limine-binary.tar.xz");
    run(Command::new("curl").args(["-fL", "-o"]).arg(&tarball).arg(format!(
        "https://github.com/Limine-Bootloader/Limine/releases/download/{VERSION}/limine-binary.tar.xz"
    )))?;
    let digest = Sha256::digest(std::fs::read(&tarball)?);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != SHA256 {
        std::fs::remove_file(&tarball)?;
        return Err(format!("limine tarball sha256 mismatch: {hex}").into());
    }
    run(Command::new("tar").arg("xf").arg(&tarball).arg("-C").arg(&dir).args(["--strip-components=1"]))?;
    run(Command::new("cc").args(["-O2", "-o"]).arg(l.tool()).arg(dir.join("limine.c")))?;
    Ok(l)
}
