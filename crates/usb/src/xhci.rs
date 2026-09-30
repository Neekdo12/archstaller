//! xHCI host controller: one command ring + one event ring, per-endpoint transfer rings,
//! all polled (no interrupts). Only what is needed for control and bulk transfers to
//! directly attached devices.
use drivers::dma::Dma;
use drivers::mmio::Mmio;
use drivers::platform;
use hal::{Error, Result};

// Capability registers (from BAR base).
const HCSPARAMS1: usize = 0x04;
const HCSPARAMS2: usize = 0x08;
const HCCPARAMS1: usize = 0x10;
const DBOFF: usize = 0x14;
const RTSOFF: usize = 0x18;

// Operational registers (from BAR + CAPLENGTH).
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const PAGESIZE: usize = 0x08;
const CRCR: usize = 0x18;
const DCBAAP: usize = 0x30;
const CONFIG: usize = 0x38;
const PORTSC: usize = 0x400; // + 0x10 * (port - 1)

const CMD_RS: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;

const STS_HCH: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;

const PSC_CCS: u32 = 1 << 0;
const PSC_PED: u32 = 1 << 1;
const PSC_PR: u32 = 1 << 4;
const PSC_PP: u32 = 1 << 9;
const PSC_PRC: u32 = 1 << 21;
const PSC_RWS_PRESERVE: u32 = PSC_PP;

// Runtime registers, interrupter 0 (from BAR + RTSOFF + 0x20).
const IMAN: usize = 0x00;
const ERSTSZ: usize = 0x08;
const ERSTBA: usize = 0x10;
const ERDP: usize = 0x18;
const ERDP_EHB: u64 = 1 << 3;

// TRB types.
pub(crate) const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_EP: u32 = 12;
const TRB_EVALUATE_CTX: u32 = 13;
const TRB_STOP_EP: u32 = 15;
const TRB_SET_TR_DEQUEUE: u32 = 16;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_COMPLETE: u32 = 33;

const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6;
const TRB_DIR_IN: u32 = 1 << 16;

const CC_SUCCESS: u8 = 1;
const CC_SHORT_PACKET: u8 = 13;

const RING_TRBS: usize = 64; // last one is the link TRB
const MAX_TRB_LEN: usize = 16 * 1024;

/// One transfer ring: producer side only, cycle bit managed per xHCI 4.9.
#[doc(hidden)] // public for host-side unit tests
pub struct Ring {
    buf: Dma,
    n: usize,
    enq: usize,
    pcs: bool,
}

impl Ring {
    pub fn new(n: usize) -> Ring {
        Ring { buf: Dma::new(n * 16, 64), n, enq: 0, pcs: true }
    }

    fn mmio(&self, idx: usize) -> Mmio {
        Mmio::new(self.buf.as_ptr()).offset(idx * 16)
    }

    /// Appends a TRB; `control`'s cycle bit is set from the producer cycle state.
    /// Returns the physical address of the written TRB.
    pub fn push(&mut self, param: u64, status: u32, control: u32) -> u64 {
        assert!(self.enq < self.n - 1, "transfer ring full");
        let m = self.mmio(self.enq);
        m.write64(0, param);
        m.write32(8, status);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        m.write32(12, (control & !1) | self.pcs as u32);
        let phys = self.buf.phys_at(self.enq * 16);
        self.enq += 1;
        if self.enq == self.n - 1 {
            let m = self.mmio(self.enq);
            m.write64(0, self.buf.phys());
            m.write32(8, 0);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            m.write32(12, (TRB_LINK << 10) | (1 << 1) | self.pcs as u32); // TC: toggle cycle
            self.enq = 0;
            self.pcs = !self.pcs;
        }
        phys
    }

    pub fn phys(&self) -> u64 {
        self.buf.phys()
    }

    #[doc(hidden)] // raw ring memory, for host-side unit tests
    pub fn memory(&self) -> &[u8] {
        self.buf.as_slice()
    }

    /// Rewinds the producer (after a stopped endpoint is reset with Set TR Dequeue).
    fn reset(&mut self) {
        self.buf.as_mut_slice().fill(0);
        self.enq = 0;
        self.pcs = true;
    }
}

struct EventRing {
    buf: Dma,
    n: usize,
    deq: usize,
    ccs: bool,
    erst: Dma,
}

pub(crate) struct Event {
    pub param: u64,
    pub code: u8,
    pub residual: u32,
    pub slot: u8,
    pub dci: u8,
    pub is_command: bool,
    pub is_transfer: bool,
}

