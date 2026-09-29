use crate::keyring::{KeyEntry, KeyMaterial, Keyring};
use crate::{Error, Result};
use alloc::vec::Vec;
use sha2::{Digest, Sha256, Sha512};

const ALGO_RSA: u8 = 1;
const ALGO_EDDSA: u8 = 22;
const HASH_SHA256: u8 = 8;
const HASH_SHA512: u8 = 10;
const SIG_BINARY: u8 = 0x00;

#[derive(Debug, Clone)]
pub struct Signature {
    pub pk_algo: u8,
    pub hash_algo: u8,
    pub created: Option<u64>,
    pub expires_after: Option<u64>,
    pub issuer_fingerprint: Option<[u8; 20]>,
    pub issuer_key_id: Option<[u8; 8]>,
    hashed: Vec<u8>,
    prefix: [u8; 2],
    /// Signature MPIs, big-endian without leading zeros.
    mpis: [Vec<u8>; 2],
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Malformed);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn mpi(&mut self) -> Result<Vec<u8>> {
        let bits = self.u16()? as usize;
        let bytes = self.take(bits.div_ceil(8))?;
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        Ok(bytes[skip..].to_vec())
    }
}

/// Extracts the body of the first packet, which must be a signature (tag 2).
fn signature_body(data: &[u8]) -> Result<&[u8]> {
    let mut r = Reader(data);
    let first = r.u8()?;
    if first & 0x80 == 0 {
        return Err(Error::Malformed);
    }
    let (tag, len) = if first & 0x40 != 0 {
        let l = r.u8()? as usize;
        let len = match l {
            0..=191 => l,
            192..=223 => ((l - 192) << 8) + r.u8()? as usize + 192,
            255 => {
                let b = r.take(4)?;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
            }
            _ => return Err(Error::Unsupported), // partial lengths
        };
        (first & 0x3f, len)
    } else {
        let len = match first & 3 {
            0 => r.u8()? as usize,
            1 => r.u16()? as usize,
            2 => {
                let b = r.take(4)?;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
            }
            _ => return Err(Error::Unsupported),
        };
        ((first >> 2) & 0xf, len)
    };
    if tag != 2 {
        return Err(Error::Malformed);
    }
    r.take(len)
}

impl Signature {
    /// Parses a binary detached signature (one signature packet).
    pub fn parse(data: &[u8]) -> Result<Signature> {
        let mut r = Reader(signature_body(data)?);
        if r.u8()? != 4 {
            return Err(Error::Unsupported);
        }
        let sig_type = r.u8()?;
        if sig_type != SIG_BINARY {
            return Err(Error::Unsupported);
        }
        let pk_algo = r.u8()?;
        let hash_algo = r.u8()?;
        let hashed_len = r.u16()? as usize;
        let hashed = r.take(hashed_len)?.to_vec();
        let unhashed_len = r.u16()? as usize;
        let unhashed = r.take(unhashed_len)?;
        let prefix_bytes = r.take(2)?;
        let prefix = [prefix_bytes[0], prefix_bytes[1]];

        let mut s = Signature {
            pk_algo,
            hash_algo,
            created: None,
            expires_after: None,
            issuer_fingerprint: None,
            issuer_key_id: None,
            hashed,
            prefix,
            mpis: [Vec::new(), Vec::new()],
        };
        let hashed = s.hashed.clone();
        s.parse_subpackets(&hashed)?;
        s.parse_subpackets(unhashed)?;

        match pk_algo {
            ALGO_RSA => s.mpis[0] = r.mpi()?,
            ALGO_EDDSA => {
                s.mpis[0] = r.mpi()?;
                s.mpis[1] = r.mpi()?;
            }
            _ => return Err(Error::Unsupported),
        }
        if hash_algo != HASH_SHA256 && hash_algo != HASH_SHA512 {
            return Err(Error::Unsupported);
        }
        Ok(s)
    }

    fn parse_subpackets(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let mut r = Reader(data);
            let l = r.u8()? as usize;
            let len = match l {
                0..=191 => l,
                192..=254 => ((l - 192) << 8) + r.u8()? as usize + 192,
                _ => {
                    let b = r.take(4)?;
                    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
                }
            };
            let body = r.take(len)?;
            data = r.0;
            let Some((&ty, val)) = body.split_first() else { return Err(Error::Malformed) };
            match ty & 0x7f {
                2 if val.len() == 4 => self.created = Some(u32::from_be_bytes(val.try_into().unwrap()) as u64),
                3 if val.len() == 4 => self.expires_after = Some(u32::from_be_bytes(val.try_into().unwrap()) as u64),
                16 if val.len() == 8 => self.issuer_key_id = Some(val.try_into().unwrap()),
                33 if val.len() == 21 && val[0] == 4 => self.issuer_fingerprint = Some(val[1..].try_into().unwrap()),
                _ => {}
            }
        }
        Ok(())
    }
}

