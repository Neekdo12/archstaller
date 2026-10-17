#![expect(dead_code, reason = "syscall bindings used across tiny-init binaries")]
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
pub unsafe fn syscall(
    nr: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> isize {
    let ret: isize;
    asm!(
        "syscall",
        inlateout("rax") nr as isize => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8") a5,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

pub struct CStr<const N: usize>(pub [u8; N]);

impl<const N: usize> CStr<N> {
    pub fn new(s: &str) -> CStr<N> {
        let mut b = [0u8; N];
        let n = s.len().min(N.saturating_sub(1));
        if let Some(dest) = b.get_mut(..n) {
            if let Some(src) = s.as_bytes().get(..n) {
                dest.copy_from_slice(src);
            }
        }
        CStr(b)
    }
    pub fn ptr(&self) -> usize {
        self.0.as_ptr() as usize
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub fn write(fd: i32, data: &[u8]) -> isize {
    // SAFETY: SYS_WRITE syscall with valid buffer pointer and length.
    unsafe {
        syscall(
            SYS_WRITE,
            fd.cast_unsigned() as usize,
            data.as_ptr() as usize,
            data.len(),
            0,
            0,
        )
    }
}

pub fn open(path: &str, flags: usize) -> isize {
    let p = CStr::<256>::new(path);
    // SAFETY: SYS_OPEN syscall with NUL-terminated path pointer.
    unsafe { syscall(SYS_OPEN, p.ptr(), flags, 0, 0, 0) }
}

pub fn close(fd: isize) {
    // SAFETY: SYS_CLOSE syscall for file descriptor.
    unsafe { syscall(SYS_CLOSE, fd.cast_unsigned(), 0, 0, 0, 0) };
}

pub fn read(fd: isize, buf: &mut [u8]) -> isize {
    // SAFETY: SYS_READ syscall with valid buffer pointer and length.
    unsafe {
        syscall(
            SYS_READ,
            fd.cast_unsigned(),
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
        )
    }
}

pub fn pread(fd: isize, buf: &mut [u8], offset: u64) -> isize {
    // SAFETY: SYS_LSEEK syscall for seeking file descriptor.
    let seek_ok = unsafe { syscall(SYS_LSEEK, fd.cast_unsigned(), offset as usize, 0, 0, 0) >= 0 };
    if !seek_ok {
        return -1;
    }
    read(fd, buf)
}

pub fn mount(src: &str, target: &str, fstype: &str, flags: usize, data: &str) -> isize {
    let s = CStr::<256>::new(src);
    let t = CStr::<256>::new(target);
    let f = CStr::<32>::new(fstype);
    let d = CStr::<64>::new(data);
    // SAFETY: SYS_MOUNT syscall with NUL-terminated string pointers.
    unsafe { syscall(SYS_MOUNT, s.ptr(), t.ptr(), f.ptr(), flags, d.ptr()) }
}

pub fn chdir(path: &str) -> isize {
    let p = CStr::<256>::new(path);
    // SAFETY: SYS_CHDIR syscall with NUL-terminated path pointer.
    unsafe { syscall(SYS_CHDIR, p.ptr(), 0, 0, 0, 0) }
}

pub fn chroot(path: &str) -> isize {
    let p = CStr::<256>::new(path);
    // SAFETY: SYS_CHROOT syscall with NUL-terminated path pointer.
    unsafe { syscall(SYS_CHROOT, p.ptr(), 0, 0, 0, 0) }
}

pub fn mkdir(path: &str, mode: usize) -> isize {
    let p = CStr::<256>::new(path);
    // SAFETY: SYS_MKDIR syscall with NUL-terminated path pointer.
    unsafe { syscall(SYS_MKDIR, p.ptr(), mode, 0, 0, 0) }
}

pub fn sleep_ms(ms: u64) {
    let ts = [(ms / 1000) as usize, (ms % 1000) as usize * 1_000_000];
    // SAFETY: SYS_NANOSLEEP syscall with valid timespec pointer.
    unsafe { syscall(SYS_NANOSLEEP, ts.as_ptr() as usize, 0, 0, 0, 0) };
}

pub fn power_off() -> ! {
    // SAFETY: SYS_REBOOT syscall with magic constants requesting power off.
    unsafe { syscall(SYS_REBOOT, 0xfee1dead, 672274793, 0x4321fedc, 0, 0) };
    loop {
        sleep_ms(1000);
    }
}

/// Anonymous read/write mapping of `len` bytes.
pub fn map_anon(len: usize) -> Option<&'static mut [u8]> {
    // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS
    // SAFETY: SYS_MMAP syscall requesting private anonymous memory.
    let p = unsafe { syscall(SYS_MMAP, 0, len, 3, 0x22, usize::MAX) };
    if p < 0 && p > -4096 {
        return None;
    }
    // SAFETY: Pointer returned by successful mmap is valid for `len` bytes.
    Some(unsafe { core::slice::from_raw_parts_mut(p as *mut u8, len) })
}

pub fn file_size(fd: isize) -> Option<usize> {
    let mut st = [0u8; 144];
    // SAFETY: SYS_FSTAT syscall writing stat struct into stack buffer.
    let r = unsafe { syscall(SYS_FSTAT, fd.cast_unsigned(), st.as_mut_ptr() as usize, 0, 0, 0) };
    if r < 0 {
        return None;
    }
    let bytes: [u8; 8] = st.get(48..56)?.try_into().ok()?;
    Some(u64::from_ne_bytes(bytes) as usize)
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
        match write!($crate::sys::Log, $($arg)*) {
            Ok(()) | Err(_) => {}
        }
        match $crate::sys::Log.write_str("\n") {
            Ok(()) | Err(_) => {}
        }
    }};
}

#[cfg(target_os = "none")]
core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call entry",
    "ud2",
);

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log!("init: panic: {info}");
    loop {
        sleep_ms(1000);
    }
}
