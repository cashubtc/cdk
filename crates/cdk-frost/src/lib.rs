//! Application-neutral secp256k1 FROST key management.

#![doc = include_str!("../README.md")]

mod canonical;
mod error;
mod key_manager;
mod types;
mod wire;

#[cfg(test)]
mod tests;

pub use crate::canonical::CanonicalRootKeyPackage;
pub use crate::error::Error;
pub use crate::key_manager::{ApplicationAuthorization, ParticipantKeyManager};
pub use crate::types::{
    AggregateSignature, ApplicationDerivationVersion, ApplicationId, ApplicationRegistration,
    AuthorizationRequest, DerivedPublicPackage, FrostCiphersuite, MemberId, NonceCommitment,
    NonceCommitmentSet, NonceId, RootKeyEpoch, RootPublicPackage, SignatureShare, SigningSessionId,
};
