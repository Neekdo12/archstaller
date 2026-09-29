//! Networking on top of smoltcp: DHCP, DNS, TCP and a small HTTP/1.1 client.
#![no_std]

extern crate alloc;

pub mod client;
pub mod http;
pub mod stack;
pub mod tls;

pub use stack::{Lease, Stack, TcpConn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Timeout,
    Dns,
    Connect,
    /// Peer closed the connection before the message was complete.
    Closed,
    Http(&'static str),
    Device(hal::Error),
}

pub type Result<T> = core::result::Result<T, Error>;

/// Blocking byte stream (plain TCP now, TLS on top later).
pub trait Stream {
    /// Blocks until at least one byte is available; returns 0 on orderly close.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;
}
