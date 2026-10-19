//! The installation flow: select the disk, fetch and verify packages, lay out the disk, write the
//! system, and hand over to the first boot of the installed system.
use crate::{println, rng, time};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use config::Config;
use core::cell::{Cell, RefCell};
use disk::fat32::{Fat32Writer, Options as FatOptions};
use disk::fs::{FileKind, FsWriter, Meta};
use disk::gpt::Layout;
use disk::region::Region;
use drivers::Devices;
#[cfg(feature = "ext4")]
use ext4w::{Ext4Writer, Options as ExtOptions};
#[cfg(feature = "btrfs")]
use btrfsw::{BtrfsWriter, Options as BtrfsOptions, SubvolConfig as BtrfsSubvolConfig};
#[cfg(feature = "xfs")]
use xfsw::{Options as XfsOptions, XfsWriter};
#[cfg(feature = "f2fs")]
use f2fsw::{F2fsWriter, Options as F2fsOptions};
use hal::BlockDevice;
use net::client::Client;
use pgp_lite::Keyring;
use pkg::desc::Package;
use pkg::io::Read;
use pkg::tar::{Kind, TarReader};
use sha2::{Digest, Sha256};

/// Files the boot loader hands to the kernel as modules.
pub struct BootFiles {
    pub tiny_init: &'static [u8],
    /// The UEFI loader (`BOOTX64.EFI`).
    pub bootx64_efi: &'static [u8],
    /// BIOS stage 1 (512 bytes) followed by stage 2 (`bootinfo::bios`).
    pub bios_boot: &'static [u8],
}

const ARCH: &str = "x86_64";
pub(crate) const REPOS: [&str; 2] = ["core", "extra"];
const CACHE_DIR: &str = "var/cache/pacman/pkg";
const STATE_DIR: &str = "var/lib/archstaller";

pub(crate) type R<T> = Result<T, String>;

// `config` checks `kernel_params` against the BIOS stage 2's command line field; keep the two in step.
const _: () = assert!(49 + config::KERNEL_PARAMS_MAX + 42 < bootinfo::bios::CMDLINE_MAX);

pub(crate) fn dbg_err<E: core::fmt::Debug>(ctx: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{ctx}: {e:?}")
}

struct RegionStorage<'a>(Region<'a>);

#[cfg(feature = "ext4")]
impl ext4w::Storage for RegionStorage<'_> {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> ext4w::Result<()> {
        self.0.write_at(offset, data).map_err(|_e| ext4w::Error::Io)
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> ext4w::Result<()> {
        self.0.read_at(offset, buf).map_err(|_e| ext4w::Error::Io)
    }
}

impl disk::fs::Storage for RegionStorage<'_> {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> disk::Result<()> {
        self.0.write_at(offset, data).map_err(|_e| disk::Error::Io)
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> disk::Result<()> {
        self.0.read_at(offset, buf).map_err(|_e| disk::Error::Io)
    }
}

/// Streams a file back out of the filesystem being written.
struct CacheReader<'a> {
    w: Rc<RefCell<dyn FsWriter + 'a>>,
    ino: u64,
    pos: u64,
    size: u64,
    buf: Vec<u8>,
    off: usize,
}

impl Read for CacheReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> pkg::Result<usize> {
        if self.off == self.buf.len() {
            let n = (self.size - self.pos).min(256 * 1024) as usize;
            if n == 0 {
                return Ok(0);
            }
            self.buf.resize(n, 0);
            self.w
                .borrow_mut()
                .read_file(self.ino, self.pos, &mut self.buf)
                .map_err(|_e| pkg::Error::Io)?;
            self.pos += n as u64;
            self.off = 0;
        }
        let n = (self.buf.len() - self.off).min(out.len());
        if let (Some(dst), Some(src)) = (out.get_mut(..n), self.buf.get(self.off..self.off + n)) {
            dst.copy_from_slice(src);
        }
        self.off += n;
        Ok(n)
    }
}

