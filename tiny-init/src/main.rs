//! Initramfs init: load modules, find the root filesystem by UUID, mount it and switch to it.
#![no_std]
#![no_main]

mod sys;

use sys::*;

const ROOT_WAIT_MS: u64 = 20_000;

fn fail(msg: &str) -> ! {
    log!("init: FATAL: {msg}");
    loop {
        sleep_ms(1000);
    }
}

fn read_all(path: &str, buf: &mut [u8]) -> usize {
    let fd = open(path, 0);
    if fd < 0 {
        return 0;
    }
    let mut n = 0;
    while n < buf.len() {
        let r = read(fd, &mut buf[n..]);
        if r <= 0 {
            break;
        }
        n += r as usize;
    }
    close(fd);
    n
}

fn load_modules() {
    static mut LIST: [u8; 8192] = [0; 8192];
    let list = unsafe { &mut *core::ptr::addr_of_mut!(LIST) };
    let n = read_all("/modules/order", list);
    let text = core::str::from_utf8(&list[..n]).unwrap_or("");
    for name in text.lines().filter(|l| !l.is_empty()) {
        let mut path = [0u8; 128];
        let prefix = b"/modules/";
        path[..prefix.len()].copy_from_slice(prefix);
        let mut len = prefix.len();
        path[len..len + name.len()].copy_from_slice(name.as_bytes());
        len += name.len();
        path[len..len + 3].copy_from_slice(b".ko");
        len += 3;
        let path = core::str::from_utf8(&path[..len]).unwrap();
        let fd = open(path, 0);
        if fd < 0 {
            log!("init: cannot open {path}");
            continue;
        }
        let Some(size) = file_size(fd) else { continue };
        let Some(image) = map_anon(size) else { fail("out of memory loading modules") };
        let mut got = 0;
        while got < size {
            let r = read(fd, &mut image[got..]);
            if r <= 0 {
                break;
            }
            got += r as usize;
        }
        close(fd);
        let params = CStr::<4>::new("");
        let r = unsafe { syscall(SYS_INIT_MODULE, image.as_ptr() as usize, size, params.ptr(), 0, 0) };
        // -EEXIST: built in or already loaded.
        if r < 0 && r != -17 {
            log!("init: module {name} failed: errno {}", -r);
        }
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    let mut n = 0;
    let mut hi: Option<u8> = None;
    for c in s.bytes() {
        if c == b'-' {
            continue;
        }
        let v = hex_val(c)?;
        match hi.take() {
            None => hi = Some(v),
            Some(h) => {
                *out.get_mut(n)? = h << 4 | v;
                n += 1;
            }
        }
    }
    (n == 16 && hi.is_none()).then_some(out)
}

/// Returns the device node of the ext4 filesystem with `uuid`, waiting for drivers to probe.
fn find_root_by_uuid(uuid: &[u8; 16], out: &mut [u8; 64]) -> Option<usize> {
    let mut waited = 0;
    loop {
        let dir = open("/sys/class/block", 0o200000); // O_DIRECTORY
        if dir >= 0 {
            let mut buf = [0u8; 2048];
            loop {
                let n = unsafe { syscall(SYS_GETDENTS64, dir as usize, buf.as_mut_ptr() as usize, buf.len(), 0, 0) };
                if n <= 0 {
                    break;
                }
                let mut off = 0;
                while off < n as usize {
                    let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
                    let name_bytes = &buf[off + 19..off + reclen];
                    let end = name_bytes.iter().position(|&c| c == 0).unwrap_or(name_bytes.len());
                    let name = core::str::from_utf8(&name_bytes[..end]).unwrap_or("");
                    off += reclen;
                    if name.starts_with('.') || name.starts_with("loop") || name.starts_with("ram") {
                        continue;
                    }
                    let mut dev = [0u8; 64];
                    let prefix = b"/dev/";
                    dev[..5].copy_from_slice(prefix);
                    dev[5..5 + name.len()].copy_from_slice(name.as_bytes());
                    let path = core::str::from_utf8(&dev[..5 + name.len()]).unwrap();
                    let fd = open(path, 0);
                    if fd < 0 {
                        continue;
                    }
                    let mut sb = [0u8; 1024];
                    let got = pread(fd, &mut sb, 1024);
                    close(fd);
                    if got == 1024 && sb[0x38] == 0x53 && sb[0x39] == 0xef && sb[0x68..0x78] == uuid[..] {
                        out[..5 + name.len()].copy_from_slice(&dev[..5 + name.len()]);
                        close(dir);
                        return Some(5 + name.len());
                    }
                }
            }
            close(dir);
        }
        if waited >= ROOT_WAIT_MS {
            return None;
        }
        sleep_ms(250);
        waited += 250;
    }
}

#[no_mangle]
extern "C" fn entry(_sp: *const usize) -> ! {
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");
    mount("proc", "/proc", "proc", 0, "");
    mount("sysfs", "/sys", "sysfs", 0, "");
    let con = open("/dev/console", 2);
    if con >= 0 {
        for fd in 0..3 {
            unsafe { syscall(33, con as usize, fd, 0, 0, 0) }; // dup2
        }
    }
    log!("init: archstaler initramfs");

    load_modules();

    static mut CMDLINE: [u8; 4096] = [0; 4096];
    let cmdline = unsafe { &mut *core::ptr::addr_of_mut!(CMDLINE) };
    let n = read_all("/proc/cmdline", cmdline);
    let cmdline = core::str::from_utf8(&cmdline[..n]).unwrap_or("");
    let root = cmdline.split_whitespace().find_map(|t| t.strip_prefix("root=")).unwrap_or_else(|| fail("no root= on the kernel command line"));

    let mut devbuf = [0u8; 64];
    let dev: &str = if let Some(u) = root.strip_prefix("UUID=") {
        let uuid = parse_uuid(u).unwrap_or_else(|| fail("malformed root=UUID="));
        let n = find_root_by_uuid(&uuid, &mut devbuf).unwrap_or_else(|| fail("root filesystem not found"));
        core::str::from_utf8(&devbuf[..n]).unwrap()
    } else if root.starts_with("/dev/") {
        root
    } else {
        fail("unsupported root= form (use UUID= or /dev/...)");
    };
    log!("init: root is {dev}");

    let fstype = cmdline.split_whitespace().find_map(|t| t.strip_prefix("rootfstype=")).unwrap_or("ext4");
    // Like the kernel's own root handling: read-only unless `rw` is given.
    let writable = cmdline.split_whitespace().rev().find(|t| *t == "rw" || *t == "ro") == Some("rw");
    let mut r = -1;
    for _ in 0..40 {
        r = mount(dev, "/newroot", fstype, if writable { 0 } else { MS_RDONLY }, "");
        if r >= 0 {
            break;
        }
        sleep_ms(250);
    }
    if r < 0 {
        log!("init: mount failed: errno {}", -r);
        fail("cannot mount the root filesystem");
    }

    // Hand the pseudo filesystems over, then make /newroot the root.
    mount("/dev", "/newroot/dev", "", MS_MOVE, "");
    mount("/proc", "/newroot/proc", "", MS_MOVE, "");
    mount("/sys", "/newroot/sys", "", MS_MOVE, "");
    if chdir("/newroot") < 0 || mount("/newroot", "/", "", MS_MOVE, "") < 0 || chroot(".") < 0 || chdir("/") < 0 {
        fail("switch_root failed");
    }
    let init = CStr::<64>::new("/usr/lib/systemd/systemd");
    let argv = [init.ptr(), 0];
    let envp = [0usize];
    unsafe { syscall(SYS_EXECVE, init.ptr(), argv.as_ptr() as usize, envp.as_ptr() as usize, 0, 0) };
    fail("cannot execute /usr/lib/systemd/systemd");
}
