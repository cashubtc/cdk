//! Client for the [Sigsum](https://www.sigsum.org/) transparency log
//! protocol.
//!
//! Sigsum logs are content-agnostic: a submitter signs an arbitrary 32-byte
//! message and submits it to a log, which appends it to an append-only
//! Merkle tree, periodically publishing a signed (and witness-cosigned)
//! tree head. This crate is a thin, dependency-light implementation of the
//! wire protocol described in
//! <https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md>,
//! intended to let a CDK mint anchor its own transparency-log checkpoints
//! (see `docs/adr/0001-append-only-transparency-log.md`) to an already
//! running public Sigsum log instead of standing up new infrastructure.
//!
//! This crate deliberately does not interpret what is being logged: the
//! mint is expected to pass in the hash of its own checkpoint (see the
//! `checkpoint` module of `cdk-common`) as the `message`.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod anchor;
mod client;
mod error;
mod hashing;
mod proof;
mod rate_limit;
mod types;

pub use anchor::anchor;
pub use client::{AddLeafOutcome, SigsumClient};
pub use error::Error;
pub use hashing::{
    checksum_of, cosignature_signing_bytes, leaf_hash, sign_leaf, tree_head_signing_bytes, Hash,
};
pub use proof::SigsumProof;
pub use rate_limit::{RateLimitKeyPair, SubmitToken};
pub use types::{ConsistencyProof, Cosignature, InclusionProof, SignedTreeHead, TreeLeaf};
