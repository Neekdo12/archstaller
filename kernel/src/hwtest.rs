//! Hardware test mode (`dry_run = true` in the config). Probes and reads, prints a PASS/FAIL report,
//! waits a minute and reboots. It never writes to a disk: every block device is wrapped so that
//! `write` and `flush` are refused, and nothing in this module calls the installer's write paths.
use crate::install::{bring_up_network, expand, fetch_optional, fetch_to_vec, now_ms, REPOS, USER_ARCHIVE_MAX, USER_FILE_MAX};
use crate::{println, time};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use config::Config;
use drivers::Devices;
use hal::BlockDevice;
use net::client::Client;
use pgp_lite::Keyring;

const WAIT_SECONDS: u64 = 60;

/// Read-only view of a disk: writes are refused, reads pass through.
struct ReadOnly(Box<dyn BlockDevice>);

impl BlockDevice for ReadOnly {
    fn model(&self) -> &str {
        self.0.model()
    }
    fn serial(&self) -> &str {
        self.0.serial()
    }
    fn sector_size(&self) -> u32 {
        self.0.sector_size()
    }
    fn sector_count(&self) -> u64 {
        self.0.sector_count()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> hal::Result<()> {
        self.0.read(lba, buf)
    }
    fn write(&mut self, _lba: u64, _buf: &[u8]) -> hal::Result<()> {
        Err(hal::Error::Unsupported)
    }
    fn flush(&mut self) -> hal::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Report {
    pass: u32,
    warn: u32,
    fail: u32,
}

impl Report {
    fn ok(&mut self, m: &str) {
        self.pass += 1;
        println!("[ OK ] {m}");
    }
    fn warn(&mut self, m: &str) {
        self.warn += 1;
        println!("[WARN] {m}");
    }
    fn bad(&mut self, m: &str) {
        self.fail += 1;
        println!("[FAIL] {m}");
    }
    fn check(&mut self, cond: bool, m: &str) {
        if cond {
            self.ok(m)
        } else {
            self.bad(m)
        }
    }
}

pub fn run(cfg: &Config, devs: Devices, keyring: Option<&Keyring>) {
    let mut r = Report::default();
    println!("\n=============== archstaler hardware test (READ-ONLY, no disk is written) ===============");
    test(cfg, devs, keyring, &mut r);
    println!("=========================================================================================");
    println!("RESULT: {}   ({} ok, {} warnings, {} failed)", if r.fail == 0 { "PASS" } else { "FAIL" }, r.pass, r.warn, r.fail);
    println!("archstaler-hwtest: done");
    let end = now_ms() + WAIT_SECONDS * 1000;
    let mut shown = u64::MAX;
    loop {
        let left = end.saturating_sub(now_ms()).div_ceil(1000);
        if left == 0 {
            break;
        }
        if left != shown && (left % 10 == 0 || left <= 5) {
            println!("rebooting in {left}s...");
        }
        shown = left;
        core::hint::spin_loop();
    }
    crate::reboot()
}

fn test(cfg: &Config, mut devs: Devices, keyring: Option<&Keyring>, r: &mut Report) {
    println!("--- platform ---");
    r.check(core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0, "RDRAND available");
    r.check(time::tsc_hz() > 100_000_000, &format!("TSC calibrated: {} MHz", time::tsc_hz() / 1_000_000));
    // 2024-01-01; a clock before that means the firmware gave no date.
    if time::unix_time() > 1_704_067_200 {
        r.ok(&format!("boot time from firmware: unix {}", time::unix_time()));
    } else {
        r.bad("firmware clock is wrong (TLS certificate checks will fail)");
    }
    let pci = drivers::pci::enumerate();
    println!("PCI devices: {}", pci.len());
    for d in &pci {
        println!("  {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}{:02x}", d.addr.bus, d.addr.dev, d.addr.func, d.vendor, d.device, d.class, d.subclass);
    }

    println!("--- disks (read only) ---");
    if devs.block.is_empty() {
        r.warn("no supported disk found (NVMe, AHCI and virtio only)");
    }
    let disks: Vec<Box<dyn BlockDevice>> = devs.block.drain(..).map(|d| Box::new(ReadOnly(d)) as Box<dyn BlockDevice>).collect();
    for mut d in disks {
        let ss = d.sector_size() as usize;
        let mib = d.sector_count() * ss as u64 >> 20;
        let name = format!("{} serial '{}' {} MiB", d.model(), d.serial(), mib);
        let mut first = alloc::vec![0u8; ss * 2];
        let mut last = alloc::vec![0u8; ss];
        let head = d.read(0, &mut first);
        let tail = d.read(d.sector_count() - 1, &mut last);
        if head.is_ok() && tail.is_ok() {
            let table = if &first[ss..ss + 8] == b"EFI PART" {
                "GPT"
            } else if first[510] == 0x55 && first[511] == 0xaa {
                "MBR"
            } else {
                "no partition table"
            };
            r.ok(&format!("{name}: first/last sector read, {table}"));
        } else {
            r.bad(&format!("{name}: read failed (first {head:?}, last {tail:?})"));
        }
        // Writes must be refused; this proves the guard, and touches nothing.
        r.check(d.write(0, &first[..ss]).is_err(), "  write guard refuses writes");
    }

    println!("--- network ---");
    println!("NICs: {}", devs.net.len());
    if devs.net.is_empty() {
        r.bad("no supported network interface");
        return;
    }
    for n in devs.net.iter_mut() {
        let m = n.mac();
        let up = n.link_up();
        println!("  {} mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link {}", n.name(), m[0], m[1], m[2], m[3], m[4], m[5], if up { "up" } else { "down" });
    }
    let mut stack = match bring_up_network(&mut devs) {
        Ok(s) => {
            r.ok("link up and DHCP lease");
            s
        }
        Err(e) => {
            r.bad(&format!("network: {e}"));
            return;
        }
    };

    println!("--- mirror and packages ---");
    match keyring {
        Some(k) => r.ok(&format!("keyring: {} signing keys", k.keys.len())),
        None => r.bad("keyring.bin missing or invalid"),
    }
    let client = Client::new(|| time::unix_time());
    let mut dbs = Vec::new();
    for repo in REPOS {
        match fetch_to_vec(&client, &mut stack, cfg, &format!("{repo}.db"), repo) {
            Ok(bytes) => {
                let len = bytes.len();
                match pkg::db::Db::parse(repo, bytes.as_slice()) {
                    Ok(db) => {
                        r.ok(&format!("{repo}.db over HTTPS: {len} bytes, parsed"));
                        dbs.push(db);
                    }
                    Err(e) => r.bad(&format!("{repo}.db parse: {e:?}")),
                }
            }
            Err(e) => r.bad(&format!("{repo}.db download: {e}")),
        }
    }
    // The largest resolved package is what the speed test downloads, if everything else passes.
    let mut speed_target: Option<String> = None;
    if dbs.len() == REPOS.len() {
        let providers: BTreeMap<String, String> = cfg.providers.iter().cloned().collect();
        match pkg::resolve::Resolver::new(&dbs, &providers).resolve(&cfg.packages) {
            Ok(res) => {
                let total: u64 = res.packages.iter().map(|s| s.pkg.csize).sum();
                r.ok(&format!("resolved {} packages, {} MiB", res.packages.len(), total >> 20));
                if let (Some(big), Some(m)) = (res.packages.iter().max_by_key(|s| s.pkg.csize), cfg.mirrors.first()) {
                    speed_target = Some(format!("{}/{}", expand(m, &big.pkg.repo), big.pkg.filename));
                }
            }
            Err(e) => r.bad(&format!("dependency resolution: {e:?}")),
        }
    }
    for f in &cfg.user_files {
        match fetch_optional(&client, &mut stack, &f.url, USER_FILE_MAX) {
            Ok(d) => r.ok(&format!("user file {}: {} bytes", f.url, d.len())),
            Err(e) => r.warn(&format!("user file {}: {e}", f.url)),
        }
    }
    for a in &cfg.user_archives {
        match fetch_optional(&client, &mut stack, &a.url, USER_ARCHIVE_MAX) {
            Ok(d) if d.starts_with(b"PK\x03\x04") => r.ok(&format!("user archive {}: {} bytes", a.url, d.len())),
            Ok(_) => r.warn(&format!("user archive {}: not a zip", a.url)),
            Err(e) => r.warn(&format!("user archive {}: {e}", a.url)),
        }
    }
    // Only when everything above passed: measure real download throughput.
    if r.fail == 0 {
        if let Some(url) = speed_target {
            speedtest(&client, &mut stack, &url, r);
        }
    }
}

/// Downloads up to `CAP` bytes of `url` (without storing them) and reports the throughput.
fn speedtest(client: &Client, stack: &mut net::Stack, url: &str, r: &mut Report) {
    const CAP: u64 = 24 << 20;
    println!("--- speed test (up to {} MiB from the mirror) ---", CAP >> 20);
    let got = core::cell::Cell::new(0u64);
    use core::sync::atomic::Ordering::Relaxed;
    let (crypto0, io0) = (net::tls::CRYPTO_TSC.load(Relaxed), net::tls::IO_TSC.load(Relaxed));
    let start = now_ms();
    let res = client.get(stack, url, &mut |d| {
        got.set(got.get() + d.len() as u64);
        if got.get() >= CAP {
            return Err(net::Error::Http("speed test done"));
        }
        Ok(())
    });
    let ms = (now_ms() - start).max(1);
    let bytes = got.get();
    let finished = bytes >= CAP || matches!(&res, Ok(resp) if resp.status == 200);
    if !finished || bytes == 0 {
        match res {
            Ok(resp) => r.warn(&format!("speed test: HTTP {}", resp.status)),
            Err(e) => r.warn(&format!("speed test: no data ({e:?})")),
        }
        return;
    }
    let to_ms = |tsc: u64| tsc / (time::tsc_hz() / 1000).max(1);
    let crypto_ms = to_ms(net::tls::CRYPTO_TSC.load(Relaxed) - crypto0);
    let io_ms = to_ms(net::tls::IO_TSC.load(Relaxed) - io0);
    println!("  of {ms} ms: {crypto_ms} ms TLS record processing (decryption), {io_ms} ms waiting for / reading bytes");
    let kbit_per_s = bytes * 8 / ms; // bits per millisecond = kbit/s
    let mib_per_s_x10 = bytes * 10_000 / ms / (1 << 20); // MiB/s * 10
    r.ok(&format!(
        "speed test: {}.{} MiB in {}.{:02} s = {}.{} MiB/s ({}.{} Mbit/s)",
        bytes >> 20,
        (bytes % (1 << 20)) * 10 >> 20,
        ms / 1000,
        ms % 1000 / 10,
        mib_per_s_x10 / 10,
        mib_per_s_x10 % 10,
        kbit_per_s / 1000,
        kbit_per_s % 1000 / 100
    ));
}
