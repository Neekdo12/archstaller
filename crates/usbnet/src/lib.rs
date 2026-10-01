//! USB Ethernet adapters as `hal::NetDevice`: Android phones in "USB tethering" mode and plain
//! USB-A Ethernet dongles. Supported: RNDIS (nearly all Android phones; `rndis.rs`), CDC-ECM,
//! CDC-NCM (`ncm.rs`) and the vendor-specific ASIX AX88179 (`ax88179.rs`, e.g. the Axagon
//! ADE-SG). For a phone there is no pairing: the user switches "USB tethering" on in the
//! settings, the phone re-enumerates with a network interface and runs DHCP on it.
//!
//! Written from the public CDC (ECM, NCM) and Microsoft RNDIS specifications and from the
//! register and framing layout the Linux `ax88179_178a` driver documents.
#![no_std]

extern crate alloc;

pub mod ax88179;
pub mod ncm;
pub mod rndis;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use drivers::dma::Dma;
use hal::{Error, NetDevice, Result};
use usb::desc::InterfaceDesc;
use usb::Device;

/// Apple devices are handled by `imobiledevice` (their tethering interface is different).
const APPLE_VENDOR: u16 = 0x05ac;

/// RNDIS control interfaces: wireless controller RNDIS, CDC ACM vendor (Linux gadget default), and
/// the Windows-compatible miscellaneous RNDIS class.
const RNDIS_CLASSES: [(u8, u8, u8); 3] = [(0xe0, 0x01, 0x03), (0x02, 0x02, 0xff), (0xef, 0x04, 0x01)];
const ECM_CLASS: (u8, u8, u8) = (0x02, 0x06, 0x00);
const NCM_CLASS: (u8, u8, u8) = (0x02, 0x0d, 0x00);

/// USB ids of AX88179-compatible gigabit adapters (ASIX and rebrands), from the Linux driver's
/// table. They run in their vendor-specific configuration (interface ff/ff/00).
const AX88179_IDS: [(u16, u16); 7] = [
    (0x0b95, 0x1790), // ASIX AX88179
    (0x2001, 0x4a00), // D-Link DUB-1312
    (0x0df6, 0x0072), // Sitecom LN-032
    (0x04e8, 0xa100), // Samsung
    (0x17ef, 0x304b), // Lenovo
    (0x050d, 0x0128), // Belkin B2B128
    (0x0930, 0x0a13), // Toshiba
];
const CDC_DATA_CLASS: u8 = 0x0a;

/// CDC functional descriptor subtypes.
const FD_UNION: u8 = 0x06;
const FD_ETHERNET: u8 = 0x0f;

const RX_BUF_RNDIS: usize = 16 * 1024;
const RX_BUF_ECM: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rndis,
    Ecm,
    Ncm,
    Ax88179,
}

struct Candidate {
    kind: Kind,
    comm: InterfaceDesc,
    /// The data interface setting that has the bulk endpoints.
    data: InterfaceDesc,
}

fn has_bulk(d: &Device, i: &InterfaceDesc) -> bool {
    d.bulk_in(i).is_some() && d.bulk_out(i).is_some()
}

/// Finds an RNDIS or ECM function in a configuration: the control interface plus the data
/// interface (named by the union descriptor, else any CDC data interface with bulk endpoints).
fn classify(d: &Device) -> Option<Candidate> {
    if d.vendor == APPLE_VENDOR {
        return None;
    }
    if AX88179_IDS.contains(&(d.vendor, d.product)) {
        if let Some(i) = d.interfaces.iter().find(|i| (i.class, i.subclass, i.protocol) == (0xff, 0xff, 0x00) && has_bulk(d, i)) {
            return Some(Candidate { kind: Kind::Ax88179, comm: i.clone(), data: i.clone() });
        }
    }
    for comm in d.interfaces.iter().filter(|i| i.alt == 0) {
        let triple = (comm.class, comm.subclass, comm.protocol);
        let kind = if RNDIS_CLASSES.contains(&triple) {
            Kind::Rndis
        } else if triple == ECM_CLASS {
            Kind::Ecm
        } else if triple == NCM_CLASS {
            Kind::Ncm
        } else {
            continue;
        };
        let named = comm.cdc_functional(FD_UNION).and_then(|u| u.get(4).copied());
        let data = d.interfaces.iter().find(|i| {
            has_bulk(d, i) && i.class == CDC_DATA_CLASS && named.is_none_or(|n| i.num == n)
        });
        if let Some(data) = data {
            return Some(Candidate { kind, comm: comm.clone(), data: data.clone() });
        }
    }
    None
}