impl EventRing {
    fn new(n: usize) -> EventRing {
        let erst = Dma::new(16, 64);
        let buf = Dma::new(n * 16, 64);
        let e = Mmio::new(erst.as_ptr());
        e.write64(0, buf.phys());
        e.write16(8, n as u16);
        EventRing { buf, n, deq: 0, ccs: true, erst }
    }

    fn next(&mut self, rt0: &Mmio) -> Option<Event> {
        let m = Mmio::new(self.buf.as_ptr()).offset(self.deq * 16);
        let control = m.read32(12);
        if (control & 1 != 0) != self.ccs {
            return None;
        }
        let ev = Event {
            param: m.read64(0),
            code: (m.read32(8) >> 24) as u8,
            residual: m.read32(8) & 0xff_ffff,
            slot: (control >> 24) as u8,
            dci: ((control >> 16) & 0x1f) as u8,
            is_command: (control >> 10) & 0x3f == TRB_COMMAND_COMPLETE,
            is_transfer: (control >> 10) & 0x3f == TRB_TRANSFER_EVENT,
        };
        self.deq += 1;
        if self.deq == self.n {
            self.deq = 0;
            self.ccs = !self.ccs;
        }
        rt0.write64(ERDP, self.buf.phys_at(self.deq * 16) | ERDP_EHB);
        Some(ev)
    }
}

/// Per-slot state the controller must keep alive while a device is addressed.
pub(crate) struct Slot {
    /// Device context; never read from the host side, but must stay allocated
    /// (its address is programmed into the DCBAA).
    #[allow(dead_code)]
    pub out_ctx: Dma,  // 32 * ctx_size
    pub in_ctx: Dma,   // 33 * ctx_size
    pub rings: alloc::vec::Vec<Ring>, // indexed by DCI - 1
    pub speed: u8,
    pub port: u8,
}

pub struct Controller {
    op: Mmio,
    rt0: Mmio,
    db: Mmio,
    ctx_size: usize,
    max_ports: u8,
    cmd: Ring,
    evt: EventRing,
    dcbaa: Dma,
    _scratch: Option<(Dma, alloc::vec::Vec<Dma>)>,
    pub(crate) slots: alloc::vec::Vec<Option<Slot>>,
}

impl Controller {
    pub fn new(mmio: Mmio) -> Result<Controller> {
        let caplen = mmio.read8(0) as usize;
        let hcs1 = mmio.read32(HCSPARAMS1);
        let max_slots = (hcs1 & 0xff).min(16);
        let max_ports = (hcs1 >> 24) as u8;
        let hcs2 = mmio.read32(HCSPARAMS2);
        let max_scratch = (((hcs2 >> 27) & 0x1f) << 5) | ((hcs2 >> 21) & 0x1f);
        let csz64 = mmio.read32(HCCPARAMS1) & (1 << 2) != 0;
        let ctx_size = if csz64 { 64 } else { 32 };
        let op = mmio.offset(caplen);
        let rt0 = mmio.offset((mmio.read32(RTSOFF) & !0x1f) as usize + 0x20);
        let db = mmio.offset((mmio.read32(DBOFF) & !3) as usize);

        // Halt, then reset.
        op.write32(USBCMD, op.read32(USBCMD) & !CMD_RS);
        platform::wait_until(1000, || op.read32(USBSTS) & STS_HCH != 0)?;
        op.write32(USBCMD, CMD_HCRST);
        platform::wait_until(1000, || op.read32(USBCMD) & CMD_HCRST == 0)?;
        platform::wait_until(1000, || op.read32(USBSTS) & STS_CNR == 0)?;

        let page = 4096usize << (op.read32(PAGESIZE) & 0xffff).trailing_zeros();
        let dcbaa = Dma::new((max_slots as usize + 1) * 8, 64);
        let scratch = if max_scratch > 0 {
            let arr = Dma::new(max_scratch as usize * 8, page);
            let bufs: alloc::vec::Vec<Dma> =
                (0..max_scratch).map(|_| Dma::new(page, page)).collect();
            for (i, b) in bufs.iter().enumerate() {
                Mmio::new(arr.as_ptr()).write64(i * 8, b.phys());
            }
            Mmio::new(dcbaa.as_ptr()).write64(0, arr.phys());
            Some((arr, bufs))
        } else {
            None
        };

        let cmd = Ring::new(RING_TRBS);
        let evt = EventRing::new(RING_TRBS);

        op.write32(CONFIG, max_slots);
        op.write64(DCBAAP, dcbaa.phys());
        // Command ring, cycle state 1.
        op.write64(CRCR, cmd.phys() | 1);
        // Event ring: one segment, interrupts masked (we poll).
        rt0.write32(IMAN, rt0.read32(IMAN)); // clear pending IP, leave IE=0
        rt0.write32(ERSTSZ, 1);
        rt0.write64(ERSTBA, evt.erst.phys());
        rt0.write64(ERDP, evt.buf.phys() | ERDP_EHB);

        op.write32(USBCMD, CMD_RS);
        platform::wait_until(1000, || op.read32(USBSTS) & STS_HCH == 0)?;

        Ok(Controller {
            op,
            rt0,
            db,
            ctx_size,
            max_ports,
            cmd,
            evt,
            dcbaa,
            _scratch: scratch,
            slots: (0..max_slots).map(|_| None).collect(),
        })
    }

