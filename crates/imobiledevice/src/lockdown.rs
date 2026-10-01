//! lockdownd client over a mux connection: plist messages framed as
//! `[u32 big-endian length][plist body]`. Only what the tethering flow needs: QueryType,
//! GetValue (the device public key) and the Pair request. Sessions (StartSession, TLS) are
//! not needed, because the hotspot data path is a plain USB interface, not a lockdown service.
use crate::mux::Channel;
use crate::pair::{self, PairRecord};
use crate::plist::{self, Value};
use alloc::vec::Vec;
use hal::{Error, Result};

const LABEL: &str = "archstaler";
const PROTOCOL_VERSION: &str = "2";

/// How lockdownd answered a Pair request.
#[derive(Debug, PartialEq, Eq)]
pub enum PairReply {
    Paired,
    /// The Trust dialog is up on the phone; send Pair again after a moment.
    DialogPending,
    /// The phone is locked (or wants its passcode); unlock it and send Pair again.
    Locked,
    /// The user tapped "Don't Trust".
    Denied,
    /// Anything else (the error text was logged).
    Failed,
}

pub struct Lockdown {
    io: Channel,
}

fn frame_send(io: &mut Channel, v: &Value) -> Result<()> {
    let body = plist::to_xml(v);
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    io.write_all(&frame)
}

fn frame_recv(io: &mut Channel, timeout_ms: u64) -> Result<Value> {
    let mut lenb = [0u8; 4];
    io.read_exact(&mut lenb, timeout_ms)?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len == 0 || len > 1 << 20 {
        return Err(Error::Io);
    }
    let mut body = alloc::vec![0u8; len];
    io.read_exact(&mut body, timeout_ms)?;
    plist::parse(&body).map_err(|_| Error::Io)
}

impl Lockdown {
    /// Takes the connection to lockdownd's port and verifies the service type via QueryType.
    pub fn new(chan: Channel) -> Result<Lockdown> {
        let mut ld = Lockdown { io: chan };
        let reply = ld.request(&Value::dict(alloc::vec![("Request", Value::str("QueryType"))]))?;
        match reply.dict_get_str("Type") {
            Some("com.apple.mobile.lockdown") => Ok(ld),
            other => {
                hal::log!("usb: lockdownd QueryType gave {other:?}");
                Err(Error::Unsupported)
            }
        }
    }

    pub fn into_channel(self) -> Channel {
        self.io
    }

    /// One request/response round trip; adds the Label key.
    fn request(&mut self, v: &Value) -> Result<Value> {
        self.send(v)?;
        frame_recv(&mut self.io, 5000)
    }

    fn send(&mut self, v: &Value) -> Result<()> {
        let req = match v.clone() {
            Value::Dict(mut kv) => {
                kv.push(("Label".into(), Value::str(LABEL)));
                Value::Dict(kv)
            }
            other => other,
        };
        frame_send(&mut self.io, &req)
    }

    /// lockdownd GetValue; returns the raw value node.
    pub fn get_value(&mut self, domain: Option<&str>, key: &str) -> Result<Value> {
        let mut req = alloc::vec![("Key", Value::str(key)), ("Request", Value::str("GetValue"))];
        if let Some(d) = domain {
            req.push(("Domain", Value::str(d)));
        }
        let reply = self.request(&Value::dict(req))?;
        if let Some(e) = reply.dict_get_str("Error") {
            hal::log!("usb: lockdownd GetValue {key} failed: {e}");
            return Err(Error::Io);
        }
        reply.get("Value").cloned().ok_or(Error::Io)
    }

    /// Reads the device public key and builds the pair record (RSA key generation: slow).
    pub fn pair_prepare(&mut self, system_buid: &str, unix_time: u64) -> Result<PairRecord> {
        let pubkey = self.get_value(None, "DevicePublicKey")?;
        let pubkey_pem = pubkey.as_data().ok_or(Error::Io)?.to_vec();
        pair::generate_pair_record(&pubkey_pem, system_buid, unix_time)
    }

    /// Sends a Pair request without waiting for the answer (`pair_reply` collects it): the
    /// phone may hold its answer until the user acts on the Trust dialog.
    pub fn pair_request(&mut self, record: &PairRecord) -> Result<()> {
        let req = Value::dict(alloc::vec![
            ("PairRecord", pair::record_plist(record)),
            ("Request", Value::str("Pair")),
            ("ProtocolVersion", Value::str(PROTOCOL_VERSION)),
            ("PairingOptions", Value::dict(alloc::vec![("ExtendedPairingErrors", Value::Bool(true))])),
        ]);
        self.send(&req)
    }

    /// Waits up to `timeout_ms` for the answer to the last `pair_request`.
    /// `Err(Timeout)` means no answer yet (the request is still outstanding).
    pub fn pair_reply(&mut self, timeout_ms: u64) -> Result<PairReply> {
        let reply = frame_recv(&mut self.io, timeout_ms)?;
        if reply.dict_get_str("Request") == Some("Pair") && reply.dict_get_str("Result") == Some("Success") {
            return Ok(PairReply::Paired);
        }
        Ok(match reply.dict_get_str("Error") {
            Some("PairingDialogResponsePending") => PairReply::DialogPending,
            Some("PasswordProtected") => PairReply::Locked,
            Some("UserDeniedPairing") => PairReply::Denied,
            other => {
                hal::log!("usb: unexpected Pair reply, Error {other:?}");
                PairReply::Failed
            }
        })
    }
}
