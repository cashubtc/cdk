//! UniFFI surface over the `cashu` crate's protocol and crypto primitives.
//!
//! The exports here are the set a JavaScript wallet cannot compute quickly on
//! its own: blinding, unblinding and NUT-13 derivation. Everything else stays
//! in the calling library.

pub mod crypto;
pub mod error;
pub mod factory;
pub mod outputs;
pub mod types;

pub use crypto::*;
pub use error::CashuFfiError;
pub use factory::DeterministicOutputFactory;
pub use outputs::*;
pub use types::{BlindPair, BlindedOutput, DleqProof, KeyEntry, P2pkOptions, SigFlag};

uniffi::setup_scaffolding!();