/// Takes a USB Ethernet function (RNDIS, CDC-ECM, CDC-NCM or an AX88179) from `scan`, switches
/// it on and returns it as a NIC. A known vendor-specific chip is preferred over the class
/// functions it may also offer. `Err(Unsupported)` when `scan` holds no such device.
pub fn probe(scan: &mut usb::Scan) -> Result<UsbEth> {
    let (ctrl, dev) = scan.take(
        |d| classify(d).map(|c| c.comm),
        |d| classify(d).is_some_and(|c| c.kind == Kind::Ax88179),
    )?;
    let cand = classify(&dev).ok_or(Error::Unsupported)?;
    hal::log!(
        "usb: {} function on interface {}, data interface {} alt {}",
        match cand.kind {
            Kind::Rndis => "RNDIS",
            Kind::Ecm => "CDC-ECM",
            Kind::Ncm => "CDC-NCM",
            Kind::Ax88179 => "AX88179",
        },
        cand.comm.num,
        cand.data.num,
        cand.data.alt
    );
    UsbEth::open(ctrl, dev, cand)
}

pub struct UsbEth {
    ctrl: usb::Controller,
    dev: Device,
    kind: Kind,
    ep_in: u8,
    ep_out: u8,
    mps_out: usize,
    mac: [u8; 6],
    rx: Dma,
    rx_len: usize,
    rx_pending: bool,
    /// Frames already unpacked from a multi-packet RNDIS transfer.
    queue: VecDeque<Vec<u8>>,
    /// CDC-NCM transmit parameters and block sequence number.
    ntb: Option<ncm::Params>,
    seq: u16,
    /// Frames sent and transfers received so far (only the first few are logged).
    tx_n: u32,
    rx_n: u32,
}

impl UsbEth {
    fn open(mut ctrl: usb::Controller, dev: Device, cand: Candidate) -> Result<UsbEth> {
        let ep_in = dev.bulk_in(&cand.data).ok_or(Error::Unsupported)?;
        let ep_out = dev.bulk_out(&cand.data).ok_or(Error::Unsupported)?;
        let mps_out = cand.data.endpoints.iter().find(|e| e.addr == ep_out).map_or(512, |e| e.max_packet as usize);
        let mut ntb = None;
        let comm = cand.comm.num as u16;
        let (mac, rx_len) = if cand.kind == Kind::Ax88179 {
            // take() already opened this interface's bulk endpoints.
            ax_bring_up(&mut ctrl, &dev)?
        } else {
            if cand.data.alt != 0 {
                ctrl.set_interface(&dev, cand.data.num, cand.data.alt)?;
            }
            let eps: Vec<usb::desc::EndpointDesc> = cand.data.endpoints.iter().filter(|e| e.is_bulk()).cloned().collect();
            ctrl.configure_endpoints(&dev, &eps).map_err(|e| {
                hal::info!("usb: configuring the data endpoints failed: {e:?}");
                e
            })?;
            match cand.kind {
                Kind::Rndis => {
                    let init = command(&mut ctrl, &dev, comm, &rndis::init_msg(1, RX_BUF_RNDIS as u32), 1)?;
                    if init.status != rndis::STATUS_SUCCESS {
                        hal::info!("usb: RNDIS initialize failed, status {:#x}", init.status);
                        return Err(Error::Io);
                    }
                    let q = command(&mut ctrl, &dev, comm, &rndis::query_msg(2, rndis::OID_802_3_PERMANENT_ADDRESS, 48), 2)?;
                    if q.status != rndis::STATUS_SUCCESS || q.info.len() < 6 {
                        hal::info!("usb: RNDIS MAC query failed, status {:#x}, {} bytes", q.status, q.info.len());
                        return Err(Error::Io);
                    }
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(&q.info[..6]);
                    let set = rndis::set_msg(3, rndis::OID_GEN_CURRENT_PACKET_FILTER, &rndis::PACKET_FILTER.to_le_bytes());
                    let s = command(&mut ctrl, &dev, comm, &set, 3)?;
                    if s.status != rndis::STATUS_SUCCESS {
                        hal::info!("usb: RNDIS set packet filter failed, status {:#x}", s.status);
                        return Err(Error::Io);
                    }
                    (mac, RX_BUF_RNDIS)
                }
                Kind::Ecm => {
                    let mac = ecm_mac(&mut ctrl, &dev, &cand.comm)?;
                    // Receive directed, broadcast and multicast frames. A device that does not
                    // implement the request may stall it; ignore that.
                    let _ = ctrl.class_out(&dev, 0x43, 0x001e, comm, &[]);
                    (mac, RX_BUF_ECM)
                }
                Kind::Ncm => {
                    let mut raw = [0u8; 28];
                    let n = ctrl.class_in(&dev, 0x80, 0, comm, &mut raw)?; // GET_NTB_PARAMETERS
                    let p = ncm::parse_params(&raw[..n]).map_err(|e| {
                        hal::info!("usb: NCM parameters unusable ({n} bytes): {e:?}");
                        e
                    })?;
                    hal::log!("usb: NCM {:?}", p);
                    let mac = ecm_mac(&mut ctrl, &dev, &cand.comm)?;
                    // Cap the size of blocks the device sends to what the receive buffer holds.
                    let rx = (p.in_max as usize).clamp(2048, 65536);
                    if p.in_max as usize != rx {
                        let _ = ctrl.class_out(&dev, 0x86, 0, comm, &(rx as u32).to_le_bytes()); // SET_NTB_INPUT_SIZE
                    }
                    let _ = ctrl.class_out(&dev, 0x43, 0x001e, comm, &[]);
                    ntb = Some(p);
                    (mac, rx)
                }
                Kind::Ax88179 => unreachable!(),
            }
        };
        hal::log!("usb: tethering NIC mac {:02x?}", mac);
        Ok(UsbEth {
            ctrl,
            dev,
            kind: cand.kind,
            ep_in,
            ep_out,
            mps_out,
            mac,
            rx: Dma::for_transfer(rx_len),
            tx_n: 0,
            rx_n: 0,
            rx_len,
            rx_pending: false,
            queue: VecDeque::new(),
            ntb,
            seq: 0,
        })
    }
}

