#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod console;
mod fb;
mod heap;
mod idt;
mod install;
mod paging;
mod platform;
mod port;
mod rng;
mod serial;
mod time;

use alloc::{boxed::Box, vec::Vec};
use core::arch::asm;
use limine::{
    memmap::MEMMAP_USABLE,
    request::{
        DateAtBootRequest, FirmwareTypeRequest, FramebufferRequest, HhdmRequest, MemmapRequest,
        ModulesRequest, StackSizeRequest,
    },
    BaseRevision, RequestsEndMarker, RequestsStartMarker,
};

#[used]
#[link_section = ".limine_requests_start"]
static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".limine_requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();
#[used]
#[link_section = ".limine_requests"]
static STACK_SIZE: StackSizeRequest = StackSizeRequest::new(256 * 1024);
#[used]
#[link_section = ".limine_requests"]
static HHDM: HhdmRequest = HhdmRequest::new();
#[used]
#[link_section = ".limine_requests"]
static MEMMAP: MemmapRequest = MemmapRequest::new();
#[used]
#[link_section = ".limine_requests"]
static FRAMEBUFFER: FramebufferRequest = FramebufferRequest::new();
#[used]
#[link_section = ".limine_requests"]
static DATE_AT_BOOT: DateAtBootRequest = DateAtBootRequest::new();
#[used]
#[link_section = ".limine_requests"]
static FIRMWARE: FirmwareTypeRequest = FirmwareTypeRequest::new();
#[used]
#[link_section = ".limine_requests"]
static MODULES: ModulesRequest = ModulesRequest::new();

#[used]
#[link_section = ".limine_requests_end"]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

pub fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { console::force_unlock() };
    println!("\n*** KERNEL PANIC: {info}");
    halt()
}

