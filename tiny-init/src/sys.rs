#![allow(dead_code)]
//! Raw Linux x86_64 system calls; no libc.
use core::arch::asm;

pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_CLOSE: usize = 3;
pub const SYS_LSEEK: usize = 8;
pub const SYS_MMAP: usize = 9;
pub const SYS_NANOSLEEP: usize = 35;
pub const SYS_EXECVE: usize = 59;
pub const SYS_CHDIR: usize = 80;
pub const SYS_MKDIR: usize = 83;
pub const SYS_CHROOT: usize = 161;
pub const SYS_MOUNT: usize = 165;
pub const SYS_REBOOT: usize = 169;
pub const SYS_INIT_MODULE: usize = 175;
pub const SYS_GETDENTS64: usize = 217;
pub const SYS_FSTAT: usize = 5;

pub const MS_RDONLY: usize = 1;
pub const MS_MOVE: usize = 8192;

#[inline]
pub unsafe fn syscall(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize) -> isize {
    let ret: isize;
    asm!(
        "syscall",
        inlateout("rax") n as isize => ret,
        in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e, in("r9") 0usize,
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

/// NUL-terminated copy of `s` on the stack.
pub struct CStr<const N: usize>([u8; N]);

impl<const N: usize> CStr<N> {
    pub fn new(s: &str) -> CStr<N> {
        let mut b = [0u8; N];
        let n = s.len().min(N - 1);
        b[..n].copy_from_slice(&s.as_bytes()[..n]);
        CStr(b)
    }
    pub fn ptr(&self) -> usize {
        self.0.as_ptr() as usize
    }
}

pub fn write(fd: i32, data: &[u8]) -> isize {
    unsafe { syscall(SYS_WRITE, fd as usize, data.as_ptr() as usize, data.len(), 0, 0) }
}

pub fn open(path: &str, flags: usize) -> isize {
    let p = CStr::<256>::new(path);
    unsafe { syscall(SYS_OPEN, p.ptr(), flags, 0, 0, 0) }
}

pub fn close(fd: isize) {
    unsafe { syscall(SYS_CLOSE, fd as usize, 0, 0, 0, 0) };
}

pub fn read(fd: isize, buf: &mut [u8]) -> isize {
    unsafe { syscall(SYS_READ, fd as usize, buf.as_mut_ptr() as usize, buf.len(), 0, 0) }
}

pub fn pread(fd: isize, buf: &mut [u8], offset: u64) -> isize {
    unsafe {
        if syscall(SYS_LSEEK, fd as usize, offset as usize, 0, 0, 0) < 0 {
            return -1;
        }
    }
    read(fd, buf)
}

pub fn mount(src: &str, target: &str, fstype: &str, flags: usize, data: &str) -> isize {
    let s = CStr::<256>::new(src);
    let t = CStr::<256>::new(target);
    let f = CStr::<32>::new(fstype);
    let d = CStr::<64>::new(data);
    unsafe { syscall(SYS_MOUNT, s.ptr(), t.ptr(), f.ptr(), flags, d.ptr()) }
}

pub fn chdir(path: &str) -> isize {
    let p = CStr::<256>::new(path);
    unsafe { syscall(SYS_CHDIR, p.ptr(), 0, 0, 0, 0) }
}

pub fn chroot(path: &str) -> isize {
    let p = CStr::<256>::new(path);
    unsafe { syscall(SYS_CHROOT, p.ptr(), 0, 0, 0, 0) }
}

pub fn mkdir(path: &str, mode: usize) -> isize {
    let p = CStr::<256>::new(path);
    unsafe { syscall(SYS_MKDIR, p.ptr(), mode, 0, 0, 0) }
}

pub fn sleep_ms(ms: u64) {
    let ts = [(ms / 1000) as usize, (ms % 1000) as usize * 1_000_000];
    unsafe { syscall(SYS_NANOSLEEP, ts.as_ptr() as usize, 0, 0, 0, 0) };
}

pub fn power_off() -> ! {
    unsafe { syscall(SYS_REBOOT, 0xfee1dead, 672274793, 0x4321fedc, 0, 0) };
    loop {
        sleep_ms(1000);
    }
}

/// Anonymous read/write mapping of `len` bytes.
pub fn map_anon(len: usize) -> Option<&'static mut [u8]> {
    // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS
    let p = unsafe { syscall(SYS_MMAP, 0, len, 3, 0x22, usize::MAX) };
    if p < 0 && p > -4096 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(p as *mut u8, len) })
}

pub fn file_size(fd: isize) -> Option<usize> {
    let mut st = [0u8; 144];
    let r = unsafe { syscall(SYS_FSTAT, fd as usize, st.as_mut_ptr() as usize, 0, 0, 0) };
    if r < 0 {
        return None;
    }
    Some(u64::from_ne_bytes(st[48..56].try_into().unwrap()) as usize) // st_size
}

pub struct Log;

impl core::fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write(2, s.as_bytes());
        Ok(())
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::sys::Log, $($arg)*);
        let _ = $crate::sys::Log.write_str("\n");
    }};
}

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call entry",
    "ud2",
);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log!("init: panic: {info}");
    loop {
        sleep_ms(1000);
    }
}