    fn portsc(&self, port: u8) -> u32 {
        self.op.read32(PORTSC + 0x10 * (port as usize - 1))
    }

    /// Ports with an enabled device: `(port, speed)`; speed 1=low 2=full 3=high 4+=super.
    pub fn enabled_ports(&mut self) -> alloc::vec::Vec<(u8, u8)> {
        let mut out = alloc::vec::Vec::new();
        for port in 1..=self.max_ports {
            let p = self.portsc(port);
            if p & PSC_CCS == 0 {
                continue;
            }
            if p & PSC_PP == 0 {
                self.op.write32(PORTSC + 0x10 * (port as usize - 1), PSC_PP);
                let _ = platform::wait_until(500, || self.portsc(port) & PSC_PP != 0);
            }
            let mut p = self.portsc(port);
            if p & PSC_PED == 0 {
                // USB2 ports need a hot reset; USB3 ports link-train on their own, and a
                // hot reset attempt on one is ignored by the xHC (PR has no effect on a
                // port that is already past polling, so this is harmless either way).
                let off = PORTSC + 0x10 * (port as usize - 1);
                let base = self.portsc(port) & PSC_RWS_PRESERVE;
                self.op.write32(off, base | PSC_PR);
                if platform::wait_until(500, || self.portsc(port) & PSC_PRC != 0).is_ok() {
                    let base = self.portsc(port) & PSC_RWS_PRESERVE;
                    self.op.write32(off, base | PSC_PRC);
                }
                let _ = platform::wait_until(500, || self.portsc(port) & PSC_PED != 0);
                p = self.portsc(port);
            }
            if p & PSC_PED != 0 {
                out.push((port, ((p >> 10) & 0xf) as u8));
            }
        }
        out
    }

    /// Runs a command TRB and waits for its completion event.
    /// Returns `(slot_id, completion_code)`.
    pub(crate) fn command(&mut self, param: u64, control: u32) -> Result<(u8, u8)> {
        let trb = self.cmd.push(param, 0, control);
        self.db.write32(0, 0);
        let deadline = platform::now_ns() + 5_000_000_000;
        loop {
            if let Some(ev) = self.evt.next(&self.rt0) {
                if ev.is_command && ev.param == trb {
                    return Ok((ev.slot, ev.code));
                }
                // Port status change or stray event: consumed and ignored.
            } else if platform::now_ns() > deadline {
                return Err(Error::Timeout);
            } else {
                core::hint::spin_loop();
            }
        }
    }

    /// Waits for a transfer event for (`slot`, `dci`) up to `timeout_ms`.
    /// Returns `(completion_code, residual_bytes)`.
    pub(crate) fn wait_transfer(&mut self, slot: u8, dci: u8, timeout_ms: u64) -> Result<(u8, u32)> {
        let deadline = platform::now_ns() + timeout_ms * 1_000_000;
        loop {
            if let Some(ev) = self.evt.next(&self.rt0) {
                if ev.is_transfer && ev.slot == slot && ev.dci == dci {
                    return Ok((ev.code, ev.residual));
                }
            } else if platform::now_ns() > deadline {
                return Err(Error::Timeout);
            } else {
                core::hint::spin_loop();
            }
        }
    }

    pub(crate) fn enable_slot(&mut self) -> Result<u8> {
        let (slot, code) = self.command(0, TRB_ENABLE_SLOT << 10)?;
        if code != CC_SUCCESS || slot == 0 {
            return Err(Error::Io);
        }
        Ok(slot)
    }

    /// Default EP0 max packet size guess by port speed (fixed up after the first
    /// device-descriptor read via Evaluate Context).
    fn ep0_mps(speed: u8) -> u16 {
        match speed {
            1 => 8,              // low
            2 | 3 => 64,         // full / high
            _ => 512,            // super and up
        }
    }

