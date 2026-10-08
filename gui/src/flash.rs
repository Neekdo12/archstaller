//! Raw flashing (plans-implement/raw-usb-flash.md): writing the hybrid ISO over a whole USB stick, offered when no
//! Ventoy volume is found. Linux only, through UDisks2: it unmounts the stick's file systems and opens the
//! device, asking for authorization through the desktop's polkit agent. Nothing here runs `sudo` or a shell.
//! Devices come from `lsblk` and sysfs; the eligibility rules and the writer are plain functions, tested
//! with fixtures and a regular file standing in for the device. Only `UDisks` touches D-Bus.
use crate::media::{MediaTarget, Phase, Report};
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Mount points a desktop uses for removable media; a stick mounted anywhere else is treated as part of
/// the running system and not offered.
const MEDIA_MOUNTS: &[&str] = &["/run/media/", "/media/", "/mnt/"];
const COLUMNS: &str = "PATH,KNAME,MAJ:MIN,TYPE,TRAN,RM,HOTPLUG,RO,SIZE,MODEL,VENDOR,SERIAL,MOUNTPOINTS";
const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub path: String,
    pub kname: String,
    pub maj_min: (u32, u32),
    /// Where its file systems are mounted (`[SWAP]` for active swap).
    pub mounts: Vec<String>,
}

/// A whole block device as the system describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDevice {
    pub path: String,
    /// Kernel name (`sdb`); the UDisks2 object is named after it.
    pub kname: String,
    pub maj_min: (u32, u32),
    /// `/sys/dev/block/M:m` resolved: where the device sits in the hardware tree.
    pub sysfs: String,
    pub size: u64,
    pub model: String,
    pub vendor: String,
    pub serial: String,
    /// Transport reported by udev (`usb`, `nvme`, `sata`, ...).
    pub tran: String,
    pub removable: bool,
    pub read_only: bool,
    /// File systems mounted on the whole device (a stick formatted without a partition table).
    pub mounts: Vec<String>,
    pub partitions: Vec<Partition>,
    /// Device mapper, RAID, LVM and other users of the disk or its partitions.
    pub holders: Vec<String>,
}

impl RawDevice {
    /// "SanDisk Ultra", or the device path when the drive reports no names.
    pub fn name(&self) -> String {
        let n = format!("{} {}", self.vendor, self.model).trim().to_string();
        if n.is_empty() {
            self.path.clone()
        } else {
            n
        }
    }

    /// The same physical device: what a hot-plug reorder or a swapped stick would change. Mounts are left
    /// out on purpose; they change when the stick is unmounted before writing.
    pub fn same_device(&self, o: &RawDevice) -> bool {
        self.kname == o.kname && self.maj_min == o.maj_min && self.sysfs == o.sysfs && self.size == o.size && self.serial == o.serial && self.model == o.model && self.vendor == o.vendor
    }

    /// (kernel name, device path, mount point) of every mounted file system on the device.
    pub fn mounted(&self) -> Vec<(String, String, String)> {
        let mut out: Vec<(String, String, String)> = self.mounts.iter().map(|m| (self.kname.clone(), self.path.clone(), m.clone())).collect();
        for p in &self.partitions {
            out.extend(p.mounts.iter().map(|m| (p.kname.clone(), p.path.clone(), m.clone())));
        }
        out
    }
}

/// A device and why it may not be flashed (empty: it may).
#[derive(Debug, Clone)]
pub struct Candidate {
    pub dev: RawDevice,
    pub problems: Vec<String>,
}

impl Candidate {
    pub fn eligible(&self) -> bool {
        self.problems.is_empty()
    }

    /// Worth listing with its reasons: a USB or removable device the user may have expected to see.
    pub fn looks_external(&self) -> bool {
        self.dev.tran == "usb" || self.dev.removable
    }
}

/// The ISO to write, checked before anything is asked or touched.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub len: u64,
    /// major:minor of the file system the ISO lives on.
    pub dev: (u32, u32),
}

impl Source {
    pub fn of(iso: &Path) -> Result<Source, String> {
        use std::os::unix::fs::MetadataExt;
        let path = std::fs::canonicalize(iso).map_err(|e| format!("{}: {e}", iso.display()))?;
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        // ISO 9660: the primary volume descriptor at 32 KiB starts with 1 "CD001".
        let mut f = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut magic = [0u8; 6];
        if meta.len() < 0x8006 || f.seek(SeekFrom::Start(0x8000)).and_then(|_| f.read_exact(&mut magic)).is_err() || &magic[1..] != b"CD001" {
            return Err(format!("{} is not an ISO image", path.display()));
        }
        Ok(Source { path, len: meta.len(), dev: split_dev(meta.dev()) })
    }
}

