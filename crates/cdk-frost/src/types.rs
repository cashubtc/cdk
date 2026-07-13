//! Stable application, participant, root, nonce, and signature value types.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::Error;

/// Maximum participant count accepted by canonical root packages.
pub const MAX_PARTICIPANTS: usize = 64;
/// Maximum opaque authorization payload accepted by the key manager.
pub const MAX_AUTHORIZATION_BYTES: usize = 64 * 1024;
/// Maximum serialized FROST commitment accepted from the wire.
pub const MAX_COMMITMENT_BYTES: usize = 256;

/// Ciphersuite encoded in every canonical root package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrostCiphersuite {
    /// ZF FROST secp256k1 SHA-256 Taproot ciphersuite version 1.
    Secp256k1Sha256TrV1,
}

/// Stable non-zero federation member identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberId(u16);

impl MemberId {
    /// Construct a non-zero member identifier.
    pub fn new(value: u16) -> Result<Self, Error> {
        if value == 0 {
            return Err(Error::InvalidParticipant {
                participant: value,
                reason: "identifier must be non-zero".to_string(),
            });
        }
        Ok(Self(value))
    }

    /// Return the numeric identifier.
    pub fn get(self) -> u16 {
        self.0
    }
}

/// Monotonic epoch for an application-neutral root key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootKeyEpoch(u64);

impl RootKeyEpoch {
    /// Construct a non-zero root key epoch.
    pub fn new(value: u64) -> Result<Self, Error> {
        if value == 0 {
            return Err(Error::malformed("root key epoch", "epoch must be non-zero"));
        }
        Ok(Self(value))
    }