/// Sends an RNDIS control message and waits for its completion.
fn command(ctrl: &mut usb::Controller, dev: &Device, comm: u16, msg: &[u8], id: u32) -> Result<rndis::Reply> {
    let want = u32::from_le_bytes(msg[0..4].try_into().unwrap()) | rndis::CMPLT;
    ctrl.class_out(dev, 0x00, 0, comm, msg).map_err(|e| {
        hal::info!("usb: SEND_ENCAPSULATED_COMMAND failed: {e:?}");
        e
    })?; // SEND_ENCAPSULATED_COMMAND
    for _ in 0..100 {
        drivers::platform::delay_us(20_000);
        let mut buf = [0u8; 1025];
        let n = ctrl.class_in(dev, 0x01, 0, comm, &mut buf).map_err(|e| {
            hal::info!("usb: GET_ENCAPSULATED_RESPONSE failed: {e:?}");
            e
        })?; // GET_ENCAPSULATED_RESPONSE
        if n < 12 {
            continue; // nothing there yet
        }
        let Ok(r) = rndis::parse_reply(&buf[..n]) else { continue };
        if r.msg_type == rndis::MSG_KEEPALIVE {
            let _ = ctrl.class_out(dev, 0x00, 0, comm, &rndis::keepalive_cmplt(r.request_id));
            continue;
        }
        if r.msg_type == want && r.request_id == id {
            return Ok(r);
        }
    }
    hal::info!("usb: RNDIS message {:#x} got no answer", want & !rndis::CMPLT);
    Err(Error::Timeout)
}

fn ax_write(ctrl: &mut usb::Controller, dev: &Device, reg: u16, data: &[u8]) -> Result<()> {
    ctrl.vendor_out(dev, ax88179::ACCESS_MAC, reg, data.len() as u16, data)
}

fn ax_read(ctrl: &mut usb::Controller, dev: &Device, reg: u16, buf: &mut [u8]) -> Result<()> {
    let want = buf.len();
    if ctrl.vendor_in(dev, ax88179::ACCESS_MAC, reg, want as u16, buf)? < want {
        return Err(Error::Io);
    }
    Ok(())
}

