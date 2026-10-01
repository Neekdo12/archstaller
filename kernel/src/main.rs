#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod console;
mod fb;
mod heap;
mod digest;
mod idle;
mod idt;
mod hwtest;
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

/// Resets the machine. The keyboard-controller pulse alone does nothing on machines
/// without a legacy 8042 (most UEFI-only boards), so fall through to the chipset reset
/// register and finally a triple fault, which every x86 CPU turns into a reset.
pub fn reboot() -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        // 8042: wait (bounded) for the input buffer to drain, then pulse the reset line.
        for _ in 0..100_000 {
            if port::inb(0x64) & 2 == 0 {
                break;
            }
        }
        port::outb(0x64, 0xfe);
        settle();
        // Chipset reset control register: system reset, then hard reset.
        port::outb(0xcf9, 0x02);
        port::outb(0xcf9, 0x06);
        settle();
        // Triple fault: an empty IDT makes the next exception escalate to a reset.
        let empty: [u8; 10] = [0; 10];
        asm!("lidt [{}]", "int3", in(reg) empty.as_ptr(), options(nostack));
    }
    halt()
}

/// About a millisecond-scale pause: port 0x80 writes take ~1 us each.
fn settle() {
    for _ in 0..50_000 {
        unsafe { port::outb(0x80, 0) };
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
    hal::set_log_hook(digest::log);

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

    println!("calibrating clock...");
    time::init(DATE_AT_BOOT.response().map_or(0, |r| r.timestamp));
    idle::init(hhdm);
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

    // USB tethering (iPhone Personal Hotspot, Android USB tethering). Not PCI, so this is an
    // explicit probe next to probe_all. A wired NIC that already has link makes it pointless
    // (and install::run would pick the wired one anyway), so it is skipped then.
    #[cfg(feature = "usb-tethering")]
    if !devs.net.iter_mut().any(|d| d.link_up()) {
        usb_tether(&mut devs);
    }

    #[cfg(feature = "usb-selftest")]
    usb_selftest();

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

    #[cfg(not(any(feature = "disk-selftest", feature = "net-selftest", feature = "usb-selftest")))]
    {
        let module = |suffix: &str| MODULES.response().and_then(|r| r.modules().iter().find(|m| m.path().ends_with(suffix))).map(|m| m.data());
        match (cfg, keyring, module("tiny-init"), module("limine-bios.sys"), module("limine-bios-hdd.bin"), module("BOOTX64.EFI")) {
            (Some(cfg), keyring, ..) if cfg.dry_run => hwtest::run(&cfg, devs, keyring.as_ref()),
            (Some(cfg), Some(keyring), Some(tiny_init), Some(bios_sys), Some(hdd), Some(efi)) => {
                let boot = install::BootFiles { tiny_init, limine_bios_sys: bios_sys, limine_bios_hdd: hdd, bootx64_efi: efi };
                match install::run(&cfg, devs, &keyring, &boot) {
                    Ok(()) => reboot(),
                    Err(e) => println!("INSTALLATION FAILED: {e}"),
                }
            }
            _ => println!("INSTALLATION FAILED: config, keyring or boot files missing from the ISO"),
        }
    }

    println!("boot OK, halting");
    #[cfg(any(feature = "disk-selftest", feature = "net-selftest", feature = "usb-selftest"))]
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

/// Brings up an iPhone or an Android phone as a network device. A phone that is attached but not
/// tethering yet (Android in file-transfer mode) re-enumerates once the user switches USB tethering
/// on, so the scan is repeated for a while.
#[cfg(feature = "usb-tethering")]
fn usb_tether(devs: &mut drivers::Devices) {
    struct Log;
    impl imobiledevice::Progress for Log {
        fn note(&mut self, msg: &str) {
            println!("{msg}");
        }
    }
    fn add(devs: &mut drivers::Devices, what: &str, dev: Box<dyn hal::NetDevice>) {
        let m = dev.mac();
        println!("usb: {what} tethering up, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]);
        devs.net.push(dev);
    }
    const ATTEMPTS: u32 = 6;
    for attempt in 1..=ATTEMPTS {
        let mut scan = usb::Scan::new();
        // Worth waiting for only if something that could be a phone (not just storage, a hub, an
        // input device, audio, video or Bluetooth) is attached, such as an Android in file-transfer mode.
        let phone_like = scan
            .devices()
            .any(|d| d.interfaces.iter().any(|i| !matches!(i.class, 0x01 | 0x03 | 0x08 | 0x09 | 0x0e | 0xe0)));
        if imobiledevice::present(&scan) {
            match imobiledevice::tether(&mut scan, time::unix_time, 120_000, &mut Log) {
                Ok(dev) => add(devs, "iPhone", Box::new(dev)),
                Err(e) => println!("usb: iPhone tethering unavailable: {e:?}"),
            }
            return;
        }
        match usbnet::probe(&mut scan) {
            Ok(dev) => return add(devs, "USB Ethernet", Box::new(dev)),
            Err(e) => {
                if !phone_like {
                    println!("usb: no phone or USB Ethernet adapter attached, no tethering");
                    return;
                }
                if !matches!(e, hal::Error::Unsupported) {
                    println!("usb: USB tethering device failed to start: {e:?}");
                    return;
                }
            }
        }
        if attempt < ATTEMPTS {
            println!("usb: devices attached but none offers tethering; switch on USB tethering (Android) or Personal Hotspot (iPhone), retry {attempt}/{ATTEMPTS}");
            drivers::platform::delay_us(3_000_000);
        }
    }
    println!("usb: no tethering device found");
}

/// Enumerates xHCI controllers and their devices; on a USB mass-storage device, runs a
/// BOT INQUIRY to exercise bulk transfers in both directions. QEMU smoke test only.
#[cfg(feature = "usb-selftest")]
fn usb_selftest() {
    println!("usb-selftest: probing xHCI controllers");
    let ctrls = usb::controllers();
    println!("usb: {} xHCI controller(s)", ctrls.len());
    for mut c in ctrls {
        let devs = match c.enumerate() {
            Ok(d) => d,
            Err(e) => {
                println!("usb: enumerate failed: {e:?}");
                continue;
            }
        };
        for mut d in devs {
            println!(
                "usb: device {:04x}:{:04x} serial='{}' interfaces {}",
                d.vendor,
                d.product,
                d.serial,
                d.interfaces.len()
            );
            for i in &d.interfaces {
                println!(
                    "usb:   if {} class {:02x}/{:02x}/{:02x} eps {}",
                    i.num,
                    i.class,
                    i.subclass,
                    i.protocol,
                    i.endpoints.len()
                );
            }
            if let Some(i) = d
                .interfaces
                .iter()
                .find(|i| i.class == 0x08 && i.subclass == 0x06 && i.protocol == 0x50)
                .cloned()
            {
                msc_selftest(&mut c, &mut d, &i);
            }
        }
    }
}

/// BOT (bulk-only transport) INQUIRY against a USB mass-storage device.
#[cfg(feature = "usb-selftest")]
fn msc_selftest(c: &mut usb::Controller, d: &mut usb::Device, i: &usb::desc::InterfaceDesc) {
    const TAG: u32 = 0x1111_1111;
    let Some(ep_in) = d.bulk_in(i) else {
        println!("usb-selftest: msc: no bulk IN endpoint");
        return;
    };
    let Some(ep_out) = d.bulk_out(i) else {
        println!("usb-selftest: msc: no bulk OUT endpoint");
        return;
    };
    let cfg = d.configuration;
    if let Err(e) = c.set_configuration(d, cfg, i) {
        println!("usb-selftest: msc: set configuration failed: {e:?}");
        return;
    }
    let mut cbw = [0u8; 31];
    cbw[0..4].copy_from_slice(&0x4342_5355u32.to_le_bytes()); // 'USBC'
    cbw[4..8].copy_from_slice(&TAG.to_le_bytes());
    cbw[8..12].copy_from_slice(&36u32.to_le_bytes()); // data-in length
    cbw[12] = 0x80; // flags: data-in
    cbw[14] = 6; // CB length
    cbw[15] = 0x12; // SCSI INQUIRY
    cbw[19] = 36; // allocation length
    let ok = (|| {
        c.bulk_write(d, ep_out, &cbw).ok()?;
        let mut data = [0u8; 36];
        if c.bulk_read(d, ep_in, &mut data, 2000).ok()? < 36 {
            return None;
        }
        let mut csw = [0u8; 13];
        if c.bulk_read(d, ep_in, &mut csw, 2000).ok()? < 13 {
            return None;
        }
        let sig = u32::from_le_bytes(csw[0..4].try_into().ok()?) == 0x5342_5355; // 'USBS'
        let tag = u32::from_le_bytes(csw[4..8].try_into().ok()?) == TAG;
        let status = csw[12] == 0;
        let vendor = core::str::from_utf8(&data[8..16]).unwrap_or("?").trim();
        let product = core::str::from_utf8(&data[16..32]).unwrap_or("?").trim();
        println!("usb: msc INQUIRY '{vendor}' '{product}', csw sig={sig} tag={tag} status={status}");
        Some(sig && tag && status)
    })();
    println!("usb-selftest: BOT INQUIRY {}", if ok == Some(true) { "OK" } else { "FAILED" });
}