/// Linux `dev_t` -> (major, minor).
fn split_dev(d: u64) -> (u32, u32) {
    let major = ((d >> 8) & 0xfff) | ((d >> 32) & !0xfff);
    let minor = (d & 0xff) | ((d >> 12) & !0xff);
    (major as u32, minor as u32)
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MiB", b >> 20)
    }
}

fn parse_maj_min(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.trim().split_once(':')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Reads `lsblk -J -b -o COLUMNS` output. `sys(kname, maj_min)` returns the resolved sysfs path and the
/// sysfs holders of a block device (injected so tests need no `/sys`).
pub fn parse_lsblk(v: &serde_json::Value, sys: &dyn Fn(&str, (u32, u32)) -> (String, Vec<String>)) -> Vec<RawDevice> {
    fn s(d: &serde_json::Value, k: &str) -> String {
        d.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
    }
    // Old lsblk versions print flags and sizes as strings.
    fn flag(d: &serde_json::Value, k: &str) -> bool {
        match d.get(k) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(t)) => t.trim() == "1",
            Some(serde_json::Value::Number(n)) => n.as_u64() == Some(1),
            _ => false,
        }
    }
    fn num(d: &serde_json::Value, k: &str) -> u64 {
        match d.get(k) {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(serde_json::Value::String(t)) => t.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }
    fn mounts(d: &serde_json::Value) -> Vec<String> {
        let mut m: Vec<String> = d.get("mountpoints").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default();
        if let Some(one) = d.get("mountpoint").and_then(|x| x.as_str()) {
            m.push(one.to_string());
        }
        m.retain(|x| !x.is_empty());
        m
    }
    fn children(d: &serde_json::Value) -> &[serde_json::Value] {
        d.get("children").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
    }
    let mut list = Vec::new();
    for d in v.get("blockdevices").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
        if s(d, "type") != "disk" {
            continue;
        }
        let kname = s(d, "kname");
        let maj_min = parse_maj_min(&s(d, "maj:min")).unwrap_or((0, 0));
        let (sysfs, mut holders) = sys(&kname, maj_min);
        let mut partitions = Vec::new();
        for c in children(d) {
            let ck = s(c, "kname");
            if s(c, "type") == "part" {
                let cm = parse_maj_min(&s(c, "maj:min")).unwrap_or((0, 0));
                holders.extend(sys(&ck, cm).1);
                for h in children(c) {
                    holders.push(format!("{} ({})", s(h, "kname"), s(h, "type")));
                }
                partitions.push(Partition { path: s(c, "path"), kname: ck, maj_min: cm, mounts: mounts(c) });
            } else {
                holders.push(format!("{} ({})", ck, s(c, "type")));
            }
        }
        holders.sort();
        holders.dedup();
        list.push(RawDevice {
            path: s(d, "path"),
            kname,
            maj_min,
            sysfs,
            size: num(d, "size"),
            model: s(d, "model"),
            vendor: s(d, "vendor"),
            serial: s(d, "serial"),
            tran: s(d, "tran"),
            removable: flag(d, "rm") || flag(d, "hotplug"),
            read_only: flag(d, "ro"),
            mounts: mounts(d),
            partitions,
            holders,
        });
    }
    list
}