pub(crate) fn now_ms() -> u64 {
    time::uptime_ns() / 1_000_000
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rng::fill(&mut b).expect("random number generator failed");
    b
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn expand(mirror: &str, repo: &str) -> String {
    mirror.replace("$repo", repo).replace("$arch", ARCH)
}

fn select_disk(cfg: &Config, devs: &mut Devices) -> R<Box<dyn BlockDevice>> {
    if cfg.disk.auto_largest {
        return select_largest(devs);
    }
    let want = cfg.disk.confirm_serial.trim();
    let mut hits: Vec<usize> = Vec::new();
    for (i, d) in devs.block.iter().enumerate() {
        let hit = match &cfg.disk.model {
            Some(m) => d.model().contains(m.as_str()),
            None => d.serial().trim() == want,
        };
        if hit {
            hits.push(i);
        }
    }
    println!("disks found: {}", devs.block.len());
    for d in &devs.block {
        println!(
            "  {} serial '{}' {} MiB",
            d.model(),
            d.serial(),
            (d.sector_count() * d.sector_size() as u64) >> 20
        );
    }
    match hits.len() {
        0 => return Err("no disk matches the selector; nothing was written".into()),
        1 => {}
        n => {
            return Err(format!(
                "selector matches {n} disks (need exactly one); nothing was written"
            ))
        }
    }
    let Some(&idx) = hits.first() else {
        return Err("selector matches 0 disks".into());
    };
    let d = devs.block.remove(idx);
    if d.serial().trim() != want {
        return Err(format!(
            "selected disk has serial '{}' but confirm_serial is '{want}'; nothing was written",
            d.serial()
        ));
    }
    Ok(d)
}

/// `disk.auto_largest`: the biggest disk is erased. A tie for the biggest is refused.
fn select_largest(devs: &mut Devices) -> R<Box<dyn BlockDevice>> {
    let size = |d: &dyn BlockDevice| d.sector_count() * d.sector_size() as u64;
    println!("disks found: {}", devs.block.len());
    for d in &devs.block {
        println!(
            "  {} serial '{}' {} MiB",
            d.model(),
            d.serial(),
            size(&**d) >> 20
        );
    }
    let max = devs
        .block
        .iter()
        .map(|d| size(&**d))
        .max()
        .ok_or("no disk found; nothing was written")?;
    let biggest: Vec<usize> = devs
        .block
        .iter()
        .enumerate()
        .filter(|(_, d)| size(&***d) == max)
        .map(|(i, _)| i)
        .collect();
    if biggest.len() != 1 {
        return Err(format!(
            "{} disks tie for the largest size ({} MiB); refusing to guess, nothing was written",
            biggest.len(),
            max >> 20
        ));
    }
    let Some(&idx) = biggest.first() else {
        return Err("no disk found; nothing was written".into());
    };
    println!("disk.auto_largest is set: the largest disk will be ERASED");
    Ok(devs.block.remove(idx))
}

pub(crate) const USER_FILE_MAX: usize = 1 << 20;

/// Downloads a small optional file; any failure is reported as text, never as a hard error.
pub(crate) fn fetch_optional(
    client: &Client,
    stack: &mut net::Stack,
    url: &str,
    max: usize,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let mut too_big = false;
    let r = client.get(stack, url, &mut |d| {
        if data.len() + d.len() > max {
            too_big = true;
            return Err(net::Error::Http("file too large"));
        }
        data.extend_from_slice(d);
        Ok(())
    });
    match r {
        Ok(resp) if resp.status == 200 => Ok(data),
        Ok(resp) => Err(format!("HTTP {}", resp.status)),
        Err(_) if too_big => Err(format!("larger than {max} bytes")),
        Err(e) => Err(format!("{e:?}")),
    }
}

pub(crate) fn bring_up_network(devs: &mut Devices) -> R<net::Stack> {
    if devs.net.is_empty() {
        return Err("no supported network interface".into());
    }
    // Wait up to 10s for at least one interface to report link
    let end = now_ms() + 10_000;
    while now_ms() < end && !devs.net.iter_mut().any(|n| n.link_up()) {
        hal::idle();
    }

    let mut last_err = String::from("no network interface acquired a DHCP lease");
    while !devs.net.is_empty() {
        // Try interfaces with link first
        let idx = devs.net.iter_mut().position(|n| n.link_up()).unwrap_or(0);
        let mut nic = devs.net.remove(idx);
        if !nic.link_up() {
            // Copper PHY auto-negotiation (e.g. e1000e 1000BASE-T) takes 1.5–3 seconds after reset.
            let wait_link = now_ms() + 4000;
            while now_ms() < wait_link && !nic.link_up() {
                hal::idle();
            }
        }
        let has_link = nic.link_up();
        if has_link {
            println!("network: {} link up; requesting DHCP lease...", nic.name());
            let settle = now_ms() + 1500;
            while now_ms() < settle {
                hal::idle();
            }
        } else {
            println!("network: {} has no link; trying anyway...", nic.name());
        }

        let mut stack = net::Stack::new(nic, now_ms, unsafe { core::arch::x86_64::_rdtsc() });
        // Campus/enterprise switches (Cisco / LIANE without PortFast) spend 30 seconds in STP
        // listening (15s) and learning (15s) states before forwarding frames. Allow 45s on interfaces
        // with physical link so DHCPDISCOVER outlasts the switch forwarding delay, matching Linux
        // systemd-networkd and dhcpcd.
        let timeout_ms = if has_link { 45_000 } else if devs.net.is_empty() { 30_000 } else { 15_000 };
        match stack.dhcp(timeout_ms) {
            Ok(lease) => {
                hal::info!(
                    "network: {:?}/{} gateway {:?} dns {:?}",
                    lease.address,
                    lease.prefix,
                    lease.router,
                    lease.dns
                );
                return Ok(stack);
            }
            Err(e) => {
                println!("network: DHCP failed on {}: {e:?}", stack.device_name());
                last_err = format!("DHCP on {}: {e:?}", stack.device_name());
            }
        }
    }
    Err(last_err)
}

pub(crate) fn fetch_to_vec(
    client: &Client,
    stack: &mut net::Stack,
    cfg: &Config,
    path: &str,
    repo: &str,
) -> R<Vec<u8>> {
    let mut last = String::new();
    for m in &cfg.mirrors {
        let url = format!("{}/{}", expand(m, repo), path);
        let mut data = Vec::new();
        // A sync database is a few MiB; an endless answer would otherwise exhaust the heap (the host side
        // caps the same download at 128 MiB too).
        match client.get(stack, &url, &mut |d| {
            if data.len() + d.len() > 128 << 20 {
                return Err(net::Error::Http("database larger than 128 MiB"));
            }
            data.extend_from_slice(d);
            Ok(())
        }) {
            Ok(r) if r.status == 200 => return Ok(data),
            Ok(r) => last = format!("{url}: HTTP {}", r.status),
            Err(e) => last = format!("{url}: {e:?}"),
        }
    }
    Err(format!("download failed: {last}"))
}

/// Downloads `p` into the cache file, verifying size, SHA-256 and the PGP signature.
fn download_package(
    client: &Client,
    stack: &mut net::Stack,
    cfg: &Config,
    w: &Rc<RefCell<dyn FsWriter + '_>>,
    keyring: &Keyring,
    p: &Package,
) -> R<u64> {
    if p.pgpsig.is_empty() {
        return Err(format!("{}: database has no signature", p.name));
    }
    let sig = pgp_lite::base64_decode(&p.pgpsig).map_err(dbg_err("PGPSIG"))?;
    let path = format!("{CACHE_DIR}/{}", p.filename);
    let meta = Meta {
        mode: 0o644,
        mtime: time::unix_time(),
        ..Default::default()
    };
    let mut last = String::new();
    for attempt in 0..cfg.mirrors.len() * 2 {
        let Some(mirror) = cfg.mirrors.get(attempt % cfg.mirrors.len()) else {
            continue;
        };
        let url = format!("{}/{}", expand(mirror, &p.repo), p.filename);
        let hasher = RefCell::new(Sha256::new());
        let verifier = RefCell::new(Some(
            pgp_lite::Verifier::from_packet(&sig).map_err(dbg_err("signature packet"))?,
        ));
        let received = Cell::new(0u64);
        w.borrow_mut()
            .begin_file(&path, &meta)
            .map_err(dbg_err("cache file"))?;
        let res = client.get(stack, &url, &mut |d| {
            hasher.borrow_mut().update(d);
            if let Some(v) = verifier.borrow_mut().as_mut() {
                v.update(d);
            }
            received.set(received.get() + d.len() as u64);
            w.borrow_mut()
                .write_file(d)
                .map_err(|_e| net::Error::Http("cache write failed"))
        });
        let ino = w.borrow_mut().end_file().map_err(dbg_err("cache file"))?;
        match res {
            Ok(r) if r.status == 200 => {}
            Ok(r) => {
                last = format!("{url}: HTTP {}", r.status);
                continue;
            }
            Err(e) => {
                last = format!("{url}: {e:?}");
                continue;
            }
        }
        if received.get() != p.csize {
            last = format!("{}: size {} != {}", p.filename, received.get(), p.csize);
            continue;
        }
        let digest = hex(&hasher.into_inner().finalize());
        if digest != p.sha256 {
            last = format!("{}: SHA-256 mismatch", p.filename);
            continue;
        }
        let v = verifier.borrow_mut().take().expect("verifier active");
        // A failed signature is not retried elsewhere: the mirror served content the maintainers did not sign.
        v.finish(keyring, time::unix_time()).map_err(|e| format!("{}: signature check failed: {e:?} (rebuild the ISO for a newer keyring if the key is unknown)", p.filename))?;
        return Ok(ino);
    }
    Err(format!("download failed after retries: {last}"))
}

/// The files of one kernel, from its module directory (`usr/lib/modules/<version>/`).
#[derive(Default)]
struct KernelFiles {
    modules: BTreeMap<String, String>,
    vmlinuz: Option<String>,
    builtin: Option<String>,
    /// The `pkgbase` file: which package the kernel came from (`linux`, `linux-zen`, ...).
    pkgbase: Option<String>,
}

/// Kernel files by module directory, so that two kernel packages do not mix their modules.
#[derive(Default)]
struct KernelIndex {
    kernels: BTreeMap<String, KernelFiles>,
}

impl KernelIndex {
    fn note(&mut self, path: &str) {
        let Some((ver, file)) = path
            .strip_prefix("usr/lib/modules/")
            .and_then(|r| r.split_once('/'))
        else {
            return;
        };
        let k = self.kernels.entry(ver.to_string()).or_default();
        match file {
            "vmlinuz" => k.vmlinuz = Some(path.into()),
            "modules.builtin" => k.builtin = Some(path.into()),
            "pkgbase" => k.pkgbase = Some(path.into()),
            _ => {
                if let Some(n) = initrd::modules::name_from_path(path) {
                    k.modules.insert(n, path.into());
                }
            }
        }
    }

    /// The kernel the first boot starts: the `linux` package's if it is installed, else the first one
    /// found. `firstboot.sh` makes the same choice for the boot loader.
    fn select<'k, 'a>(&'k self, w: &Rc<RefCell<dyn FsWriter + 'a>>) -> R<&'k KernelFiles> {
        let mut first = None;
        for k in self.kernels.values().filter(|k| k.vmlinuz.is_some()) {
            let base = k.pkgbase.as_deref().and_then(|p| read_whole(w, p).ok());
            if base.as_deref().map(|b| b.trim_ascii()) == Some(b"linux") {
                return Ok(k);
            }
            first.get_or_insert(k);
        }
        first.ok_or_else(|| {
            "the installed packages contain no kernel (add the 'linux' package)".into()
        })
    }
}

fn clean_path(mut p: &str) -> &str {
    while let Some(stripped) = p.strip_prefix("./") {
        p = stripped;
    }
    p = p.trim_start_matches('/');
    p.trim_end_matches('/')
}

fn is_pacman_meta(p: &str) -> bool {
    matches!(
        p,
        ".PKGINFO" | ".MTREE" | ".BUILDINFO" | ".INSTALL" | ".CHANGELOG"
    ) || p.starts_with(".PKGINFO")
        || p.starts_with(".INSTALL")
        || p.starts_with(".CHANGELOG")
}

