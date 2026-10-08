//! Stand-in for systemd in QEMU tests: proves the new root was entered, then powers off.
#![no_std]
#![no_main]

#[path = "../sys.rs"]
mod sys;

#[no_mangle]
extern "C" fn entry(_sp: *const usize) -> ! {
    let mut buf = [0u8; 64];
    let fd = sys::open("/etc/archstaller-marker", 0);
    let n = if fd >= 0 { sys::read(fd, &mut buf).max(0) as usize } else { 0 };
    log!("test-payload: running as pid 1 in new root, marker={}", core::str::from_utf8(&buf[..n]).unwrap_or("?").trim());
    sys::power_off()
}