/// Why `d` must not be flashed with `src`; empty when it may. Unknown or conflicting information counts
/// against the device.
pub fn problems(d: &RawDevice, src: &Source) -> Vec<String> {
    let mut p = Vec::new();
    let kname_ok = !d.kname.is_empty() && d.kname.chars().all(|c| c.is_ascii_alphanumeric());
    if !kname_ok || d.maj_min == (0, 0) || d.sysfs.is_empty() || d.size == 0 || !d.path.starts_with("/dev/") {
        p.push("its identity or size is unknown".to_string());
    }
    match (d.tran == "usb", d.sysfs.contains("/usb")) {
        (true, true) => {}
        (false, false) => p.push(format!("not a USB device ({})", if d.tran.is_empty() { "unknown bus" } else { d.tran.as_str() })),
        _ => p.push("the system's USB information about it is inconsistent".to_string()),
    }
    if !d.removable {
        p.push("not marked removable or hot-pluggable".to_string());
    }
    if d.read_only {
        p.push("read-only".to_string());
    }
    if d.size != 0 && d.size < src.len {
        p.push(format!("too small: {}, the ISO needs {}", human(d.size), human(src.len)));
    }
    if !d.holders.is_empty() {
        p.push(format!("in use by {}", d.holders.join(", ")));
    }
    for (_, path, m) in d.mounted() {
        if m == "[SWAP]" {
            p.push(format!("{path} is active swap"));
        } else if !MEDIA_MOUNTS.iter().any(|pre| m.starts_with(pre)) {
            p.push(format!("{path} is mounted at {m}, which looks like part of the running system"));
        } else if src.path.starts_with(&m) {
            p.push(format!("the ISO itself is on {path}"));
        }
    }
    if d.maj_min == src.dev || d.partitions.iter().any(|x| x.maj_min == src.dev) {
        p.push("the ISO itself is on this device".to_string());
    }
    p.dedup();
    p
}

