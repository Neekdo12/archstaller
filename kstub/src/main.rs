//! Loader for the `--super-small` ISO. Limine cannot unpack a compressed kernel, so it loads this
//! stub instead. The stub inflates the real kernel (module `kernel.z`) to the address the kernel
//! is linked at, hands over the answers Limine wrote into the stub's own requests, and jumps to it.
//!
//! `kernel.z` is `[entry: u64 le][image length: u64 le][raw deflate]`, built by xtask.
#![no_std]
#![no_main]

use core::arch::asm;
use limine::{
    request::{DateAtBootRequest, FirmwareTypeRequest, FramebufferRequest, HhdmRequest, MemmapRequest, ModulesRequest, StackSizeRequest},
    BaseRevision, RequestsEndMarker, RequestsStartMarker,
};
use miniz_oxide::inflate::core::{decompress, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

// These must be the requests the kernel makes (kernel/src/main.rs); a request missing here would
// get no answer in the kernel.
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

const PAYLOAD_BASE: u64 = 0xffff_ffff_8000_0000;
/// Must equal PAYLOAD_SIZE in linker.ld.
const PAYLOAD_SIZE: usize = 4 << 20;

const COMMON_MAGIC: [u64; 2] = limine::COMMON_MAGIC;
const BASE_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

extern "C" {
    static __req_start: u8;
    static __req_end: u8;
}

fn serial(msg: &str) {
    for b in msg.bytes() {
        unsafe { asm!("out dx, al", in("dx") 0x3f8u16, in("al") b, options(nomem, nostack)) };
    }
}

fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    serial("kstub: panic\n");
    halt()
}

fn fail(msg: &str) -> ! {
    serial("kstub: ");
    serial(msg);
    serial("\n");
    halt()
}

/// Copies the answers Limine wrote next to each of the stub's requests into the same request in the
/// unpacked kernel (found by its id), so the kernel sees them as if Limine had loaded it.
unsafe fn forward_responses(image: &mut [u8]) {
    let words = image.len() / 8;
    let img = image.as_mut_ptr() as *mut u64;
    let rd = |p: *const u64, i: usize| core::ptr::read_volatile(p.add(i));
    let start = &__req_start as *const u8 as *const u64;
    let n = (&__req_end as *const u8 as usize - start as usize) / 8;
    let mut i = 0;
    while i < n {
        let (key, len, copy_from, copy_len) = if rd(start, i) == COMMON_MAGIC[0] && i + 6 <= n && rd(start, i + 1) == COMMON_MAGIC[1] {
            (4, 6, 5, 1) // id[4], revision, response: forward the response pointer
        } else if rd(start, i) == BASE_MAGIC[0] && i + 3 <= n {
            (2, 3, 1, 2) // magic[2], revision: Limine rewrites both
        } else {
            i += 1;
            continue;
        };
        // Limine may rewrite the base revision's second word, so match the kernel's copy by the constants.
        let wanted = |k: usize| if key == 2 { BASE_MAGIC[k] } else { rd(start, i + k) };
        if let Some(j) = (0..words.saturating_sub(len)).find(|&j| (0..key.min(4)).all(|k| *img.add(j + k) == wanted(k))) {
            for k in 0..copy_len {
                *img.add(j + copy_from + k) = rd(start, i + copy_from + k);
            }
        }
        i += len;
    }
}

#[no_mangle]
extern "C" fn _start() -> ! {
    if !BASE_REVISION.is_supported() {
        fail("Limine base revision not supported");
    }
    let blob = MODULES
        .response()
        .and_then(|r| r.modules().iter().find(|m| m.path().ends_with("kernel.z")))
        .map(|m| m.data())
        .unwrap_or_else(|| fail("kernel.z module missing"));
    if blob.len() < 16 {
        fail("kernel.z too short");
    }
    let entry = u64::from_le_bytes(blob[0..8].try_into().unwrap());
    let len = u64::from_le_bytes(blob[8..16].try_into().unwrap()) as usize;
    if len > PAYLOAD_SIZE {
        fail("kernel does not fit");
    }
    let image = unsafe { core::slice::from_raw_parts_mut(PAYLOAD_BASE as *mut u8, len) };
    let mut d = DecompressorOxide::new();
    let (status, _, out) = decompress(&mut d, &blob[16..], image, 0, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
    if status != TINFLStatus::Done || out != len {
        fail("kernel.z is corrupt");
    }
    unsafe {
        forward_responses(image);
        // Enter like a called function: 16-byte aligned stack, then a return address.
        asm!("and rsp, -16", "push 0", "jmp {}", in(reg) entry, options(noreturn));
    }
}
