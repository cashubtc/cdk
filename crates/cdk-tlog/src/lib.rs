//! RFC 6962 Merkle tree math and C2SP checkpoint/signed-note/cosignature
//! formats.
//!
//! This crate is the shared foundation for a mint's own transparency log
//! (see `docs/adr/0001-append-only-transparency-log.md`) and for a
//! built-in witness that can cosign *other* mints' checkpoints. It has no
//! database or HTTP dependency — it is pure hashing, proof, and signature
//! format logic, so it can be unit tested against RFC 6962's own worked
//! examples independent of any storage backend.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub mod checkpoint;
pub mod merkle;
pub mod witness;

pub use checkpoint::{Checkpoint, SignatureLine, SignedCheckpoint};
pub use merkle::{Error as MerkleError, Hash, TreeHead};
pub use witness::{AddCheckpointRequest, WitnessError};