#[no_mangle]
extern "C" fn _start() -> ! {
    console::init_serial();
    println!("archstaler kernel starting");
    if !BASE_REVISION.is_supported() {
        println!("Limine base revision not supported");
        halt();
    }

    idt::init();

    if let Some(resp) = FRAMEBUFFER.response() {
        if let Some(fb) = resp.framebuffers().first() {
            if let Some(c) = fb::FbConsole::new(fb) {
                console::init_fb(c);
                println!("archstaler kernel starting");
            }
        }
    }

    let hhdm = HHDM.response().expect("no HHDM").offset;
    let memmap = MEMMAP.response().expect("no memory map").entries();
    heap::init(hhdm, memmap);

    let usable: u64 = memmap.iter().filter(|e| e.type_ == MEMMAP_USABLE).map(|e| e.length).sum();
    let fw = match FIRMWARE.response().map(|r| r.firmware_type) {
        Some(0) => "BIOS",
        Some(1) => "UEFI 32",
        Some(2) => "UEFI 64",
        _ => "unknown",
    };
    println!("firmware: {fw}");
    println!("usable memory: {} MiB in {} entries", usable >> 20, memmap.len());
    println!("heap: {} MiB", heap::size() >> 20);

    let mut v: Vec<u64> = (0..1000).collect();
    v.reverse();
    let b = Box::new(v.iter().sum::<u64>());
    println!("heap test: {}", if *b == 499_500 && v[0] == 999 { "OK" } else { "FAILED" });

    time::init(DATE_AT_BOOT.response().map_or(0, |r| r.timestamp));
    println!("tsc: {} MHz, unix time {}", time::tsc_hz() / 1_000_000, time::unix_time());

    let cfg = match MODULES.response().and_then(|r| r.modules().iter().find(|m| m.path().ends_with("config.bin"))) {
        Some(m) => match postcard::from_bytes::<config::Config>(m.data()) {
            Ok(c) => {
                println!(
                    "config: hostname={} tz={} locale={} keymap={} packages={}",
                    c.hostname,
                    c.timezone,
                    c.locale,
                    c.keymap,
                    c.packages.len()
                );
                Some(c)
            }
            Err(e) => {
                println!("config.bin parse error: {e:?}");
                None
            }
        },
        None => {
            println!("config.bin module missing");
            None
        }
    };

    drivers::platform::init(Box::leak(Box::new(platform::KernelPlatform { hhdm })));
    let mut devs = drivers::probe_all();
    println!("block devices: {}", devs.block.len());
    for (i, d) in devs.block.iter_mut().enumerate() {
        println!(
            "  blk{i}: {} serial='{}' {} sectors x {} B",
            d.model(),
            d.serial(),
            d.sector_count(),
            d.sector_size()
        );
        let mut buf = alloc::vec![0u8; d.sector_size() as usize];
        match d.read(0, &mut buf) {
            Ok(()) => println!("  blk{i}: LBA0 read OK, sig {:02x}{:02x}", buf[510], buf[511]),
            Err(e) => println!("  blk{i}: LBA0 read failed: {e:?}"),
        }
        #[cfg(feature = "disk-selftest")]
        {
            // Destructive: only for the scratch disk xtask attaches.
            let ss = d.sector_size() as usize;
            let lba = d.sector_count() - 300;
            let pat: Vec<u8> = (0..ss * 200).map(|x| (x * 7 + 3) as u8).collect();
            let mut back = alloc::vec![0u8; pat.len()];
            let ok = d.write(lba, &pat).is_ok() && d.flush().is_ok() && d.read(lba, &mut back).is_ok();
            println!("  blk{i}: write/read {}", if ok && back == pat { "OK" } else { "FAILED" });
        }
    }
    println!("net devices: {}", devs.net.len());
    for (i, d) in devs.net.iter_mut().enumerate() {
        let m = d.mac();
        let link = if d.link_up() { "up" } else { "down" };
        println!(
            "  net{i}: {} mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link {}",
            d.name(),
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5],
            link
        );
        #[cfg(feature = "net-selftest")]
        {
            let ok = net_selftest(d.as_mut(), m);
            println!("  net{i}: ARP round trip {}", if ok { "OK" } else { "FAILED" });
        }
    }

    let keyring = MODULES
        .response()
        .and_then(|r| r.modules().iter().find(|m| m.path().ends_with("keyring.bin")))
        .and_then(|m| pgp_lite::Keyring::from_bytes(m.data()));
    match &keyring {
        Some(k) => println!("keyring: {} signing keys", k.keys.len()),
        None => println!("keyring.bin module missing or invalid"),
    }
    #[cfg(feature = "net-selftest")]
    if let Some(k) = &keyring {
        pgp_selftest(k);
    }

    #[cfg(feature = "net-selftest")]
    if let Some(nic) = devs.net.pop() {
        stack_selftest(nic);
    }

    #[cfg(feature = "fault-test")]
    unsafe {
        println!("triggering ud2");
        asm!("ud2");
    }

    #[cfg(not(any(feature = "disk-selftest", feature = "net-selftest")))]
    {
        let module = |suffix: &str| MODULES.response().and_then(|r| r.modules().iter().find(|m| m.path().ends_with(suffix))).map(|m| m.data());
        match (cfg, keyring, module("tiny-init"), module("limine-bios.sys"), module("limine-bios-hdd.bin"), module("BOOTX64.EFI")) {
            (Some(cfg), Some(keyring), Some(tiny_init), Some(bios_sys), Some(hdd), Some(efi)) => {
                let boot = install::BootFiles { tiny_init, limine_bios_sys: bios_sys, limine_bios_hdd: hdd, bootx64_efi: efi };
                match install::run(&cfg, devs, &keyring, &boot) {
                    Ok(()) => unsafe {
                        // Reset through the keyboard controller.
                        port::outb(0x64, 0xfe);
                    },
                    Err(e) => println!("INSTALLATION FAILED: {e}"),
                }
            }
            _ => println!("INSTALLATION FAILED: config, keyring or boot files missing from the ISO"),
        }
    }

    println!("boot OK, halting");
    #[cfg(any(feature = "disk-selftest", feature = "net-selftest"))]
    unsafe {
        // QEMU isa-debug-exit
        port::outb(0xf4, 0);
    }
    halt()
}

/// Sends an ARP request for the QEMU user-net gateway (10.0.2.2) and waits for the reply.
#[cfg(feature = "net-selftest")]
fn net_selftest(d: &mut dyn hal::NetDevice, mac: [u8; 6]) -> bool {
    let mut f = [0u8; 42];
    f[0..6].fill(0xff);
    f[6..12].copy_from_slice(&mac);
    f[12..14].copy_from_slice(&[0x08, 0x06]);
    f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    f[22..28].copy_from_slice(&mac);
    f[28..32].copy_from_slice(&[10, 0, 2, 15]);
    f[38..42].copy_from_slice(&[10, 0, 2, 2]);
    if d.transmit(&f).is_err() {
        return false;
    }
    let deadline = time::uptime_ns() + 2_000_000_000;
    let mut buf = [0u8; 2048];
    while time::uptime_ns() < deadline {
        if let Some(n) = d.receive(&mut buf) {
            if n >= 42 && buf[12..14] == [0x08, 0x06] && buf[20..22] == [0, 2] && buf[28..32] == [10, 0, 2, 2] {
                return true;
            }
        }
    }
    false
}

