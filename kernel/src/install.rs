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
use disk::gpt::Layout;
use disk::region::Region;
use drivers::Devices;
use ext4w::{Ext4Writer, FileKind, Meta, Options as ExtOptions, Storage};
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
    pub limine_bios_sys: &'static [u8],
    pub limine_bios_hdd: &'static [u8],
    pub bootx64_efi: &'static [u8],
}

const ARCH: &str = "x86_64";
const REPOS: [&str; 2] = ["core", "extra"];
const CACHE_DIR: &str = "var/cache/pacman/pkg";
const STATE_DIR: &str = "var/lib/archstaler";

type R<T> = Result<T, String>;

fn dbg_err<E: core::fmt::Debug>(ctx: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{ctx}: {e:?}")
}

struct RegionStorage<'a>(Region<'a>);

impl Storage for RegionStorage<'_> {
    fn write_at(&mut self, offset: u64, data: &[u8]) -> ext4w::Result<()> {
        self.0.write_at(offset, data).map_err(|_| ext4w::Error::Io)
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> ext4w::Result<()> {
        self.0.read_at(offset, buf).map_err(|_| ext4w::Error::Io)
    }
}

/// Streams a file back out of the filesystem being written.
struct CacheReader<S: Storage> {
    w: Rc<RefCell<Ext4Writer<S>>>,
    ino: u32,
    pos: u64,
    size: u64,
    buf: Vec<u8>,
    off: usize,
}

impl<S: Storage> Read for CacheReader<S> {
    fn read(&mut self, out: &mut [u8]) -> pkg::Result<usize> {
        if self.off == self.buf.len() {
            let n = (self.size - self.pos).min(256 * 1024) as usize;
            if n == 0 {
                return Ok(0);
            }
            self.buf.resize(n, 0);
            self.w.borrow_mut().read_file(self.ino, self.pos, &mut self.buf).map_err(|_| pkg::Error::Io)?;
            self.pos += n as u64;
            self.off = 0;
        }
        let n = (self.buf.len() - self.off).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.off..self.off + n]);
        self.off += n;
        Ok(n)
    }
}

fn now_ms() -> u64 {
    time::uptime_ns() / 1_000_000
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rng::fill(&mut b).expect("RDRAND is required");
    b
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn expand(mirror: &str, repo: &str) -> String {
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
        println!("  {} serial '{}' {} MiB", d.model(), d.serial(), d.sector_count() * d.sector_size() as u64 >> 20);
    }
    match hits.len() {
        0 => return Err("no disk matches the selector; nothing was written".into()),
        1 => {}
        n => return Err(format!("selector matches {n} disks (need exactly one); nothing was written")),
    }
    let d = devs.block.remove(hits[0]);
    if d.serial().trim() != want {
        return Err(format!("selected disk has serial '{}' but confirm_serial is '{want}'; nothing was written", d.serial()));
    }
    Ok(d)
}

/// `disk.auto_largest`: the biggest disk is erased. A tie for the biggest is refused.
fn select_largest(devs: &mut Devices) -> R<Box<dyn BlockDevice>> {
    let size = |d: &Box<dyn BlockDevice>| d.sector_count() * d.sector_size() as u64;
    println!("disks found: {}", devs.block.len());
    for d in &devs.block {
        println!("  {} serial '{}' {} MiB", d.model(), d.serial(), size(d) >> 20);
    }
    let max = devs.block.iter().map(size).max().ok_or("no disk found; nothing was written")?;
    let biggest: Vec<usize> = (0..devs.block.len()).filter(|&i| size(&devs.block[i]) == max).collect();
    if biggest.len() != 1 {
        return Err(format!("{} disks tie for the largest size ({} MiB); refusing to guess, nothing was written", biggest.len(), max >> 20));
    }
    println!("disk.auto_largest is set: the largest disk will be ERASED");
    Ok(devs.block.remove(biggest[0]))
}

const USER_FILE_MAX: usize = 1 << 20;
const USER_ARCHIVE_MAX: usize = 16 << 20;

