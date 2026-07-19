//! Application-neutral secp256k1 FROST key management.

#![doc = include_str!("../README.md")]

mod canonical;
mod dkg;
mod error;
mod key_manager;
mod nonce;
mod nonce_redb;
mod protocol;
mod protocol_redb;
mod sealed;
mod threshold_dh;
mod types;
mod virtual_musig;
mod wire;

#[cfg(test)]
mod tests;

pub use crate::canonical::CanonicalRootKeyPackage;
pub use crate::dkg::{
    advance_dkg, finish_dkg, start_dkg, DkgContext, DkgRound1Package, DkgRound1Secret,
    DkgRound2Package, DkgRound2Secret,
};
pub use crate::error::Error;
pub use crate::key_manager::{
    signing_package_hash, ApplicationAuthorization, ParticipantKeyManager,
};
pub use crate::nonce::{
    CustodyProtocol, DurableNonceStore, MemoryNonceStore, NonceBurnReason, NonceLifecycle,
    NonceRecord, NonceReservation, NonceTombstone, SigningAuthorizationScope,
    SigningSessionContext, SigningSlotId, DEFAULT_TERMINAL_NONCE_RETENTION,
};
pub use crate::nonce_redb::RedbNonceStore;
pub use crate::protocol::{
    DurableProtocolStore, ThresholdDhLifecycle, ThresholdDhRecord, ThresholdDhReservation,
    ThresholdDhTombstone, VirtualMusigLifecycle, VirtualMusigRecord, VirtualMusigReservation,
    VirtualMusigTombstone,
};
pub use crate::sealed::{EncryptedSecret, SecretEncryptionKey};
pub use crate::threshold_dh::{
    combine_shares as combine_threshold_dh_shares, verify_share as verify_threshold_dh_share,
    ThresholdDhProof, ThresholdDhRequest, ThresholdDhShare,
};
pub use crate::types::{
    AggregateSignature, ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest, DerivedPublicPackage,
    DerivedSigningPublicPackage, FrostCiphersuite, MemberId, NonceCommitment, NonceCommitmentSet,
    NonceId, RootKeyEpoch, RootPublicPackage, SignatureMode, SignatureShare, SigningDerivation,
    SigningSessionId,
};
pub use crate::virtual_musig::{
    combine_partial_shares as combine_virtual_musig_partial_shares,
    verify_partial_share as verify_virtual_musig_partial_share, VirtualMusigNonceCommitment,
    VirtualMusigNonceCommitmentSet, VirtualMusigNonceId, VirtualMusigPartialShare,
    VirtualMusigTranscript,
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

/// Derive an application-scoped public package without participant secrets.
pub fn derive_application_public(
    root: &RootPublicPackage,
    registration: ApplicationRegistration,
) -> Result<DerivedPublicPackage, Error> {
    canonical::derive_application_public(root, registration)
}
