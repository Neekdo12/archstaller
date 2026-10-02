//! Boots a Linux bzImage with the 64-bit boot protocol (`Documentation/arch/x86/boot.rst`): the
//! protected-mode kernel goes to an aligned address, `boot_params` (the "zero page") describes the
//! command line, initramfs, memory map and framebuffer, and the CPU enters the kernel's 64-bit
//! entry point with `rsi` = the zero page and the first 4 GiB identity mapped.
use bootinfo::Framebuffer;
use core::arch::asm;
use loadcore::Map;

/// Zero page and command line live in the low memory real mode used: free by now, identity mapped.
const ZERO_PAGE: usize = 0x8_0000;
const CMDLINE: usize = 0x8_1000;

fn put<T: Copy>(base: usize, off: usize, v: T) {
    unsafe { core::ptr::write_unaligned((base + off) as *mut T, v) };
}

fn get<T: Copy>(k: &[u8], off: usize) -> T {
    assert!(off + core::mem::size_of::<T>() <= k.len(), "the kernel image is too small");
    unsafe { core::ptr::read_unaligned(k.as_ptr().add(off) as *const T) }
}

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

static GDT: [u64; 4] = [0, 0, 0x00af_9a00_0000_ffff, 0x00cf_9200_0000_ffff]; // 0x10 code64, 0x18 data
static mut GDTR: Gdtr = Gdtr { limit: 4 * 8 - 1, base: 0 };

/// `bz`: the bzImage, `initrd`: where the initramfs lies, `e820`: the raw BIOS memory map
/// (count at 0x500, entries at 0x600).
///
/// # Safety
/// Last step of the loader: it never returns.
pub unsafe fn boot(map: &Map, bz: &[u8], initrd: (u64, u64), cmdline: &str, fb: &Framebuffer, staged_end: u64) -> ! {
    if get::<u32>(bz, 0x202) != 0x5372_6448 {
        panic!("the kernel is not a Linux bzImage");
    }
    let version: u16 = get(bz, 0x206);
    let xloadflags: u16 = get(bz, 0x236);
    if version < 0x020c || xloadflags & 1 == 0 {
        panic!("the kernel does not have a 64-bit entry point");
    }
    let setup_sects = match bz[0x1f1] {
        0 => 4,
        n => n as usize,
    };
    let pm = &bz[(setup_sects + 1) * 512..];
    let align = (get::<u32>(bz, 0x230) as u64).max(0x20_0000);
    let init_size = get::<u32>(bz, 0x260) as u64;
    let dest = (staged_end + align - 1) & !(align - 1);
    // The decompressor works in [dest, dest + init_size): it has to be ordinary memory below 4 GiB.
    let ok = map.entries().iter().any(|e| e.kind == bootinfo::MEM_USABLE && e.base <= dest && dest + init_size.max(pm.len() as u64) <= e.base + e.len);
    if !ok || dest + init_size > 4 << 30 {
        panic!("no room for the kernel");
    }
    core::ptr::copy_nonoverlapping(pm.as_ptr(), dest as *mut u8, pm.len());

    // Zero page: the setup header from the image, then what the loader knows.
    core::ptr::write_bytes(ZERO_PAGE as *mut u8, 0, 4096);
    core::ptr::copy_nonoverlapping(bz.as_ptr().add(0x1f1), (ZERO_PAGE + 0x1f1) as *mut u8, 0x268 - 0x1f1);
    put(ZERO_PAGE, 0x210, 0xffu8); // type_of_loader: unregistered
    let loadflags: u8 = get(bz, 0x211);
    put(ZERO_PAGE, 0x211, loadflags | 1); // loaded high
    let max = (get::<u32>(bz, 0x238) as usize).clamp(256, 2048) - 1;
    let n = cmdline.len().min(max);
    core::ptr::copy_nonoverlapping(cmdline.as_ptr(), CMDLINE as *mut u8, n);
    put(CMDLINE, n, 0u8);
    put(ZERO_PAGE, 0x228, CMDLINE as u32);
    put(ZERO_PAGE, 0x218, initrd.0 as u32);
    put(ZERO_PAGE, 0x21c, initrd.1 as u32);

    // Memory map: the BIOS's own, entry by entry.
    let mut n = 0;
    for i in 0..core::ptr::read_volatile(0x500 as *const u32) as usize {
        let e = 0x600 + i * 24;
        if n == 128 || core::ptr::read_volatile((e + 20) as *const u32) & 1 == 0 {
            continue;
        }
        let at = 0x2d0 + n * 20;
        core::ptr::copy_nonoverlapping(e as *const u8, (ZERO_PAGE + at) as *mut u8, 20);
        n += 1;
    }
    put(ZERO_PAGE, 0x1e8, n as u8);

    // Framebuffer (screen_info at the start of the zero page): a linear VESA framebuffer.
    if fb.addr != 0 {
        put(ZERO_PAGE, 0x0f, 0x23u8); // VIDEO_TYPE_VLFB
        put(ZERO_PAGE, 0x12, fb.width as u16);
        put(ZERO_PAGE, 0x14, fb.height as u16);
        put(ZERO_PAGE, 0x16, 32u16);
        put(ZERO_PAGE, 0x18, fb.addr as u32);
        put(ZERO_PAGE, 0x1c, (fb.pitch as u64 * fb.height as u64).div_ceil(65536) as u32);
        put(ZERO_PAGE, 0x24, fb.pitch as u16);
        put(ZERO_PAGE, 0x26, 8u8);
        put(ZERO_PAGE, 0x27, fb.red_shift);
        put(ZERO_PAGE, 0x28, 8u8);
        put(ZERO_PAGE, 0x29, fb.green_shift);
        put(ZERO_PAGE, 0x2a, 8u8);
        put(ZERO_PAGE, 0x2b, fb.blue_shift);
    }

    loadcore::log("boot: starting Linux\n");
    (*(&raw mut GDTR)).base = GDT.as_ptr() as u64;
    asm!(
        "cli",
        "lgdt [rcx]",
        "mov ax, 0x18",
        "mov ds, ax",
        "mov es, ax",
        "mov ss, ax",
        "mov fs, ax",
        "mov gs, ax",
        "push 0x10",
        "push rdx",
        "retfq",
        in("rcx") &raw const GDTR,
        in("rdx") dest + 0x200,
        in("rsi") ZERO_PAGE,
        options(noreturn)
    )
}
