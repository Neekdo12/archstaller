//! Apple iPhone/iPad USB tethering (Personal Hotspot) for the installer, per
//! `docs/iphone-tethering.md`: the device-side usbmux protocol over the phone's mux
//! interface (`mux.rs`), lockdownd pairing so the user can tap "Trust" (RSA-2048 host
//! identity, self-signed certs; `lockdown.rs`, `pair.rs`), then the phone's standard
//! tethering interface as a `hal::NetDevice` (`netdev.rs`).
//!
//! Clean-room implementation; the libimobiledevice and usbmuxd sources and the Linux
//! `ipheth` driver were used as wire-format documentation only. Never run against a
//! real phone in CI: see `OVERVIEW.md` for what is and is not verified.
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

/// Progress callbacks so the kernel can narrate the (possibly long) pairing flow.
pub trait Progress {
    fn note(&mut self, msg: &str);
}

/// Whether `scan` holds an Apple device with the usbmux interface.
pub fn present(scan: &usb::Scan) -> bool {
    scan.devices().any(|d| {
        d.vendor == mux::APPLE_VENDOR && d.interfaces.iter().any(|i| (i.class, i.subclass, i.protocol) == mux::MUX_CLASS)
    })
}

/// Takes a tethered iPhone from `scan`, pairs with it and brings up its hotspot interface as a
/// `hal::NetDevice`.
///
/// Blocks through pairing; while the phone's Trust dialog is pending this keeps waiting
/// for up to `pair_timeout_ms`. The pairing record is not persisted.
pub fn tether(
    scan: &mut usb::Scan,
    unix_time: fn() -> u64,
    pair_timeout_ms: u64,
    progress: &mut impl Progress,
) -> Result<netdev::IphoneNet> {
    let mut mux = mux::Muxer::find(scan)?;
    progress.note("usb: found Apple device, starting mux");
    mux.handshake()?;
    progress.note("usb: mux handshake done, connecting to lockdownd");
    let chan = mux.connect(mux::LOCKDOWN_PORT)?;
    let mut ld = lockdown::Lockdown::new(chan)?;
    progress.note("usb: lockdownd reachable, preparing pairing keys");

    let buid = pair::uuid4();
    let record = ld.pair_prepare(&buid, unix_time())?;
    progress.note("usb: pairing keys ready, asking the iPhone to trust this computer");

    // Pair. lockdownd either answers at once with "dialog pending" / "locked" (then the
    // request is repeated) or holds the answer until the user acts on the phone.
    let start = drivers::platform::now_ns();
    let timed_out = || drivers::platform::now_ns() - start > pair_timeout_ms * 1_000_000;
    let (mut told_trust, mut told_unlock, mut told_wait) = (false, false, false);
    let mut sent = false;
    loop {
        if !sent {
            ld.pair_request(&record)?;
            sent = true;
        }
        match ld.pair_reply(3000) {
            Ok(lockdown::PairReply::Paired) => break,
            Ok(lockdown::PairReply::DialogPending) => {
                if !told_trust {
                    progress.note("usb: tap \"Trust\" on the iPhone (and enter its passcode if asked)");
                    told_trust = true;
                }
                drivers::platform::delay_us(1_000_000);
                sent = false;
            }
            Ok(lockdown::PairReply::Locked) => {
                if !told_unlock {
                    progress.note("usb: unlock the iPhone");
                    told_unlock = true;
                }
                drivers::platform::delay_us(2_000_000);
                sent = false;
            }
            Ok(lockdown::PairReply::Denied) => {
                progress.note("usb: the iPhone refused to trust this computer");
                return Err(Error::Unsupported);
            }
            Ok(lockdown::PairReply::Failed) => return Err(Error::Io),
            Err(Error::Timeout) => {
                if !told_wait {
                    progress.note("usb: waiting for the iPhone to answer (tap \"Trust\" / enter the passcode)");
                    told_wait = true;
                }
            }
            Err(e) => return Err(e),
        }
        if timed_out() {
            return Err(Error::Timeout);
        }
    }
    progress.note("usb: paired");

    let (ctrl, dev) = ld.into_channel().into_muxer().into_parts();
    let mut net = netdev::IphoneNet::open(ctrl, dev)?;
    progress.note("usb: tethering interface open, waiting for the hotspot link");
    let start = drivers::platform::now_ns();
    while !net.carrier() {
        if drivers::platform::now_ns() - start > 60_000_000_000 {
            progress.note("usb: no hotspot link (is Personal Hotspot on?)");
            return Err(Error::Timeout);
        }
        drivers::platform::delay_us(500_000);
    }
    Ok(net)
}