enum Hasher {
    Sha256(Sha256),
    Sha512(Sha512),
}

/// Streaming verification: `update` with the signed data, then `finish`.
pub struct Verifier {
    sig: Signature,
    hasher: Hasher,
}

impl Verifier {
    pub fn new(sig: Signature) -> Verifier {
        let hasher = if sig.hash_algo == HASH_SHA256 {
            Hasher::Sha256(Sha256::new())
        } else {
            Hasher::Sha512(Sha512::new())
        };
        Verifier { sig, hasher }
    }

    /// Parses a signature packet and starts verification.
    pub fn from_packet(data: &[u8]) -> Result<Verifier> {
        Ok(Verifier::new(Signature::parse(data)?))
    }

    pub fn signature(&self) -> &Signature {
        &self.sig
    }

    pub fn update(&mut self, data: &[u8]) {
        match &mut self.hasher {
            Hasher::Sha256(h) => h.update(data),
            Hasher::Sha512(h) => h.update(data),
        }
    }

    /// Checks the signature against `keyring`. `now` is the current Unix time.
    /// Returns the fingerprint of the signing key.
    pub fn finish(mut self, keyring: &Keyring, now: u64) -> Result<[u8; 20]> {
        let s = &self.sig;
        let key = match (&s.issuer_fingerprint, &s.issuer_key_id) {
            (Some(f), _) => keyring.find_by_fingerprint(f),
            (None, Some(id)) => keyring.find_by_key_id(id),
            (None, None) => return Err(Error::NoIssuer),
        }
        .ok_or(Error::UnknownKey)?;
        if key.expires.is_some_and(|e| now >= e) {
            return Err(Error::Expired);
        }
        if let (Some(c), Some(x)) = (s.created, s.expires_after) {
            if x != 0 && now >= c + x {
                return Err(Error::Expired);
            }
        }

        // Trailer: signed header fields, then 0x04 0xff and the big-endian hashed length.
        let mut trailer = Vec::with_capacity(12 + s.hashed.len());
        trailer.extend_from_slice(&[4, SIG_BINARY, s.pk_algo, s.hash_algo]);
        trailer.extend_from_slice(&(s.hashed.len() as u16).to_be_bytes());
        trailer.extend_from_slice(&s.hashed);
        let total = trailer.len() as u32;
        trailer.extend_from_slice(&[4, 0xff]);
        trailer.extend_from_slice(&total.to_be_bytes());
        self.update(&trailer);

        let digest: Vec<u8> = match self.hasher {
            Hasher::Sha256(h) => h.finalize().to_vec(),
            Hasher::Sha512(h) => h.finalize().to_vec(),
        };
        if digest[..2] != self.sig.prefix {
            return Err(Error::BadSignature);
        }
        verify_raw(&self.sig, key, &digest)?;
        Ok(key.fingerprint)
    }
}

fn verify_raw(sig: &Signature, key: &KeyEntry, digest: &[u8]) -> Result<()> {
    match (&key.material, sig.pk_algo) {
        (KeyMaterial::Rsa { n, e }, ALGO_RSA) => {
            use rsa::{BigUint, Pkcs1v15Sign, RsaPublicKey};
            let pk = RsaPublicKey::new(BigUint::from_bytes_be(n), BigUint::from_bytes_be(e)).map_err(|_| Error::BadSignature)?;
            // Signature must be left-padded to the modulus length.
            let mut padded = alloc::vec![0u8; n.len()];
            let raw = &sig.mpis[0];
            if raw.len() > padded.len() {
                return Err(Error::BadSignature);
            }
            let off = padded.len() - raw.len();
            padded[off..].copy_from_slice(raw);
            let scheme = if sig.hash_algo == HASH_SHA256 {
                Pkcs1v15Sign::new::<Sha256>()
            } else {
                Pkcs1v15Sign::new::<Sha512>()
            };
            pk.verify(scheme, digest, &padded).map_err(|_| Error::BadSignature)
        }
        (KeyMaterial::Ed25519(pk), ALGO_EDDSA) => {
            use ed25519_dalek::{Signature as EdSig, VerifyingKey};
            let (r, s) = (&sig.mpis[0], &sig.mpis[1]);
            if r.len() > 32 || s.len() > 32 {
                return Err(Error::BadSignature);
            }
            let mut raw = [0u8; 64];
            raw[32 - r.len()..32].copy_from_slice(r);
            raw[64 - s.len()..].copy_from_slice(s);
            let vk = VerifyingKey::from_bytes(pk).map_err(|_| Error::BadSignature)?;
            vk.verify_strict(digest, &EdSig::from_bytes(&raw)).map_err(|_| Error::BadSignature)
        }
        _ => Err(Error::Unsupported),
    }
}
