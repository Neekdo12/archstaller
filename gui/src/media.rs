//! Putting the ISO on removable media. Phase 4: copying it as a file onto a mounted Ventoy volume.
//! `MediaTarget` is the seam where a Linux-only raw flash (UDisks2) can be added later.
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    pub mount: PathBuf,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
}

/// Mounted volumes, on every platform sysinfo supports.
pub fn volumes() -> Vec<Volume> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|d| Volume {
            name: d.name().to_string_lossy().into_owned(),
            mount: d.mount_point().to_path_buf(),
            fs: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
        })
        .collect()
}

/// Whether `v` is a Ventoy data volume. A label alone is not proof: the volume must carry Ventoy's
/// own files (a `ventoy` directory or `ventoy.json`). A sibling `VTOYEFI` volume is reported as
/// extra evidence where the system shows it.
pub fn is_ventoy(v: &Volume, all: &[Volume]) -> bool {
    let marker = v.mount.join("ventoy").is_dir() || v.mount.join("ventoy.json").is_file();
    let _sibling = all.iter().any(|o| o.name.to_ascii_uppercase().contains("VTOYEFI"));
    marker
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub dest: PathBuf,
    pub size: u64,
    pub sha256: String,
}

pub trait MediaTarget {
    /// Writes the ISO and verifies it; `progress(done, total)` is called as it goes.
    fn write(&self, iso: &Path, progress: &mut dyn FnMut(Phase, u64, u64), cancel: &AtomicBool) -> Result<Report, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Copying,
    Verifying,
}

/// Copies the ISO as a regular file into a directory of a mounted volume.
pub struct FolderCopy {
    pub dir: PathBuf,
    /// Free bytes of the volume the directory is on, when known.
    pub available: Option<u64>,
    pub overwrite: bool,
}

const CHUNK: usize = 1 << 20;

impl MediaTarget for FolderCopy {
    fn write(&self, iso: &Path, progress: &mut dyn FnMut(Phase, u64, u64), cancel: &AtomicBool) -> Result<Report, String> {
        let name = iso.file_name().ok_or("the ISO has no file name")?;
        let dest = self.dir.join(name);
        let part = self.dir.join(format!(".{}.part", name.to_string_lossy()));
        let total = std::fs::metadata(iso).map_err(|e| format!("{}: {e}", iso.display()))?.len();
        if !self.dir.is_dir() {
            return Err(format!("{} is not a directory", self.dir.display()));
        }
        if dest.exists() && !self.overwrite {
            return Err(format!("{} already exists", dest.display()));
        }
        let have_back = if dest.exists() { std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) } else { 0 };
        if let Some(free) = self.available {
            if free + have_back < total {
                return Err(format!("not enough free space: the ISO is {total} bytes, {free} are free"));
            }
        }
        let result = (|| -> Result<Report, String> {
            let mut src = std::fs::File::open(iso).map_err(|e| e.to_string())?;
            let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            let mut hash = Sha256::new();
            let mut buf = vec![0u8; CHUNK];
            let mut done = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let n = src.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(|e| format!("write failed: {e}"))?;
                done += n as u64;
                progress(Phase::Copying, done, total);
            }
            out.flush().map_err(|e| e.to_string())?;
            out.sync_all().map_err(|e| format!("could not flush to the device: {e}"))?;
            drop(out);
            let sha256: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
            std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            // Verify what is on the medium, read back from the final file.
            let mut back = std::fs::File::open(&dest).map_err(|e| e.to_string())?;
            let size = back.metadata().map_err(|e| e.to_string())?.len();
            if size != total {
                return Err(format!("the copy is {size} bytes, expected {total}"));
            }
            let mut h2 = Sha256::new();
            let mut read = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let n = back.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                h2.update(&buf[..n]);
                read += n as u64;
                progress(Phase::Verifying, read, total);
            }
            let got: String = h2.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if got != sha256 {
                return Err(format!("verification failed: the copy hashes to {got}, the ISO to {sha256}"));
            }
            Ok(Report { dest: dest.clone(), size, sha256 })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("archstaler-media-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn iso(dir: &Path, len: usize) -> PathBuf {
        let p = dir.join("a.iso");
        std::fs::write(&p, (0..len).map(|i| (i * 7 + 3) as u8).collect::<Vec<u8>>()).unwrap();
        p
    }

    #[test]
    fn copies_and_verifies() {
        let d = tmp("ok");
        let src = iso(&d, 3 * CHUNK + 17);
        let dest = d.join("stick");
        std::fs::create_dir_all(&dest).unwrap();
        let mut phases = vec![];
        let r = FolderCopy { dir: dest.clone(), available: Some(u64::MAX), overwrite: false }
            .write(&src, &mut |p, _, _| phases.push(p), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(std::fs::read(&r.dest).unwrap(), std::fs::read(&src).unwrap());
        assert!(phases.contains(&Phase::Copying) && phases.contains(&Phase::Verifying));
        assert!(!dest.join(".a.iso.part").exists());
        // A second copy needs an explicit overwrite.
        let again = FolderCopy { dir: dest.clone(), available: None, overwrite: false }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false));
        assert!(again.unwrap_err().contains("already exists"));
        assert!(FolderCopy { dir: dest, available: None, overwrite: true }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_without_space_and_cleans_up_on_cancel() {
        let d = tmp("space");
        let src = iso(&d, CHUNK);
        let dest = d.join("stick");
        std::fs::create_dir_all(&dest).unwrap();
        let e = FolderCopy { dir: dest.clone(), available: Some(10), overwrite: false }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("not enough free space"));
        let e = FolderCopy { dir: dest.clone(), available: None, overwrite: false }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(e, "cancelled");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "no partial file is left");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ventoy_needs_its_own_files() {
        let d = tmp("ventoy");
        let v = |p: &Path| Volume { name: "Ventoy".into(), mount: p.to_path_buf(), fs: "exfat".into(), total: 1, available: 1, removable: true };
        assert!(!is_ventoy(&v(&d), &[]), "a label alone is not proof");
        std::fs::create_dir(d.join("ventoy")).unwrap();
        assert!(is_ventoy(&v(&d), &[]));
        let _ = std::fs::remove_dir_all(&d);
    }
}
