//! Minimal OpenPGP detached-signature verification (v4 signatures, RSA PKCS#1 v1.5 and EdDSA)
//! against a compact keyring blob built by xtask.
#![no_std]

extern crate alloc;

mod base64;
mod keyring;
mod sig;

pub use base64::decode as base64_decode;
pub use keyring::{KeyEntry, KeyMaterial, Keyring};
pub use sig::{Signature, Verifier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Malformed packet or base64.
    Malformed,
    /// Signature version, type, or algorithm not supported.
    Unsupported,
    /// Signature carries no usable issuer.
    NoIssuer,
    /// Issuer key is not in the keyring; rebuild the ISO with a newer keyring.
    UnknownKey,
    /// Key or signature is past its expiration time.
    Expired,
    /// Digest prefix or cryptographic check failed.
    BadSignature,
}

pub type Result<T> = core::result::Result<T, Error>;
