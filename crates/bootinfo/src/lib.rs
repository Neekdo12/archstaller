//! The contract between the boot loader (`boot/uefi`, `boot/bios`, both finishing in `loadcore`)
//! and the installer kernel, plus the layout of the `payload` bundle the loader unpacks.
//!
//! The kernel is entered in 64-bit mode with paging on, interrupts off, `rdi` = pointer to a
//! [`BootInfo`] (a higher-half-direct-map address) and a 256 KiB stack mapped in the direct map.
#![no_std]

pub const MAGIC: u64 = 0x4152_4348_424f_4f54; // "ARCHBOOT"

/// Virtual address of physical address 0 (every usable, ACPI and loader range is mapped, plus at least the first 4 GiB).
pub const HHDM_BASE: u64 = 0xffff_8000_0000_0000;
/// Where the kernel image is linked and mapped.
pub const KERNEL_BASE: u64 = 0xffff_ffff_8000_0000;
/// The most memory the kernel image (including .bss) may need; `xtask` and `loadcore` both check it.
pub const KERNEL_MAX: u64 = 4 << 20;
pub const STACK_SIZE: u64 = 256 << 10;

pub const FIRMWARE_BIOS: u32 = 0;
pub const FIRMWARE_UEFI: u32 = 1;

pub const MEM_USABLE: u32 = 0;
pub const MEM_RESERVED: u32 = 1;
pub const MEM_ACPI_RECLAIMABLE: u32 = 2;
pub const MEM_ACPI_NVS: u32 = 3;
pub const MEM_BAD: u32 = 4;
/// Kernel image, modules, page tables, boot info, stack, and the loader's own memory. Never reused.
pub const MEM_LOADER: u32 = 5;

/// Sorted by `base`, non-overlapping. `Usable` ranges are 4 KiB aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemEntry {
    pub base: u64,
    pub len: u64,
    pub kind: u32,
    pub _pad: u32,
}

/// `addr == 0`: no framebuffer. Only 32 bpp is handed over.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Framebuffer {
    /// Physical address.
    pub addr: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub pitch: u32,
    pub bpp: u32,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
    pub _pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Module {
    /// NUL padded.
    pub name: [u8; 16],
    /// Direct-map address of the (already unpacked) contents.
    pub addr: u64,
    pub len: u64,
}

impl Module {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..n]).unwrap_or("")
    }
}

#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub hhdm: u64,
    pub firmware: u32,
    pub _pad: u32,
    pub fb: Framebuffer,
    /// Direct-map addresses.
    pub mem: *const MemEntry,
    pub mem_len: u64,
    pub modules: *const Module,
    pub modules_len: u64,
}

impl BootInfo {
    /// # Safety
    /// `self` must be the structure the loader built (the pointers are trusted).
    pub unsafe fn memory(&self) -> &[MemEntry] {
        core::slice::from_raw_parts(self.mem, self.mem_len as usize)
    }
    /// # Safety
    /// As for [`BootInfo::memory`].
    pub unsafe fn modules(&self) -> &[Module] {
        core::slice::from_raw_parts(self.modules, self.modules_len as usize)
    }
}

/// The payload bundle (`payload.bin`, built by `xtask`):
///
/// ```text
/// [PAYLOAD_MAGIC u32][count u32][kernel entry u64][kernel memory size u32][pad u32][count x Entry]  then the stored data, each entry at `offset`
/// Entry: name[16], offset u32, stored_len u32, raw_len u32, flags u32 (bit 0: raw deflate)
/// ```
/// The entry named `kernel` is the flat kernel image (as it lies in memory from `KERNEL_BASE`, entered at the virtual address `kernel entry`,
/// occupying `kernel memory size` bytes with .bss); every other entry becomes a [`Module`].
pub const PAYLOAD_MAGIC: u32 = 0x4c59_5041; // "APYL"
/// Entries start after the 8-byte head and the kernel fields (24 bytes in all).
pub const PAYLOAD_HEADER: usize = 8;
pub const PAYLOAD_ENTRY: usize = 32;
pub const FLAG_DEFLATE: u32 = 1;

/// The BIOS boot chain (`boot/bios`): a 512-byte stage 1 (the MBR, or the El Torito boot sector of
/// the ISO) and a stage 2 that `xtask` (ISO) or the installer (target disk) patch with where the
/// things to load lie on the boot medium. A `bios-boot` module is `stage 1 ++ stage 2`.
pub mod bios {
    pub const SECTOR: usize = 512;
    /// Stage 1: `u64` byte offset of stage 2 on the boot medium, then `u32` stage 2 length in bytes.
    pub const S1_PATCH: usize = 424;
    /// Stage 2: where the header starts (stage 2 begins with a jump over it).
    pub const S2_HEADER: usize = 0x20;
    pub const S2_MAGIC: u32 = 0x3253_5241; // "ARS2"
    pub const MODE_PAYLOAD: u32 = 0;
    pub const MODE_LINUX: u32 = 1;
    pub const CMDLINE_MAX: usize = 512;
    /// Header: magic u32, mode u32, three extents (offset u64, length u64), the kernel command line (NUL terminated).
    pub const S2_HEADER_LEN: usize = 8 + 3 * 16 + CMDLINE_MAX;
    /// Where stage 2 is linked and run (real mode, below the 1 MiB mark).
    pub const S2_BASE: u32 = 0x8000;
    /// Stage 2 starts at this multiple of the largest sector size inside `bios-boot` images on the ISO (stage 1 is padded to it).
    pub const S2_ALIGN: usize = 2048;

    /// Locates stage 2 for stage 1.
    pub fn patch_stage1(stage1: &mut [u8], stage2_off: u64, stage2_len: u32) {
        stage1[S1_PATCH..S1_PATCH + 8].copy_from_slice(&stage2_off.to_le_bytes());
        stage1[S1_PATCH + 8..S1_PATCH + 12].copy_from_slice(&stage2_len.to_le_bytes());
    }

    /// Tells stage 2 what to load: `mode`, up to three `(byte offset on the boot medium, length)` extents, and a command line.
    pub fn patch_stage2(stage2: &mut [u8], mode: u32, extents: &[(u64, u64)], cmdline: &str) -> Result<(), &'static str> {
        if extents.len() > 3 || cmdline.len() >= CMDLINE_MAX {
            return Err("too many extents or a command line that is too long");
        }
        let h = &mut stage2[S2_HEADER..S2_HEADER + S2_HEADER_LEN];
        h[0..4].copy_from_slice(&S2_MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&mode.to_le_bytes());
        for (i, &(off, len)) in extents.iter().enumerate() {
            h[8 + i * 16..16 + i * 16].copy_from_slice(&off.to_le_bytes());
            h[16 + i * 16..24 + i * 16].copy_from_slice(&len.to_le_bytes());
        }
        let c = &mut h[8 + 48..];
        c.fill(0);
        c[..cmdline.len()].copy_from_slice(cmdline.as_bytes());
        Ok(())
    }
}