fn extract_package(
    w: &Rc<RefCell<dyn FsWriter + '_>>,
    ino: u64,
    size: u64,
    idx: &mut KernelIndex,
) -> R<()> {
    let reader = CacheReader {
        w: w.clone(),
        ino,
        pos: 0,
        size,
        buf: Vec::new(),
        off: 0,
    };
    let dec = pkg::compress::open(reader).map_err(dbg_err("package compression"))?;
    let mut tar = TarReader::new(dec);
    let mut buf = alloc::vec![0u8; 64 * 1024];
    while let Some(e) = tar.next_entry().map_err(dbg_err("package archive"))? {
        let path = clean_path(&e.path).to_string();
        if path.is_empty() || is_pacman_meta(&path) || path.split('/').any(|seg| seg == "..") {
            continue;
        }
        let meta = Meta {
            mode: (e.mode & 0o7777) as u16,
            uid: e.uid,
            gid: e.gid,
            mtime: e.mtime,
            xattrs: e.xattrs.clone(),
        };
        let ctx = |what: &str| format!("{path}: {what}");
        match e.kind {
            Kind::Dir => {
                w.borrow_mut()
                    .mkdir(&path, &meta)
                    .map_err(|er| ctx(&format!("mkdir {er:?}")))?;
            }
            Kind::File => {
                w.borrow_mut()
                    .begin_file(&path, &meta)
                    .map_err(|er| ctx(&format!("create {er:?}")))?;
                loop {
                    let n = tar.read_data(&mut buf).map_err(dbg_err("package data"))?;
                    if n == 0 {
                        break;
                    }
                    let Some(chunk) = buf.get(..n) else {
                        break;
                    };
                    w.borrow_mut()
                        .write_file(chunk)
                        .map_err(|er| ctx(&format!("write {er:?}")))?;
                }
                w.borrow_mut()
                    .end_file()
                    .map_err(|er| ctx(&format!("close {er:?}")))?;
                idx.note(&path);
            }
            Kind::Symlink => {
                w.borrow_mut()
                    .symlink(&path, &e.link, &meta)
                    .map_err(|er| ctx(&format!("symlink {er:?}")))?;
            }
            Kind::Hardlink => {
                w.borrow_mut()
                    .hardlink(&path, clean_path(&e.link))
                    .map_err(|er| ctx(&format!("hardlink {er:?}")))?;
            }
            Kind::CharDev => {
                w.borrow_mut()
                    .mknod(&path, FileKind::CharDev, e.dev_major, e.dev_minor, &meta)
                    .map_err(|er| ctx(&format!("mknod {er:?}")))?;
            }
            Kind::BlockDev => {
                w.borrow_mut()
                    .mknod(&path, FileKind::BlockDev, e.dev_major, e.dev_minor, &meta)
                    .map_err(|er| ctx(&format!("mknod {er:?}")))?;
            }
            Kind::Fifo => {
                w.borrow_mut()
                    .mknod(&path, FileKind::Fifo, 0, 0, &meta)
                    .map_err(|er| ctx(&format!("mkfifo {er:?}")))?;
            }
        }
    }
    Ok(())
}

fn read_whole(w: &Rc<RefCell<dyn FsWriter + '_>>, path: &str) -> R<Vec<u8>> {
    let mut wr = w.borrow_mut();
    let ino = wr
        .lookup(path)
        .ok_or_else(|| format!("{path} not found in the installed system"))?;
    let mut out = alloc::vec![0u8; wr.file_size(ino) as usize];
    wr.read_file(ino, 0, &mut out)
        .map_err(dbg_err("read back"))?;
    Ok(out)
}

struct FsModules<'a> {
    w: Rc<RefCell<dyn FsWriter + 'a>>,
    idx: &'a BTreeMap<String, String>,
}

impl initrd::modules::ModuleSource for FsModules<'_> {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        read_whole(&self.w, self.idx.get(name)?).ok()
    }
}

fn put(w: &Rc<RefCell<dyn FsWriter + '_>>, path: &str, mode: u16, data: &[u8]) -> R<()> {
    let meta = Meta {
        mode,
        mtime: time::unix_time(),
        ..Default::default()
    };
    let mut wr = w.borrow_mut();
    wr.begin_file(path, &meta).map_err(dbg_err("write file"))?;
    wr.write_file(data).map_err(dbg_err("write file"))?;
    wr.end_file().map_err(dbg_err("write file"))?;
    Ok(())
}

fn charset_of(locale: &str) -> &str {
    match locale.split_once('.') {
        Some((_, cs)) => cs.split('@').next().unwrap_or("UTF-8"),
        None => "UTF-8",
    }
}

/// What gets resolved: the configured packages, plus the official packages the AUR recipes need (to build
/// and to run). The AUR packages themselves are built by the installed system (firstboot/archstaller-aur.sh).
pub(crate) fn wanted_packages(cfg: &Config) -> Vec<String> {
    let mut wanted = cfg.packages.clone();
    for a in &cfg.aur {
        for d in &a.deps {
            if !wanted.contains(d) {
                wanted.push(d.clone());
            }
        }
    }
    wanted
}

/// Bytes the root file system needs for `resolution`: the packages stay in the cache for the first boot's
/// `pacman -U` and are unpacked beside it, plus room for metadata, the journal and the first boot's initramfs.
pub(crate) fn space_needed(resolution: &pkg::resolve::Resolution) -> u64 {
    let downloaded: u64 = resolution.packages.iter().map(|s| s.pkg.csize).sum();
    let installed: u64 = resolution.packages.iter().map(|s| s.pkg.isize).sum();
    downloaded + installed + installed / 10 + (1 << 30)
}