/// DHCP + HTTP GET against the xtask test server on the QEMU host (10.0.2.2:8000).
#[cfg(feature = "net-selftest")]
fn stack_selftest(nic: Box<dyn hal::NetDevice>) {
    use net::Stream as _;
    fn now_ms() -> u64 {
        time::uptime_ns() / 1_000_000
    }
    let seed = unsafe { core::arch::x86_64::_rdtsc() };
    let mut stack = net::Stack::new(nic, now_ms, seed);
    match stack.dhcp(10_000) {
        Ok(l) => println!("dhcp: {:?}/{} router {:?} dns {:?}", l.address, l.prefix, l.router, l.dns),
        Err(e) => {
            println!("dhcp failed: {e:?}");
            return;
        }
    }
    tls_selftest(&mut stack);
    for path in ["/pattern.bin", "/chunked.bin"] {
        let mut conn = match stack.connect([10, 0, 2, 2], 8000, 5000) {
            Ok(c) => c,
            Err(e) => {
                println!("http: connect failed: {e:?}");
                return;
            }
        };
        let mut total = 0usize;
        let mut bad = false;
        let t0 = time::uptime_ns();
        let r = net::http::get(&mut conn, "10.0.2.2:8000", path, &mut |data| {
            for (i, b) in data.iter().enumerate() {
                if *b != ((total + i) * 7 + 3) as u8 {
                    bad = true;
                }
            }
            total += data.len();
            Ok(())
        });
        let _ = conn.read(&mut []);
        match r {
            Ok(resp) => println!(
                "http {path}: status {} body {} bytes {} in {} ms",
                resp.status,
                total,
                if !bad && total == 1_000_000 { "OK" } else { "FAILED" },
                (time::uptime_ns() - t0) / 1_000_000
            ),
            Err(e) => println!("http {path}: error {e:?}"),
        }
    }
}

#[cfg(feature = "net-selftest")]
fn tls_selftest(stack: &mut net::Stack) {
    const HOST: &str = "geo.mirror.pkgbuild.com";
    let ip = match stack.resolve(HOST, 5000) {
        Ok(ip) => ip,
        Err(e) => {
            println!("dns {HOST}: {e:?} (offline?)");
            return;
        }
    };
    println!("dns {HOST}: {ip:?}");
    let conn = match stack.connect(ip, 443, 5000) {
        Ok(c) => c,
        Err(e) => {
            println!("https: connect failed: {e:?}");
            return;
        }
    };
    let cfg = net::tls::client_config(|| time::unix_time());
    let mut tls = match net::tls::TlsStream::connect(conn, cfg, HOST) {
        Ok(t) => t,
        Err(e) => {
            println!("https: handshake failed: {e:?}");
            return;
        }
    };
    let mut total = 0usize;
    let r = net::http::get(&mut tls, HOST, "/core/os/x86_64/core.db", &mut |d| {
        total += d.len();
        Ok(())
    });
    match r {
        Ok(resp) => println!("https core.db: status {} body {} bytes loc {:?}", resp.status, total, resp.location),
        Err(e) => println!("https core.db: error {e:?}"),
    }
}

/// Verifies two real Arch package signatures (RSA and EdDSA) against the keyring module.
#[cfg(feature = "net-selftest")]
fn pgp_selftest(k: &pgp_lite::Keyring) {
    let cases: [(&str, &[u8], &[u8]); 2] = [
        (
            "rsa",
            include_bytes!("../../crates/pgp-lite/tests/fixtures/pambase-20260616-1-any.pkg.tar.zst"),
            include_bytes!("../../crates/pgp-lite/tests/fixtures/pambase-20260616-1-any.pkg.tar.zst.sig"),
        ),
        (
            "eddsa",
            include_bytes!("../../crates/pgp-lite/tests/fixtures/systemd-sysvcompat-261.1-1-x86_64.pkg.tar.zst"),
            include_bytes!("../../crates/pgp-lite/tests/fixtures/systemd-sysvcompat-261.1-1-x86_64.pkg.tar.zst.sig"),
        ),
    ];
    for (name, data, sig) in cases {
        let r = pgp_lite::Verifier::from_packet(sig).and_then(|mut v| {
            v.update(data);
            v.finish(k, time::unix_time())
        });
        println!("pgp {name}: {}", if r.is_ok() { "OK" } else { "FAILED" });
    }
}
