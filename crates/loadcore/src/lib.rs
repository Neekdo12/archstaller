//! The firmware-independent end of the boot loader. The firmware front ends (`boot/uefi`,
//! `boot/bios`) hand over a physical memory map, a framebuffer, the payload bundle and an arena of
//! memory they allocated for the loader. This crate unpacks the kernel and its modules into the
//! arena, builds the page tables (direct map, kernel, one identity-mapped trampoline page), fills
//! in a [`bootinfo::BootInfo`] and jumps to the kernel.
//!
//! Runs in 64-bit mode with every address it touches identity mapped.
#![no_std]

mod map;
mod paging;

pub use map::Map;

use bootinfo::*;
use core::arch::{asm, global_asm};
use miniz_oxide::inflate::core::{decompress, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

const PAGE: u64 = 4096;
const HUGE: u64 = 2 << 20;

/// Writes to COM1 (no setup: QEMU and most firmware leave it usable; a machine without it ignores the writes).
pub fn log(msg: &str) {
    for b in msg.bytes() {
        unsafe { asm!("out dx, al", in("dx") 0x3f8u16, in("al") b, options(nomem, nostack)) };
    }
}

pub fn log_hex(mut v: u64) {
    let mut buf = [0u8; 16];
    for i in (0..16).rev() {
        buf[i] = b"0123456789abcdef"[(v & 15) as usize];
        v >>= 4;
    }
    log("0x");
    log(core::str::from_utf8(&buf).unwrap());
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn up(v: u64, to: u64) -> u64 {
    (v + to - 1) & !(to - 1)
}

/// What the payload asks for: the kernel, then modules.
struct Payload<'a> {
    bytes: &'a [u8],
    count: usize,
    kernel_entry: u64,
    kernel_mem: u64,
}

impl<'a> Payload<'a> {
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < PAYLOAD_HEADER || le32(bytes, 0) != PAYLOAD_MAGIC {
            return None;
        }
        let count = le32(bytes, 4) as usize;
        let kernel_entry = le64(bytes, 8);
        let kernel_mem = le32(bytes, 16) as u64;
        (bytes.len() >= PAYLOAD_HEADER + 16 + count * PAYLOAD_ENTRY).then_some(Payload { bytes, count, kernel_entry, kernel_mem })
    }
    /// (name, stored data, raw length, deflated)
    fn entry(&self, i: usize) -> (&'a [u8], &'a [u8], usize, bool) {
        let at = PAYLOAD_HEADER + 16 + i * PAYLOAD_ENTRY;
        let name = &self.bytes[at..at + 16];
        let (off, stored, raw, flags) = (le32(self.bytes, at + 16) as usize, le32(self.bytes, at + 20) as usize, le32(self.bytes, at + 24) as usize, le32(self.bytes, at + 28));
        (name, &self.bytes[off..off + stored], raw, flags & FLAG_DEFLATE != 0)
    }
}

/// The number of bytes of arena `boot` needs for this payload and a machine whose memory ends at `max_phys`.
pub fn arena_size(payload: &[u8], max_phys: u64) -> Option<u64> {
    let p = Payload::parse(payload)?;
    let mut n = up(p.kernel_mem, HUGE) + HUGE; // the kernel is 2 MiB aligned inside the arena
    for i in 0..p.count {
        n += up(p.entry(i).2 as u64, PAGE) + PAGE;
    }
    n += STACK_SIZE + 64 * PAGE; // stack, boot info, memory map, trampoline
    let gib = up(max_phys.max(4 << 30), 1 << 30) >> 30;
    n += (gib + 16) * PAGE; // direct map: one page directory per GiB, plus the fixed tables
    Some(up(n, PAGE))
}

struct Arena {
    next: u64,
    end: u64,
}

impl Arena {
    fn take(&mut self, len: u64, align: u64) -> u64 {
        let at = up(self.next, align);
        if at + len > self.end {
            panic!("loader arena exhausted");
        }
        self.next = at + len;
        unsafe { core::ptr::write_bytes(at as *mut u8, 0, len as usize) };
        at
    }
}

pub struct Boot<'a> {
    pub firmware: u32,
    pub fb: Framebuffer,
    pub payload: &'a [u8],
    /// Physical range, 4 KiB aligned, at least `arena_size` bytes, already allocated from the firmware.
    pub arena: (u64, u64),
}

