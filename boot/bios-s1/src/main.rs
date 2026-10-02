//! BIOS stage 1: 512 bytes that are the MBR of a disk or the El Torito boot sector of the ISO. It
//! reads stage 2 (location patched in at `bootinfo::bios::S1_PATCH` by whoever wrote the image)
//! to 0x8000 with INT 13h extended reads and jumps to it with `dl` = the boot drive and `bx` = the
//! medium's sector size (a CD has 2048-byte sectors, a disk usually 512).
//!
//! The BIOS may start this code at 07C0:0000 or 0000:7C00, or have trashed any register but `dl`,
//! so it normalizes `cs` first; the way to ask for the sector size and the fallback when it
//! cannot are from Limine's stage 1.
#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(
    r#"
    .section .s1,"ax"
    .code16
    .global s1_start
s1_start:
    cli
    cld
    .byte 0xea
    .word 2f
    .word 0
2:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00
    sti
    mov byte ptr [s1_drive], dl

    # INT 13h extensions present?
    mov ah, 0x41
    mov bx, 0x55aa
    int 0x13
    jc 90f
    cmp bx, 0xaa55
    jne 91f

    # Sector size. The word after the size field must be clear: a PhoenixBIOS 4.0 R6.0 fails the call otherwise.
    mov si, 0x600
    mov word ptr [si], 0x1e
    mov word ptr [si+2], 0
    mov ah, 0x48
    mov dl, byte ptr [s1_drive]
    int 0x13
    mov bp, 512
    jc 5f
    mov bp, [si+24]
    # Only a power of two from 512 to 4096 is believable.
    lea ax, [bp-1]
    test ax, bp
    jnz 4f
    test bp, 0x1e00
    jnz 5f
4:
    mov bp, 512
5:
    # A CD whose BIOS did not say otherwise (the El Torito boot drive is 0x80 or above).
    # (Nothing to do: 512 is what a disk has, and CD BIOSes answer function 48h.)

    # eax = first sector = offset / sector size; the patched offset is a multiple of the sector size.
    mov eax, dword ptr [s1_off]
    movzx ecx, bp
    xor edx, edx
    div ecx
    mov dword ptr [s1_lba], eax
    # number of sectors = ceil(length / sector size)
    mov eax, dword ptr [s1_len]
    add eax, ecx
    dec eax
    xor edx, edx
    div ecx
    mov dword ptr [s1_left], eax
    # chunk = 32 KiB / sector size, in sectors
    mov eax, 32768
    xor edx, edx
    div ecx
    mov word ptr [s1_chunk], ax
    mov word ptr [s1_seg], 0x0800        # stage 2 lives at 0x8000

6:
    mov eax, dword ptr [s1_left]
    test eax, eax
    jz 8f
    movzx ecx, word ptr [s1_chunk]
    cmp eax, ecx
    jbe 7f
    mov eax, ecx
7:
    mov word ptr [s1_dap + 2], ax
    mov cx, 3                             # tries
10:
    push cx
    mov eax, dword ptr [s1_lba]
    mov dword ptr [s1_dap + 8], eax
    mov ax, word ptr [s1_seg]
    mov word ptr [s1_dap + 6], ax
    mov si, offset s1_dap
    mov ah, 0x42
    mov dl, byte ptr [s1_drive]
    int 0x13
    pop cx
    jnc 11f
    xor ax, ax
    mov dl, byte ptr [s1_drive]
    int 0x13                              # reset the controller, try again
    dec cx
    jnz 10b
    jmp 92f
11:
    movzx eax, word ptr [s1_dap + 2]
    add dword ptr [s1_lba], eax
    sub dword ptr [s1_left], eax
    mul bp                                # bytes read
    shr ax, 4
    add word ptr [s1_seg], ax
    jmp 6b
8:
    mov dl, byte ptr [s1_drive]
    mov bx, bp
    .byte 0xea
    .word 0x8000
    .word 0

92:
    inc si
91:
    inc si
90:
    add si, 0x4f30
    push 0xb800
    pop es
    mov word ptr es:[0], si
    cli
99:
    hlt
    jmp 99b

    .balign 4
s1_dap:
    .byte 0x10, 0
    .word 0
    .word 0
    .word 0
    .quad 0
s1_lba:    .long 0
s1_left:   .long 0
s1_chunk:  .word 0
s1_seg:    .word 0
s1_drive:  .byte 0

    .org 424
s1_off:    .quad 0
s1_len:    .long 0
    "#
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