pub fn run(cfg: &Config, devs: &mut Devices, keyring: &Keyring, boot: &BootFiles) -> R<()> {
    // 11. Select the disk. Nothing is written before this succeeds.
    let mut disk = select_disk(cfg, devs)?;
    println!("target disk: {} serial '{}'", disk.model(), disk.serial());

    // 12. Network, databases, resolution.
    let mut stack = bring_up_network(devs)?;
    let client = Client::new(time::unix_time);
    let mut dbs = Vec::new();
    let mut db_bytes: Vec<(&str, Vec<u8>)> = Vec::new();
    for repo in REPOS {
        let bytes = fetch_to_vec(&client, &mut stack, cfg, &format!("{repo}.db"), repo)?;
        println!("{repo}.db: {} bytes", bytes.len());
        dbs.push(pkg::db::Db::parse(repo, bytes.as_slice()).map_err(dbg_err("database"))?);
        db_bytes.push((repo, bytes));
    }
    let providers: BTreeMap<String, String> = cfg.providers.iter().cloned().collect();
    let resolution = pkg::resolve::Resolver::new(&dbs, &providers)
        .resolve(&wanted_packages(cfg))
        .map_err(dbg_err("dependency resolution"))?;
    for a in &resolution.ambiguities {
        println!(
            "note: {} is provided by {:?}; using {} (set providers.{} to choose)",
            a.dep, a.candidates, a.chosen, a.dep
        );
    }
    let total: u64 = resolution.packages.iter().map(|s| s.pkg.csize).sum();
    println!(
        "resolved {} packages, {} MiB to download",
        resolution.packages.len(),
        total >> 20
    );

    // 13. Partition.
    let ss = disk.sector_size();
    // FAT32 needs 65525 clusters of at least a sector each, more than a small ESP holds on a disk with
    // 4 KiB sectors; formatting it would only fail after every package has been downloaded.
    let esp_bytes = (cfg.disk.esp_mib as u64 * 1024 * 1024).max(disk::fat32::min_volume_bytes(ss));
    if esp_bytes >> 20 > cfg.disk.esp_mib as u64 {
        println!(
            "note: the ESP is {} MiB instead of {} MiB: FAT32 on {ss}-byte sectors needs that much",
            esp_bytes >> 20,
            cfg.disk.esp_mib
        );
    }
    // The packages stay in the cache for the first boot's `pacman -U` and are unpacked beside it; with
    // room for metadata, the journal and the first boot's initramfs. A disk that is too small would
    // otherwise fail half way through the downloads, after it was already erased.
    let need = space_needed(&resolution);
    let have = (disk.sector_count() * ss as u64).saturating_sub(esp_bytes + (4 << 20));
    if need > have {
        return Err(format!(
            "the disk is too small: the packages need about {} MiB, the root file system would have {} MiB; nothing was written",
            need >> 20,
            have >> 20
        ));
    }
    let guids = [
        random::<16>(),
        random::<16>(),
        random::<16>(),
        random::<16>(),
    ];
    let layout =
        Layout::plan(ss, disk.sector_count(), esp_bytes, guids).map_err(dbg_err("partitioning"))?;
    layout
        .write(&mut Region::whole(disk.as_mut()))
        .map_err(dbg_err("writing the partition table"))?;
    println!("partition table written");

    let mut idx = KernelIndex::default();
    let root_uuid: [u8; 16] = random();
    let (root_off, root_len) = layout.byte_range(Layout::ROOT);
    let now = time::unix_time();
    // The directory hash seed is all random, as mke2fs makes it: a predictable one lets anyone who can
    // name files force hash collisions once the kernel indexes a directory.
    let seed: [u8; 16] = random();
    let (chunks, _) = seed.as_chunks::<4>();
    let mut hash_seed = [0u32; 4];
    for (dst, chunk) in hash_seed.iter_mut().zip(chunks) {
        *dst = u32::from_le_bytes(*chunk);
    }
    let storage = RegionStorage(Region::new(disk.as_mut(), root_off, root_len));
    let w: Rc<RefCell<dyn FsWriter + '_>> = match cfg.disk.filesystem {
        #[cfg(feature = "ext4")]
        config::FsType::Ext4 => {
            let writer = Ext4Writer::new(
                storage,
                root_len,
                ExtOptions {
                    label: "archroot".into(),
                    uuid: root_uuid,
                    hash_seed,
                    now,
                    reserved_percent: 1,
                },
            )
            .map_err(dbg_err("mkfs.ext4"))?;
            Rc::new(RefCell::new(writer))
        }
        #[cfg(feature = "btrfs")]
        config::FsType::Btrfs => {
            let default_subvols = config::default_btrfs_subvolumes();
            let subvols_cfg = match &cfg.disk.btrfs {
                Some(b) => &b.subvolumes,
                None => &default_subvols,
            };
            let subvolumes = subvols_cfg
                .iter()
                .map(|s| BtrfsSubvolConfig {
                    name: s.name.clone(),
                    mount: s.mount.clone(),
                })
                .collect();
            let writer = BtrfsWriter::new(
                storage,
                root_len,
                BtrfsOptions {
                    label: "archroot".into(),
                    uuid: root_uuid,
                    now,
                    subvolumes,
                },
            )
            .map_err(dbg_err("mkfs.btrfs"))?;
            Rc::new(RefCell::new(writer))
        }
        #[cfg(feature = "xfs")]
        config::FsType::Xfs => {
            let writer = XfsWriter::new(
                storage,
                root_len,
                XfsOptions {
                    label: "archroot".into(),
                    uuid: root_uuid,
                    now,
                },
            )
            .map_err(dbg_err("mkfs.xfs"))?;
            Rc::new(RefCell::new(writer))
        }
        #[cfg(feature = "f2fs")]
        config::FsType::F2fs => {
            let writer = F2fsWriter::new(
                storage,
                root_len,
                F2fsOptions {
                    label: "archroot".into(),
                    uuid: root_uuid,
                    now,
                },
            )
            .map_err(dbg_err("mkfs.f2fs"))?;
            Rc::new(RefCell::new(writer))
        }
        #[cfg(not(all(
            feature = "ext4",
            feature = "btrfs",
            feature = "xfs",
            feature = "f2fs"
        )))]
        _ => {
            return Err(format!(
                "filesystem '{}' is not compiled into this installer kernel build",
                cfg.disk.filesystem.as_str()
            ));
        }
    };
    {
        let mut wr = w.borrow_mut();
        for (d, mode) in [
            ("dev", 0o755),
            ("proc", 0o755),
            ("sys", 0o755),
            ("run", 0o755),
            ("tmp", 0o1777),
            ("etc", 0o755),
            ("boot", 0o755),
            ("var", 0o755),
            (CACHE_DIR, 0o755),
            (STATE_DIR, 0o755),
        ] {
            wr.mkdir(
                d,
                &Meta {
                    mode,
                    mtime: now,
                    ..Default::default()
                },
            )
            .map_err(dbg_err("mkdir"))?;
        }
    }

    // 14. Download, verify, extract.
    let n = resolution.packages.len();
    let (mut done, started) = (0u64, now_ms());
    for (i, s) in resolution.packages.iter().enumerate() {
        println!(
            "[{}/{}] {} {} ({} KiB)   {}%  {} of {} MiB",
            i + 1,
            n,
            s.pkg.name,
            s.pkg.version,
            s.pkg.csize >> 10,
            (done * 100).checked_div(total).unwrap_or(100),
            done >> 20,
            total >> 20
        );
        let ino = download_package(&client, &mut stack, cfg, &w, keyring, s.pkg)?;
        extract_package(&w, ino, s.pkg.csize, &mut idx)?;
        done += s.pkg.csize;
    }
    let secs = ((now_ms() - started) / 1000).max(1);
    println!(
        "{n} packages downloaded, verified and unpacked: {} MiB in {secs} s ({} MiB/s)",
        done >> 20,
        (done >> 20) / secs
    );
    for (repo, bytes) in &db_bytes {
        put(&w, &format!("var/lib/pacman/sync/{repo}.db"), 0o644, bytes)?;
    }

    // 15. System configuration. Everything under /etc is also stored as an overlay that the first
    // boot re-applies after `pacman -U --overwrite '*'` has replaced package-owned files.
    let esp_volume_id = u32::from_le_bytes(random::<4>());
    let root_uuid_text = uuid_text(&root_uuid);
    let esp_uuid_text = format!("{:04X}-{:04X}", esp_volume_id >> 16, esp_volume_id & 0xffff);
    let cmdline = format_cmdline(cfg, &root_uuid_text);
    let overlay = |p: &str| format!("{STATE_DIR}/overlay/{p}");
    let mut etc_files: Vec<(String, u16, Vec<u8>)> = Vec::new();
    etc_files.push((
        "etc/fstab".into(),
        0o644,
        format_fstab(cfg, &root_uuid_text, &esp_uuid_text).into_bytes(),
    ));
    etc_files.push((
        "etc/hostname".into(),
        0o644,
        format!("{}\n", cfg.hostname).into_bytes(),
    ));
    // The login prompt (agetty expands \\r and \\l) and the message of the day show the banner.
    let art = crate::banner::render("ARCHSTALLER");
    etc_files.push((
        "etc/issue".into(),
        0o644,
        format!("\n{art}\nArch Linux \\r (\\l)\n\n").into_bytes(),
    ));
    etc_files.push((
        "etc/motd".into(),
        0o644,
        format!("\n{art}\nInstalled by archstaller.\n\n").into_bytes(),
    ));
    etc_files.push((
        "etc/locale.conf".into(),
        0o644,
        format!("LANG={}\n", cfg.locale).into_bytes(),
    ));
    etc_files.push((
        "etc/locale.gen".into(),
        0o644,
        format!("{} {}\n", cfg.locale, charset_of(&cfg.locale)).into_bytes(),
    ));
    etc_files.push((
        "etc/vconsole.conf".into(),
        0o644,
        format!("KEYMAP={}\n", cfg.keymap).into_bytes(),
    ));
    let mut mirrorlist = String::from("# Generated by archstaller\n");
    for m in &cfg.mirrors {
        mirrorlist.push_str(&format!("Server = {m}\n"));
    }
    etc_files.push((
        "etc/pacman.d/mirrorlist".into(),
        0o644,
        mirrorlist.into_bytes(),
    ));
    etc_files.push((
        "etc/sudoers.d/10-wheel".into(),
        0o440,
        b"%wheel ALL=(ALL:ALL) ALL\n".to_vec(),
    ));
    // systemd-networkd configures nothing without a .network file, and Arch ships none: DHCP on every wired
    // interface, as the installer itself did (systemd's own 89-ethernet.network.example).
    if cfg.services.iter().any(|s| s == "systemd-networkd.service") {
        etc_files.push((
            "etc/systemd/network/20-wired.network".into(),
            0o644,
            b"[Match]\nKind=!*\nType=ether\n\n[Network]\nDHCP=yes\n".to_vec(),
        ));
    }
    for (path, mode, data) in &etc_files {
        put(&w, path, *mode, data)?;
        put(&w, &overlay(path), *mode, data)?;
    }
    // The ly login manager draws the banner above its login box (firstboot.sh points ly's config at this file).
    if cfg.packages.iter().any(|p| p == "ly") {
        let dur = crate::banner::dur("ARCHSTALLER");
        put(&w, "etc/ly/archstaller.dur", 0o644, &dur)?;
        put(&w, &overlay("etc/ly/archstaller.dur"), 0o644, &dur)?;
        let conf = ly_config().as_bytes();
        put(&w, "etc/ly/config.ini", 0o644, conf)?;
        put(&w, &overlay("etc/ly/config.ini"), 0o644, conf)?;
    }

    if cfg.packages.iter().any(|p| p == "greetd" || p == "greetd-tuigreet") {
        let conf = greetd_config().as_bytes();
        put(&w, "etc/greetd/config.toml", 0o644, conf)?;
        put(&w, &overlay("etc/greetd/config.toml"), 0o644, conf)?;
    }

    if cfg.packages.iter().any(|p| p == "sddm") {
        let conf = sddm_config().as_bytes();
        put(&w, "etc/sddm.conf.d/10-archstaller.conf", 0o644, conf)?;
        put(&w, &overlay("etc/sddm.conf.d/10-archstaller.conf"), 0o644, conf)?;
    }

    if cfg.packages.iter().any(|p| p == "lightdm" || p == "lightdm-gtk-greeter") {
        let conf = lightdm_config().as_bytes();
        put(&w, "etc/lightdm/lightdm-gtk-greeter.conf", 0o644, conf)?;
        put(&w, &overlay("etc/lightdm/lightdm-gtk-greeter.conf"), 0o644, conf)?;
    }

    let svg_logo = crate::banner::svg("ARCHSTALLER");
    let svg_bytes = svg_logo.as_bytes();
    put(&w, "usr/share/pixmaps/archstaller.svg", 0o644, svg_bytes)?;
    put(&w, &overlay("usr/share/pixmaps/archstaller.svg"), 0o644, svg_bytes)?;


    let tz_target = format!("../usr/share/zoneinfo/{}", cfg.timezone);
    let link_meta = Meta {
        mode: 0o777,
        mtime: now,
        ..Default::default()
    };
    for path in ["etc/localtime".to_string(), overlay("etc/localtime")] {
        w.borrow_mut()
            .symlink(&path, &tz_target, &link_meta)
            .map_err(dbg_err("localtime"))?;
    }

    for sf in &cfg.system_files {
        if let Some(content) = &sf.content {
            let clean_path = sf.path.trim_start_matches('/');
            put(&w, clean_path, sf.mode, content)?;
            put(&w, &overlay(clean_path), sf.mode, content)?;
            println!(
                "system file {} ({} bytes, mode {:o})",
                clean_path,
                content.len(),
                sf.mode
            );
        }
    }

    // Per-user files from third-party servers. An unreachable server only costs a warning.
    let mut userfiles = String::new();
    for (i, f) in cfg.user_files.iter().enumerate() {
        match fetch_optional(&client, &mut stack, &f.url, USER_FILE_MAX) {
            Ok(data) => {
                put(&w, &format!("{STATE_DIR}/userfiles/{i}"), 0o644, &data)?;
                userfiles.push_str(&format!("{i}:{}\n", f.dest));
                println!("user file {} -> ~/{} ({} bytes)", f.url, f.dest, data.len());
            }
            Err(e) => println!("warning: skipping {}: {e}", f.url),
        }
    }
    put(
        &w,
        &format!("{STATE_DIR}/userfiles.list"),
        0o644,
        userfiles.as_bytes(),
    )?;
    let mut userarchives = String::new();
    for (i, a) in cfg.user_archives.iter().enumerate() {
        let sha = a.sha256.as_deref().unwrap_or("");
        if let Some(data) = &a.content {
            if !config::is_supported_archive(data) {
                println!("warning: skipping user archive #{i}: unknown archive format");
                continue;
            }
            put(&w, &format!("{STATE_DIR}/userarchives/{i}"), 0o644, data)?;
            userarchives.push_str(&format!("{i}:embedded:::{sha}\n"));
            println!("user archive #{i} (embedded, {} bytes) -> ~/", data.len());
        } else if let Some(url) = &a.url {
            userarchives.push_str(&format!("{i}:url:{url}::{sha}\n"));
            println!("user archive #{i} from {url} will be downloaded on first boot");
        } else if let Some(uuid) = &a.disk_uuid {
            let path = a.path.as_deref().unwrap_or("home.tar.zst");
            userarchives.push_str(&format!("{i}:uuid:{uuid}:{path}:{sha}\n"));
            println!("user archive #{i} from disk UUID {uuid} ({path}) will be loaded on first boot");
        } else if let Some(file) = &a.file {
            userarchives.push_str(&format!("{i}:file:{file}::{sha}\n"));
            println!("user archive #{i} from file {file} will be loaded on first boot");
        }
    }
    put(
        &w,
        &format!("{STATE_DIR}/userarchives.list"),
        0o644,
        userarchives.as_bytes(),
    )?;

    // First-boot scripts: built-in and local ones travel inside the config; a downloaded one must
    // match its digest (a mismatch fails the install, an unreachable server only skips the script).
    let mut scripts = String::new();
    for (i, s) in cfg.scripts.iter().enumerate() {
        let data = match (&s.content, &s.url, &s.sha256) {
            (Some(c), ..) => c.clone(),
            (None, Some(url), Some(want)) => {
                match fetch_optional(&client, &mut stack, url, config::SCRIPT_MAX) {
                    Ok(d) => {
                        let got: String = Sha256::digest(&d)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        if &got != want {
                            return Err(format!(
                                "script {} from {url} does not match its sha256 (got {got})",
                                s.id
                            ));
                        }
                        d
                    }
                    Err(e) => {
                        println!("warning: skipping script {}: {e}", s.id);
                        continue;
                    }
                }
            }
            _ => {
                println!("warning: skipping script {}: no content", s.id);
                continue;
            }
        };
        put(&w, &format!("{STATE_DIR}/scripts/{i}.sh"), 0o700, &data)?;
        scripts.push_str(&format_script_entry(i, s));
        println!(
            "script {} ({} bytes) will run at the end of the first boot",
            s.id,
            data.len()
        );
    }
    put(
        &w,
        &format!("{STATE_DIR}/scripts.list"),
        0o644,
        scripts.as_bytes(),
    )?;

    // Custom systemd units: installed into /etc/systemd/system/ (root) or ~/.config/systemd/user/ (user).
    let mut custom_services = String::new();
    for (i, cs) in cfg.custom_services.iter().enumerate() {
        let data = match &cs.content {
            Some(d) => d.clone(),
            None => {
                println!("warning: skipping custom service {}: no content", cs.name);
                continue;
            }
        };
        put(
            &w,
            &format!("{STATE_DIR}/custom_services/{i}"),
            0o644,
            &data,
        )?;
        let user = cs.user.as_deref().unwrap_or("root");
        let name = &cs.name;
        let en = u8::from(cs.enable);
        custom_services.push_str(&format!("{i}:{user}:{en}:{name}\n"));
        println!(
            "custom service {} ({} bytes, user {}) will be installed",
            name,
            data.len(),
            user
        );
    }
    if !cfg.custom_services.is_empty() {
        put(
            &w,
            &format!("{STATE_DIR}/custom_services.list"),
            0o644,
            custom_services.as_bytes(),
        )?;
    }

    let files: Vec<String> = resolution
        .packages
        .iter()
        .map(|s| s.pkg.filename.clone())
        .collect();
    // Packages named only because an AUR recipe needs them are dependencies, not explicit installs.
    let aur_only = |n: &str| {
        !cfg.packages.iter().any(|p| p == n)
            && cfg.aur.iter().any(|a| a.deps.iter().any(|d| d == n))
    };
    let deps: Vec<String> = resolution
        .packages
        .iter()
        .filter(|s| !s.explicit || aur_only(&s.pkg.name))
        .map(|s| s.pkg.name.clone())
        .collect();
    let mut users = String::new();
    for u in &cfg.users {
        users.push_str(&format!(
            "{}:{}:{}:{}\n",
            u.name,
            u.password_hash,
            u.groups.join(","),
            u.shell
        ));
    }
    let lines = |v: &[String]| v.iter().map(|s| format!("{s}\n")).collect::<String>();
    put(
        &w,
        &format!("{STATE_DIR}/packages.list"),
        0o644,
        lines(&files).as_bytes(),
    )?;
    put(
        &w,
        &format!("{STATE_DIR}/deps.list"),
        0o644,
        lines(&deps).as_bytes(),
    )?;
    put(
        &w,
        &format!("{STATE_DIR}/users.list"),
        0o600,
        users.as_bytes(),
    )?;
    let dotfiles = format_dotfiles_list(&cfg.users);
    if !dotfiles.is_empty() {
        put(
            &w,
            &format!("{STATE_DIR}/dotfiles.list"),
            0o644,
            dotfiles.as_bytes(),
        )?;
    }
    put(
        &w,
        &format!("{STATE_DIR}/services.list"),
        0o644,
        lines(&cfg.services).as_bytes(),
    )?;
    put(
        &w,
        &format!("{STATE_DIR}/cmdline"),
        0o644,
        cmdline.as_bytes(),
    )?;
    put(
        &w,
        &format!("{STATE_DIR}/bootloader"),
        0o644,
        format!("{}\n", cfg.bootloader.as_str()).as_bytes(),
    )?;
    if let Some(h) = &cfg.root_password_hash {
        put(&w, &format!("{STATE_DIR}/root.hash"), 0o600, h.as_bytes())?;
    }
    // AUR packages: one `name:pkgbase:commit:sha256:vcs:as_dep:services` line each, in build order. The
    // build-only official packages are listed so the build script can offer to remove them.
    let mut aur = String::new();
    let mut aur_builddeps: Vec<String> = Vec::new();
    for a in &cfg.aur {
        aur.push_str(&format!(
            "{}:{}:{}:{}:{}:{}:{}\n",
            a.name,
            a.pkgbase,
            a.commit,
            a.sha256,
            u8::from(a.vcs),
            u8::from(a.as_dep),
            a.services.join(",")
        ));
        for d in &a.build_deps {
            if !aur_builddeps.contains(d) && !cfg.packages.contains(d) {
                aur_builddeps.push(d.clone());
            }
        }
        println!(
            "AUR package {} ({}) will be built on the first boot after the install",
            a.name,
            if a.vcs {
                "VCS source, not pinned by the commit"
            } else {
                "pinned"
            }
        );
    }
    if !cfg.aur.is_empty() {
        put(&w, &format!("{STATE_DIR}/aur.list"), 0o644, aur.as_bytes())?;
        put(
            &w,
            &format!("{STATE_DIR}/aur_builddeps.list"),
            0o644,
            lines(&aur_builddeps).as_bytes(),
        )?;
        put(
            &w,
            "usr/lib/systemd/system/archstaller-aur.service",
            0o644,
            include_bytes!("../../firstboot/archstaller-aur.service"),
        )?;
        put(
            &w,
            "usr/lib/archstaller/aur.sh",
            0o755,
            include_bytes!("../../firstboot/archstaller-aur.sh"),
        )?;
    }
    put(&w, &format!("{STATE_DIR}/firstboot"), 0o644, b"1\n")?;
    put(
        &w,
        "usr/lib/systemd/system/archstaller-firstboot.target",
        0o644,
        include_bytes!("../../firstboot/archstaller-firstboot.target"),
    )?;
    put(
        &w,
        "usr/lib/systemd/system/archstaller-firstboot.service",
        0o644,
        include_bytes!("../../firstboot/archstaller-firstboot.service"),
    )?;
    put(
        &w,
        "usr/lib/archstaller/firstboot.sh",
        0o755,
        include_bytes!("../../firstboot/firstboot.sh"),
    )?;

    // 16. Initramfs for the first boot and the kernel image.
    let kernel = idx.select(&w)?;
    let vmlinuz_path = kernel.vmlinuz.clone().unwrap_or_default();
    println!("kernel: {vmlinuz_path}");
    let vmlinuz = read_whole(&w, &vmlinuz_path)?;
    let builtin = match &kernel.builtin {
        Some(p) => initrd::modules::parse_builtin(&String::from_utf8_lossy(&read_whole(&w, p)?)),
        None => Default::default(),
    };
    // Root device drivers, plus vfat for /boot: modules.dep does not exist until the first boot's
    // depmod hook has run, so nothing can be modprobed before then.
    // The "ata_*"/"pata_*" drivers are for controllers the firmware set to IDE ("compatible") mode, which
    // the AHCI driver does not handle; a module the kernel package lacks is skipped.
    let fs_module = match cfg.disk.filesystem {
        config::FsType::Ext4 => "ext4",
        config::FsType::Btrfs => "btrfs",
        config::FsType::Xfs => "xfs",
        config::FsType::F2fs => "f2fs",
    };
    let mut wanted = alloc::vec![
        fs_module,
        "nvme",
        "ahci",
        "ata_piix",
        "ata_generic",
        "pata_acpi",
        "pata_amd",
        "pata_atiixp",
        "pata_via",
        "pata_sis",
        "pata_jmicron",
        "sd_mod",
        "virtio_blk",
        "virtio_pci",
        "crc32c_intel",
        "crc32c_generic",
        "vfat",
        "fat",
        "nls_cp437",
        "nls_ascii",
    ];
    if cfg.disk.filesystem == config::FsType::Btrfs {
        wanted.push("zstd");
        wanted.push("xxhash_generic");
    }
    let resolved = {
        let mut src = FsModules {
            w: w.clone(),
            idx: &kernel.modules,
        };
        initrd::modules::resolve(&mut src, &[], &wanted, &builtin)
            .map_err(dbg_err("initramfs modules"))?
    };
    println!("initramfs modules: {:?}", resolved.order);
    let initramfs = initrd::modules::build_initramfs(boot.tiny_init, &resolved);

    w.borrow_mut()
        .finish()
        .map_err(dbg_err("finishing the root filesystem"))?;
    drop(w);
    println!("root filesystem written");

    // ESP and boot loader.
    let (esp_off, esp_len) = layout.byte_range(Layout::ESP);
    let bios_extents: [(u64, u64); 2];
    let first_boot_cmdline = format!("{cmdline} systemd.unit=archstaller-firstboot.target");
    {
        let mut region = Region::new(disk.as_mut(), esp_off, esp_len);
        let hidden_sectors = layout.partitions.get(Layout::ESP).map_or(0, |p| p.first_lba as u32);
        let mut fat = Fat32Writer::format(
            &mut region,
            FatOptions {
                label: "ESP".into(),
                volume_id: esp_volume_id,
                hidden_sectors,
                now,
            },
        )
        .map_err(dbg_err("formatting the ESP"))?;
        let loader_cfg = format!("kernel=\\vmlinuz-linux\ninitrd=\\initramfs-archstaller.img\ncmdline={first_boot_cmdline}\n");
        fat.write_file("EFI/BOOT/BOOTX64.EFI", boot.bootx64_efi)
            .map_err(dbg_err("ESP"))?;
        fat.write_file("archstaller.cfg", loader_cfg.as_bytes())
            .map_err(dbg_err("ESP"))?;
        fat.write_file("vmlinuz-linux", &vmlinuz)
            .map_err(dbg_err("ESP"))?;
        fat.write_file("initramfs-archstaller.img", &initramfs)
            .map_err(dbg_err("ESP"))?;
        let abs = |p: &str| {
            fat.file_extent(p)
                .map(|(o, l)| (esp_off + o, l))
                .ok_or_else(|| format!("internal error: {p} has no extent"))
        };
        bios_extents = [abs("vmlinuz-linux")?, abs("initramfs-archstaller.img")?];
        fat.finish().map_err(dbg_err("ESP"))?;
    }
    // BIOS: stage 1 in the MBR, stage 2 (told where the kernel and initramfs lie on the disk) in the BIOS boot partition.
    {
        use bootinfo::bios;
        let (bios_off, bios_len) = layout.byte_range(Layout::BIOS);
        if boot.bios_boot.len() <= bios::SECTOR {
            return Err("the BIOS boot module is too small".into());
        }
        let Some(stage2_slice) = boot.bios_boot.get(bios::SECTOR..) else {
            return Err("the BIOS boot module is too small".into());
        };
        let mut stage2 = stage2_slice.to_vec();
        stage2.resize(stage2.len().next_multiple_of(bios::SECTOR), 0);
        if stage2.len() as u64 > bios_len {
            return Err("BIOS boot partition too small for stage 2".into());
        }
        bios::patch_stage2(
            &mut stage2,
            bios::MODE_LINUX,
            &bios_extents,
            &first_boot_cmdline,
        )?;
        let mut region = Region::whole(disk.as_mut());
        region
            .write_at(bios_off, &stage2)
            .map_err(dbg_err("BIOS boot code"))?;
        let mut mbr = alloc::vec![0u8; bios::SECTOR];
        region.read_at(0, &mut mbr).map_err(dbg_err("MBR"))?;
        let Some(stage1_slice) = boot.bios_boot.get(..bios::SECTOR) else {
            return Err("the BIOS boot module is too small".into());
        };
        let mut stage1 = stage1_slice.to_vec();
        bios::patch_stage1(&mut stage1, bios_off, stage2.len() as u32);
        if let (Some(dst), Some(src)) = (stage1.get_mut(440..), mbr.get(440..)) {
            dst.copy_from_slice(src); // disk signature, partition table, 0x55AA
        }
        region.write_at(0, &stage1).map_err(dbg_err("MBR"))?;
    }
    disk.flush().map_err(dbg_err("flushing the disk"))?;
    println!("installation finished; rebooting into the first boot");
    Ok(())
}