fn unpack(stored: &[u8], raw: usize, deflated: bool, dest: u64) {
    let out = unsafe { core::slice::from_raw_parts_mut(dest as *mut u8, raw) };
    if !deflated {
        out.copy_from_slice(&stored[..raw]);
        return;
    }
    let mut d = DecompressorOxide::new();
    let (status, _, n) = decompress(&mut d, stored, out, 0, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
    if status != TINFLStatus::Done || n != raw {
        panic!("payload entry is corrupt");
    }
}

global_asm!(
    r#"
    .text
    .balign 16
    .global loadcore_tramp_start
    .global loadcore_tramp_end
    .global loadcore_tramp_gdtr
loadcore_tramp_start:
    cli
    lgdt [rip + loadcore_tramp_gdtr]
    mov cr3, rax
    mov rsp, rcx
    lea r9, [rip + 2f]
    push 0x08
    push r9
    retfq
2:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov rdi, rdx
    xor ebp, ebp
    push 0
    jmp r8
    .balign 16
loadcore_tramp_gdtr:
    .quad 0
    .quad 0
loadcore_tramp_end:
    "#
);

extern "C" {
    static loadcore_tramp_start: u8;
    static loadcore_tramp_end: u8;
    static loadcore_tramp_gdtr: u8;
}

/// Unpacks the payload and enters the kernel. `map` is the final memory map (the firmware must not
/// allocate or free after it was taken).
///
/// # Safety
/// The caller is the boot loader at its last step: everything in `map` and the arena is identity mapped and unused.
pub unsafe fn boot(map: &mut Map, b: Boot) -> ! {
    let payload = Payload::parse(b.payload).expect("payload bundle is missing or damaged");
    if payload.kernel_mem == 0 || payload.kernel_mem > KERNEL_MAX {
        panic!("kernel image size is out of range");
    }
    map.carve(b.arena.0, b.arena.1, MEM_LOADER);
    let mut arena = Arena { next: b.arena.0, end: b.arena.0 + b.arena.1 };

    // The kernel.
    let kernel_phys = arena.take(up(payload.kernel_mem, HUGE), HUGE);
    let mut modules = [Module { name: [0; 16], addr: 0, len: 0 }; 16];
    let mut nmod = 0;
    let mut have_kernel = false;
    for i in 0..payload.count {
        let (name, stored, raw, deflated) = payload.entry(i);
        let n = name.iter().position(|&c| c == 0).unwrap_or(16);
        if &name[..n] == b"kernel" {
            if raw as u64 > payload.kernel_mem {
                panic!("kernel image is larger than its memory size");
            }
            unpack(stored, raw, deflated, kernel_phys);
            have_kernel = true;
        } else {
            if nmod == modules.len() {
                panic!("too many payload modules");
            }
            let dest = arena.take(raw as u64, PAGE);
            unpack(stored, raw, deflated, dest);
            modules[nmod] = Module { name: name.try_into().unwrap(), addr: HHDM_BASE + dest, len: raw as u64 };
            nmod += 1;
        }
    }
    if !have_kernel {
        panic!("payload has no kernel");
    }
    log("loader: payload unpacked\n");

    let stack = arena.take(STACK_SIZE, PAGE);
    let tramp = arena.take(PAGE, PAGE);

    // Boot info: the structure, the modules, the memory map.
    let info_len = up(core::mem::size_of::<BootInfo>() as u64 + (nmod * core::mem::size_of::<Module>()) as u64 + (map.len() * core::mem::size_of::<MemEntry>()) as u64 + 64, PAGE);
    let info_phys = arena.take(info_len, PAGE);

    let cr3 = paging::build(&mut arena, map, kernel_phys, payload.kernel_mem, tramp, &b.fb);

    // Modules and memory map, after the page tables took their pages out of the map's accounting.
    let mods_phys = info_phys + core::mem::size_of::<BootInfo>() as u64;
    let mem_phys = mods_phys + (nmod * core::mem::size_of::<Module>()) as u64;
    core::ptr::copy_nonoverlapping(modules.as_ptr(), mods_phys as *mut Module, nmod);
    core::ptr::copy_nonoverlapping(map.entries().as_ptr(), mem_phys as *mut MemEntry, map.len());
    let info = info_phys as *mut BootInfo;
    info.write(BootInfo {
        magic: MAGIC,
        hhdm: HHDM_BASE,
        firmware: b.firmware,
        _pad: 0,
        fb: b.fb,
        mem: (HHDM_BASE + mem_phys) as *const MemEntry,
        mem_len: map.len() as u64,
        modules: (HHDM_BASE + mods_phys) as *const Module,
        modules_len: nmod as u64,
    });

    // Trampoline page: code, then a GDT (null, 64-bit code, data) the code loads.
    let start = &loadcore_tramp_start as *const u8;
    let code_len = &loadcore_tramp_end as *const u8 as usize - start as usize;
    core::ptr::copy_nonoverlapping(start, tramp as *mut u8, code_len);
    let gdtr_at = tramp + (&loadcore_tramp_gdtr as *const u8 as usize - start as usize) as u64;
    let gdt_at = tramp + PAGE - 64;
    let gdt = gdt_at as *mut u64;
    gdt.write(0);
    gdt.add(1).write(0x00af_9a00_0000_ffff);
    gdt.add(2).write(0x00cf_9200_0000_ffff);
    (gdtr_at as *mut u16).write(3 * 8 - 1);
    ((gdtr_at + 2) as *mut u64).write(gdt_at);

    log("loader: entering kernel\n");
    asm!(
        "jmp {t}",
        t = in(reg) tramp,
        in("rax") cr3,
        in("rdx") HHDM_BASE + info_phys,
        in("rcx") HHDM_BASE + stack + STACK_SIZE,
        in("r8") payload.kernel_entry,
        options(noreturn)
    )
}
