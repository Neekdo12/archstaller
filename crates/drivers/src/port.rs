use core::arch::asm;

#[inline]
pub unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn outl(port: u16, val: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}

#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}

#[inline]
pub unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}

/// Reads `count` 16-bit words from `port` into `buf` (`rep insw`).
#[inline]
pub unsafe fn insw(port: u16, buf: *mut u16, count: usize) {
    asm!("cld", "rep insw", in("dx") port, inout("rdi") buf => _, inout("rcx") count => _, options(nostack));
}

/// Writes `count` 16-bit words from `buf` to `port` (`rep outsw`).
#[inline]
pub unsafe fn outsw(port: u16, buf: *const u16, count: usize) {
    asm!("cld", "rep outsw", in("dx") port, inout("rsi") buf => _, inout("rcx") count => _, options(nostack));
}