    /// Fills the input context for `slot`. With no `eps` this is the Address Device form
    /// (slot + EP0 contexts). With `eps` (`(dci, xHCI endpoint type, max packet)`) it is the
    /// Configure Endpoint form: only the listed endpoints are added, EP0 is left alone.
    pub(crate) fn build_input_ctx(&mut self, slot: u8, config_value: u8, eps: &[(u8, u32, u32)]) {
        let cs = self.ctx_size;
        let Some(s) = &mut self.slots[(slot - 1) as usize] else { return };
        let in_ctx = Mmio::new(s.in_ctx.as_ptr());
        s.in_ctx.as_mut_slice().fill(0);
        let mut add = 1u32; // slot context
        let mut entries = 1u8;
        if eps.is_empty() {
            add |= 1 << 1;
        }
        for &(dci, _, _) in eps {
            add |= 1 << dci;
            entries = entries.max(dci);
        }
        in_ctx.write32(4, add); // add flags (drop flags stay 0)
        if !eps.is_empty() {
            in_ctx.write32(7 * 4, config_value as u32);
        }
        // Slot context.
        let sctx = in_ctx.offset(cs);
        sctx.write32(0, ((entries as u32) << 27) | ((s.speed as u32) << 20));
        sctx.write32(4, (s.port as u32) << 16);
        if eps.is_empty() {
            let ectx = in_ctx.offset(cs * 2);
            ectx.write32(4, (3 << 1) | (4 << 3) | ((Self::ep0_mps(s.speed) as u32) << 16));
            ectx.write64(8, s.rings[0].phys() | 1); // TR dequeue + DCS
            ectx.write32(16, 8);
        }
        for &(dci, ty, mps) in eps {
            let ectx = in_ctx.offset(cs * (1 + dci as usize));
            ectx.write32(0, 0);
            ectx.write32(4, (3 << 1) | (ty << 3) | (mps << 16));
            ectx.write64(8, s.rings[(dci - 1) as usize].phys() | 1);
            ectx.write32(16, 1024);
        }
    }

    pub(crate) fn address_device(&mut self, slot: u8) -> Result<()> {
        let in_ctx = self.slots[(slot - 1) as usize].as_ref().unwrap().in_ctx.phys();
        let (got, code) = self.command(in_ctx, (TRB_ADDRESS_DEVICE << 10) | (slot as u32) << 24)?;
        if code != CC_SUCCESS || got != slot {
            return Err(Error::Io);
        }
        Ok(())
    }

    pub(crate) fn configure(&mut self, slot: u8, trb_type: u32) -> Result<()> {
        let in_ctx = self.slots[(slot - 1) as usize].as_ref().unwrap().in_ctx.phys();
        let (_, code) = self.command(in_ctx, (trb_type << 10) | (slot as u32) << 24)?;
        if code != CC_SUCCESS {
            return Err(Error::Io);
        }
        Ok(())
    }

    /// Fixes EP0's max packet size after reading the device descriptor's byte 7.
    pub(crate) fn set_ep0_mps(&mut self, slot: u8, mps: u16) -> Result<()> {
        let cs = self.ctx_size;
        let Some(s) = &mut self.slots[(slot - 1) as usize] else { return Err(Error::InvalidArgument) };
        let in_ctx = Mmio::new(s.in_ctx.as_ptr());
        s.in_ctx.as_mut_slice().fill(0);
        in_ctx.write32(4, 1 << 1); // add DCI 1 only
        let sctx = in_ctx.offset(cs);
        sctx.write32(0, (1u32 << 27) | ((s.speed as u32) << 20));
        sctx.write32(4, (s.port as u32) << 16);
        let ectx = in_ctx.offset(cs * 2);
        ectx.write32(4, (3 << 1) | (4 << 3) | ((mps as u32) << 16));
        self.configure(slot, TRB_EVALUATE_CTX)
    }

    /// Adds a device on `port` with `speed`: enable slot, build contexts, address it.
    pub(crate) fn add_device(&mut self, port: u8, speed: u8) -> Result<u8> {
        let slot = self.enable_slot()?;
        let result = (|| {
            let out_ctx = Dma::new(32 * self.ctx_size, 64);
            let in_ctx = Dma::new(33 * self.ctx_size, 64);
            Mmio::new(self.dcbaa.as_ptr()).write64(slot as usize * 8, out_ctx.phys());
            let rings = alloc::vec![Ring::new(RING_TRBS)];
            self.slots[(slot - 1) as usize] = Some(Slot { out_ctx, in_ctx, rings, speed, port });
            self.build_input_ctx(slot, 0, &[]);
            self.address_device(slot)
        })();
        if result.is_err() {
            self.slots[(slot - 1) as usize] = None;
        }
        result.map(|_| slot)
    }

