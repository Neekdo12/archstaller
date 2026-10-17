//! BIOS stage 2 (64-bit part). `entry.s` collected the memory map and a framebuffer, read what to
//! boot into high memory, and entered long mode with the first 4 GiB identity mapped; this hands
//! over to `loadcore` (installer ISO) or boots the installed system's Linux kernel (first boot).
#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code, unused_imports))]

use bootinfo::bios;
use bootinfo::*;
#[cfg(not(test))]
use core::arch::global_asm;
use loadcore::{log, log_hex, Boot, Map};

pub mod linux;

#[cfg(not(test))]
global_asm!(include_str!("entry.s"));

/// Page tables for the identity map, zeroed and filled by `entry.s`.
#[repr(C, align(4096))]
pub struct PageTables([u8; 0x6000]);
#[no_mangle]
pub static mut boot_pt: PageTables = PageTables([0; 0x6000]);

static mut MAP: Map = Map::new();

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log("boot: ");
    log(info.message().as_str().unwrap_or("panic"));
    log("\n");
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}

fn rd32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

extern "C" {
    static s2_header: u8;
}

#[cfg(not(test))]
#[no_mangle]
extern "C" fn bios_main() -> ! {
    log("boot: long mode\n");
    #[allow(clippy::deref_addrof)]
    unsafe {
        let map = &mut *(&raw mut MAP);
        // E820 entries: base u64, length u64, type u32, extended attributes u32 (bit 0 clear: ignore the entry).
        for i in 0..rd32(0x500) as usize {
            let e = 0x600 + i * 24;
            let base = (rd32(e + 4) as u64) << 32 | rd32(e) as u64;
            let len = (rd32(e + 12) as u64) << 32 | rd32(e + 8) as u64;
            if rd32(e + 20) & 1 == 0 {
                continue;
            }
            let kind = match rd32(e + 16) {
                1 => MEM_USABLE,
                3 => MEM_ACPI_RECLAIMABLE,
                4 => MEM_ACPI_NVS,
                5 => MEM_BAD,
                _ => MEM_RESERVED,
            };
            map.push(base, len, kind);
        }
        // Real mode land (and the BIOS data): never ours to give away.
        map.push(0, 0x10_0000, MEM_LOADER);
        let mut staged = [(0u64, 0u64); 3];
        for (i, s) in staged.iter_mut().enumerate() {
            *s = (rd32(0x540 + i * 8) as u64, rd32(0x544 + i * 8) as u64);
            map.push(s.0, (s.1 + 4095) & !4095, MEM_LOADER);
        }
        map.normalize();

        let h = &s2_header as *const u8;
        let mode = core::ptr::read_unaligned(h.add(4) as *const u32);
        if core::ptr::read_unaligned(h as *const u32) != bios::S2_MAGIC {
            panic!("stage 2 header is not set up");
        }
        let fb = Framebuffer {
            addr: rd32(0x510) as u64,
            width: rd32(0x514),
            height: rd32(0x518),
            pitch: rd32(0x51c),
            bpp: 32,
            red_shift: rd32(0x520) as u8,
            green_shift: (rd32(0x520) >> 8) as u8,
            blue_shift: (rd32(0x520) >> 16) as u8,
            _pad: 0,
        };
        if mode == bios::MODE_PAYLOAD {
            let payload =
                core::slice::from_raw_parts(staged[0].0 as *const u8, staged[0].1 as usize);
            let size = loadcore::arena_size(payload, map.top())
                .unwrap_or_else(|| panic!("payload is damaged"));
            let size = (size + 4095) & !4095;
            let arena = map
                .find_free(size, 4096, 0x10_0000, 4 << 30)
                .unwrap_or_else(|| panic!("not enough memory"));
            log("boot: arena at ");
            log_hex(arena);
            log("\n");
            // The real-mode probe (entry.s `probe_boot_medium`) read LBA0 and the sector count of the
            // drive the firmware loaded us from into low memory. Fingerprint them here, where the 64-bit
            // hash is available; an absent probe leaves the medium unknown and the log dump skips.
            let boot_medium = if rd32(0x560) != 0 {
                let sector_size = rd32(0x564);
                let sectors = (rd32(0x568) as u64) | ((rd32(0x56c) as u64) << 32);
                let lba0 = core::slice::from_raw_parts(0x570 as *const u8, 512);
                BootMedium {
                    source: MEDIUM_SOURCE_BIOS,
                    sector_size,
                    sectors,
                    lba0_hash: BootMedium::fingerprint(lba0, sector_size, sectors),
                }
            } else {
                BootMedium::default()
            };
            loadcore::boot(
                map,
                Boot {
                    firmware: FIRMWARE_BIOS,
                    fb,
                    payload,
                    arena: (arena, size),
                    boot_medium,
                },
            );
        }
        if mode == bios::MODE_LINUX {
            let cmd = core::slice::from_raw_parts(h.add(8 + 48), bios::CMDLINE_MAX);
            let cmd = core::str::from_utf8(&cmd[..cmd.iter().position(|&b| b == 0).unwrap_or(0)])
                .unwrap_or("");
            let bz = core::slice::from_raw_parts(staged[0].0 as *const u8, staged[0].1 as usize);
            let end = staged[1].0 + ((staged[1].1 + 0xfffff) & !0xfffff);
            linux::boot(map, bz, staged[1], cmd, &fb, end);
        }
        panic!("this boot mode is not supported");
    }
}
