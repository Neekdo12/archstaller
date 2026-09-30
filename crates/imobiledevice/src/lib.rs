//! Apple iPhone/iPad USB tethering (Personal Hotspot) for the installer, per
//! `docs/iphone-tethering.md`: usbmuxd framing over USB bulk, lockdownd pairing
//! (RSA-2048 host identity, self-signed certs, user taps "Trust"), a TLS-wrapped
//! lockdownd session, StartService for the hotspot relay, and a `hal::NetDevice`
//! on the resulting channel.
//!
//! Clean-room implementation; the libimobiledevice project's sources were used as
//! wire-format documentation only. Parts that cannot be verified without a real
//! device are marked UNVERIFIED (see `lockdown::HOTSPOT_SERVICE` and `DEFAULT_DEVICE_ID`).
#![no_std]

extern crate alloc;

pub mod cert;
pub mod lockdown;
pub mod mux;
pub mod netdev;
pub mod pair;
pub mod plist;
mod xml;

use hal::{Error, Result};

/// Device id to use in mux `Connect` when talking directly to the phone (no host
/// usbmuxd daemon). UNVERIFIED without real hardware; the reference daemon assigns
/// ids from 2 upward, so 2 is the first (and here only) device.
pub const DEFAULT_DEVICE_ID: u32 = 2;

/// Whether the hotspot relay channel carries raw IP packets (true) or Ethernet frames
/// (false). UNVERIFIED: the spec requires confirming against real traffic
/// (`docs/iphone-tethering.md`, step 6). Raw IP is the point-to-point behavior seen
/// in tethered connections; the NetDevice adapter handles both.
const RELAY_RAW_IP: bool = true;

/// Progress callbacks so the kernel can narrate the (possibly long) pairing flow.
pub trait Progress {
    fn note(&mut self, msg: &str);
}

/// Finds a tethered iPhone and brings it to a `hal::NetDevice` on the hotspot relay.
///
/// Blocks through pairing; while the phone's Trust dialog is pending this loops,
/// calling `progress.note` once, for up to `pair_timeout_ms`.
pub fn tether(
    unix_time: fn() -> u64,
    pair_timeout_ms: u64,
    progress: &mut impl Progress,
) -> Result<netdev::IphoneNet> {
    let mut mux = mux::Muxer::find()?;
    let serial = alloc::string::String::from(mux.device_serial());
    progress.note("usb: found Apple device, starting pairing");

    let buid = mux.read_buid().unwrap_or_else(|_| pair::uuid4());
    let chan = mux.connect(DEFAULT_DEVICE_ID, mux::LOCKDOWN_PORT)?;
    let mut ld = lockdown::Lockdown::new(chan)?;

    // Pair, retrying while the Trust dialog is pending.
    let start = drivers::platform::now_ns();
    let mut nagged = false;
    let record = loop {
        match ld.pair(&buid, unix_time()) {
            Ok(r) => break r,
            Err(Error::Timeout) => {
                if !nagged {
                    progress.note("usb: tap \"Trust\" on the iPhone to continue");
                    nagged = true;
                }
                if drivers::platform::now_ns() - start > pair_timeout_ms * 1_000_000 {
                    return Err(Error::Timeout);
                }
            }
            Err(e) => return Err(e),
        }
    };
    ld.validate_pair(&record)?;
    progress.note("usb: paired");

    let session = ld.start_session(&record)?;
    let mut chan = ld.into_channel();

    // TLS-wrapped session phase: StartService, then StopSession.
    let port = {
        let cfg = lockdown::session_config(&record, unix_time)?;
        let name = if serial.is_empty() { "device" } else { serial.as_str() };
        let tls = net::tls::TlsStream::connect(chan.as_stream(), cfg, name).map_err(|_| Error::Io)?;
        let mut tld: lockdown::Lockdown<net::tls::TlsStream<netdev::ChannelStream>> =
            lockdown::Lockdown::from_io(tls);
        let (port, ssl) = tld.start_service(lockdown::HOTSPOT_SERVICE)?;
        if ssl {
            // A service that wants its own TLS on top: not what the relay needs;
            // bail rather than guess.
            return Err(Error::Unsupported);
        }
        let _ = tld.stop_session(&session);
        port
    };
    progress.note("usb: hotspot relay started");

    let relay = chan.into_muxer().connect(DEFAULT_DEVICE_ID, port)?;
    Ok(netdev::IphoneNet::new(relay, netdev::IphoneNet::mac_from_serial(&serial), RELAY_RAW_IP))
}
