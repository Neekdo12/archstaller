use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyMaterial {
    /// Big-endian modulus and exponent without leading zeros.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ed25519([u8; 32]),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    pub fingerprint: [u8; 20],
    pub material: KeyMaterial,
    /// Absolute expiration (Unix seconds), if any.
    pub expires: Option<u64>,
}

/// Trusted signing keys (primary keys and signing subkeys).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keyring {
    pub keys: Vec<KeyEntry>,
}

impl Keyring {
    pub fn from_bytes(b: &[u8]) -> Option<Keyring> {
        postcard::from_bytes(b).ok()
    }

    #[cfg(feature = "std")]
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("keyring serializes")
    }

    pub fn find_by_fingerprint(&self, fpr: &[u8; 20]) -> Option<&KeyEntry> {
        self.keys.iter().find(|k| &k.fingerprint == fpr)
    }

    /// Key IDs are the low 64 bits of the fingerprint.
    pub fn find_by_key_id(&self, id: &[u8; 8]) -> Option<&KeyEntry> {
        self.keys.iter().find(|k| &k.fingerprint[12..] == id)
    }
}
