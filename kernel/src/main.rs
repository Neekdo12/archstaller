#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod console;
mod fb;
mod heap;
mod idt;
mod port;
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

    match MODULES.response().and_then(|r| r.modules().iter().find(|m| m.path().ends_with("config.bin"))) {
        Some(m) => match postcard::from_bytes::<config::Config>(m.data()) {
            Ok(c) => println!(
                "config: hostname={} tz={} locale={} keymap={} packages={}",
                c.hostname,
                c.timezone,
                c.locale,
                c.keymap,
                c.packages.len()
            ),
            Err(e) => println!("config.bin parse error: {e:?}"),
        },
        None => println!("config.bin module missing"),
    }

    #[cfg(feature = "fault-test")]
    unsafe {
        println!("triggering ud2");
        asm!("ud2");
    }

    println!("boot OK, halting");
    halt()
}