    /// Creates transfer rings for the given bulk endpoints and issues Configure Endpoint.
    pub(crate) fn open_bulk_endpoints(
        &mut self,
        slot: u8,
        config_value: u8,
        eps: &[crate::desc::EndpointDesc],
    ) -> Result<()> {
        let max_dci = eps.iter().map(|e| e.dci()).max().unwrap_or(1);
        {
            let Some(s) = &mut self.slots[(slot - 1) as usize] else { return Err(Error::InvalidArgument) };
            while (s.rings.len() as u8) < max_dci {
                s.rings.push(Ring::new(RING_TRBS));
            }
        }
        let list: alloc::vec::Vec<(u8, u32, u32)> = eps
            .iter()
            .map(|ep| (ep.dci(), if ep.is_in() { 6u32 } else { 2u32 }, ep.max_packet as u32))
            .collect();
        self.build_input_ctx(slot, config_value, &list);
        self.configure(slot, TRB_CONFIGURE_EP)
    }

    /// Pushes a TD (one or more chained TRBs) on the endpoint ring and waits for completion.
    /// Returns the number of bytes actually transferred.
    pub(crate) fn transfer(
        &mut self,
        slot: u8,
        dci: u8,
        buf_phys: u64,
        len: usize,
        timeout_ms: u64,
    ) -> Result<usize> {
        self.submit_normal(slot, dci, buf_phys, len)?;
        self.db.write32(slot as usize * 4, dci as u32);
        match self.wait_transfer(slot, dci, timeout_ms) {
            Ok((code, residual)) => {
                if code != CC_SUCCESS && code != CC_SHORT_PACKET {
                    return Err(Error::Io);
                }
                Ok(len - residual as usize)
            }
            Err(e) => {
                let _ = self.reset_endpoint(slot, dci);
                Err(e)
            }
        }
    }

    fn submit_normal(&mut self, slot: u8, dci: u8, buf_phys: u64, len: usize) -> Result<()> {
        let Some(s) = &mut self.slots[(slot - 1) as usize] else { return Err(Error::InvalidArgument) };
        let ring = &mut s.rings[(dci - 1) as usize];
        let mut done = 0usize;
        while done < len {
            let chunk = (len - done).min(MAX_TRB_LEN);
            let last = done + chunk == len;
            let control = (TRB_NORMAL << 10) | if last { TRB_IOC } else { 1 << 4 }; // chain bit
            ring.push(buf_phys + done as u64, chunk as u32, control);
            done += chunk;
        }
        Ok(())
    }

    /// Pushes a full control transfer (setup + optional data + status) on EP0.
    pub(crate) fn control(
        &mut self,
        slot: u8,
        setup: [u8; 8],
        data_phys: u64,
        data_len: usize,
        data_in: bool,
    ) -> Result<usize> {
        {
            let Some(s) = &mut self.slots[(slot - 1) as usize] else { return Err(Error::InvalidArgument) };
            let ring = &mut s.rings[0];
            let trt = if data_len == 0 { 0 } else if data_in { 3 } else { 2 };
            let sp = u64::from_le_bytes(setup);
            ring.push(sp, 8, (TRB_SETUP << 10) | TRB_IDT | (trt << 16));
            if data_len > 0 {
                let dir = if data_in { TRB_DIR_IN } else { 0 };
                ring.push(data_phys, data_len as u32, (TRB_DATA << 10) | dir);
            }
            let status_dir = if data_len == 0 || !data_in { TRB_DIR_IN } else { 0 };
            ring.push(0, 0, (TRB_STATUS << 10) | TRB_IOC | status_dir);
        }
        self.db.write32(slot as usize * 4, 1);
        let (code, residual) = self.wait_transfer(slot, 1, 5000)?;
        if code != CC_SUCCESS && code != CC_SHORT_PACKET {
            return Err(Error::Io);
        }
        // The status stage event reports the residual of the data stage.
        Ok(data_len - residual as usize)
    }

    /// Stops the endpoint and rewinds its ring after a timeout.
    fn reset_endpoint(&mut self, slot: u8, dci: u8) -> Result<()> {
        let sel = (slot as u32) << 24 | (dci as u32) << 16;
        let _ = self.command(0, (TRB_STOP_EP << 10) | sel);
        let Some(s) = &mut self.slots[(slot - 1) as usize] else { return Err(Error::InvalidArgument) };
        let ring = &mut s.rings[(dci - 1) as usize];
        ring.reset();
        let phys = ring.phys();
        let (_, code) = self.command(phys | 1, (TRB_SET_TR_DEQUEUE << 10) | sel)?;
        if code != CC_SUCCESS {
            return Err(Error::Io);
        }
        Ok(())
    }
}
