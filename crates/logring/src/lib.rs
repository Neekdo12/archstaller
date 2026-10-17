//! A bounded, allocation-free byte ring used to capture everything the installer kernel prints, so
//! the whole boot and install log can be written out at the end (the `logdump` step).
//!
//! Pushing never allocates and never panics, so it is safe to feed from the panic path and from before
//! the heap exists. Once the ring is full the oldest bytes are dropped, and [`Ring::is_truncated`]
//! reports it; [`Ring::snapshot`] then prefixes a one-line marker saying how much was lost.
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use core::fmt::{self, Write};

/// A byte ring of a fixed capacity. `CAP` is a compile-time constant so a small ring can be used in
/// tests without touching the kernel's 128 KiB.
pub struct Ring<const CAP: usize> {
    buf: [u8; CAP],
    /// Total bytes ever written; the write position is `written % CAP`. Using a running total (rather
    /// than a wrap flag) makes "exactly full but nothing dropped" distinct from "overwrote the oldest".
    written: usize,
}

impl<const CAP: usize> Default for Ring<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> Ring<CAP> {
    pub const fn new() -> Self {
        Self {
            buf: [0; CAP],
            written: 0,
        }
    }

    /// Write position: where the next byte goes.
    fn head(&self) -> usize {
        self.written % CAP
    }

    /// Appends raw bytes, dropping the oldest when full.
    ///
    /// `written` counts every byte ever pushed and only ever grows. It is advanced with `wrapping_add`
    /// so the counter's behavior is defined instead of panicking in debug and wrapping in release: at
    /// 2^64 bytes (`usize::MAX` on x86_64) a byte counter cannot stay exact anyway, and that many bytes
    /// cannot be printed in any real boot.
    pub fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let h = self.head();
            self.buf[h] = b;
            self.written = self.written.wrapping_add(1);
        }
    }

    /// Whether any bytes were dropped (the ring overwrote at least one byte).
    pub fn is_truncated(&self) -> bool {
        self.written > CAP
    }

    /// Number of bytes currently held.
    pub fn len(&self) -> usize {
        self.written.min(CAP)
    }

    pub fn is_empty(&self) -> bool {
        self.written == 0
    }

    /// Copies the captured bytes, oldest first, appending to `out`. When the ring dropped bytes, a
    /// `[log truncated: N bytes dropped]` line is written first. Never allocates by itself.
    pub fn write_snapshot(&self, out: &mut Vec<u8>) {
        let start = self.written % CAP;
        if self.is_truncated() {
            let dropped = self.written - CAP;
            out.extend_from_slice(format!("[log truncated: {dropped} bytes dropped]\n").as_bytes());
            // Oldest byte is at `start`; the buffer wrapped, so read to the end then from the top.
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..start]);
        } else {
            // Nothing dropped: the data is one contiguous run at the start of the buffer. `start` is
            // `written` here unless the buffer is exactly full, where it is 0 and the run is the whole
            // buffer; `written` covers both.
            out.extend_from_slice(&self.buf[..self.written.min(CAP)]);
        }
    }

    /// The captured bytes as a fresh `Vec` (oldest first, with the truncation marker when it dropped).
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len() + 64);
        self.write_snapshot(&mut out);
        out
    }

    /// Formats `args` straight into the ring, so a caller never needs a temporary `String`.
    pub fn write_fmt(&mut self, args: fmt::Arguments) -> fmt::Result {
        struct Sink<'a, const C: usize>(&'a mut Ring<C>);
        impl<const C: usize> Write for Sink<'_, C> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0.push(s.as_bytes());
                Ok(())
            }
        }
        fmt::write(&mut Sink(self), args)
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    #[test]
    fn keeps_bytes_in_order() {
        let mut r: Ring<16> = Ring::new();
        r.push(b"hello ");
        r.push(b"world");
        assert_eq!(r.snapshot(), b"hello world");
        assert!(!r.is_truncated());
        assert_eq!(r.len(), 11);
    }

    #[test]
    fn wraps_and_reports_the_drop() {
        let mut r: Ring<8> = Ring::new();
        r.push(b"0123456789"); // 10 bytes into an 8-byte ring
        assert!(r.is_truncated());
        assert_eq!(r.len(), 8);
        let s = r.snapshot();
        let text = core::str::from_utf8(&s).unwrap();
        assert!(
            text.starts_with("[log truncated: 2 bytes dropped]\n"),
            "got {text:?}"
        );
        assert!(text.ends_with("23456789"), "got {text:?}");
    }

    #[test]
    fn write_fmt_matches_push() {
        let mut a: Ring<64> = Ring::new();
        a.write_fmt(format_args!("x={} y={}", 1, 2)).unwrap();
        let mut b: Ring<64> = Ring::new();
        b.push(b"x=1 y=2");
        assert_eq!(a.snapshot(), b.snapshot());
    }

    #[test]
    fn exact_capacity_does_not_mark_truncated() {
        let mut r: Ring<4> = Ring::new();
        r.push(b"abcd");
        assert!(!r.is_truncated());
        assert_eq!(r.len(), 4);
        assert_eq!(r.snapshot(), b"abcd");
    }

    #[test]
    fn one_more_byte_drops_exactly_one() {
        let mut r: Ring<4> = Ring::new();
        r.push(b"abcde");
        assert_eq!(r.snapshot(), b"[log truncated: 1 bytes dropped]\nbcde");
    }

    #[test]
    fn a_single_oversized_chunk_keeps_the_tail() {
        let mut r: Ring<4> = Ring::new();
        r.push(b"abcdef");
        assert_eq!(r.snapshot(), b"[log truncated: 2 bytes dropped]\ncdef");
    }

    #[test]
    fn several_wraps_keep_the_most_recent_capacity() {
        let mut r: Ring<4> = Ring::new();
        for _ in 0..10 {
            r.push(b"abcd");
        }
        assert_eq!(r.snapshot(), b"[log truncated: 36 bytes dropped]\nabcd");
    }

    #[test]
    fn a_write_spanning_the_wrap_keeps_the_newest_and_marks_the_drop() {
        // Fill exactly, then push a span that crosses the physical end of the buffer. The oldest bytes
        // (the start of the second chunk) must be the ones dropped, and the newest must be intact.
        let mut r: Ring<8> = Ring::new();
        r.push(b"abcd"); // head = 4
        r.push(b"efghij"); // 6 bytes: head wraps 4->7, then 0->1; "abcd" + "ef" dropped, "ghij" kept
        assert!(r.is_truncated());
        assert_eq!(r.len(), 8);
        assert_eq!(r.snapshot(), b"[log truncated: 2 bytes dropped]\ncdefghij");
    }

    #[test]
    fn kernel_sized_ring_keeps_a_full_capacity_without_truncation() {
        // Mirror the kernel's 128 KiB ring: exactly CAP bytes are kept and not reported as truncated.
        const CAP: usize = 128 * 1024;
        let mut r: Ring<CAP> = Ring::new();
        let chunk = [b'x'; 4096];
        for _ in 0..CAP / 4096 {
            r.push(&chunk);
        }
        assert_eq!(r.len(), CAP);
        assert!(!r.is_truncated());
        assert_eq!(r.snapshot().len(), CAP);
    }
}