fn sysfs_info(kname: &str, mm: (u32, u32)) -> (String, Vec<String>) {
    let sysfs = std::fs::canonicalize(format!("/sys/dev/block/{}:{}", mm.0, mm.1)).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let holders = std::fs::read_dir(format!("/sys/class/block/{kname}/holders")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    (sysfs, holders)
}

fn lsblk(dev: Option<&str>) -> Result<Vec<RawDevice>, String> {
    let mut cmd = std::process::Command::new("lsblk");
    cmd.args(["-J", "-b", "-o", COLUMNS]);
    if let Some(d) = dev {
        cmd.arg(d);
    }
    let out = cmd.output().map_err(|e| format!("lsblk: {e}"))?;
    if !out.status.success() {
        return Err(format!("lsblk failed (util-linux 2.37 or newer is needed): {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("lsblk: {e}"))?;
    Ok(parse_lsblk(&v, &sysfs_info))
}

/// Every whole disk with the reasons it may not be flashed.
pub fn scan(src: &Source) -> Result<Vec<Candidate>, String> {
    Ok(lsblk(None)?.into_iter().map(|dev| Candidate { problems: problems(&dev, src), dev }).collect())
}

/// What raw flashing needs from the system. `UDisks` is the real one; tests use a file.
pub trait Backend: Send {
    /// The device with this kernel name as the system describes it now.
    fn rescan(&self, kname: &str) -> Result<RawDevice, String>;
    /// Unmounts the file system on the block device `kname`.
    fn unmount(&self, kname: &str) -> Result<(), String>;
    /// Opens the whole device for reading and writing, exclusively, and checks that the handle really
    /// is `dev` (its device number).
    fn open(&self, dev: &RawDevice) -> Result<File, String>;
    /// Drops cached pages so the read-back comes from the medium, not from memory.
    fn drop_cache(&self, f: &File);
    fn power_off(&self, dev: &RawDevice) -> Result<(), String>;
}

/// Writes the ISO over a whole USB device, then reads it back.
pub struct RawBlockFlash {
    pub dev: RawDevice,
    pub backend: Box<dyn Backend>,
}

impl RawBlockFlash {
    /// The selected device, re-read now: still the same physical device and still eligible.
    fn current(&self, src: &Source) -> Result<RawDevice, String> {
        let now = self.backend.rescan(&self.dev.kname).map_err(|e| format!("{} is gone ({e}); nothing was written", self.dev.path))?;
        if !now.same_device(&self.dev) {
            return Err(format!("{} is no longer the device you selected; select it again. Nothing was written", self.dev.path));
        }
        let p = problems(&now, src);
        if !p.is_empty() {
            return Err(format!("{} can no longer be flashed: {}. Nothing was written", self.dev.path, p.join("; ")));
        }
        Ok(now)
    }
}

impl MediaTarget for RawBlockFlash {
    fn write(&self, iso: &Path, progress: &mut dyn FnMut(Phase, u64, u64), cancel: &AtomicBool) -> Result<Report, String> {
        let src = Source::of(iso)?;
        progress(Phase::Unmounting, 0, 1);
        let now = self.current(&src)?;
        for (kname, path, _) in now.mounted() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled; nothing was written".into());
            }
            self.backend.unmount(&kname).map_err(|e| format!("could not unmount {path}: {e}. Nothing was written"))?;
        }
        let now = self.current(&src)?;
        if let Some((_, path, m)) = now.mounted().first() {
            return Err(format!("{path} is still mounted at {m}; nothing was written"));
        }
        progress(Phase::Unmounting, 1, 1);
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled; nothing was written".into());
        }
        let mut f = self.backend.open(&now).map_err(|e| format!("could not open {}: {e}. Nothing was written", now.path))?;
        // Again with the device held: nothing may have changed between the check and the open.
        let now = self.current(&src)?;
        if !now.mounted().is_empty() {
            return Err(format!("{} was mounted again; nothing was written", now.path));
        }
        let backend = &self.backend;
        let sha256 = write_verify(&mut f, &src.path, src.len, progress, cancel, &|f| backend.drop_cache(f))?;
        drop(f);
        progress(Phase::Ejecting, 0, 1);
        let note = match self.backend.power_off(&now) {
            Ok(()) => "It has been powered off and can be unplugged.".to_string(),
            Err(e) => format!("It could not be powered off ({e}); use the desktop's safe removal before unplugging."),
        };
        progress(Phase::Ejecting, 1, 1);
        Ok(Report { dest: PathBuf::from(&now.path), size: src.len, sha256, note: Some(note), flashed: true })
    }
}

/// Streams `iso` (`total` bytes) to offset 0 of `dev`, flushes it, drops the cache and reads the same
/// number of bytes back. Returns the SHA-256 both sides agree on. The rest of the device is not touched.
pub fn write_verify(dev: &mut File, iso: &Path, total: u64, progress: &mut dyn FnMut(Phase, u64, u64), cancel: &AtomicBool, drop_cache: &dyn Fn(&File)) -> Result<String, String> {
    let mut src = File::open(iso).map_err(|e| format!("{}: {e}. Nothing was written", iso.display()))?;
    dev.seek(SeekFrom::Start(0)).map_err(|e| format!("{e}; nothing was written"))?;
    let partial = |done: u64| format!("the device is partially overwritten ({done} of {total} bytes) and not verified; flash it again before using it");
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = dev.sync_all();
            return Err(format!("cancelled: {}", partial(done)));
        }
        let n = src.read(&mut buf).map_err(|e| format!("reading the ISO failed: {e}; {}", partial(done)))?;
        if n == 0 {
            break;
        }
        if done + n as u64 > total {
            return Err(format!("the ISO grew while it was written; {}", partial(done)));
        }
        hash.update(&buf[..n]);
        // write_all retries short writes and fails on a write of zero bytes.
        dev.write_all(&buf[..n]).map_err(|e| format!("write failed at byte {done}: {e}; {}", partial(done)))?;
        done += n as u64;
        progress(Phase::Writing, done, total);
    }
    if done != total {
        return Err(format!("the ISO shrank while it was written; {}", partial(done)));
    }
    progress(Phase::Flushing, 0, 1);
    dev.flush().and_then(|_| dev.sync_all()).map_err(|e| format!("flushing failed: {e}; {}", partial(done)))?;
    drop_cache(dev);
    progress(Phase::Flushing, 1, 1);
    let want: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    dev.seek(SeekFrom::Start(0)).map_err(|e| format!("{e}; the device is written but not verified"))?;
    let mut back = Sha256::new();
    let mut read = 0u64;
    while read < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled during verification: the device is written but not verified".into());
        }
        let n = CHUNK.min((total - read) as usize);
        dev.read_exact(&mut buf[..n]).map_err(|e| format!("reading back failed at byte {read}: {e}; the device is written but not verified"))?;
        back.update(&buf[..n]);
        read += n as u64;
        progress(Phase::Verifying, read, total);
    }
    let got: String = back.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err(format!("verification failed: the device reads back as {got}, the ISO is {want}; do not boot from it, flash it again"));
    }
    Ok(want)
}

/// UDisks2 over the system D-Bus. Every call may show the desktop's authorization dialog.
pub struct UDisks;

const BUS: &str = "org.freedesktop.UDisks2";
/// OpenDevice appeared in UDisks 2.7.3.
const MIN_VERSION: (u32, u32, u32) = (2, 7, 3);
/// Long enough for the user to answer an authorization dialog.
const TIMEOUT_MS: i32 = 300_000;