fn ax_phy_read(ctrl: &mut usb::Controller, dev: &Device, reg: u16) -> Result<u16> {
    let mut b = [0u8; 2];
    if ctrl.vendor_in(dev, ax88179::ACCESS_PHY, ax88179::PHY_ID, reg, &mut b)? < 2 {
        return Err(Error::Io);
    }
    Ok(u16::from_le_bytes(b))
}

/// AX88179 reset and link bring-up, in the order the Linux driver does it. Returns the MAC and
/// the receive buffer size matching the negotiated speed.
fn ax_bring_up(ctrl: &mut usb::Controller, dev: &Device) -> Result<([u8; 6], usize)> {
    use ax88179::*;
    let step = |what: &'static str| move |e: Error| {
        hal::info!("usb: AX88179 {what} failed: {e:?}");
        e
    };
    // Power-cycle the PHY, then select the clock.
    ax_write(ctrl, dev, PHYPWR_RSTCTL, &0u16.to_le_bytes()).map_err(step("PHY power"))?;
    ax_write(ctrl, dev, PHYPWR_RSTCTL, &PHYPWR_IPRL.to_le_bytes()).map_err(step("PHY reset"))?;
    drivers::platform::delay_us(500_000);
    ax_write(ctrl, dev, CLK_SELECT, &[CLK_ACS_BCS]).map_err(step("clock select"))?;
    drivers::platform::delay_us(200_000);
    let mut mac = [0u8; 6];
    ax_read(ctrl, dev, NODE_ID, &mut mac).map_err(step("MAC read"))?;
    ax_write(ctrl, dev, RX_BULKIN_QCTRL, &[7, 0x4f, 0, 0x12, 0xff]).map_err(step("RX queue"))?;
    ax_write(ctrl, dev, PAUSE_WATERLVL_LOW, &[0x34])?;
    ax_write(ctrl, dev, PAUSE_WATERLVL_HIGH, &[0x52])?;
    ax_write(ctrl, dev, RXCOE_CTL, &[COE_ALL])?;
    ax_write(ctrl, dev, TXCOE_CTL, &[COE_ALL])?;
    ax_write(ctrl, dev, RX_CTL, &RX_CTL_RUN.to_le_bytes()).map_err(step("RX control"))?;
    ax_write(ctrl, dev, MONITOR_MOD, &[MONITOR_DEFAULT])?;
    let medium = MEDIUM_RECEIVE_EN | MEDIUM_TXFLOW | MEDIUM_RXFLOW | MEDIUM_FULL_DUPLEX | MEDIUM_GIGA;
    ax_write(ctrl, dev, MEDIUM_STATUS_MODE, &medium.to_le_bytes()).map_err(step("medium mode"))?;
    // Restart auto-negotiation (BMCR: auto-negotiation enable + restart).
    ctrl.vendor_out(dev, ACCESS_PHY, PHY_ID, PHY_BMCR, &0x1200u16.to_le_bytes()).map_err(step("PHY restart"))?;

    let mut told = false;
    let mut physr = 0u16;
    for _ in 0..150 {
        physr = ax_phy_read(ctrl, dev, PHY_PHYSR).map_err(step("PHY status"))?;
        if physr & PHYSR_LINK != 0 {
            break;
        }
        if !told {
            hal::info!("usb: AX88179 waiting for the Ethernet link (is a cable plugged in?)");
            told = true;
        }
        drivers::platform::delay_us(100_000);
    }
    if physr & PHYSR_LINK == 0 {
        hal::info!("usb: AX88179 has no Ethernet link");
        return Err(Error::Timeout);
    }
    let mut link_sts = [0u8; 1];
    ax_read(ctrl, dev, PHYSICAL_LINK_STATUS, &mut link_sts).map_err(step("link status"))?;
    let (medium, queue, rx_len) = link_settings(physr, link_sts[0]);
    ax_write(ctrl, dev, RX_BULKIN_QCTRL, &queue).map_err(step("RX queue"))?;
    ax_write(ctrl, dev, MEDIUM_STATUS_MODE, &medium.to_le_bytes()).map_err(step("medium mode"))?;
    hal::log!("usb: AX88179 link up, PHY status {physr:#06x}, USB link {:#04x}, medium {medium:#06x}", link_sts[0]);
    Ok((mac, rx_len))
}

