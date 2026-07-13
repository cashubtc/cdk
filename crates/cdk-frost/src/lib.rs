//! Application-neutral secp256k1 FROST key management.

#![doc = include_str!("../README.md")]

mod canonical;
mod dkg;
mod error;
mod key_manager;
mod sealed;
mod types;
mod wire;

#[cfg(test)]
mod tests;

pub use crate::canonical::CanonicalRootKeyPackage;
pub use crate::dkg::{
    advance_dkg, finish_dkg, start_dkg, DkgContext, DkgRound1Package, DkgRound1Secret,
    DkgRound2Package, DkgRound2Secret,
};
pub use crate::error::Error;
pub use crate::key_manager::{ApplicationAuthorization, ParticipantKeyManager};
pub use crate::sealed::{EncryptedSecret, SecretEncryptionKey};
pub use crate::types::{
    AggregateSignature, ApplicationDerivationVersion, ApplicationId, ApplicationRegistration,
    AuthorizationRequest, AuthorizedSigningRequest, DerivedPublicPackage,
    DerivedSigningPublicPackage, FrostCiphersuite, MemberId, NonceCommitment, NonceCommitmentSet,
    NonceId, RootKeyEpoch, RootPublicPackage, SignatureMode, SignatureShare, SigningDerivation,
    SigningSessionId,
};
/// RNG traits and operating-system RNG version used by the pinned FROST library.
pub use frost_secp256k1_tr::rand_core;

/// Derive a public signing target without access to any participant secret.
pub fn derive_signing_public(
    application: &DerivedPublicPackage,
    derivation: &SigningDerivation,
) -> Result<DerivedSigningPublicPackage, Error> {
    canonical::derive_signing_public(application, derivation)
}