fn object_path(kname: &str) -> String {
    let mut p = String::from("/org/freedesktop/UDisks2/block_devices/");
    for c in kname.chars() {
        if c.is_ascii_alphanumeric() {
            p.push(c);
        } else {
            p.push_str(&format!("_{:02x}", c as u32));
        }
    }
    p
}

/// A D-Bus `a{sv}` options dictionary.
fn options(pairs: Vec<(&str, glib::Variant)>) -> HashMap<String, glib::Variant> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn version_at_least(v: &str, min: (u32, u32, u32)) -> bool {
    let mut it = v.trim().split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().unwrap_or(0));
    let got = (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0));
    got >= min
}

impl UDisks {
    fn bus() -> Result<gio::DBusConnection, String> {
        gio::bus_get_sync(gio::BusType::System, None::<&gio::Cancellable>).map_err(|e| format!("system D-Bus: {}", e.message()))
    }

    fn call(c: &gio::DBusConnection, path: &str, iface: &str, method: &str, args: glib::Variant, reply: &str) -> Result<glib::Variant, String> {
        let ty = glib::VariantTy::new(reply).map_err(|e| e.to_string())?;
        c.call_sync(Some(BUS), path, iface, method, Some(&args), Some(ty), gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, TIMEOUT_MS, None::<&gio::Cancellable>).map_err(|e| e.message().to_string())
    }

    fn property(c: &gio::DBusConnection, path: &str, iface: &str, name: &str) -> Result<glib::Variant, String> {
        let r = Self::call(c, path, "org.freedesktop.DBus.Properties", "Get", (iface, name).to_variant(), "(v)")?;
        r.child_value(0).as_variant().ok_or_else(|| format!("{name}: not a variant"))
    }

    /// Whether raw flashing can work here: UDisks2 runs and is new enough to hand out device handles.
    pub fn ready() -> Result<(), String> {
        let c = Self::bus()?;
        let v = Self::property(&c, "/org/freedesktop/UDisks2/Manager", "org.freedesktop.UDisks2.Manager", "Version").map_err(|e| format!("the UDisks2 service is not available ({e})"))?;
        let v = v.str().unwrap_or("").to_string();
        if !version_at_least(&v, MIN_VERSION) {
            return Err(format!("UDisks2 {v} is too old; 2.7.3 or newer is needed"));
        }
        Ok(())
    }
}

impl Backend for UDisks {
    fn rescan(&self, kname: &str) -> Result<RawDevice, String> {
        lsblk(Some(&format!("/dev/{kname}")))?.into_iter().find(|d| d.kname == kname).ok_or_else(|| "not found".to_string())
    }

    fn unmount(&self, kname: &str) -> Result<(), String> {
        let c = Self::bus()?;
        Self::call(&c, &object_path(kname), "org.freedesktop.UDisks2.Filesystem", "Unmount", (options(vec![]),).to_variant(), "()").map(|_| ())
    }