fn uuid_text(u: &[u8; 16]) -> String {
    let p0 = u.get(0..4).unwrap_or(&[]);
    let p1 = u.get(4..6).unwrap_or(&[]);
    let p2 = u.get(6..8).unwrap_or(&[]);
    let p3 = u.get(8..10).unwrap_or(&[]);
    let p4 = u.get(10..16).unwrap_or(&[]);
    format!(
        "{}-{}-{}-{}-{}",
        hex(p0),
        hex(p1),
        hex(p2),
        hex(p3),
        hex(p4),
    )
}

pub fn format_fstab(cfg: &Config, root_uuid: &str, esp_uuid: &str) -> String {
    let mut fstab = String::new();
    match cfg.disk.filesystem {
        config::FsType::Ext4 => {
            fstab.push_str(&format!("UUID={root_uuid} / ext4 rw,relatime 0 1\n"));
        }
        config::FsType::Btrfs => {
            let default_subvols = config::default_btrfs_subvolumes();
            let (subvols, compress) = match &cfg.disk.btrfs {
                Some(b) => (&b.subvolumes, b.compress.as_str()),
                None => (&default_subvols, "zstd"),
            };
            for sv in subvols {
                fstab.push_str(&format!(
                    "UUID={root_uuid} {} btrfs rw,noatime,compress={},subvol={} 0 0\n",
                    sv.mount, compress, sv.name
                ));
            }
        }
        config::FsType::Xfs => {
            fstab.push_str(&format!("UUID={root_uuid} / xfs rw,relatime 0 1\n"));
        }
        config::FsType::F2fs => {
            fstab.push_str(&format!("UUID={root_uuid} / f2fs rw,noatime 0 1\n"));
        }
    }
    fstab.push_str(&format!(
        "UUID={esp_uuid} /boot vfat rw,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=ascii,shortname=mixed,utf8,errors=remount-ro 0 2\n"
    ));
    fstab
}