    /// Return the numeric epoch.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable 32-byte application identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApplicationId([u8; 32]);

impl ApplicationId {
    /// Derive a stable identifier from a globally unique application label.
    pub fn from_label(label: &str) -> Result<Self, Error> {
        if label.is_empty() || label.len() > 255 {
            return Err(Error::Application(
                "application label must contain 1 through 255 bytes".to_string(),
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cdk/frost/application-id/v1");
        hasher.update((label.len() as u16).to_be_bytes());
        hasher.update(label.as_bytes());
        Ok(Self(hasher.finalize().into()))
    }

    /// Construct an identifier from canonical bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, Error> {
        if bytes == [0; 32] {
            return Err(Error::Application(
                "application identifier cannot be all zeroes".to_string(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Return canonical identifier bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Version of an application's additive derivation contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApplicationDerivationVersion(u16);

impl ApplicationDerivationVersion {
    /// Construct a non-zero derivation version.
    pub fn new(value: u16) -> Result<Self, Error> {
        if value == 0 {
            return Err(Error::Application(
                "derivation version must be non-zero".to_string(),
            ));
        }
        Ok(Self(value))
    }

    /// Return the numeric version.
    pub fn get(self) -> u16 {
        self.0
    }
}

/// Immutable metadata for a reviewed application authorization adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationRegistration {
    id: ApplicationId,
    derivation_version: ApplicationDerivationVersion,
    authorization_policy: [u8; 32],
}

impl ApplicationRegistration {
    /// Construct an application registration bound to a policy commitment.
    pub fn new(
        id: ApplicationId,
        derivation_version: ApplicationDerivationVersion,
        authorization_policy: [u8; 32],
    ) -> Result<Self, Error> {
        if authorization_policy == [0; 32] {
            return Err(Error::Application(
                "authorization policy commitment cannot be all zeroes".to_string(),
            ));
        }
        Ok(Self {
            id,
            derivation_version,
            authorization_policy,
        })
    }

    /// Stable application identifier.
    pub fn id(self) -> ApplicationId {
        self.id
    }

    /// Application derivation version.
    pub fn derivation_version(self) -> ApplicationDerivationVersion {
        self.derivation_version
    }

    /// Commitment to the adapter's authorization policy.
    pub fn authorization_policy(self) -> [u8; 32] {
        self.authorization_policy
    }
}

/// Unique identifier for one application signing session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SigningSessionId([u8; 32]);

impl SigningSessionId {
    /// Construct a non-zero session identifier.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, Error> {
        if bytes == [0; 32] {
            return Err(Error::malformed(
                "signing session",
                "identifier cannot be all zeroes",
            ));
        }
        Ok(Self(bytes))
    }

    /// Return canonical bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Opaque identifier for one single-use nonce pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonceId([u8; 32]);

impl NonceId {
    pub(crate) fn from_bytes_unchecked(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return canonical bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

pub(crate) fn derive_nonce_id(
    epoch: RootKeyEpoch,
    registration: ApplicationRegistration,
    session_id: SigningSessionId,
    participant: MemberId,
    commitments: &[u8],
) -> NonceId {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/nonce-id/v1");
    hasher.update(epoch.get().to_be_bytes());
    hasher.update(registration.id().to_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    hasher.update(session_id.to_bytes());
    hasher.update(participant.get().to_be_bytes());
    hasher.update((commitments.len() as u16).to_be_bytes());
    hasher.update(commitments);
    NonceId::from_bytes_unchecked(hasher.finalize().into())
}

/// Opaque authorization input interpreted by a registered application adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    application_id: ApplicationId,
    derivation_version: ApplicationDerivationVersion,
    session_id: SigningSessionId,
    payload: Vec<u8>,
}

impl AuthorizationRequest {
    /// Construct an application-scoped authorization request.
    pub fn new(
        application_id: ApplicationId,
        derivation_version: ApplicationDerivationVersion,
        session_id: SigningSessionId,
        payload: Vec<u8>,
    ) -> Result<Self, Error> {
        if payload.len() > MAX_AUTHORIZATION_BYTES {
            return Err(Error::OversizedInput {
                kind: "authorization request",
                max: MAX_AUTHORIZATION_BYTES,
            });
        }
        Ok(Self {
            application_id,
            derivation_version,
            session_id,
            payload,
        })
    }

    /// Requested application identifier.
    pub fn application_id(&self) -> ApplicationId {
        self.application_id
    }

    /// Requested derivation version.
    pub fn derivation_version(&self) -> ApplicationDerivationVersion {
        self.derivation_version
    }

    /// Requested session identifier.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Adapter-specific authorization payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// Canonical untweaked public root package shared by every participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPublicPackage {
    pub(crate) epoch: RootKeyEpoch,
    pub(crate) dkg_protocol_version: u16,
    pub(crate) ciphersuite: FrostCiphersuite,
    pub(crate) threshold: u16,
    pub(crate) verifying_shares: BTreeMap<MemberId, [u8; 33]>,
    pub(crate) group_key: [u8; 33],
    pub(crate) transcript_binding: [u8; 32],
}

impl RootPublicPackage {
    /// Root key epoch.
    pub fn epoch(&self) -> RootKeyEpoch {
        self.epoch
    }

    /// CDK DKG protocol version.
    pub fn dkg_protocol_version(&self) -> u16 {
        self.dkg_protocol_version
    }

    /// FROST ciphersuite used by this root package.
    pub fn ciphersuite(&self) -> FrostCiphersuite {
        self.ciphersuite
    }

    /// Signing threshold.
    pub fn threshold(&self) -> u16 {
        self.threshold
    }

    /// Participant verification shares in canonical member order.
    pub fn verifying_shares(&self) -> &BTreeMap<MemberId, [u8; 33]> {
        &self.verifying_shares
    }

    /// Untweaked compressed group key.
    pub fn group_key(&self) -> [u8; 33] {
        self.group_key
    }

    /// Transcript binding produced by the root ceremony.
    pub fn transcript_binding(&self) -> [u8; 32] {
        self.transcript_binding
    }
}

/// Public application-derived verification package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPublicPackage {
    pub(crate) root_epoch: RootKeyEpoch,
    pub(crate) registration: ApplicationRegistration,
    pub(crate) threshold: u16,
    pub(crate) verifying_shares: BTreeMap<MemberId, [u8; 33]>,
    pub(crate) group_key: [u8; 33],
}

impl DerivedPublicPackage {
    /// Root epoch from which this package was derived.
    pub fn root_epoch(&self) -> RootKeyEpoch {
        self.root_epoch
    }

    /// Registered application metadata.
    pub fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    /// Signing threshold.
    pub fn threshold(&self) -> u16 {
        self.threshold
    }

    /// Application-derived verification shares.
    pub fn verifying_shares(&self) -> &BTreeMap<MemberId, [u8; 33]> {
        &self.verifying_shares
    }

    /// Application-derived compressed group key before any later child or
    /// Taproot tweak.
    pub fn group_key(&self) -> [u8; 33] {
        self.group_key
    }
}

/// Public commitment for one single-use participant nonce pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonceCommitment {
    pub(crate) root_epoch: RootKeyEpoch,
    pub(crate) registration: ApplicationRegistration,
    pub(crate) session_id: SigningSessionId,
    pub(crate) nonce_id: NonceId,
    pub(crate) participant: MemberId,
    pub(crate) commitments: Vec<u8>,
}

impl NonceCommitment {
    /// Root epoch bound to this commitment.
    pub fn root_epoch(&self) -> RootKeyEpoch {
        self.root_epoch
    }

    /// Application registration bound to this commitment.
    pub fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    /// Signing session bound to this commitment.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Single-use nonce identifier.
    pub fn nonce_id(&self) -> NonceId {
        self.nonce_id
    }

    /// Participant that generated this commitment.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    pub(crate) fn has_valid_nonce_id(&self) -> bool {
        self.nonce_id
            == derive_nonce_id(
                self.root_epoch,
                self.registration,
                self.session_id,
                self.participant,
                &self.commitments,
            )
    }
}

/// Canonically ordered set of signing commitments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonceCommitmentSet {
    commitments: BTreeMap<MemberId, NonceCommitment>,
}

impl NonceCommitmentSet {
    /// Validate and construct a commitment set, rejecting duplicates.
    pub fn new(commitments: impl IntoIterator<Item = NonceCommitment>) -> Result<Self, Error> {
        let mut ordered = BTreeMap::new();
        for commitment in commitments {
            let participant = commitment.participant;
            if ordered.insert(participant, commitment).is_some() {
                return Err(Error::InvalidParticipant {
                    participant: participant.get(),
                    reason: "duplicate commitment".to_string(),
                });
            }
        }
        Ok(Self {
            commitments: ordered,
        })
    }

    /// Ordered participant commitments.
    pub fn commitments(&self) -> &BTreeMap<MemberId, NonceCommitment> {
        &self.commitments
    }
}

/// One participant's application-authorized signature share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureShare {
    pub(crate) root_epoch: RootKeyEpoch,
    pub(crate) registration: ApplicationRegistration,
    pub(crate) session_id: SigningSessionId,
    pub(crate) participant: MemberId,
    pub(crate) share: [u8; 32],
}

impl SignatureShare {
    /// Participant that produced the share.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    /// Signing session bound to the share.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Application registration bound to the share.
    pub fn registration(&self) -> ApplicationRegistration {
        self.registration
    }
}

/// BIP340-compatible 64-byte aggregate Schnorr signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggregateSignature([u8; 64]);

impl AggregateSignature {
    pub(crate) fn from_bytes_unchecked(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Return compact BIP340 signature bytes.
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }
}