    fn open(&self, dev: &RawDevice) -> Result<File, String> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let c = Self::bus()?;
        let args = ("rw", options(vec![("flags", (libc::O_EXCL | libc::O_CLOEXEC).to_variant())])).to_variant();
        let ty = glib::VariantTy::new("(h)").map_err(|e| e.to_string())?;
        let (reply, fds) = c
            .call_with_unix_fd_list_sync(Some(BUS), &object_path(&dev.kname), "org.freedesktop.UDisks2.Block", "OpenDevice", Some(&args), Some(ty), gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, TIMEOUT_MS, None::<&gio::UnixFDList>, None::<&gio::Cancellable>)
            .map_err(|e| {
                let m = e.message().to_string();
                if m.contains("NotAuthorized") {
                    format!("{m}. Opening a drive needs your password: start a polkit authentication agent (for example polkit-kde-authentication-agent-1) in your session and try again")
                } else {
                    m
                }
            })?;
        let idx = reply.child_value(0).get::<glib::variant::Handle>().ok_or("UDisks2 answered without a handle")?.0;
        let fd = fds.ok_or("UDisks2 sent no file descriptor")?.get(idx).map_err(|e| e.message().to_string())?;
        let f = File::from(fd);
        let meta = f.metadata().map_err(|e| e.to_string())?;
        if !meta.file_type().is_block_device() || split_dev(meta.rdev()) != dev.maj_min {
            return Err("the opened handle is not the selected device".into());
        }
        Ok(f)
    }

    fn drop_cache(&self, f: &File) {
        use std::os::fd::AsRawFd;
        // SAFETY: a plain advisory call on a descriptor this function borrows.
        unsafe {
            libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }

    fn power_off(&self, dev: &RawDevice) -> Result<(), String> {
        let c = Self::bus()?;
        let drive = Self::property(&c, &object_path(&dev.kname), "org.freedesktop.UDisks2.Block", "Drive")?;
        let drive = drive.str().filter(|d| *d != "/").ok_or("no drive object")?.to_string();
        let none = || (options(vec![]),).to_variant();
        Self::call(&c, &drive, "org.freedesktop.UDisks2.Drive", "PowerOff", none(), "()").or_else(|_| Self::call(&c, &drive, "org.freedesktop.UDisks2.Drive", "Eject", none(), "()")).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const LSBLK: &str = r#"{"blockdevices": [
      {"path":"/dev/sda","kname":"sda","maj:min":"8:0","type":"disk","tran":"usb","rm":true,"hotplug":true,"ro":false,"size":16008609792,"model":"Ultra ","vendor":"SanDisk ","serial":"4C53","mountpoints":[null],
       "children":[{"path":"/dev/sda1","kname":"sda1","maj:min":"8:1","type":"part","tran":null,"rm":true,"hotplug":true,"ro":false,"size":16007561216,"model":null,"vendor":null,"serial":null,"mountpoints":["/run/media/u/STICK"]}]},
      {"path":"/dev/nvme0n1","kname":"nvme0n1","maj:min":"259:0","type":"disk","tran":"nvme","rm":false,"hotplug":false,"ro":false,"size":512110190592,"model":"Samsung SSD","vendor":null,"serial":"S4","mountpoints":[null],
       "children":[{"path":"/dev/nvme0n1p1","kname":"nvme0n1p1","maj:min":"259:1","type":"part","mountpoints":["/boot"]},
                   {"path":"/dev/nvme0n1p2","kname":"nvme0n1p2","maj:min":"259:2","type":"part","mountpoints":["/"]}]},
      {"path":"/dev/sdb","kname":"sdb","maj:min":"8:16","type":"disk","tran":"usb","rm":"1","hotplug":"1","ro":"0","size":"8004304896","model":"Flash","vendor":"Kingston","serial":"K1","mountpoints":[null],
       "children":[{"path":"/dev/sdb1","kname":"sdb1","maj:min":"8:17","type":"part","mountpoints":[null],"children":[{"kname":"dm-0","type":"crypt"}]}]},
      {"path":"/dev/sr0","kname":"sr0","maj:min":"11:0","type":"rom","tran":"sata","rm":true}
    ]}"#;

    fn sys(kname: &str, mm: (u32, u32)) -> (String, Vec<String>) {
        let bus = if kname.starts_with("nvme") { "pci0000:00/0000:00:1d.0/nvme/nvme0" } else { "pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0" };
        (format!("/sys/devices/{bus}/block/{kname}/{}:{}", mm.0, mm.1), vec![])
    }

    fn devices() -> Vec<RawDevice> {
        parse_lsblk(&serde_json::from_str(LSBLK).unwrap(), &sys)
    }

    fn src(len: u64) -> Source {
        Source { path: PathBuf::from("/home/u/archstaler.iso"), len, dev: (259, 2) }
    }

    #[test]
    fn reads_lsblk() {
        let d = devices();
        assert_eq!(d.iter().map(|d| d.kname.as_str()).collect::<Vec<_>>(), ["sda", "nvme0n1", "sdb"], "only whole disks");
        assert_eq!(d[0].name(), "SanDisk Ultra");
        assert_eq!(d[0].mounted(), [("sda1".to_string(), "/dev/sda1".to_string(), "/run/media/u/STICK".to_string())]);
        assert!(d[2].removable && !d[2].read_only && d[2].size == 8004304896, "string flags and sizes of old lsblk");
        assert_eq!(d[2].holders, ["dm-0 (crypt)"]);
    }

    #[test]
    fn only_a_free_usb_stick_is_eligible() {
        let d = devices();
        assert!(problems(&d[0], &src(1 << 20)).is_empty(), "a mounted USB stick may be flashed (it is unmounted first)");
        let internal = problems(&d[1], &src(1 << 20)).join("; ");
        assert!(internal.contains("not a USB device") && internal.contains("mounted at /boot") && internal.contains("the ISO itself"), "{internal}");
        assert!(problems(&d[2], &src(1 << 20)).join(";").contains("in use by dm-0"));
        assert!(problems(&d[0], &src(1 << 40)).join(";").contains("too small"));
    }

    #[test]
    fn unclear_or_dangerous_devices_are_refused() {
        let base = devices().remove(0);
        let has = |d: &RawDevice, s: &Source, what: &str| problems(d, s).join("; ").contains(what);
        let mut d = base.clone();
        d.sysfs = "/sys/devices/pci0000:00/0000:00:17.0/ata1/host0/block/sda".into();
        assert!(has(&d, &src(1), "inconsistent"));
        let mut d = base.clone();
        d.maj_min = (0, 0);
        assert!(has(&d, &src(1), "identity"));
        let mut d = base.clone();
        d.read_only = true;
        assert!(has(&d, &src(1), "read-only"));
        let mut d = base.clone();
        d.partitions[0].mounts = vec!["[SWAP]".into()];
        assert!(has(&d, &src(1), "active swap"));
        let mut d = base.clone();
        d.partitions[0].mounts = vec!["/home".into()];
        assert!(has(&d, &src(1), "running system"));
        let mut d = base.clone();
        d.removable = false;
        assert!(has(&d, &src(1), "removable"));
        let on_stick = Source { path: PathBuf::from("/run/media/u/STICK/a.iso"), len: 1, dev: (8, 1) };
        assert!(has(&base, &on_stick, "the ISO itself"));
    }

    #[test]
    fn identity_ignores_mounts_only() {
        let a = devices().remove(0);
        let mut b = a.clone();
        b.partitions[0].mounts.clear();
        assert!(a.same_device(&b));
        b.maj_min = (8, 32);
        assert!(!a.same_device(&b));
        let mut c = a.clone();
        c.serial = "other".into();
        assert!(!a.same_device(&c));
    }

    #[test]
    fn helpers() {
        assert!(version_at_least("2.10.1", MIN_VERSION) && version_at_least("2.7.3", MIN_VERSION) && !version_at_least("2.7.2", MIN_VERSION) && !version_at_least("", MIN_VERSION));
        assert_eq!(object_path("sdb"), "/org/freedesktop/UDisks2/block_devices/sdb");
        assert_eq!(object_path("a-b"), "/org/freedesktop/UDisks2/block_devices/a_2db");
        assert_eq!(split_dev(0x0811), (8, 17));
        assert_eq!(human(3 << 29), "1.5 GiB");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("archstaler-flash-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An ISO-looking file: the `CD001` signature where Source::of looks for it.
    fn iso(dir: &Path, len: usize) -> PathBuf {
        let mut data: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
        data[0x8001..0x8006].copy_from_slice(b"CD001");
        let p = dir.join("a.iso");
        std::fs::write(&p, data).unwrap();
        p
    }

    fn device_file(dir: &Path, len: usize) -> PathBuf {
        let p = dir.join("device");
        std::fs::write(&p, vec![0xeeu8; len]).unwrap();
        p
    }

    fn rw(p: &Path) -> File {
        std::fs::OpenOptions::new().read(true).write(true).open(p).unwrap()
    }

    #[test]
    fn writes_at_offset_zero_and_verifies() {
        let d = tmp("ok");
        let src = iso(&d, 2 * CHUNK + 5);
        let dev = device_file(&d, 4 * CHUNK);
        let mut phases = vec![];
        let sha = write_verify(&mut rw(&dev), &src, (2 * CHUNK + 5) as u64, &mut |p, _, _| phases.push(p), &AtomicBool::new(false), &|_| {}).unwrap();
        let (a, b) = (std::fs::read(&src).unwrap(), std::fs::read(&dev).unwrap());
        assert_eq!(&b[..a.len()], &a[..]);
        assert!(b[a.len()..].iter().all(|&x| x == 0xee), "the rest of the device is untouched");
        assert_eq!(sha.len(), 64);
        assert!([Phase::Writing, Phase::Flushing, Phase::Verifying].iter().all(|p| phases.contains(p)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_and_bad_read_back_are_reported_as_unverified() {
        let d = tmp("bad");
        let src = iso(&d, CHUNK + 9);
        let dev = device_file(&d, 2 * CHUNK);
        let e = write_verify(&mut rw(&dev), &src, (CHUNK + 9) as u64, &mut |_, _, _| {}, &AtomicBool::new(true), &|_| {}).unwrap_err();
        assert!(e.starts_with("cancelled") && e.contains("partially overwritten"), "{e}");
        // The medium changes under the cache: the read-back must notice.
        let corrupt = |_: &File| {
            let mut f = rw(&dev);
            f.seek(SeekFrom::Start(100)).unwrap();
            f.write_all(b"xx").unwrap();
        };
        let e = write_verify(&mut rw(&dev), &src, (CHUNK + 9) as u64, &mut |_, _, _| {}, &AtomicBool::new(false), &corrupt).unwrap_err();
        assert!(e.contains("verification failed"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A device that is a file; records what the flash asked of it.
    struct Fake {
        file: PathBuf,
        dev: RawDevice,
        /// After this many rescans the device "changes" (another stick at the same name).
        swap_after: usize,
        log: std::sync::Arc<Mutex<Vec<String>>>,
    }

    impl Backend for Fake {
        fn rescan(&self, _: &str) -> Result<RawDevice, String> {
            let mut log = self.log.lock().unwrap();
            let n = log.iter().filter(|l| *l == "rescan").count();
            log.push("rescan".into());
            let mut d = self.dev.clone();
            if log.iter().any(|l| l.starts_with("unmount")) {
                d.partitions.iter_mut().for_each(|p| p.mounts.clear());
            }
            if n >= self.swap_after {
                d.serial = "someone else's".into();
            }
            Ok(d)
        }
        fn unmount(&self, kname: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("unmount {kname}"));
            Ok(())
        }
        fn open(&self, _: &RawDevice) -> Result<File, String> {
            self.log.lock().unwrap().push("open".into());
            Ok(rw(&self.file))
        }
        fn drop_cache(&self, _: &File) {}
        fn power_off(&self, _: &RawDevice) -> Result<(), String> {
            self.log.lock().unwrap().push("power off".into());
            Ok(())
        }
    }

    /// The USB stick of the fixture, with device numbers no real disk under the test's temp dir has.
    fn stick(len: u64) -> RawDevice {
        let mut d = devices().remove(0);
        d.size = len;
        d.maj_min = (4000, 0);
        d.partitions[0].maj_min = (4000, 1);
        d
    }

    #[test]
    fn flashes_after_unmounting_and_powers_off() {
        let d = tmp("flash");
        let src = iso(&d, CHUNK + 3);
        let file = device_file(&d, 2 * CHUNK);
        let log = std::sync::Arc::new(Mutex::new(vec![]));
        let t = RawBlockFlash { dev: stick(2 * CHUNK as u64), backend: Box::new(Fake { file: file.clone(), dev: stick(2 * CHUNK as u64), swap_after: usize::MAX, log: log.clone() }) };
        let r = t.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap();
        assert!(r.flashed && r.dest == Path::new("/dev/sda") && r.size == (CHUNK + 3) as u64);
        assert_eq!(&std::fs::read(&file).unwrap()[..CHUNK + 3], &std::fs::read(&src).unwrap()[..]);
        let log = log.lock().unwrap().clone();
        assert_eq!(log, ["rescan", "unmount sda1", "rescan", "open", "rescan", "power off"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_device_is_never_written() {
        let d = tmp("swap");
        let src = iso(&d, CHUNK + 3);
        let file = device_file(&d, 2 * CHUNK);
        for swap_after in [0, 1, 2] {
            let log = std::sync::Arc::new(Mutex::new(vec![]));
            let t = RawBlockFlash { dev: stick(2 * CHUNK as u64), backend: Box::new(Fake { file: file.clone(), dev: stick(2 * CHUNK as u64), swap_after, log }) };
            let e = t.write(&src, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap_err();
            assert!(e.contains("no longer the device") && e.contains("Nothing was written"), "{e}");
            assert!(std::fs::read(&file).unwrap().iter().all(|&x| x == 0xee));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn source_must_be_an_iso() {
        let d = tmp("src");
        let p = d.join("x.iso");
        std::fs::write(&p, vec![0u8; 0x9000]).unwrap();
        assert!(Source::of(&p).unwrap_err().contains("not an ISO"));
        assert_eq!(Source::of(&iso(&d, 0x9000)).unwrap().len, 0x9000);
        let _ = std::fs::remove_dir_all(&d);
    }
}