pub fn format_cmdline(cfg: &Config, root_uuid: &str) -> String {
    let mut cmdline = match cfg.disk.filesystem {
        config::FsType::Ext4 | config::FsType::Xfs | config::FsType::F2fs => {
            format!("root=UUID={root_uuid} rw")
        }
        config::FsType::Btrfs => {
            let root_subvol = cfg
                .disk
                .btrfs
                .as_ref()
                .and_then(|b| b.subvolumes.iter().find(|s| s.mount == "/"))
                .map(|s| s.name.as_str())
                .unwrap_or("@");
            format!("root=UUID={root_uuid} rw rootflags=subvol={root_subvol}")
        }
    };
    for k in &cfg.kernel_params {
        cmdline.push(' ');
        cmdline.push_str(k);
    }
    cmdline
}

pub(crate) fn format_dotfiles_list(users: &[config::User]) -> String {
    let mut out = String::new();
    for u in users {
        if let Some(df) = &u.dotfiles {
            let repo = config::normalize_github_repo(&df.repo).unwrap_or_else(|| df.repo.clone());
            let branch = df.branch.as_deref().unwrap_or("");
            let stow = if df.stow { "1" } else { "0" };
            let pkgs = df.packages.join(" ");
            out.push_str(&format!(
                "{}:{}:{}:{}:{}:{}\n",
                u.name, repo, df.dir, branch, stow, pkgs
            ));
        }
    }
    out
}