/// Downloads a small optional file; any failure is reported as text, never as a hard error.
fn fetch_optional(client: &Client, stack: &mut net::Stack, url: &str, max: usize) -> Result<Vec<u8>, String> {
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

fn bring_up_network(devs: &mut Devices) -> R<net::Stack> {
    if devs.net.is_empty() {
        return Err("no supported network interface".into());
    }
    let mut idx = 0;
    let end = now_ms() + 10_000;
    'wait: while now_ms() < end {
        for (i, n) in devs.net.iter_mut().enumerate() {
            if n.link_up() {
                idx = i;
                break 'wait;
            }
        }
    }
    let nic = devs.net.remove(idx);
    println!("network: {} link {}", nic.name(), "up");
    let mut stack = net::Stack::new(nic, now_ms, unsafe { core::arch::x86_64::_rdtsc() });
    let lease = stack.dhcp(30_000).map_err(dbg_err("DHCP"))?;
    println!("network: {:?}/{} gateway {:?} dns {:?}", lease.address, lease.prefix, lease.router, lease.dns);
    Ok(stack)
}

fn fetch_to_vec(client: &Client, stack: &mut net::Stack, cfg: &Config, path: &str, repo: &str) -> R<Vec<u8>> {
    let mut last = String::new();
    for m in &cfg.mirrors {
        let url = format!("{}/{}", expand(m, repo), path);
        let mut data = Vec::new();
        match client.get(stack, &url, &mut |d| {
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
fn download_package<S: Storage>(
    client: &Client,
    stack: &mut net::Stack,
    cfg: &Config,
    w: &Rc<RefCell<Ext4Writer<S>>>,
    keyring: &Keyring,
    p: &Package,
) -> R<u32> {
    if p.pgpsig.is_empty() {
        return Err(format!("{}: database has no signature", p.name));
    }
    let sig = pgp_lite::base64_decode(&p.pgpsig).map_err(dbg_err("PGPSIG"))?;
    let path = format!("{CACHE_DIR}/{}", p.filename);
    let meta = Meta { mode: 0o644, mtime: time::unix_time(), ..Default::default() };
    let mut last = String::new();
    for attempt in 0..cfg.mirrors.len() * 2 {
        let mirror = &cfg.mirrors[attempt % cfg.mirrors.len()];
        let url = format!("{}/{}", expand(mirror, &p.repo), p.filename);
        let hasher = RefCell::new(Sha256::new());
        let verifier = RefCell::new(Some(pgp_lite::Verifier::from_packet(&sig).map_err(dbg_err("signature packet"))?));
        let received = Cell::new(0u64);
        w.borrow_mut().begin_file(&path, &meta).map_err(dbg_err("cache file"))?;
        let res = client.get(stack, &url, &mut |d| {
            hasher.borrow_mut().update(d);
            if let Some(v) = verifier.borrow_mut().as_mut() {
                v.update(d);
            }
            received.set(received.get() + d.len() as u64);
            w.borrow_mut().write_file(d).map_err(|_| net::Error::Http("cache write failed"))
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
        let v = verifier.borrow_mut().take().unwrap();
        // A failed signature is not retried elsewhere: the mirror served content the maintainers did not sign.
        v.finish(keyring, time::unix_time()).map_err(|e| format!("{}: signature check failed: {e:?} (rebuild the ISO for a newer keyring if the key is unknown)", p.filename))?;
        return Ok(ino);
    }
    Err(format!("download failed after retries: {last}"))
}

#[derive(Default)]
struct KernelIndex {
    modules: BTreeMap<String, String>,
    vmlinuz: Option<String>,
    builtin: Option<String>,
}

fn clean_path(p: &str) -> &str {
    p.trim_start_matches("./").trim_end_matches('/')
}

fn extract_package<S: Storage>(w: &Rc<RefCell<Ext4Writer<S>>>, ino: u32, size: u64, idx: &mut KernelIndex) -> R<()> {
    let reader = CacheReader { w: w.clone(), ino, pos: 0, size, buf: Vec::new(), off: 0 };
    let dec = pkg::compress::open(reader).map_err(dbg_err("package compression"))?;
    let mut tar = TarReader::new(dec);
    let mut buf = alloc::vec![0u8; 64 * 1024];
    while let Some(e) = tar.next_entry().map_err(dbg_err("package archive"))? {
        let path = clean_path(&e.path).to_string();
        if path.is_empty() || (path.starts_with('.') && !path.contains('/')) {
            continue; // .PKGINFO, .MTREE, .BUILDINFO, .INSTALL, .CHANGELOG
        }
        let meta = Meta { mode: (e.mode & 0o7777) as u16, uid: e.uid, gid: e.gid, mtime: e.mtime, xattrs: e.xattrs.clone() };
        let ctx = |what: &str| format!("{path}: {what}");
        match e.kind {
            Kind::Dir => {
                w.borrow_mut().mkdir(&path, &meta).map_err(|er| ctx(&format!("mkdir {er:?}")))?;
            }
            Kind::File => {
                w.borrow_mut().begin_file(&path, &meta).map_err(|er| ctx(&format!("create {er:?}")))?;
                loop {
                    let n = tar.read_data(&mut buf).map_err(dbg_err("package data"))?;
                    if n == 0 {
                        break;
                    }
                    w.borrow_mut().write_file(&buf[..n]).map_err(|er| ctx(&format!("write {er:?}")))?;
                }
                w.borrow_mut().end_file().map_err(|er| ctx(&format!("close {er:?}")))?;
                if path.starts_with("usr/lib/modules/") {
                    if path.ends_with("/vmlinuz") {
                        idx.vmlinuz = Some(path.clone());
                    } else if path.ends_with("/modules.builtin") {
                        idx.builtin = Some(path.clone());
                    } else if let Some(n) = initrd::modules::name_from_path(&path) {
                        idx.modules.insert(n, path.clone());
                    }
                }
            }
            Kind::Symlink => {
                w.borrow_mut().symlink(&path, &e.link, &meta).map_err(|er| ctx(&format!("symlink {er:?}")))?;
            }
            Kind::Hardlink => {
                w.borrow_mut().hardlink(&path, clean_path(&e.link)).map_err(|er| ctx(&format!("hardlink {er:?}")))?;
            }
            Kind::CharDev => {
                w.borrow_mut().mknod(&path, FileKind::CharDev, e.dev_major, e.dev_minor, &meta).map_err(|er| ctx(&format!("mknod {er:?}")))?;
            }
            Kind::BlockDev => {
                w.borrow_mut().mknod(&path, FileKind::BlockDev, e.dev_major, e.dev_minor, &meta).map_err(|er| ctx(&format!("mknod {er:?}")))?;
            }
            Kind::Fifo => {
                w.borrow_mut().mknod(&path, FileKind::Fifo, 0, 0, &meta).map_err(|er| ctx(&format!("mkfifo {er:?}")))?;
            }
        }
    }
    Ok(())
}

fn read_whole<S: Storage>(w: &Rc<RefCell<Ext4Writer<S>>>, path: &str) -> R<Vec<u8>> {
    let mut wr = w.borrow_mut();
    let ino = wr.lookup(path).ok_or_else(|| format!("{path} not found in the installed system"))?;
    let mut out = alloc::vec![0u8; wr.file_size(ino) as usize];
    wr.read_file(ino, 0, &mut out).map_err(dbg_err("read back"))?;
    Ok(out)
}

struct ExtModules<'a, S: Storage> {
    w: &'a Rc<RefCell<Ext4Writer<S>>>,
    idx: &'a BTreeMap<String, String>,
}

impl<S: Storage> initrd::modules::ModuleSource for ExtModules<'_, S> {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        read_whole(self.w, self.idx.get(name)?).ok()
    }
}

fn put<S: Storage>(w: &Rc<RefCell<Ext4Writer<S>>>, path: &str, mode: u16, data: &[u8]) -> R<()> {
    let meta = Meta { mode, mtime: time::unix_time(), ..Default::default() };
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

pub fn run(cfg: &Config, mut devs: Devices, keyring: &Keyring, boot: &BootFiles) -> R<()> {
    // 11. Select the disk. Nothing is written before this succeeds.
    let mut disk = select_disk(cfg, &mut devs)?;
    println!("target disk: {} serial '{}'", disk.model(), disk.serial());

    // 12. Network, databases, resolution.
    let mut stack = bring_up_network(&mut devs)?;
    let client = Client::new(|| time::unix_time());
    let mut dbs = Vec::new();
    let mut db_bytes: Vec<(&str, Vec<u8>)> = Vec::new();
    for repo in REPOS {
        let bytes = fetch_to_vec(&client, &mut stack, cfg, &format!("{repo}.db"), repo)?;
        println!("{repo}.db: {} bytes", bytes.len());
        dbs.push(pkg::db::Db::parse(repo, bytes.as_slice()).map_err(dbg_err("database"))?);
        db_bytes.push((repo, bytes));
    }
    let providers: BTreeMap<String, String> = cfg.providers.iter().cloned().collect();
    let resolution = pkg::resolve::Resolver::new(&dbs, &providers).resolve(&cfg.packages).map_err(dbg_err("dependency resolution"))?;
    for a in &resolution.ambiguities {
        println!("note: {} is provided by {:?}; using {} (set providers.{} to choose)", a.dep, a.candidates, a.chosen, a.dep);
    }
    let total: u64 = resolution.packages.iter().map(|s| s.pkg.csize).sum();
    println!("resolved {} packages, {} MiB to download", resolution.packages.len(), total >> 20);

    // 13. Partition.
    let ss = disk.sector_size();
    let guids = [random::<16>(), random::<16>(), random::<16>(), random::<16>()];
    let layout = Layout::plan(ss, disk.sector_count(), cfg.disk.esp_mib as u64 * 1024 * 1024, guids).map_err(dbg_err("partitioning"))?;
    layout.write(&mut Region::whole(disk.as_mut())).map_err(dbg_err("writing the partition table"))?;
    println!("partition table written");

    let root_uuid: [u8; 16] = random();
    let (root_off, root_len) = layout.byte_range(Layout::ROOT);
    let now = time::unix_time();
    let storage = RegionStorage(Region::new(disk.as_mut(), root_off, root_len));
    let writer = Ext4Writer::new(
        storage,
        root_len,
        ExtOptions { label: "archroot".into(), uuid: root_uuid, hash_seed: [random::<4>().map(|b| b as u32)[0], 0x1234_5678, 0x9abc_def0, 0x0fed_cba9], now, reserved_percent: 1 },
    )
    .map_err(dbg_err("mkfs"))?;
    let w = Rc::new(RefCell::new(writer));
    {
        let mut wr = w.borrow_mut();
        for (d, mode) in [("dev", 0o755), ("proc", 0o755), ("sys", 0o755), ("run", 0o755), ("tmp", 0o1777), ("etc", 0o755), ("boot", 0o755), ("var", 0o755), (CACHE_DIR, 0o755), (STATE_DIR, 0o755)] {
            wr.mkdir(d, &Meta { mode, mtime: now, ..Default::default() }).map_err(dbg_err("mkdir"))?;
        }
    }

    // 14. Download, verify, extract.
    let mut idx = KernelIndex::default();
    let n = resolution.packages.len();
    for (i, s) in resolution.packages.iter().enumerate() {
        println!("[{}/{}] {} {} ({} KiB)", i + 1, n, s.pkg.name, s.pkg.version, s.pkg.csize >> 10);
        let ino = download_package(&client, &mut stack, cfg, &w, keyring, s.pkg)?;
        extract_package(&w, ino, s.pkg.csize, &mut idx)?;
    }
    for (repo, bytes) in &db_bytes {
        put(&w, &format!("var/lib/pacman/sync/{repo}.db"), 0o644, bytes)?;
    }

    // 15. System configuration. Everything under /etc is also stored as an overlay that the first
    // boot re-applies after `pacman -U --overwrite '*'` has replaced package-owned files.
    let esp_volume_id = u32::from_le_bytes(random::<4>());
    let root_uuid_text = uuid_text(&root_uuid);
    let esp_uuid_text = format!("{:04X}-{:04X}", esp_volume_id >> 16, esp_volume_id & 0xffff);
    let mut cmdline = format!("root=UUID={root_uuid_text} rw");
    for k in &cfg.kernel_params {
        cmdline.push(' ');
        cmdline.push_str(k);
    }
    let overlay = |p: &str| format!("{STATE_DIR}/overlay/{p}");
    let mut etc_files: Vec<(String, u16, Vec<u8>)> = Vec::new();
    etc_files.push(("etc/fstab".into(), 0o644, format!(
        "UUID={root_uuid_text} / ext4 rw,relatime 0 1\nUUID={esp_uuid_text} /boot vfat rw,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=ascii,shortname=mixed,utf8,errors=remount-ro 0 2\n"
    ).into_bytes()));
    etc_files.push(("etc/hostname".into(), 0o644, format!("{}\n", cfg.hostname).into_bytes()));
    etc_files.push(("etc/locale.conf".into(), 0o644, format!("LANG={}\n", cfg.locale).into_bytes()));
    etc_files.push(("etc/locale.gen".into(), 0o644, format!("{} {}\n", cfg.locale, charset_of(&cfg.locale)).into_bytes()));
    etc_files.push(("etc/vconsole.conf".into(), 0o644, format!("KEYMAP={}\n", cfg.keymap).into_bytes()));
    let mut mirrorlist = String::from("# Generated by archstaler\n");
    for m in &cfg.mirrors {
        mirrorlist.push_str(&format!("Server = {m}\n"));
    }
    etc_files.push(("etc/pacman.d/mirrorlist".into(), 0o644, mirrorlist.into_bytes()));
    etc_files.push(("etc/sudoers.d/10-wheel".into(), 0o440, b"%wheel ALL=(ALL:ALL) ALL\n".to_vec()));
    for (path, mode, data) in &etc_files {
        put(&w, path, *mode, data)?;
        put(&w, &overlay(path), *mode, data)?;
    }
    let tz_target = format!("../usr/share/zoneinfo/{}", cfg.timezone);
    let link_meta = Meta { mode: 0o777, mtime: now, ..Default::default() };
    for path in ["etc/localtime".to_string(), overlay("etc/localtime")] {
        w.borrow_mut().symlink(&path, &tz_target, &link_meta).map_err(dbg_err("localtime"))?;
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
    put(&w, &format!("{STATE_DIR}/userfiles.list"), 0o644, userfiles.as_bytes())?;
    let mut userarchives = String::new();
    for (i, a) in cfg.user_archives.iter().enumerate() {
        match fetch_optional(&client, &mut stack, &a.url, USER_ARCHIVE_MAX) {
            // A server that answers 200 with an error page must not be unpacked as an archive.
            Ok(data) if !data.starts_with(b"PK\x03\x04") => println!("warning: skipping {}: not a zip archive", a.url),
            Ok(data) => {
                put(&w, &format!("{STATE_DIR}/userarchives/{i}"), 0o644, &data)?;
                userarchives.push_str(&format!("{i}\n"));
                println!("user archive {} -> ~/ ({} bytes)", a.url, data.len());
            }
            Err(e) => println!("warning: skipping {}: {e}", a.url),
        }
    }
    put(&w, &format!("{STATE_DIR}/userarchives.list"), 0o644, userarchives.as_bytes())?;

    let files: Vec<String> = resolution.packages.iter().map(|s| s.pkg.filename.clone()).collect();
    let deps: Vec<String> = resolution.packages.iter().filter(|s| !s.explicit).map(|s| s.pkg.name.clone()).collect();
    let mut users = String::new();
    for u in &cfg.users {
        users.push_str(&format!("{}:{}:{}:{}\n", u.name, u.password_hash, u.groups.join(","), u.shell));
    }
    let lines = |v: &[String]| v.iter().map(|s| format!("{s}\n")).collect::<String>();
    put(&w, &format!("{STATE_DIR}/packages.list"), 0o644, lines(&files).as_bytes())?;
    put(&w, &format!("{STATE_DIR}/deps.list"), 0o644, lines(&deps).as_bytes())?;
    put(&w, &format!("{STATE_DIR}/users.list"), 0o600, users.as_bytes())?;
    put(&w, &format!("{STATE_DIR}/services.list"), 0o644, lines(&cfg.services).as_bytes())?;
    put(&w, &format!("{STATE_DIR}/cmdline"), 0o644, cmdline.as_bytes())?;
    if let Some(h) = &cfg.root_password_hash {
        put(&w, &format!("{STATE_DIR}/root.hash"), 0o600, h.as_bytes())?;
    }
    put(&w, &format!("{STATE_DIR}/firstboot"), 0o644, b"1\n")?;
    put(&w, "usr/lib/systemd/system/archstaler-firstboot.target", 0o644, include_bytes!("../../firstboot/archstaler-firstboot.target"))?;
    put(&w, "usr/lib/systemd/system/archstaler-firstboot.service", 0o644, include_bytes!("../../firstboot/archstaler-firstboot.service"))?;
    put(&w, "usr/lib/archstaler/firstboot.sh", 0o755, include_bytes!("../../firstboot/firstboot.sh"))?;
    // Inert unless a config enables it (the "tester" preset does).
    put(&w, "usr/lib/systemd/system/archstaler-selftest.service", 0o644, include_bytes!("../../firstboot/archstaler-selftest.service"))?;
    put(&w, "usr/lib/archstaler/selftest.sh", 0o755, include_bytes!("../../firstboot/selftest.sh"))?;

    // 16. Initramfs for the first boot and the kernel image.
    let vmlinuz_path = idx.vmlinuz.clone().ok_or("the installed packages contain no kernel (add the 'linux' package)")?;
    let vmlinuz = read_whole(&w, &vmlinuz_path)?;
    let builtin = match &idx.builtin {
        Some(p) => initrd::modules::parse_builtin(&String::from_utf8_lossy(&read_whole(&w, p)?)),
        None => Default::default(),
    };
    // Root device drivers, plus vfat for /boot: modules.dep does not exist until the first boot's
    // depmod hook has run, so nothing can be modprobed before then.
    let wanted = ["ext4", "nvme", "ahci", "sd_mod", "virtio_blk", "virtio_pci", "crc32c_intel", "crc32c_generic", "vfat", "fat", "nls_cp437", "nls_ascii"];
    let resolved = {
        let mut src = ExtModules { w: &w, idx: &idx.modules };
        initrd::modules::resolve(&mut src, &[], &wanted, &builtin).map_err(dbg_err("initramfs modules"))?
    };
    println!("initramfs modules: {:?}", resolved.order);
    let initramfs = initrd::modules::build_initramfs(boot.tiny_init, &resolved);

    let rw = Rc::try_unwrap(w).map_err(|_| "internal error: filesystem still shared")?.into_inner();
    let mut rw = rw;
    rw.finish().map_err(dbg_err("finishing the root filesystem"))?;
    drop(rw);
    println!("root filesystem written");

    // ESP and boot loader.
    let (esp_off, esp_len) = layout.byte_range(Layout::ESP);
    {
        let mut region = Region::new(disk.as_mut(), esp_off, esp_len);
        let mut fat = Fat32Writer::format(&mut region, FatOptions { label: "ESP".into(), volume_id: esp_volume_id, hidden_sectors: layout.partitions[Layout::ESP].first_lba as u32, now }).map_err(dbg_err("formatting the ESP"))?;
        let limine_conf = format!(
            "timeout: 0\n\n/Arch Linux (first boot)\n    protocol: linux\n    path: boot():/vmlinuz-linux\n    cmdline: {cmdline} systemd.unit=archstaler-firstboot.target\n    module_path: boot():/initramfs-archstaler.img\n"
        );
        fat.write_file("EFI/BOOT/BOOTX64.EFI", boot.bootx64_efi).map_err(dbg_err("ESP"))?;
        fat.write_file("limine/limine.conf", limine_conf.as_bytes()).map_err(dbg_err("ESP"))?;
        fat.write_file("limine/limine-bios.sys", boot.limine_bios_sys).map_err(dbg_err("ESP"))?;
        fat.write_file("vmlinuz-linux", &vmlinuz).map_err(dbg_err("ESP"))?;
        fat.write_file("initramfs-archstaler.img", &initramfs).map_err(dbg_err("ESP"))?;
        fat.finish().map_err(dbg_err("ESP"))?;
    }
    let (bios_off, bios_len) = layout.byte_range(Layout::BIOS);
    disk::limine::bios_install(&mut Region::whole(disk.as_mut()), boot.limine_bios_hdd, bios_off, bios_len).map_err(dbg_err("BIOS boot code"))?;
    disk.flush().map_err(dbg_err("flushing the disk"))?;
    println!("installation finished; rebooting into the first boot");
    Ok(())
}

fn uuid_text(u: &[u8; 16]) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        hex(&u[0..4]),
        hex(&u[4..6]),
        hex(&u[6..8]),
        hex(&u[8..10]),
        hex(&u[10..16])
    )
}
