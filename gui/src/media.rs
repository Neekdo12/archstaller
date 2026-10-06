//! Putting the ISO on removable media. Phase 4: copying it as a file onto a mounted Ventoy volume.
//! `MediaTarget` is the seam where a Linux-only raw flash (UDisks2) can be added later.
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct Volume {
    /// The block device (`/dev/sda1`) where the system lists devices; the volume name elsewhere.
    pub name: String,
    pub label: String,
    /// Where it is mounted; empty while `mounted` is false.
    pub mount: PathBuf,
    pub mounted: bool,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    /// Removable or hot-plugged: a USB stick or card rather than an internal disk.
    pub removable: bool,
}

/// Every volume with a file system, mounted or not. Linux asks `lsblk`, so unmounted sticks show up
/// too; elsewhere only mounted volumes are known (sysinfo).
pub fn volumes() -> Vec<Volume> {
    #[cfg(target_os = "linux")]
    if let Some(v) = lsblk_volumes() {
        return v;
    }
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|d| Volume {
            name: d.name().to_string_lossy().into_owned(),
            label: String::new(),
            mount: d.mount_point().to_path_buf(),
            mounted: true,
            fs: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn lsblk_volumes() -> Option<Vec<Volume>> {
    let out = std::process::Command::new("lsblk").args(["-J", "-b", "-o", "PATH,LABEL,FSTYPE,SIZE,MOUNTPOINT,FSAVAIL,RM,HOTPLUG,TYPE"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut list = Vec::new();
    for d in v.get("blockdevices")?.as_array()? {
        let s = |k: &str| d.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let n = |k: &str| d.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let b = |k: &str| d.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let fs = s("fstype");
        let path = s("path");
        if fs.is_empty() || ["swap", "LVM2_member", "crypto_LUKS", "linux_raid_member", "zfs_member"].contains(&fs.as_str()) || path.starts_with("/dev/zram") || path.starts_with("/dev/loop") {
            continue;
        }
        let mount = s("mountpoint");
        list.push(Volume {
            name: path,
            label: s("label"),
            mounted: !mount.is_empty(),
            mount: PathBuf::from(mount),
            fs,
            total: n("size"),
            available: n("fsavail"),
            removable: b("rm") || b("hotplug"),
        });
    }
    Some(list)
}

/// Mounts `dev` the way a desktop does (udisks, no root needed) and returns its mount point.
pub fn mount(dev: &str) -> Result<PathBuf, String> {
    let out = std::process::Command::new("udisksctl").args(["mount", "--no-user-interaction", "-b", dev]).output().map_err(|e| format!("udisksctl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        return Err(format!("could not mount {dev}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // "Mounted /dev/sda1 at /run/media/user/Ventoy"
    text.split(" at ").nth(1).map(|p| PathBuf::from(p.trim().trim_end_matches('.'))).ok_or_else(|| format!("mounted {dev}, but its mount point is unknown: {}", text.trim()))
}

/// Flushes everything to the device, then unmounts it, so the stick can be pulled out.
pub fn sync_and_unmount(dev: &str) -> Result<(), String> {
    let _ = std::process::Command::new("sync").status();
    let out = std::process::Command::new("udisksctl").args(["unmount", "--no-user-interaction", "-b", dev]).output().map_err(|e| format!("udisksctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("could not unmount {dev}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// `/dev/sda1` -> `/dev/sda`, `/dev/nvme0n1p2` -> `/dev/nvme0n1`.
fn disk_of(dev: &str) -> String {
    let t = dev.trim_end_matches(|c: char| c.is_ascii_digit());
    if t.len() < dev.len() {
        if let Some(base) = t.strip_suffix('p') {
            if base.ends_with(|c: char| c.is_ascii_digit()) {
                return base.to_string();
            }
        }
        return t.to_string();
    }
    dev.to_string()
}

/// The device node carrying a file system label (Linux: `/dev/disk/by-label`).
fn device_by_label(label: &str) -> Option<String> {
    std::fs::canonicalize(Path::new("/dev/disk/by-label").join(label)).ok().map(|p| p.to_string_lossy().into_owned())
}

/// Whether `v` is a Ventoy data volume. A fresh Ventoy stick has no `ventoy` folder yet, so the proofs are:
/// Ventoy's own files on the volume, a `VTOYEFI` partition on the same disk, or the volume itself being
/// labelled `Ventoy` (on systems that report labels as the volume name).
pub fn is_ventoy(v: &Volume, _all: &[Volume]) -> bool {
    if v.mounted && (v.mount.join("ventoy").is_dir() || v.mount.join("ventoy.json").is_file()) {
        return true;
    }
    if v.name.eq_ignore_ascii_case("ventoy") || v.label.eq_ignore_ascii_case("ventoy") {
        return true;
    }
    if let Some(efi) = device_by_label("VTOYEFI") {
        if v.name.starts_with("/dev/") && disk_of(&efi) == disk_of(&v.name) && efi != v.name {
            return true;
        }
    }
    device_by_label("Ventoy").is_some_and(|d| d == v.name)
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
    /// Older ISOs to delete before copying (the user agreed to it in the prompt).
    pub delete: Vec<PathBuf>,
    /// Root of the volume: after every deleted ISO its `.Trash-<uid>` folder is removed too.
    pub trash_root: Option<PathBuf>,
    /// Name of the copy on the stick when it must differ from the ISO's own (the "rename" answer).
    pub rename_to: Option<String>,
}

/// The base of an ISO name: lower case, without extension and without a trailing `-2`, `_20260101` and so on.
fn base_name(name: &str) -> String {
    let stem = name.strip_suffix(".iso").or_else(|| name.strip_suffix(".ISO")).unwrap_or(name).to_ascii_lowercase();
    let t = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    match t.strip_suffix(['-', '_']) {
        Some(b) if t.len() < stem.len() && !b.is_empty() => b.to_string(),
        _ => stem,
    }
}

/// ISOs already in `dir` that look like earlier builds of `iso_name`: the same name, or the same base
/// name with a number or date suffix. Sorted by name.
pub fn older_isos(dir: &Path, iso_name: &str) -> Vec<PathBuf> {
    let base = base_name(iso_name);
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            n.to_ascii_lowercase().ends_with(".iso") && !n.starts_with('.') && base_name(&n) == base
        })
        .collect();
    found.sort();
    found
}

/// A name for the new copy that does not exist in `dir`: `a.iso`, then `a-2.iso`, `a-3.iso`, ...
pub fn free_name(dir: &Path, iso_name: &str) -> String {
    if !dir.join(iso_name).exists() {
        return iso_name.to_string();
    }
    let stem = iso_name.strip_suffix(".iso").unwrap_or(iso_name);
    (2u32..)
        .map(|n| format!("{stem}-{n}.iso"))
        .find(|n| !dir.join(n).exists())
        .unwrap()
}

/// The folder a file manager makes for deleted files on a volume: `.Trash-<uid>` in its top directory.
#[cfg(unix)]
fn trash_dir(root: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").ok()?.uid();
    Some(root.join(format!(".Trash-{uid}")))
}

#[cfg(not(unix))]
fn trash_dir(_root: &Path) -> Option<PathBuf> {
    None
}

/// Deletes `file` for good and then the volume's trash folder, so a deleted ISO does not keep using space.
pub fn delete_iso(file: &Path, root: Option<&Path>) -> Result<(), String> {
    std::fs::remove_file(file).map_err(|e| format!("cannot delete {}: {e}", file.display()))?;
    if let Some(t) = root.and_then(trash_dir) {
        if t.is_dir() {
            let _ = std::fs::remove_dir_all(&t);
        }
    }
    Ok(())
}

const CHUNK: usize = 1 << 20;

impl MediaTarget for FolderCopy {
    fn write(&self, iso: &Path, progress: &mut dyn FnMut(Phase, u64, u64), cancel: &AtomicBool) -> Result<Report, String> {
        let name = match &self.rename_to {
            Some(n) => std::ffi::OsString::from(n),
            None => iso.file_name().ok_or("the ISO has no file name")?.to_os_string(),
        };
        let dest = self.dir.join(&name);
        let part = self.dir.join(format!(".{}.part", name.to_string_lossy()));
        let total = std::fs::metadata(iso).map_err(|e| format!("{}: {e}", iso.display()))?.len();
        if !self.dir.is_dir() {
            return Err(format!("{} is not a directory", self.dir.display()));
        }
        let mut freed = 0u64;
        for f in &self.delete {
            freed += std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            delete_iso(f, self.trash_root.as_deref())?;
        }
        if dest.exists() && !self.overwrite {
            return Err(format!("{} already exists", dest.display()));
        }
        let have_back = freed + if dest.exists() { std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) } else { 0 };
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
        let r = FolderCopy { dir: dest.clone(), available: Some(u64::MAX), overwrite: false, delete: vec![], trash_root: None, rename_to: None }
            .write(&src, &mut |p, _, _| phases.push(p), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(std::fs::read(&r.dest).unwrap(), std::fs::read(&src).unwrap());
        assert!(phases.contains(&Phase::Copying) && phases.contains(&Phase::Verifying));
        assert!(!dest.join(".a.iso.part").exists());
        // A second copy needs an explicit overwrite.
        let again = FolderCopy { dir: dest.clone(), available: None, overwrite: false, delete: vec![], trash_root: None, rename_to: None }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false));
        assert!(again.unwrap_err().contains("already exists"));
        assert!(FolderCopy { dir: dest, available: None, overwrite: true, delete: vec![], trash_root: None, rename_to: None }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_without_space_and_cleans_up_on_cancel() {
        let d = tmp("space");
        let src = iso(&d, CHUNK);
        let dest = d.join("stick");
        std::fs::create_dir_all(&dest).unwrap();
        let e = FolderCopy { dir: dest.clone(), available: Some(10), overwrite: false, delete: vec![], trash_root: None, rename_to: None }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("not enough free space"));
        let e = FolderCopy { dir: dest.clone(), available: None, overwrite: false, delete: vec![], trash_root: None, rename_to: None }.write(&src, &mut |_, _, _| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(e, "cancelled");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "no partial file is left");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_older_builds_and_names_the_new_one() {
        let d = tmp("older");
        for n in ["archstaler.iso", "archstaler-2.iso", "Archstaler_20260101.ISO", "ubuntu.iso", ".archstaler.iso.part", "archstaler.txt"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let names: Vec<String> = older_isos(&d, "archstaler.iso").iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Archstaler_20260101.ISO", "archstaler-2.iso", "archstaler.iso"]);
        assert_eq!(free_name(&d, "archstaler.iso"), "archstaler-3.iso");
        assert_eq!(free_name(&d, "new.iso"), "new.iso");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_and_rename_work_and_trash_goes() {
        let d = tmp("del");
        let src = iso(&d, 1000);
        let stick = d.join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        std::fs::write(stick.join("a.iso"), vec![1u8; 5000]).unwrap();
        let trash = trash_dir(&stick);
        if let Some(t) = &trash {
            std::fs::create_dir_all(t.join("files")).unwrap();
        }
        let r = FolderCopy { dir: stick.clone(), available: Some(1000), overwrite: false, delete: vec![stick.join("a.iso")], trash_root: Some(stick.clone()), rename_to: None }
            .write(&src, &mut |_, _, _| {}, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(std::fs::read(&r.dest).unwrap(), std::fs::read(&src).unwrap());
        if let Some(t) = &trash {
            assert!(!t.exists(), "the trash folder is removed");
        }
        let r = FolderCopy { dir: stick.clone(), available: None, overwrite: false, delete: vec![], trash_root: None, rename_to: Some("a-2.iso".into()) }
            .write(&src, &mut |_, _, _| {}, &AtomicBool::new(false))
            .unwrap();
        assert!(r.dest.ends_with("a-2.iso") && stick.join("a.iso").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn disk_names() {
        assert_eq!(disk_of("/dev/sda1"), "/dev/sda");
        assert_eq!(disk_of("/dev/nvme0n1p2"), "/dev/nvme0n1");
        assert_eq!(disk_of("/dev/mmcblk0p1"), "/dev/mmcblk0");
    }

    #[test]
    fn ventoy_is_recognised_by_its_files() {
        let d = tmp("ventoy");
        let v = |p: &Path| Volume { name: "/dev/zzz9".into(), mount: p.to_path_buf(), fs: "exfat".into(), total: 1, available: 1, removable: true, label: String::new(), mounted: true };
        assert!(!is_ventoy(&v(&d), &[]), "an ordinary volume is not Ventoy");
        std::fs::create_dir(d.join("ventoy")).unwrap();
        assert!(is_ventoy(&v(&d), &[]));
        let _ = std::fs::remove_dir_all(&d);
    }
}
