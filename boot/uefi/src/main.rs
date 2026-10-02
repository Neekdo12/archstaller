//! UEFI front end of the boot loader (`BOOTX64.EFI`). It has two jobs, chosen by what is on the
//! volume it was started from:
//!
//! * `\payload.bin` (the installer ISO's boot image): unpack the installer kernel and enter it.
//! * `\archstaler.cfg` (the target disk's ESP, first boot): start the Arch Linux kernel, which is a
//!   UEFI application itself, with a command line and an initramfs read from the ESP. The config
//!   is `kernel=<path>`, `initrd=<path>`, `cmdline=<rest of line>`, one per line.
#![no_std]
#![no_main]

use bootinfo::*;
use core::ffi::c_void;
use core::ptr::{null, null_mut};
use loadcore::{log, log_hex, Boot, Map};

type Handle = *mut c_void;
type Status = usize;

const ERR: usize = 1 << 63;
const BUFFER_TOO_SMALL: Status = ERR | 5;
const INVALID_PARAMETER: Status = ERR | 2;
const UNSUPPORTED: Status = ERR | 3;

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

const LOADED_IMAGE: Guid = Guid(0x5b1b31a1, 0x9562, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const SIMPLE_FS: Guid = Guid(0x964e5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const FILE_INFO: Guid = Guid(0x09576e92, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const GOP: Guid = Guid(0x9042a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);
const DEVICE_PATH: Guid = Guid(0x09576e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const LOAD_FILE2: Guid = Guid(0x4006c0c1, 0xfcb3, 0x403e, [0x99, 0x6d, 0x4a, 0x6c, 0x87, 0x24, 0xe0, 0x6e]);
/// The vendor-media device path the Linux EFI stub asks for to find its initramfs.
const LINUX_INITRD_MEDIA: Guid = Guid(0x5568e427, 0x68fc, 0x4f3d, [0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68]);

#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    size: u32,
    crc: u32,
    reserved: u32,
}

#[repr(C)]
struct SimpleTextOutput {
    reset: usize,
    output_string: extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
}

#[repr(C)]
struct SystemTable {
    hdr: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    con_in: *mut c_void,
    console_out_handle: Handle,
    con_out: *mut SimpleTextOutput,
    standard_error_handle: Handle,
    std_err: *mut c_void,
    runtime_services: *mut c_void,
    boot_services: *mut BootServices,
}

#[repr(C)]
struct BootServices {
    hdr: TableHeader,
    raise_tpl: usize,
    restore_tpl: usize,
    allocate_pages: extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    free_pages: usize,
    get_memory_map: extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    allocate_pool: usize,
    free_pool: usize,
    create_event: usize,
    set_timer: usize,
    wait_for_event: usize,
    signal_event: usize,
    close_event: usize,
    check_event: usize,
    install_protocol_interface: extern "efiapi" fn(*mut Handle, *const Guid, u32, *mut c_void) -> Status,
    reinstall_protocol_interface: usize,
    uninstall_protocol_interface: usize,
    handle_protocol: extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    reserved: usize,
    register_protocol_notify: usize,
    locate_handle: usize,
    locate_device_path: usize,
    install_configuration_table: usize,
    load_image: extern "efiapi" fn(u8, Handle, *mut c_void, *const u8, usize, *mut Handle) -> Status,
    start_image: extern "efiapi" fn(Handle, *mut usize, *mut *mut u16) -> Status,
    exit: usize,
    unload_image: usize,
    exit_boot_services: extern "efiapi" fn(Handle, usize) -> Status,
    get_next_monotonic_count: usize,
    stall: extern "efiapi" fn(usize) -> Status,
    set_watchdog_timer: extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    connect_controller: usize,
    disconnect_controller: usize,
    open_protocol: usize,
    close_protocol: usize,
    open_protocol_information: usize,
    protocols_per_handle: usize,
    locate_handle_buffer: extern "efiapi" fn(u32, *const Guid, *mut c_void, *mut usize, *mut *mut Handle) -> Status,
    locate_protocol: extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

#[repr(C)]
struct LoadedImage {
    revision: u32,
    parent_handle: Handle,
    system_table: *mut SystemTable,
    device_handle: Handle,
    file_path: *mut c_void,
    reserved: *mut c_void,
    load_options_size: u32,
    load_options: *mut c_void,
    image_base: *mut c_void,
    image_size: u64,
    image_code_type: u32,
    image_data_type: u32,
    unload: usize,
}

#[repr(C)]
struct SimpleFs {
    revision: u64,
    open_volume: extern "efiapi" fn(*mut SimpleFs, *mut *mut File) -> Status,
}

#[repr(C)]
struct File {
    revision: u64,
    open: extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    close: extern "efiapi" fn(*mut File) -> Status,
    delete: usize,
    read: extern "efiapi" fn(*mut File, *mut usize, *mut u8) -> Status,
    write: usize,
    get_position: usize,
    set_position: usize,
    get_info: extern "efiapi" fn(*mut File, *const Guid, *mut usize, *mut u8) -> Status,
}

#[repr(C)]
struct GopModeInfo {
    version: u32,
    horizontal: u32,
    vertical: u32,
    pixel_format: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    reserved_mask: u32,
    pixels_per_scan_line: u32,
}

#[repr(C)]
struct GopMode {
    max_mode: u32,
    mode: u32,
    info: *const GopModeInfo,
    size_of_info: usize,
    frame_buffer_base: u64,
    frame_buffer_size: usize,
}

#[repr(C)]
struct Gop {
    query_mode: usize,
    set_mode: usize,
    blt: usize,
    mode: *const GopMode,
}

#[repr(C)]
struct LoadFile2 {
    load_file: extern "efiapi" fn(*mut LoadFile2, *const c_void, u8, *mut usize, *mut u8) -> Status,
}

static mut ST: *mut SystemTable = null_mut();
static mut MAP: Map = Map::new();

fn bs() -> &'static BootServices {
    unsafe { &*(*ST).boot_services }
}

/// Prints on the serial port and on the firmware console.
fn say(msg: &str) {
    log(msg);
    unsafe {
        let out = (*ST).con_out;
        if out.is_null() {
            return;
        }
        let mut buf = [0u16; 64];
        let mut n = 0;
        for c in msg.chars() {
            if c == '\n' {
                buf[n] = '\r' as u16;
                n += 1;
            }
            buf[n] = if c.is_ascii() { c as u16 } else { '?' as u16 };
            n += 1;
            if n >= 60 {
                buf[n] = 0;
                ((*out).output_string)(out, buf.as_ptr());
                n = 0;
            }
        }
        buf[n] = 0;
        ((*out).output_string)(out, buf.as_ptr());
    }
}

fn say_status(what: &str, s: Status) {
    say(what);
    say(": status ");
    log_hex(s as u64);
    say("\n");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(m) = info.message().as_str() {
        say("boot: ");
        say(m);
        say("\n");
    } else {
        say("boot: panic\n");
    }
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}

fn utf16<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (i, c) in s.bytes().enumerate().take(N - 1) {
        out[i] = c as u16;
    }
    out
}

fn alloc_pages(bytes: usize) -> Result<u64, Status> {
    let mut addr = 0u64;
    let s = (bs().allocate_pages)(0, 2, bytes.div_ceil(4096), &mut addr); // any pages, EfiLoaderData
    if s == 0 {
        Ok(addr)
    } else {
        Err(s)
    }
}

fn open_root(device: Handle) -> Option<*mut File> {
    let mut fs: *mut c_void = null_mut();
    if (bs().handle_protocol)(device, &SIMPLE_FS, &mut fs) != 0 || fs.is_null() {
        return None;
    }
    let fs = fs as *mut SimpleFs;
    let mut root: *mut File = null_mut();
    unsafe { ((*fs).open_volume)(fs, &mut root) };
    (!root.is_null()).then_some(root)
}

/// Reads a whole file into freshly allocated loader memory.
fn read_file(root: *mut File, path: &str) -> Option<(*mut u8, usize)> {
    unsafe {
        let name: [u16; 128] = utf16(path);
        let mut f: *mut File = null_mut();
        if ((*root).open)(root, &mut f, name.as_ptr(), 1, 0) != 0 || f.is_null() {
            return None;
        }
        let mut info = [0u64; 64];
        let mut size = core::mem::size_of_val(&info);
        if ((*f).get_info)(f, &FILE_INFO, &mut size, info.as_mut_ptr() as *mut u8) != 0 {
            ((*f).close)(f);
            return None;
        }
        let len = info[1] as usize; // FileSize follows the Size field
        let buf = alloc_pages(len.max(1)).ok()? as *mut u8;
        let mut done = 0;
        while done < len {
            let mut n = len - done;
            if ((*f).read)(f, &mut n, buf.add(done)) != 0 || n == 0 {
                ((*f).close)(f);
                return None;
            }
            done += n;
        }
        ((*f).close)(f);
        Some((buf, len))
    }
}

/// Where to look for files: the volume this image came from first, then every other volume.
fn volumes(image: Handle, f: &mut dyn FnMut(*mut File) -> bool) {
    unsafe {
        let mut li: *mut c_void = null_mut();
        if (bs().handle_protocol)(image, &LOADED_IMAGE, &mut li) == 0 {
            if let Some(root) = open_root((*(li as *mut LoadedImage)).device_handle) {
                if f(root) {
                    return;
                }
            }
        }
        let mut count = 0usize;
        let mut handles: *mut Handle = null_mut();
        if (bs().locate_handle_buffer)(2, &SIMPLE_FS, null_mut(), &mut count, &mut handles) == 0 {
            for i in 0..count {
                if let Some(root) = open_root(*handles.add(i)) {
                    if f(root) {
                        return;
                    }
                }
            }
        }
    }
}

fn find_file(image: Handle, path: &str) -> Option<(*mut File, *mut u8, usize)> {
    let mut found = None;
    volumes(image, &mut |root| match read_file(root, path) {
        Some((p, n)) => {
            found = Some((root, p, n));
            true
        }
        None => false,
    });
    found
}

fn gop_framebuffer() -> Framebuffer {
    let mut fb = Framebuffer::default();
    let mut p: *mut c_void = null_mut();
    if (bs().locate_protocol)(&GOP, null_mut(), &mut p) != 0 || p.is_null() {
        say("boot: no graphics output, serial console only\n");
        return fb;
    }
    unsafe {
        let mode = &*(*(p as *mut Gop)).mode;
        if mode.info.is_null() || mode.frame_buffer_base == 0 {
            return fb;
        }
        let i = &*mode.info;
        let eight = |m: u32| (m.count_ones() == 8 && m.trailing_zeros() % 8 == 0).then(|| m.trailing_zeros() as u8);
        let shifts = match i.pixel_format {
            0 => Some((0, 8, 16)),
            1 => Some((16, 8, 0)),
            2 => eight(i.red_mask).zip(eight(i.green_mask)).zip(eight(i.blue_mask)).map(|((r, g), b)| (r, g, b)),
            _ => None,
        };
        if let Some((r, g, b)) = shifts {
            fb = Framebuffer {
                addr: mode.frame_buffer_base,
                width: i.horizontal,
                height: i.vertical,
                pitch: i.pixels_per_scan_line * 4,
                bpp: 32,
                red_shift: r,
                green_shift: g,
                blue_shift: b,
                _pad: 0,
            };
        }
    }
    fb
}

/// A snapshot of the firmware memory map, in `buf`.
struct Descriptors {
    buf: *mut u8,
    cap: usize,
    size: usize,
    desc_size: usize,
    key: usize,
}

impl Descriptors {
    fn new() -> Self {
        // The map changes between calls (our own allocations add entries), so leave generous room.
        let cap = 64 * 1024;
        let buf = alloc_pages(cap).unwrap_or_else(|_| panic!("cannot allocate the memory map buffer")) as *mut u8;
        Descriptors { buf, cap, size: 0, desc_size: 0, key: 0 }
    }
    fn refresh(&mut self) {
        let mut ver = 0u32;
        self.size = self.cap;
        let s = (bs().get_memory_map)(&mut self.size, self.buf, &mut self.key, &mut self.desc_size, &mut ver);
        if s != 0 {
            say_status("boot: GetMemoryMap", s);
            panic!("cannot read the memory map");
        }
    }
    /// (kind, physical start, pages)
    fn each(&self, mut f: impl FnMut(u32, u64, u64)) {
        let mut off = 0;
        while off + 32 <= self.size {
            unsafe {
                let d = self.buf.add(off);
                f(*(d as *const u32), *(d.add(8) as *const u64), *(d.add(24) as *const u64));
            }
            off += self.desc_size;
        }
    }
}

fn translate(kind: u32) -> u32 {
    match kind {
        1 | 2 => MEM_LOADER,         // loader code, data: us, and what the arena was allocated as
        3 | 4 | 7 => MEM_USABLE,     // boot services code, data, conventional (free once services are gone)
        9 => MEM_ACPI_RECLAIMABLE,
        10 => MEM_ACPI_NVS,
        8 => MEM_BAD,
        _ => MEM_RESERVED,           // runtime services, MMIO, persistent, unaccepted, ...
    }
}

fn boot_installer(image: Handle, payload: *mut u8, payload_len: usize) {
    let payload = unsafe { core::slice::from_raw_parts(payload, payload_len) };
    let fb = gop_framebuffer();
    let mut d = Descriptors::new();
    d.refresh();
    let mut top = 0;
    d.each(|k, start, pages| {
        if matches!(k, 1..=7 | 9 | 10) {
            top = top.max(start + pages * 4096);
        }
    });
    let size = loadcore::arena_size(payload, top).unwrap_or_else(|| panic!("payload.bin is damaged"));
    let arena = alloc_pages(size as usize).unwrap_or_else(|_| panic!("not enough memory for the loader"));

    // Nothing may allocate between the last GetMemoryMap and ExitBootServices.
    let mut tries = 0;
    loop {
        d.refresh();
        let s = (bs().exit_boot_services)(image, d.key);
        if s == 0 {
            break;
        }
        tries += 1;
        if tries > 8 {
            say_status("boot: ExitBootServices", s);
            panic!("cannot exit boot services");
        }
    }
    unsafe {
        let map = &mut *(&raw mut MAP);
        d.each(|k, start, pages| map.push(start, pages * 4096, translate(k)));
        map.normalize();
        loadcore::boot(map, Boot { firmware: FIRMWARE_UEFI, fb, payload, arena: (arena, (size + 4095) & !4095) });
    }
}

static mut INITRD: (*const u8, usize) = (null(), 0);

extern "efiapi" fn initrd_load(_: *mut LoadFile2, _: *const c_void, boot_policy: u8, size: *mut usize, buf: *mut u8) -> Status {
    unsafe {
        if boot_policy != 0 {
            return UNSUPPORTED;
        }
        if size.is_null() {
            return INVALID_PARAMETER;
        }
        let (src, len) = INITRD;
        if buf.is_null() || *size < len {
            *size = len;
            return BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(src, buf, len);
        *size = len;
        0
    }
}

static mut INITRD_PROTO: LoadFile2 = LoadFile2 { load_file: initrd_load };
/// Vendor media node (type 4, subtype 3, 20 bytes) followed by the end node.
static mut INITRD_PATH: [u8; 24] = [4, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x7f, 0xff, 4, 0];

fn boot_linux(image: Handle, cfg: &[u8]) {
    let text = core::str::from_utf8(cfg).unwrap_or("");
    let (mut kernel, mut initrd, mut cmdline) = ("", "", "");
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("kernel=") {
            kernel = v;
        } else if let Some(v) = line.strip_prefix("initrd=") {
            initrd = v;
        } else if let Some(v) = line.strip_prefix("cmdline=") {
            cmdline = v;
        }
    }
    let Some((root, kbuf, klen)) = find_file(image, kernel).map(|(r, p, n)| (r, p, n)) else {
        say("boot: cannot read the kernel\n");
        return;
    };
    let Some((ibuf, ilen)) = read_file(root, initrd) else {
        say("boot: cannot read the initramfs\n");
        return;
    };
    unsafe {
        INITRD = (ibuf, ilen);
        let key = LINUX_INITRD_MEDIA;
        (&raw mut INITRD_PATH as *mut u8).add(4).copy_from_nonoverlapping(&key as *const Guid as *const u8, 16);
        let mut h: Handle = null_mut();
        let s = (bs().install_protocol_interface)(&mut h, &DEVICE_PATH, 0, &raw mut INITRD_PATH as *mut c_void);
        let s2 = if s == 0 { (bs().install_protocol_interface)(&mut h, &LOAD_FILE2, 0, &raw mut INITRD_PROTO as *mut c_void) } else { s };
        if s2 != 0 {
            say_status("boot: cannot offer the initramfs", s2);
            return;
        }

        let mut li: *mut c_void = null_mut();
        let own = if (bs().handle_protocol)(image, &LOADED_IMAGE, &mut li) == 0 { (*(li as *mut LoadedImage)).file_path } else { null_mut() };
        let mut child: Handle = null_mut();
        let mut s = (bs().load_image)(0, image, own, kbuf, klen, &mut child);
        if s != 0 {
            s = (bs().load_image)(0, image, null_mut(), kbuf, klen, &mut child);
        }
        if s != 0 {
            say_status("boot: LoadImage (the kernel is not an EFI application?)", s);
            return;
        }
        // The command line, as the UTF-16 string the Linux EFI stub reads from the load options.
        static mut OPTS: [u16; 2048] = [0; 2048];
        let opts = &mut *(&raw mut OPTS);
        let mut n = 0;
        for b in cmdline.bytes().take(opts.len() - 1) {
            opts[n] = b as u16;
            n += 1;
        }
        let mut kli: *mut c_void = null_mut();
        if (bs().handle_protocol)(child, &LOADED_IMAGE, &mut kli) == 0 {
            let k = kli as *mut LoadedImage;
            (*k).load_options = opts.as_mut_ptr() as *mut c_void;
            (*k).load_options_size = ((n + 1) * 2) as u32;
        }
        let s = (bs().start_image)(child, null_mut(), null_mut());
        say_status("boot: the kernel returned", s);
    }
}

#[no_mangle]
extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    unsafe { ST = st };
    // A firmware watchdog resets the machine after five minutes by default; the installer runs longer.
    (bs().set_watchdog_timer)(0, 0, 0, null());
    log("boot: UEFI loader\n");
    if let Some((_, cfg, n)) = find_file(image, "\\archstaler.cfg") {
        boot_linux(image, unsafe { core::slice::from_raw_parts(cfg, n) });
    } else if let Some((_, p, n)) = find_file(image, "\\payload.bin") {
        boot_installer(image, p, n);
    } else {
        say("boot: neither archstaler.cfg nor payload.bin found\n");
    }
    (bs().stall)(10_000_000);
    ERR | 1
}