pub(crate) fn format_script_entry(idx: usize, script: &config::Script) -> String {
    let phase = script.phase.as_deref().unwrap_or("post-service");
    format!("{idx}:{}:{phase}:{}\n", script.id, script.args.join(" "))
}

/// Default configuration for the `ly` login manager displaying the `archstaller.dur` banner movie.
#[must_use]
pub fn ly_config() -> &'static str {
    "animation = dur_file\ndur_file_path = /etc/ly/archstaller.dur\ndur_offset_alignment = topcenter\ndur_y_offset = 2\n"
}

/// Default configuration for `greetd` using `tuigreet` with the `/etc/issue` ASCII banner.
#[must_use]
pub fn greetd_config() -> &'static str {
    "[terminal]\nvt = 1\n\n[default_session]\ncommand = \"tuigreet --time --remember --issue\"\nuser = \"greeter\"\n"
}

/// Default branding configuration for `sddm` displaying the Archstaller theme and avatars.
#[must_use]
pub fn sddm_config() -> &'static str {
    "[Theme]\nCurrent=maldives\nFacesDir=/usr/share/sddm/faces\nEnableAvatars=true\n"
}

/// Default configuration for `lightdm-gtk-greeter` displaying the Archstaller logo and background.
#[must_use]
pub fn lightdm_config() -> &'static str {
    "[greeter]\nbackground = #1a1b26\nlogo = /usr/share/pixmaps/archstaller.svg\ndefault-user-image = /usr/share/pixmaps/archstaller.svg\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            hostname: "test".into(),
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk {
                model: None,
                confirm_serial: "serial".into(),
                auto_largest: false,
                esp_mib: 512,
                filesystem: config::FsType::Ext4,
                btrfs: None,
            },
            bootloader: config::Bootloader::Grub,
            mirrors: alloc::vec![],
            packages: alloc::vec![],
            providers: alloc::vec![],
            root_password_hash: None,
            users: alloc::vec![],
            services: alloc::vec![],
            kernel_params: alloc::vec![],
            user_files: alloc::vec![],
            user_archives: alloc::vec![],
            system_files: alloc::vec![],
            dry_run: false,
            scripts: alloc::vec![],
            custom_services: alloc::vec![],
            aur: alloc::vec![],
        }
    }

    #[test]
    fn parses_locale_charset() {
        assert_eq!(charset_of("en_US.UTF-8"), "UTF-8");
        assert_eq!(charset_of("ca_ES.ISO-8859-1@euro"), "ISO-8859-1");
        assert_eq!(charset_of("POSIX"), "UTF-8");
        assert_eq!(charset_of("en_US"), "UTF-8");
    }

    #[test]
    fn formats_valid_fstab_for_all_filesystems() {
        let mut cfg = test_config();

        // Ext4
        cfg.disk.filesystem = config::FsType::Ext4;
        let fstab = format_fstab(&cfg, "1234-5678", "ABCD-EF01");
        assert!(fstab.contains("UUID=1234-5678 / ext4 rw,relatime 0 1"));
        assert!(fstab.contains("UUID=ABCD-EF01 /boot vfat"));

        // Btrfs (default subvolumes)
        cfg.disk.filesystem = config::FsType::Btrfs;
        let fstab = format_fstab(&cfg, "1234-5678", "ABCD-EF01");
        assert!(fstab.contains("UUID=1234-5678 / btrfs rw,noatime,compress=zstd,subvol=@ 0 0"));
        assert!(fstab.contains("UUID=1234-5678 /home btrfs rw,noatime,compress=zstd,subvol=@home 0 0"));
        assert!(fstab.contains("UUID=ABCD-EF01 /boot vfat"));

        // Btrfs (custom subvolumes & compression)
        cfg.disk.btrfs = Some(config::BtrfsConfig {
            subvolumes: alloc::vec![
                config::BtrfsSubvol { name: "rootfs".into(), mount: "/".into() },
                config::BtrfsSubvol { name: "data".into(), mount: "/mnt/data".into() },
            ],
            compress: "lzo".into(),
        });
        let fstab = format_fstab(&cfg, "1234-5678", "ABCD-EF01");
        assert!(fstab.contains("UUID=1234-5678 / btrfs rw,noatime,compress=lzo,subvol=rootfs 0 0"));
        assert!(fstab.contains("UUID=1234-5678 /mnt/data btrfs rw,noatime,compress=lzo,subvol=data 0 0"));

        // Xfs
        cfg.disk.filesystem = config::FsType::Xfs;
        let fstab = format_fstab(&cfg, "1234-5678", "ABCD-EF01");
        assert!(fstab.contains("UUID=1234-5678 / xfs rw,relatime 0 1"));
        assert!(fstab.contains("UUID=ABCD-EF01 /boot vfat"));

        // F2fs
        cfg.disk.filesystem = config::FsType::F2fs;
        let fstab = format_fstab(&cfg, "1234-5678", "ABCD-EF01");
        assert!(fstab.contains("UUID=1234-5678 / f2fs rw,noatime 0 1"));
        assert!(fstab.contains("UUID=ABCD-EF01 /boot vfat"));
    }

    #[test]
    fn formats_kernel_cmdline_for_all_filesystems() {
        let mut cfg = test_config();
        cfg.kernel_params = alloc::vec!["quiet".to_string(), "loglevel=3".to_string()];

        // Ext4
        cfg.disk.filesystem = config::FsType::Ext4;
        let cmdline = format_cmdline(&cfg, "test-uuid-1234");
        assert_eq!(cmdline, "root=UUID=test-uuid-1234 rw quiet loglevel=3");

        // Btrfs (default)
        cfg.disk.filesystem = config::FsType::Btrfs;
        let cmdline = format_cmdline(&cfg, "test-uuid-1234");
        assert_eq!(cmdline, "root=UUID=test-uuid-1234 rw rootflags=subvol=@ quiet loglevel=3");

        // Btrfs (custom root subvolume)
        cfg.disk.btrfs = Some(config::BtrfsConfig {
            subvolumes: alloc::vec![
                config::BtrfsSubvol { name: "arch_root".into(), mount: "/".into() },
            ],
            compress: "zstd".into(),
        });
        let cmdline = format_cmdline(&cfg, "test-uuid-1234");
        assert_eq!(cmdline, "root=UUID=test-uuid-1234 rw rootflags=subvol=arch_root quiet loglevel=3");

        // Xfs
        cfg.disk.filesystem = config::FsType::Xfs;
        let cmdline = format_cmdline(&cfg, "test-uuid-1234");
        assert_eq!(cmdline, "root=UUID=test-uuid-1234 rw quiet loglevel=3");

        // F2fs
        cfg.disk.filesystem = config::FsType::F2fs;
        let cmdline = format_cmdline(&cfg, "test-uuid-1234");
        assert_eq!(cmdline, "root=UUID=test-uuid-1234 rw quiet loglevel=3");
    }

    #[test]
    fn formats_uuid_text_correctly() {
        let bytes: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        assert_eq!(uuid_text(&bytes), "01234567-89ab-cdef-fedc-ba9876543210");
    }

    #[test]
    fn formats_dotfiles_list_correctly() {
        let users = alloc::vec![
            config::User {
                name: "alice".into(),
                password_hash: "$6$hash".into(),
                groups: alloc::vec![],
                shell: "/bin/bash".into(),
                dotfiles: Some(config::UserDotfiles {
                    repo: "alice/dots".into(),
                    dir: "dotfiles".into(),
                    branch: Some("main".into()),
                    stow: true,
                    packages: alloc::vec!["nvim".into(), "tmux".into()],
                }),
            },
            config::User {
                name: "bob".into(),
                password_hash: "$6$hash".into(),
                groups: alloc::vec![],
                shell: "/bin/bash".into(),
                dotfiles: None,
            },
        ];
        let out = format_dotfiles_list(&users);
        assert_eq!(out, "alice:alice/dots:dotfiles:main:1:nvim tmux\n");
    }

    #[test]
    fn formats_script_entry_with_phases() {
        let s1 = config::Script {
            id: "pre-script".into(),
            phase: Some("pre-service".into()),
            args: alloc::vec!["--arg1".into(), "value".into()],
            ..Default::default()
        };
        let s2 = config::Script {
            id: "post-script".into(),
            phase: None,
            args: alloc::vec![],
            ..Default::default()
        };
        assert_eq!(format_script_entry(0, &s1), "0:pre-script:pre-service:--arg1 value\n");
        assert_eq!(format_script_entry(1, &s2), "1:post-script:post-service:\n");
    }

    #[test]
    fn test_ly_config_contains_permanent_banner_settings() {
        let conf = ly_config();
        assert!(conf.ends_with('\n'));
        let lines: alloc::vec::Vec<&str> = conf.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "animation = dur_file");
        assert_eq!(lines[1], "dur_file_path = /etc/ly/archstaller.dur");
        assert_eq!(lines[2], "dur_offset_alignment = topcenter");
        assert_eq!(lines[3], "dur_y_offset = 2");
    }

    #[test]
    fn test_greetd_config() {
        let conf = greetd_config();
        assert!(conf.contains("[terminal]"));
        assert!(conf.contains("command = \"tuigreet --time --remember --issue\""));
    }

    #[test]
    fn test_sddm_config() {
        let conf = sddm_config();
        assert!(conf.contains("[Theme]"));
        assert!(conf.contains("Current=maldives"));
        assert!(conf.contains("EnableAvatars=true"));
    }

    #[test]
    fn test_lightdm_config() {
        let conf = lightdm_config();
        assert!(conf.contains("[greeter]"));
        assert!(conf.contains("background = #1a1b26"));
        assert!(conf.contains("logo = /usr/share/pixmaps/archstaller.svg"));
    }

    #[test]
    fn test_bring_up_network_empty_devices_fails() {
        let mut devs = drivers::Devices::default();
        let r = bring_up_network(&mut devs);
        match r {
            Err(e) => assert_eq!(e, "no supported network interface"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn test_format_dotfiles_list_normalizes_repo() {
        let users = [config::User {
            name: "alice".into(),
            password_hash: "$6$abc$12345678901234567890".into(),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: Some(config::UserDotfiles {
                repo: "https://github.com/alice/dotfiles.git".into(),
                dir: "dotfiles".into(),
                branch: Some("main".into()),
                stow: true,
                packages: vec!["bash".into(), "vim".into()],
            }),
        }];
        let out = format_dotfiles_list(&users);
        assert_eq!(out, "alice:alice/dotfiles:dotfiles:main:1:bash vim\n");
    }

    #[test]
    fn test_clean_path_and_pacman_meta() {
        assert_eq!(clean_path("./usr/bin/cat"), "usr/bin/cat");
        assert_eq!(clean_path("/usr/bin/cat"), "usr/bin/cat");
        assert_eq!(clean_path("///usr/bin/cat///"), "usr/bin/cat");
        assert_eq!(clean_path("././usr/bin"), "usr/bin");

        assert!(is_pacman_meta(".PKGINFO"));
        assert!(is_pacman_meta(".MTREE"));
        assert!(is_pacman_meta(".BUILDINFO"));
        assert!(is_pacman_meta(".INSTALL"));
        assert!(is_pacman_meta(".CHANGELOG"));
        assert!(!is_pacman_meta(".bashrc"));
        assert!(!is_pacman_meta(".profile"));
    }
}