/// MAC address of a CDC-ECM function: a 12-digit hex string named by the Ethernet
/// functional descriptor.
fn ecm_mac(ctrl: &mut usb::Controller, dev: &Device, comm: &InterfaceDesc) -> Result<[u8; 6]> {
    let idx = comm.cdc_functional(FD_ETHERNET).and_then(|d| d.get(3).copied()).filter(|i| *i != 0).ok_or(Error::Unsupported)?;
    let s: String = ctrl.string_descriptor(dev, idx).ok_or(Error::Io)?;
    let b = s.as_bytes();
    if b.len() < 12 {
        return Err(Error::Io);
    }
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut mac = [0u8; 6];
    for (i, m) in mac.iter_mut().enumerate() {
        *m = hex(b[2 * i]).ok_or(Error::Io)? << 4 | hex(b[2 * i + 1]).ok_or(Error::Io)?;
    }
    Ok(mac)
}

impl NetDevice for UsbEth {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Rndis => "usb-rndis",
            Kind::Ecm => "usb-ecm",
            Kind::Ncm => "usb-ncm",
            Kind::Ax88179 => "usb-ax88179",
        }
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&mut self) -> bool {
        if self.kind == Kind::Ax88179 {
            return ax_phy_read(&mut self.ctrl, &self.dev, ax88179::PHY_PHYSR).is_ok_and(|v| v & ax88179::PHYSR_LINK != 0);
        }
        true
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<()> {
        let wrapped;
        let data: &[u8] = match self.kind {
            Kind::Rndis => {
                wrapped = rndis::wrap_frame(frame);
                &wrapped
            }
            Kind::Ecm => frame,
            Kind::Ncm => {
                let p = self.ntb.as_ref().ok_or(Error::Io)?;
                self.seq = self.seq.wrapping_add(1);
                wrapped = ncm::build_ntb(frame, self.seq, p)?;
                &wrapped
            }
            Kind::Ax88179 => {
                wrapped = ax88179::tx_frame(frame, self.mps_out);
                &wrapped
            }
        };
        if self.tx_n < 4 {
            hal::log!("usb: tx frame {} bytes ({} in the transfer)", frame.len(), data.len());
        }
        self.tx_n += 1;
        self.ctrl.bulk_write(&self.dev, self.ep_out, data)?;
        // A transfer ending on a packet boundary needs a zero-length packet to end it (the
        // AX88179 header asks the chip to pad instead).
        if self.kind != Kind::Ax88179 && data.len() % self.mps_out == 0 {
            self.ctrl.bulk_write_zlp(&self.dev, self.ep_out)?;
        }
        Ok(())
    }

    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        if let Some(f) = self.queue.pop_front() {
            let n = f.len().min(buf.len());
            buf[..n].copy_from_slice(&f[..n]);
            return Some(n);
        }
        if !self.rx_pending {
            self.ctrl.bulk_in_submit(&self.dev, self.ep_in, &self.rx, self.rx_len).ok()?;
            self.rx_pending = true;
        }
        let n = match self.ctrl.bulk_in_poll(&self.dev, self.ep_in, self.rx_len)? {
            Ok(n) => n,
            Err(_) => {
                self.rx_pending = false;
                return None;
            }
        };
        self.rx_pending = false;
        let data = &self.rx.as_slice()[..n];
        let first = self.rx_n < 6;
        self.rx_n += 1;
        match self.kind {
            Kind::Ecm => {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                Some(len)
            }
            Kind::Rndis | Kind::Ncm | Kind::Ax88179 => {
                match self.kind {
                    Kind::Rndis => self.queue.extend(rndis::unwrap_frames(data)),
                    Kind::Ncm => self.queue.extend(ncm::parse_ntb(data)),
                    _ => self.queue.extend(ax88179::parse_rx(data)),
                }
                if first {
                    hal::log!(
                        "usb: rx transfer {n} bytes -> {} frames{}",
                        self.queue.len(),
                        if self.queue.is_empty() { alloc::format!(", last bytes {:02x?}", &data[n.saturating_sub(8)..]) } else { String::new() }
                    );
                }
                let f = self.queue.pop_front()?;
                let len = f.len().min(buf.len());
                buf[..len].copy_from_slice(&f[..len]);
                Some(len)
            }
        }
    }
}
