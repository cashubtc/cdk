//! Shared federation configuration types.
//!
//! These types describe a static Cashu federation roster and its threshold BLS
//! key material. They are deliberately transport-agnostic so mintd, tests,
//! private REST peers, and future DKG tooling can reuse the same validation
//! rules.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

mod consensus;
mod journal;
mod operation;
mod signing;
mod transport;
mod trusted_dealer;

pub use self::consensus::{
    FederationConsensusDefaultItem, FederationConsensusEngine, FederationConsensusItem,
    FederationConsensusItemAdmission, FederationConsensusItemQueue, FederationConsensusModuleFact,
    FederationConsensusRuntime, InMemoryFederationConsensus, InMemoryFederationConsensusAdapter,
    InMemoryFederationConsensusNetwork, FEDERATION_CONSENSUS_ITEM_VERSION,
    FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
};
pub use self::journal::{
    FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationMaterializedStateRow, FederationMaterializedStateSnapshot, FederationOperationStatus,
    FederationStateDigest, InMemoryFederationJournal,
};
pub use self::operation::{
    FederationMeltOperation, FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation,
    FederationMintOperation, FederationMintQuoteOperation, FederationMintQuotePaymentObservation,
    FederationMintRequest, FederationOperation, FederationOperationConflictKey,
    FederationOperationEnvelope, FederationOperationId, FederationOperationKind,
    FederationPaymentObservation, FederationPaymentObservationEvidenceGroup,
    FederationPaymentObservationEvidenceGroupSummary, FederationPaymentObservationQuarantine,
    FederationPaymentObservationQuarantineReason, FederationPaymentObservationQuarantineRecord,
    FederationPaymentObservationQuorum, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportAction,
    FederationPaymentObservationReportStatus, FederationPaymentObservationReportSummary,
    FederationRestoreOperation, FederationSwapOperation, FEDERATION_MAX_OPERATION_ENVELOPE_BYTES,
    FEDERATION_OPERATION_VERSION,
};
pub use self::signing::{FederationBlindSignatureShare, FederationBlindSignatureShareSet};
pub use self::transport::{
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationPeerStatus,
    FederationPeerStatusRequest, FederationRestBodyHash, FederationRestMessageKind,
    FederationRestSequenceTracker, FederationRestSignature, FederationSessionOutcome,
    FederationSessionOutcomeQuorum, FederationSignedRequest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, FEDERATION_REST_API_VERSION,
    FEDERATION_SESSION_OUTCOME_VERSION,
};
pub use self::trusted_dealer::{
    generate_trusted_dealer_configs, TrustedDealerConfigBundle, TrustedDealerKeysetSpec,
    TrustedDealerMember,
};

use crate::nuts::nut01::{
    aggregate_public_key_shares, BlsAggregatePublicKey, BlsPublicKeyShare, BlsSecretKeyShare,
    BlsSignerId,
};
use crate::nuts::nut02::{KeySet, KeySetVersion};
use crate::{
    mint_url::MintUrl, Amount, BlindedMessage, CurrencyUnit, Id, Keys, PublicKey, SecretKey,
};

const FEDERATION_ROSTER_HASH_VERSION: &[u8] = b"cdk-federation-roster-v1";
const FEDERATION_DKG_COMMITMENT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-commitment-v1";
const FEDERATION_DKG_TRANSCRIPT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-transcript-v1";
const FEDERATION_DKG_TRANSCRIPT_SIGNATURE_VERSION: &[u8] =
    b"cdk-federation-dkg-transcript-signature-v1";

/// Result alias for federation configuration validation.
pub type Result<T> = std::result::Result<T, FederationError>;

/// Errors returned by federation configuration helpers.
#[derive(Debug, Error)]
pub enum FederationError {
    /// A federation ID must be exactly 32 bytes.
    #[error("Invalid federation id size: expected={expected}, found={found}")]
    InvalidFederationIdSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation operation ID must be exactly 32 bytes.
    #[error("Invalid federation operation id size: expected={expected}, found={found}")]
    InvalidOperationIdSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation journal digest must be exactly 32 bytes.
    #[error("Invalid federation journal digest size: expected={expected}, found={found}")]
    InvalidJournalDigestSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation state digest must be exactly 32 bytes.
    #[error("Invalid federation state digest size: expected={expected}, found={found}")]
    InvalidStateDigestSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// Member IDs are non-zero u16 values.
    #[error("Invalid federation member id: {0}")]
    InvalidMemberId(u16),
    /// Threshold parameters must be non-zero and no greater than the member count.
    #[error(
        "Invalid federation threshold params: members={members}, signature_threshold={signature_threshold}, consensus_threshold={consensus_threshold}"
    )]
    InvalidThresholdParams {
        /// Configured federation member count.
        members: u16,
        /// Configured BLS signature threshold.
        signature_threshold: u16,
        /// Configured consensus threshold.
        consensus_threshold: u16,
    },
    /// A real payment observation policy must require a safe member quorum.
    #[error(
        "Invalid federation payment observation quorum for {context}: required={required}, members={members}, consensus_threshold={consensus_threshold}"
    )]
    InvalidPaymentObservationQuorum {
        /// Payment observation context being validated.
        context: FederationPaymentObservationContext,
        /// Required matching observations.
        required: u16,
        /// Configured federation member count.
        members: u16,
        /// Configured consensus threshold.
        consensus_threshold: u16,
    },
    /// Payment observation limits must be non-zero.
    #[error("Invalid federation payment observation limit {field}: value={value}")]
    InvalidPaymentObservationLimit {
        /// Configured limit field.
        field: &'static str,
        /// Invalid value.
        value: u64,
    },
    /// A single-member payment observation cannot finalize under the configured policy.
    #[error(
        "Federation payment observation quorum not satisfied for {context}: required={required}, found={found}"
    )]
    PaymentObservationQuorumNotSatisfied {
        /// Payment observation context being checked.
        context: FederationPaymentObservationContext,
        /// Required matching observations.
        required: u16,
        /// Matching observations currently available.
        found: u16,
    },
    /// Payment observations contain conflicting evidence that must be resolved first.
    #[error(
        "Federation payment observation conflict for {context}: matching={matching}, conflicting={conflicting}"
    )]
    PaymentObservationConflict {
        /// Payment observation context being checked.
        context: FederationPaymentObservationContext,
        /// Matching observation count.
        matching: u16,
        /// Conflicting observation count.
        conflicting: u16,
    },
    /// A consensus step requires more reachable members than are currently online.
    #[error("Federation consensus quorum unavailable: reachable={reachable}, required={required}")]
    ConsensusQuorumUnavailable {
        /// Reachable member count.
        reachable: u16,
        /// Required quorum threshold.
        required: u16,
    },
    /// A consensus item exceeded the configured canonical size limit.
    #[error("Federation consensus item too large: actual={actual}, max={max}")]
    ConsensusItemTooLarge {
        /// Actual canonical item size.
        actual: usize,
        /// Maximum allowed canonical item size.
        max: usize,
    },
    /// The roster has a different member count than the threshold params.
    #[error("Federation member count mismatch: expected={expected}, found={found}")]
    MemberCountMismatch {
        /// Expected member count.
        expected: u16,
        /// Actual roster size.
        found: usize,
    },
    /// A member was required but is missing.
    #[error("Federation member {member_id} is missing")]
    MissingMember {
        /// Missing member ID.
        member_id: MemberId,
    },
    /// A member exists in one section but not in the federation roster.
    #[error("Unknown federation member {member_id}")]
    UnknownMember {
        /// Unknown member ID.
        member_id: MemberId,
    },
    /// The member ID stored inside a config value does not match its map key.
    #[error("Federation member id mismatch: expected={expected}, found={found}")]
    MemberIdMismatch {
        /// Expected member ID.
        expected: MemberId,
        /// Found member ID.
        found: MemberId,
    },
    /// Trusted-dealer setup received a duplicate member.
    #[error("Duplicate federation member {member_id}")]
    DuplicateMember {
        /// Duplicated member ID.
        member_id: MemberId,
    },
    /// Federation member identity keys must be secp256k1 keys.
    #[error("Federation member {member_id} identity key must be secp256k1")]
    InvalidIdentityKeyKind {
        /// Member with the invalid key kind.
        member_id: MemberId,
    },
    /// The private identity key does not derive to the public roster key.
    #[error("Federation member {member_id} private identity key does not match public roster key")]
    PrivateIdentityMismatch {
        /// Member with the mismatched private key.
        member_id: MemberId,
    },
    /// A federated keyset must contain at least one amount key.
    #[error("Federated keyset {keyset_id} has no aggregate keys")]
    EmptyFederatedKeyset {
        /// Keyset ID being validated.
        keyset_id: Id,
    },
    /// Federated BLS aggregate keysets must use Cashu keyset v3.
    #[error("Federated keyset {keyset_id} must use Cashu keyset v3")]
    InvalidFederatedKeysetVersion {
        /// Keyset ID being validated.
        keyset_id: Id,
    },
    /// Output references a keyset that is not in the public federation config.
    #[error("Unknown federated keyset {keyset_id}")]
    UnknownFederatedKeyset {
        /// Unknown keyset ID.
        keyset_id: Id,
    },
    /// Output references an amount that is not in a federated aggregate keyset.
    #[error("Federated keyset {keyset_id} does not support amount {amount}")]
    UnknownFederatedKeysetAmount {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Unsupported output amount.
        amount: Amount,
    },
    /// Members must not sign outputs for inactive keysets.
    #[error("Federated keyset {keyset_id} is inactive")]
    InactiveFederatedKeyset {
        /// Inactive keyset ID.
        keyset_id: Id,
    },
    /// Trusted-dealer setup needs at least one amount per keyset.
    #[error("Trusted-dealer keyset for unit {unit} has no amount coefficients")]
    EmptyTrustedDealerKeyset {
        /// Unit of the empty keyset.
        unit: CurrencyUnit,
    },
    /// Trusted-dealer polynomial coefficient count must match the signature threshold.
    #[error(
        "Trusted-dealer coefficient count mismatch for amount {amount}: expected={expected}, found={found}"
    )]
    InvalidTrustedDealerCoefficientCount {
        /// Amount being generated.
        amount: Amount,
        /// Expected coefficient count.
        expected: usize,
        /// Actual coefficient count.
        found: usize,
    },
    /// The keyset ID does not match its aggregate public keys and metadata.
    #[error("Federated keyset id mismatch: expected={expected}, found={found}")]
    KeysetIdMismatch {
        /// Derived keyset ID.
        expected: Id,
        /// Configured keyset ID.
        found: Id,
    },
    /// A federation member did not advertise an aggregate keyset from the roster.
    #[error("Federation member {member_id} did not advertise keyset {keyset_id}")]
    MissingAdvertisedFederatedKeyset {
        /// Member whose advertisement was checked.
        member_id: MemberId,
        /// Missing keyset ID.
        keyset_id: Id,
    },
    /// A federation member advertised keys or metadata that do not match the aggregate keyset.
    #[error("Federation member {member_id} advertised mismatched keyset {keyset_id}")]
    AdvertisedFederatedKeysetMismatch {
        /// Member whose advertisement was checked.
        member_id: MemberId,
        /// Mismatched keyset ID.
        keyset_id: Id,
    },
    /// The configured federation ID does not match the derived roster ID.
    #[error("Federation id mismatch: expected={expected}, found={found}")]
    FederationIdMismatch {
        /// Derived federation ID.
        expected: FederationId,
        /// Configured federation ID.
        found: FederationId,
    },
    /// A keyset has no public shares for a member.
    #[error("Federated keyset {keyset_id} has no public shares for member {member_id}")]
    MissingPublicShares {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Member missing public shares.
        member_id: MemberId,
    },
    /// A keyset member has no public share for an aggregate amount key.
    #[error(
        "Federated keyset {keyset_id} has no public share for member {member_id} amount {amount}"
    )]
    MissingPublicShareAmount {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Member missing the share.
        member_id: MemberId,
        /// Amount missing the share.
        amount: Amount,
    },
    /// A public share's signer ID must match the federation member ID.
    #[error(
        "Federated keyset {keyset_id} public share signer mismatch for member {member_id}: signer={signer_id}"
    )]
    PublicShareSignerMismatch {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Member whose share was checked.
        member_id: MemberId,
        /// Signer ID encoded in the share.
        signer_id: u16,
    },
    /// Public shares did not interpolate to the configured aggregate key.
    #[error("Federated keyset {keyset_id} aggregate key mismatch for amount {amount}")]
    AggregateKeyMismatch {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Amount whose aggregate key did not match.
        amount: Amount,
    },
    /// Private config contains a keyset that is not in the public config.
    #[error("Private federation config has unknown keyset {keyset_id}")]
    UnknownPrivateKeyset {
        /// Unknown keyset ID.
        keyset_id: Id,
    },
    /// Private config is missing secret shares for a public keyset.
    #[error("Private federation config is missing keyset {keyset_id}")]
    MissingPrivateKeyset {
        /// Missing keyset ID.
        keyset_id: Id,
    },
    /// Private config is missing a secret share for a keyset amount.
    #[error("Private federation config is missing keyset {keyset_id} amount {amount}")]
    MissingSecretShare {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Missing amount.
        amount: Amount,
    },
    /// A secret share's signer ID must match the local federation member.
    #[error(
        "Private federation keyset {keyset_id} secret share signer mismatch for member {member_id}: signer={signer_id}"
    )]
    SecretShareSignerMismatch {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Local member ID.
        member_id: MemberId,
        /// Signer ID encoded in the secret share.
        signer_id: u16,
    },
    /// A private secret share does not derive to the configured public share.
    #[error("Private federation keyset {keyset_id} share mismatch for member {member_id} amount {amount}")]
    PrivateShareMismatch {
        /// Keyset ID being validated.
        keyset_id: Id,
        /// Local member ID.
        member_id: MemberId,
        /// Amount whose share did not match.
        amount: Amount,
    },
    /// Federated signing requires BLS G1 blinded messages.
    #[error(
        "Federation output for keyset {keyset_id} amount {amount} must use a BLS blinded secret"
    )]
    InvalidFederationBlindedSecret {
        /// Keyset ID being signed.
        keyset_id: Id,
        /// Output amount.
        amount: Amount,
    },
    /// A generated signature share failed verification against the public share.
    #[error(
        "Federation signature share verification failed for keyset {keyset_id} member {member_id} amount {amount}"
    )]
    SignatureShareVerificationFailed {
        /// Keyset ID being signed.
        keyset_id: Id,
        /// Signing member ID.
        member_id: MemberId,
        /// Output amount.
        amount: Amount,
    },
    /// An operation conflicts with an existing quote-scoped operation.
    #[error(
        "Conflicting federation operation for key {conflict_key}: existing={existing_operation_id}, attempted={attempted_operation_id}"
    )]
    ConflictingOperation {
        /// Canonical conflict key.
        conflict_key: String,
        /// Existing operation ID in the journal.
        existing_operation_id: String,
        /// Attempted conflicting operation ID.
        attempted_operation_id: String,
    },
    /// Operation envelope exceeds the configured consensus admission size.
    #[error("Federation operation envelope too large: actual={actual}, max={max}")]
    OperationEnvelopeTooLarge {
        /// Canonical operation envelope bytes.
        actual: usize,
        /// Maximum admitted canonical envelope bytes.
        max: usize,
    },
    /// A requested operation ID was not found in the journal.
    #[error("Unknown federation operation {operation_id}")]
    UnknownOperation {
        /// Operation ID that could not be found.
        operation_id: String,
    },
    /// Operation envelope belongs to a different federation.
    #[error("Federation operation id mismatch: expected={expected}, found={found}")]
    OperationFederationMismatch {
        /// Expected federation ID.
        expected: FederationId,
        /// Found federation ID.
        found: FederationId,
    },
    /// Catch-up response belongs to a different federation.
    #[error("Federation catch-up mismatch: expected={expected}, found={found}")]
    CatchUpFederationMismatch {
        /// Expected federation ID.
        expected: FederationId,
        /// Found federation ID.
        found: FederationId,
    },
    /// Catch-up response did not start at the next local journal index.
    #[error("Federation catch-up index mismatch: expected={expected}, found={found}")]
    CatchUpIndexMismatch {
        /// Expected journal index.
        expected: u64,
        /// Found journal index.
        found: u64,
    },
    /// Catch-up response did not match the expected journal digest.
    #[error(
        "Federation catch-up digest mismatch at index {index}: expected={expected}, found={found}"
    )]
    CatchUpDigestMismatch {
        /// Journal index whose digest did not match.
        index: u64,
        /// Expected journal digest.
        expected: FederationJournalDigest,
        /// Found journal digest.
        found: FederationJournalDigest,
    },
    /// Catch-up entry operation ID did not match its envelope.
    #[error("Federation catch-up operation id mismatch at index {index}: expected={expected}, found={found}")]
    CatchUpOperationIdMismatch {
        /// Journal index whose operation ID did not match.
        index: u64,
        /// Operation ID computed from the entry envelope.
        expected: FederationOperationId,
        /// Operation ID carried by the entry.
        found: FederationOperationId,
    },
    /// Stored journal conflict metadata did not match the canonical operation.
    #[error("Federation journal conflict metadata mismatch at index {index}")]
    JournalConflictKeysMismatch {
        /// Journal index whose conflict metadata did not match.
        index: u64,
    },
    /// A federation REST body hash must be exactly 32 bytes.
    #[error("Invalid federation REST body hash size: expected={expected}, found={found}")]
    InvalidRestBodyHashSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation REST signature must be exactly 64 bytes.
    #[error("Invalid federation REST signature size: expected={expected}, found={found}")]
    InvalidRestSignatureSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation REST signature could not be decoded as a Schnorr signature.
    #[error("Invalid federation REST signature encoding")]
    InvalidRestSignature,
    /// A federation DKG commitment hash must be exactly 32 bytes.
    #[error("Invalid federation DKG commitment hash size: expected={expected}, found={found}")]
    InvalidDkgCommitmentHashSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation DKG commitment message does not match the expected setup.
    #[error("Federation DKG commitment mismatch for member {member_id}: {reason}")]
    DkgCommitmentMismatch {
        /// Member whose DKG commitment material did not match.
        member_id: MemberId,
        /// Human-readable mismatch reason.
        reason: String,
    },
    /// A federation DKG commitment round does not match the public config.
    #[error("Federation DKG commitment round mismatch: {reason}")]
    DkgCommitmentRoundMismatch {
        /// Human-readable mismatch reason.
        reason: String,
    },
    /// A federation DKG transcript hash must be exactly 32 bytes.
    #[error("Invalid federation DKG transcript hash size: expected={expected}, found={found}")]
    InvalidDkgTranscriptHashSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation DKG transcript signature must be exactly 64 bytes.
    #[error(
        "Invalid federation DKG transcript signature size: expected={expected}, found={found}"
    )]
    InvalidDkgTranscriptSignatureSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation DKG transcript signature could not be decoded as a Schnorr signature.
    #[error("Invalid federation DKG transcript signature encoding")]
    InvalidDkgTranscriptSignature,
    /// A federation DKG transcript does not match the public config.
    #[error("Federation DKG transcript mismatch: {reason}")]
    DkgTranscriptMismatch {
        /// Human-readable mismatch reason.
        reason: String,
    },
    /// A federation DKG transcript had an invalid member identity signature.
    #[error("Federation DKG transcript signature verification failed for member {member_id}")]
    DkgTranscriptSignatureVerificationFailed {
        /// Member whose signature failed.
        member_id: MemberId,
    },
    /// Signed REST request used an unsupported payload version.
    #[error("Federation REST version mismatch: expected={expected}, found={found}")]
    RestVersionMismatch {
        /// Expected private REST payload version.
        expected: u16,
        /// Found private REST payload version.
        found: u16,
    },
    /// Signed REST request belongs to a different federation.
    #[error("Federation REST id mismatch: expected={expected}, found={found}")]
    RestFederationMismatch {
        /// Expected federation ID.
        expected: FederationId,
        /// Found federation ID.
        found: FederationId,
    },
    /// Signed REST request was not addressed to the local member.
    #[error("Federation REST receiver mismatch: expected={expected}, found={found}")]
    RestReceiverMismatch {
        /// Expected receiver member.
        expected: MemberId,
        /// Found receiver member.
        found: MemberId,
    },
    /// Signed REST request body did not match its advertised hash.
    #[error("Federation REST body hash mismatch: expected={expected}, found={found}")]
    RestBodyHashMismatch {
        /// Advertised body hash.
        expected: FederationRestBodyHash,
        /// Recomputed body hash.
        found: FederationRestBodyHash,
    },
    /// Signed REST request had an invalid member identity signature.
    #[error("Federation REST signature verification failed for member {member_id}")]
    RestSignatureVerificationFailed {
        /// Sender whose signature was checked.
        member_id: MemberId,
    },
    /// Signed session outcome used an unsupported payload version.
    #[error("Federation session outcome version mismatch: expected={expected}, found={found}")]
    SessionOutcomeVersionMismatch {
        /// Expected session outcome version.
        expected: u16,
        /// Found session outcome version.
        found: u16,
    },
    /// Signed session outcome did not match the catch-up response it accompanied.
    #[error("Federation session outcome mismatch: {reason}")]
    SessionOutcomeMismatch {
        /// Human-readable mismatch reason.
        reason: String,
    },
    /// Signed session outcome had an invalid member identity signature.
    #[error("Federation session outcome signature verification failed for member {member_id}")]
    SessionOutcomeSignatureVerificationFailed {
        /// Member whose signature was checked.
        member_id: MemberId,
    },
    /// Not enough distinct members signed matching session outcomes.
    #[error("Federation session outcome quorum not reached: required={required}, found={found}")]
    SessionOutcomeQuorumNotReached {
        /// Required distinct member signatures.
        required: u16,
        /// Valid distinct member signatures found.
        found: u16,
    },
    /// Signed REST request was sent to an endpoint for a different message kind.
    #[error("Federation REST message kind mismatch: expected={expected:?}, found={found:?}")]
    RestMessageKindMismatch {
        /// Expected message kind for the endpoint.
        expected: FederationRestMessageKind,
        /// Found message kind in the signed envelope.
        found: FederationRestMessageKind,
    },
    /// Signed REST request replayed or moved backwards in the member sequence.
    #[error("Federation REST stale sequence from member {member_id}: last={last}, found={found}")]
    RestStaleSequence {
        /// Sender whose sequence was checked.
        member_id: MemberId,
        /// Highest previously accepted sequence from the sender.
        last: u64,
        /// Sequence carried by the request.
        found: u64,
    },
    /// Hex parsing error.
    #[error(transparent)]
    Hex(#[from] crate::util::hex::Error),
    /// NUT-01 threshold BLS error.
    #[error(transparent)]
    Nut01(#[from] crate::nuts::nut01::Error),
    /// NUT-02 keyset ID error.
    #[error(transparent)]
    Nut02(#[from] crate::nuts::nut02::Error),
    /// Canonical operation encoding error.
    #[error("Federation canonical encoding failed: {0}")]
    CanonicalEncoding(#[from] serde_json::Error),
}

/// Stable identifier for a federation roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationId([u8; 32]);

impl FederationId {
    /// Create a federation ID from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a federation ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidFederationIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a federation ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this federation ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this federation ID as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Non-zero federation member identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberId(u16);

impl MemberId {
    /// Create a member ID.
    pub fn new(id: u16) -> Result<Self> {
        if id == 0 {
            return Err(FederationError::InvalidMemberId(id));
        }

        Ok(Self(id))
    }

    /// Return this member ID as a u16.
    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }

    /// Return this member ID as a threshold BLS signer ID.
    #[must_use]
    pub fn to_bls_signer_id(self) -> BlsSignerId {
        BlsSignerId::new(self.0).expect("member id is non-zero")
    }
}

impl fmt::Display for MemberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for MemberId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for MemberId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = u16::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

/// Federation threshold settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ThresholdParams {
    /// Number of federation members.
    pub member_count: u16,
    /// Number of BLS shares needed to produce a mint signature.
    pub signature_threshold: u16,
    /// Number of members needed to commit consensus items.
    pub consensus_threshold: u16,
}

impl ThresholdParams {
    /// Create validated threshold parameters.
    pub fn new(
        member_count: u16,
        signature_threshold: u16,
        consensus_threshold: u16,
    ) -> Result<Self> {
        let params = Self {
            member_count,
            signature_threshold,
            consensus_threshold,
        };
        params.validate()?;
        Ok(params)
    }

    /// Create threshold params with the default BFT consensus quorum.
    pub fn with_bft_consensus(member_count: u16, signature_threshold: u16) -> Result<Self> {
        let max_faulty = member_count.saturating_sub(1) / 3;
        let consensus_threshold = member_count.saturating_sub(max_faulty);
        Self::new(member_count, signature_threshold, consensus_threshold)
    }

    /// Validate threshold parameters.
    pub fn validate(&self) -> Result<()> {
        if self.member_count == 0
            || self.signature_threshold == 0
            || self.consensus_threshold == 0
            || self.signature_threshold > self.member_count
            || self.consensus_threshold > self.member_count
            || self.consensus_threshold < self.signature_threshold
        {
            return Err(FederationError::InvalidThresholdParams {
                members: self.member_count,
                signature_threshold: self.signature_threshold,
                consensus_threshold: self.consensus_threshold,
            });
        }

        Ok(())
    }
}

const DEFAULT_MAX_PENDING_PAYMENT_QUOTES: u32 = 10_000;
const DEFAULT_PAYMENT_QUOTE_PENDING_TTL_SECS: u64 = 60 * 60;
const DEFAULT_STALE_PAYMENT_OBSERVATION_SECS: u64 = 15 * 60;

fn default_max_pending_payment_quotes() -> u32 {
    DEFAULT_MAX_PENDING_PAYMENT_QUOTES
}

fn default_payment_quote_pending_ttl_secs() -> u64 {
    DEFAULT_PAYMENT_QUOTE_PENDING_TTL_SECS
}

fn default_stale_payment_observation_secs() -> u64 {
    DEFAULT_STALE_PAYMENT_OBSERVATION_SECS
}

/// Payment-observation context with separate mint and melt policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentObservationContext {
    /// Incoming payment observation for a mint quote.
    MintQuote,
    /// Outgoing payment observation for a melt quote.
    MeltQuote,
}

impl fmt::Display for FederationPaymentObservationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MintQuote => f.write_str("mint_quote"),
            Self::MeltQuote => f.write_str("melt_quote"),
        }
    }
}

/// Public policy for deciding when payment observations can mutate quote state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPaymentObservationPolicy {
    /// Incoming mint quote payment verification policy.
    #[serde(default = "FederationPaymentVerificationPolicy::fake_wallet_for_testing")]
    pub mint_quotes: FederationPaymentVerificationPolicy,
    /// Outgoing melt quote payment verification policy.
    #[serde(default = "FederationPaymentVerificationPolicy::fake_wallet_for_testing")]
    pub melt_quotes: FederationPaymentVerificationPolicy,
    /// Maximum pending quotes operators expect before back-pressure or alerting.
    #[serde(default = "default_max_pending_payment_quotes")]
    pub max_pending_quotes: u32,
    /// Seconds a quote may remain pending before operator inspection is expected.
    #[serde(default = "default_payment_quote_pending_ttl_secs")]
    pub pending_quote_ttl_secs: u64,
    /// Seconds after which an unresolved observation is considered stale.
    #[serde(default = "default_stale_payment_observation_secs")]
    pub stale_observation_secs: u64,
}

impl Default for FederationPaymentObservationPolicy {
    fn default() -> Self {
        Self::fake_wallet_for_testing()
    }
}

impl FederationPaymentObservationPolicy {
    /// Create the current fake-wallet MVP policy.
    #[must_use]
    pub fn fake_wallet_for_testing() -> Self {
        Self {
            mint_quotes: FederationPaymentVerificationPolicy::fake_wallet_for_testing(),
            melt_quotes: FederationPaymentVerificationPolicy::fake_wallet_for_testing(),
            max_pending_quotes: DEFAULT_MAX_PENDING_PAYMENT_QUOTES,
            pending_quote_ttl_secs: DEFAULT_PAYMENT_QUOTE_PENDING_TTL_SECS,
            stale_observation_secs: DEFAULT_STALE_PAYMENT_OBSERVATION_SECS,
        }
    }

    /// Create a real-backend policy requiring matching member observations.
    #[must_use]
    pub fn member_quorum(required: u16, trust_model: FederationPaymentTrustModel) -> Self {
        let policy = FederationPaymentVerificationPolicy::member_quorum(required, trust_model);
        Self {
            mint_quotes: policy,
            melt_quotes: policy,
            max_pending_quotes: DEFAULT_MAX_PENDING_PAYMENT_QUOTES,
            pending_quote_ttl_secs: DEFAULT_PAYMENT_QUOTE_PENDING_TTL_SECS,
            stale_observation_secs: DEFAULT_STALE_PAYMENT_OBSERVATION_SECS,
        }
    }

    /// Return the policy for one payment observation context.
    #[must_use]
    pub fn policy_for(
        &self,
        context: FederationPaymentObservationContext,
    ) -> &FederationPaymentVerificationPolicy {
        match context {
            FederationPaymentObservationContext::MintQuote => &self.mint_quotes,
            FederationPaymentObservationContext::MeltQuote => &self.melt_quotes,
        }
    }

    /// Validate this policy against federation threshold parameters.
    pub fn validate(&self, threshold_params: ThresholdParams) -> Result<()> {
        if self.max_pending_quotes == 0 {
            return Err(FederationError::InvalidPaymentObservationLimit {
                field: "max_pending_quotes",
                value: u64::from(self.max_pending_quotes),
            });
        }
        if self.pending_quote_ttl_secs == 0 {
            return Err(FederationError::InvalidPaymentObservationLimit {
                field: "pending_quote_ttl_secs",
                value: self.pending_quote_ttl_secs,
            });
        }
        if self.stale_observation_secs == 0 {
            return Err(FederationError::InvalidPaymentObservationLimit {
                field: "stale_observation_secs",
                value: self.stale_observation_secs,
            });
        }

        self.mint_quotes.validate(
            FederationPaymentObservationContext::MintQuote,
            threshold_params,
        )?;
        self.melt_quotes.validate(
            FederationPaymentObservationContext::MeltQuote,
            threshold_params,
        )?;

        Ok(())
    }
}

/// How members are expected to verify an external payment observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentTrustModel {
    /// Every member can independently read settlement from the same backend.
    SharedBackendReadOnly,
    /// The origin member proves settlement and other members acknowledge it.
    OriginMemberProof,
    /// A threshold gateway or payment processor emits payment observations.
    ThresholdGateway,
    /// Members verify onchain proofs against a consensus block-height view.
    OnchainProof,
}

/// Policy for one class of payment observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FederationPaymentVerificationPolicy {
    /// Test-only policy for the fake wallet backend where one observation finalizes.
    FakeWallet,
    /// Real-backend policy requiring matching observations from a member quorum.
    MemberQuorum {
        /// Required number of matching member observations.
        required: u16,
        /// Trust model describing how members verify the backend event.
        trust_model: FederationPaymentTrustModel,
    },
}

impl FederationPaymentVerificationPolicy {
    /// Create the fake-wallet testing policy.
    #[must_use]
    pub fn fake_wallet_for_testing() -> Self {
        Self::FakeWallet
    }

    /// Create a real-backend member-quorum policy.
    #[must_use]
    pub fn member_quorum(required: u16, trust_model: FederationPaymentTrustModel) -> Self {
        Self::MemberQuorum {
            required,
            trust_model,
        }
    }

    /// Return true when one accepted observation may finalize quote state.
    #[must_use]
    pub fn permits_single_observation_finality(self) -> bool {
        matches!(self, Self::FakeWallet)
    }

    /// Number of matching observations required by this policy.
    #[must_use]
    pub fn required_matching_observations(self) -> u16 {
        match self {
            Self::FakeWallet => 1,
            Self::MemberQuorum { required, .. } => required,
        }
    }

    /// Validate an accumulated observation quorum against this policy.
    pub fn evaluate_quorum(self, quorum: &FederationPaymentObservationQuorum) -> Result<()> {
        if quorum.has_conflicts() {
            return Err(FederationError::PaymentObservationConflict {
                context: quorum.context,
                matching: quorum.matching_count(),
                conflicting: quorum.conflicting_count(),
            });
        }

        let required = self.required_matching_observations();
        if quorum.matching_count() < required {
            return Err(FederationError::PaymentObservationQuorumNotSatisfied {
                context: quorum.context,
                required,
                found: quorum.matching_count(),
            });
        }

        Ok(())
    }

    fn validate(
        self,
        context: FederationPaymentObservationContext,
        threshold_params: ThresholdParams,
    ) -> Result<()> {
        match self {
            Self::FakeWallet => Ok(()),
            Self::MemberQuorum { required, .. } => {
                if required == 0
                    || required > threshold_params.member_count
                    || required < threshold_params.consensus_threshold
                {
                    return Err(FederationError::InvalidPaymentObservationQuorum {
                        context,
                        required,
                        members: threshold_params.member_count,
                        consensus_threshold: threshold_params.consensus_threshold,
                    });
                }

                Ok(())
            }
        }
    }
}

/// Public configuration for one federation member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberConfigPublic {
    /// Stable federation member ID.
    pub member_id: MemberId,
    /// Public Cashu mint REST URL exposed to wallets.
    pub public_mint_url: MintUrl,
    /// Private federation REST URL exposed to federation peers.
    pub federation_api_url: MintUrl,
    /// Public secp256k1 identity key used to authenticate peer traffic.
    pub identity_public_key: PublicKey,
}

impl MemberConfigPublic {
    /// Validate this member against its map key.
    pub fn validate(&self, expected_member_id: MemberId) -> Result<()> {
        if self.member_id != expected_member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: expected_member_id,
                found: self.member_id,
            });
        }

        if !matches!(self.identity_public_key, PublicKey::Secp256k1(_)) {
            return Err(FederationError::InvalidIdentityKeyKind {
                member_id: self.member_id,
            });
        }

        Ok(())
    }
}

/// Public aggregate keyset plus every member's public BLS shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedKeyset {
    /// Cashu keyset ID derived from the aggregate public keys.
    pub id: Id,
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether members should sign new outputs for this keyset.
    pub active: bool,
    /// Input fee in parts per thousand.
    #[serde(default)]
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_expiry: Option<u64>,
    /// Aggregate BLS public key per amount, exposed to wallets as the mint keyset.
    pub aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
    /// Public BLS share per member and amount.
    pub public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
}

impl FederatedKeyset {
    /// Create a federated keyset with a computed NUT-02 v3 keyset ID.
    pub fn new(
        unit: CurrencyUnit,
        active: bool,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
        public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
    ) -> Self {
        let id = Self::derive_keyset_id(&unit, input_fee_ppk, final_expiry, &aggregate_keys);
        Self {
            id,
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            aggregate_keys,
            public_shares,
        }
    }

    /// Derive a NUT-02 v3 keyset ID from aggregate BLS public keys.
    #[must_use]
    pub fn derive_keyset_id(
        unit: &CurrencyUnit,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        aggregate_keys: &BTreeMap<Amount, BlsAggregatePublicKey>,
    ) -> Id {
        let keys = Self::aggregate_keys_as_cashu_keys(aggregate_keys);
        Id::v3_from_data(&keys, unit, input_fee_ppk, final_expiry)
    }

    /// Convert aggregate BLS public keys into a standard Cashu `Keys` map.
    #[must_use]
    pub fn aggregate_keys_as_cashu_keys(
        aggregate_keys: &BTreeMap<Amount, BlsAggregatePublicKey>,
    ) -> Keys {
        Keys::new(
            aggregate_keys
                .iter()
                .map(|(amount, key)| (*amount, PublicKey::from(*key)))
                .collect(),
        )
    }

    /// Return aggregate public keys as a standard Cashu `Keys` map.
    #[must_use]
    pub fn keys(&self) -> Keys {
        Self::aggregate_keys_as_cashu_keys(&self.aggregate_keys)
    }

    /// Validate aggregate keys, keyset ID, member shares, and interpolation.
    pub fn validate(
        &self,
        threshold_params: ThresholdParams,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<()> {
        if self.aggregate_keys.is_empty() {
            return Err(FederationError::EmptyFederatedKeyset { keyset_id: self.id });
        }
        if self.id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion { keyset_id: self.id });
        }

        let expected_id = Self::derive_keyset_id(
            &self.unit,
            self.input_fee_ppk,
            self.final_expiry,
            &self.aggregate_keys,
        );
        if expected_id != self.id {
            return Err(FederationError::KeysetIdMismatch {
                expected: expected_id,
                found: self.id,
            });
        }

        for member_id in self.public_shares.keys() {
            if !members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }

        for member_id in members.keys() {
            let member_shares =
                self.public_shares
                    .get(member_id)
                    .ok_or(FederationError::MissingPublicShares {
                        keyset_id: self.id,
                        member_id: *member_id,
                    })?;

            for amount in self.aggregate_keys.keys() {
                let share =
                    member_shares
                        .get(amount)
                        .ok_or(FederationError::MissingPublicShareAmount {
                            keyset_id: self.id,
                            member_id: *member_id,
                            amount: *amount,
                        })?;
                if share.signer_id().get() != member_id.get() {
                    return Err(FederationError::PublicShareSignerMismatch {
                        keyset_id: self.id,
                        member_id: *member_id,
                        signer_id: share.signer_id().get(),
                    });
                }
            }
        }

        for (amount, aggregate_key) in &self.aggregate_keys {
            let mut shares = Vec::with_capacity(members.len());
            for member_id in members.keys() {
                let member_shares = self.public_shares.get(member_id).ok_or(
                    FederationError::MissingPublicShares {
                        keyset_id: self.id,
                        member_id: *member_id,
                    },
                )?;
                let share =
                    member_shares
                        .get(amount)
                        .ok_or(FederationError::MissingPublicShareAmount {
                            keyset_id: self.id,
                            member_id: *member_id,
                            amount: *amount,
                        })?;
                shares.push(*share);
            }

            let derived = aggregate_public_key_shares(
                usize::from(threshold_params.signature_threshold),
                &shares,
            )?;
            if derived != *aggregate_key {
                return Err(FederationError::AggregateKeyMismatch {
                    keyset_id: self.id,
                    amount: *amount,
                });
            }
        }

        Ok(())
    }

    fn validate_advertised_by_member(
        &self,
        member_id: MemberId,
        advertised: &KeySet,
    ) -> Result<()> {
        if advertised.id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion {
                keyset_id: advertised.id,
            });
        }

        advertised.verify_id()?;
        if advertised.id != self.id
            || advertised.unit != self.unit
            || advertised.input_fee_ppk != self.input_fee_ppk
            || advertised.final_expiry != self.final_expiry
            || advertised.keys != self.keys()
        {
            return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                member_id,
                keyset_id: self.id,
            });
        }
        if let Some(active) = advertised.active {
            if active != self.active {
                return Err(FederationError::AdvertisedFederatedKeysetMismatch {
                    member_id,
                    keyset_id: self.id,
                });
            }
        }

        Ok(())
    }
}

/// Stable hash of one member's revealed DKG polynomial commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgCommitmentHash([u8; 32]);

impl FederationDkgCommitmentHash {
    /// Create a commitment hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a commitment hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgCommitmentHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a commitment hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this commitment hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this commitment hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn for_reveal(reveal: &FederationDkgParticipantCommitmentReveal) -> Result<Self> {
        let canonical = serde_json::to_vec(reveal)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_DKG_COMMITMENT_HASH_VERSION);
        append_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationDkgCommitmentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgCommitmentHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgCommitmentHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgCommitmentHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// A member's pre-reveal DKG commitment hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FederationDkgParticipantCommitment {
    /// Federation roster hash this commitment belongs to.
    pub federation_id: FederationId,
    /// Member that committed to later reveal material.
    pub member_id: MemberId,
    /// Hash of the member's later public commitment reveal.
    pub commitment_hash: FederationDkgCommitmentHash,
}

impl FederationDkgParticipantCommitment {
    /// Build a commitment from a participant reveal.
    pub fn from_reveal(reveal: &FederationDkgParticipantCommitmentReveal) -> Result<Self> {
        Ok(Self {
            federation_id: reveal.federation_id,
            member_id: reveal.member_id,
            commitment_hash: reveal.commitment_hash()?,
        })
    }

    /// Validate this commitment against the federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "federation id mismatch".to_string(),
            });
        }
        let member =
            public_config
                .member(self.member_id)
                .ok_or(FederationError::UnknownMember {
                    member_id: self.member_id,
                })?;
        if member.member_id != self.member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: self.member_id,
                found: member.member_id,
            });
        }

        Ok(())
    }

    /// Validate that a reveal matches this earlier commitment.
    pub fn validate_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        public_config: &FederationConfig,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        reveal.validate_against_config(public_config)?;
        if reveal.member_id != self.member_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "reveal member id mismatch".to_string(),
            });
        }
        let reveal_hash = reveal.commitment_hash()?;
        if reveal_hash != self.commitment_hash {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "reveal hash mismatch".to_string(),
            });
        }

        Ok(())
    }
}

/// Revealed public DKG polynomial commitments for one participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgParticipantCommitmentReveal {
    /// Federation roster hash this reveal belongs to.
    pub federation_id: FederationId,
    /// Member that reveals public polynomial commitments.
    pub member_id: MemberId,
    /// Per-amount public polynomial commitments in G2.
    ///
    /// The vector length for each amount must equal the signature threshold.
    /// Entry 0 is the participant's public constant-term commitment.
    pub coefficient_commitments: BTreeMap<Amount, Vec<BlsAggregatePublicKey>>,
}

impl FederationDkgParticipantCommitmentReveal {
    /// Return the commitment hash that should have been announced before reveal.
    pub fn commitment_hash(&self) -> Result<FederationDkgCommitmentHash> {
        FederationDkgCommitmentHash::for_reveal(self)
    }

    /// Validate this reveal against the federation roster and threshold.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "federation id mismatch".to_string(),
            });
        }
        if !public_config.members.contains_key(&self.member_id) {
            return Err(FederationError::UnknownMember {
                member_id: self.member_id,
            });
        }
        if self.coefficient_commitments.is_empty() {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "missing coefficient commitments".to_string(),
            });
        }

        let expected_coefficients = usize::from(public_config.threshold_params.signature_threshold);
        for (amount, commitments) in &self.coefficient_commitments {
            if commitments.len() != expected_coefficients {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "coefficient commitment count mismatch for amount {amount}: expected={expected_coefficients}, found={}",
                        commitments.len()
                    ),
                });
            }
        }

        Ok(())
    }
}

/// DKG commitment round collected before participants reveal public commitments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgCommitmentRound {
    /// Federation roster hash for this setup round.
    pub federation_id: FederationId,
    /// Deterministic participant order.
    pub participant_order: Vec<MemberId>,
    /// Pre-reveal commitment hashes by member.
    pub commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
}

impl FederationDkgCommitmentRound {
    /// Create and validate a commitment round for a public federation config.
    pub fn new(
        public_config: &FederationConfig,
        commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
    ) -> Result<Self> {
        let round = Self {
            federation_id: public_config.federation_id,
            participant_order: public_config.members.keys().copied().collect(),
            commitments,
        };
        round.validate_against_config(public_config)?;
        Ok(round)
    }

    /// Validate this commitment round against the federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "federation id mismatch".to_string(),
            });
        }
        let expected_order = public_config.members.keys().copied().collect::<Vec<_>>();
        if self.participant_order != expected_order {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "participant order mismatch".to_string(),
            });
        }
        for member_id in self.commitments.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }
        if self.commitments.len() != public_config.members.len() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "commitment count mismatch".to_string(),
            });
        }
        for member_id in public_config.members.keys() {
            let commitment = self.commitments.get(member_id).ok_or(
                FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing commitment for member {member_id}"),
                },
            )?;
            if commitment.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: commitment.member_id,
                });
            }
            commitment.validate_against_config(public_config)?;
        }

        Ok(())
    }

    /// Validate revealed participant commitments against this earlier commitment round.
    pub fn validate_reveals(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        for member_id in reveals.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }
        if reveals.len() != public_config.members.len() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "reveal count mismatch".to_string(),
            });
        }
        for member_id in public_config.members.keys() {
            let reveal =
                reveals
                    .get(member_id)
                    .ok_or(FederationError::DkgCommitmentRoundMismatch {
                        reason: format!("missing reveal for member {member_id}"),
                    })?;
            let commitment = self.commitments.get(member_id).ok_or(
                FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing commitment for member {member_id}"),
                },
            )?;
            commitment.validate_reveal(reveal, public_config)?;
        }

        Ok(())
    }
}

/// Stable hash of the public federation DKG/setup transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgTranscriptHash([u8; 32]);

impl FederationDkgTranscriptHash {
    /// Create a transcript hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a transcript hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgTranscriptHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transcript hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this transcript hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this transcript hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn for_payload(payload: &FederationDkgTranscriptPayload) -> Result<Self> {
        let canonical = serde_json::to_vec(payload)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_DKG_TRANSCRIPT_HASH_VERSION);
        append_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationDkgTranscriptHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgTranscriptHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgTranscriptHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgTranscriptHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Schnorr identity signature over a DKG/setup transcript hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationDkgTranscriptSignature([u8; 64]);

impl FederationDkgTranscriptSignature {
    /// Create a transcript signature from raw Schnorr bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgTranscriptSignatureSize {
                    expected: 64,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transcript signature from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this signature as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    /// Return this signature as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn sign(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        let signature = private_config
            .identity_secret_key
            .sign(&dkg_signature_transcript(transcript_hash))?;
        Ok(Self(signature.serialize()))
    }

    fn verify(
        self,
        transcript_hash: FederationDkgTranscriptHash,
        member: &MemberConfigPublic,
    ) -> Result<()> {
        let signature = Signature::from_slice(&self.0)
            .map_err(|_| FederationError::InvalidDkgTranscriptSignature)?;
        member
            .identity_public_key
            .verify(&dkg_signature_transcript(transcript_hash), &signature)
            .map_err(
                |_| FederationError::DkgTranscriptSignatureVerificationFailed {
                    member_id: member.member_id,
                },
            )
    }
}

impl fmt::Display for FederationDkgTranscriptSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgTranscriptSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgTranscriptSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgTranscriptSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Key generation method recorded by a federation setup transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationDkgSetupMethod {
    /// Test/development setup where one process locally generated all shares.
    TrustedDealer,
    /// Future dealer-free Pedersen DKG setup.
    PedersenDkg,
}

/// Public keyset material committed by a DKG/setup transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgKeysetTranscript {
    /// Federated keyset ID.
    pub keyset_id: Id,
    /// Aggregate BLS public key per amount.
    pub aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
    /// Public BLS shares per member and amount.
    pub public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
}

/// Public, signed DKG/setup transcript payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgTranscriptPayload {
    /// Setup method used to derive the public key material.
    pub setup_method: FederationDkgSetupMethod,
    /// Federation ID bound by this transcript.
    pub federation_id: FederationId,
    /// Threshold parameters bound by this transcript.
    pub threshold_params: ThresholdParams,
    /// Deterministic member order used as DKG participant order.
    pub participant_order: Vec<MemberId>,
    /// Member identity public keys bound by this transcript.
    pub member_identity_public_keys: BTreeMap<MemberId, PublicKey>,
    /// Public aggregate keysets and public member shares.
    pub keysets: Vec<FederationDkgKeysetTranscript>,
}

/// Signed public DKG/setup transcript for audit and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgTranscript {
    /// Public transcript payload.
    pub payload: FederationDkgTranscriptPayload,
    /// Canonical hash of the public payload.
    pub transcript_hash: FederationDkgTranscriptHash,
    /// Identity signatures over the transcript hash.
    pub signatures: BTreeMap<MemberId, FederationDkgTranscriptSignature>,
}

impl FederationDkgTranscript {
    /// Build an unsigned public transcript payload from a public config.
    pub fn payload_from_config(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
    ) -> FederationDkgTranscriptPayload {
        FederationDkgTranscriptPayload {
            setup_method,
            federation_id: public_config.federation_id,
            threshold_params: public_config.threshold_params,
            participant_order: public_config.members.keys().copied().collect(),
            member_identity_public_keys: public_config
                .members
                .iter()
                .map(|(member_id, member)| (*member_id, member.identity_public_key))
                .collect(),
            keysets: public_config
                .keysets
                .iter()
                .map(|keyset| FederationDkgKeysetTranscript {
                    keyset_id: keyset.id,
                    aggregate_keys: keyset.aggregate_keys.clone(),
                    public_shares: keyset.public_shares.clone(),
                })
                .collect(),
        }
    }

    /// Create a signed transcript from already-generated trusted-dealer material.
    pub fn trusted_dealer(
        public_config: &FederationConfig,
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        let payload =
            Self::payload_from_config(FederationDkgSetupMethod::TrustedDealer, public_config);
        let transcript_hash = FederationDkgTranscriptHash::for_payload(&payload)?;
        let mut signatures = BTreeMap::new();
        for member_id in payload.member_identity_public_keys.keys() {
            let private_config =
                private_configs
                    .get(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?;
            signatures.insert(
                *member_id,
                FederationDkgTranscriptSignature::sign(transcript_hash, private_config)?,
            );
        }

        let transcript = Self {
            payload,
            transcript_hash,
            signatures,
        };
        transcript.validate_against_config(public_config)?;
        Ok(transcript)
    }

    /// Validate this transcript against a public federation config.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        let expected_hash = FederationDkgTranscriptHash::for_payload(&self.payload)?;
        if expected_hash != self.transcript_hash {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "transcript hash does not match payload".to_string(),
            });
        }
        if self.payload.federation_id != public_config.federation_id {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "federation id mismatch".to_string(),
            });
        }
        if self.payload.threshold_params != public_config.threshold_params {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "threshold parameter mismatch".to_string(),
            });
        }

        let expected_order = public_config.members.keys().copied().collect::<Vec<_>>();
        if self.payload.participant_order != expected_order {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "participant order mismatch".to_string(),
            });
        }
        let expected_identity_keys = public_config
            .members
            .iter()
            .map(|(member_id, member)| (*member_id, member.identity_public_key))
            .collect::<BTreeMap<_, _>>();
        if self.payload.member_identity_public_keys != expected_identity_keys {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "member identity public keys mismatch".to_string(),
            });
        }

        let expected_keysets = public_config
            .keysets
            .iter()
            .map(|keyset| FederationDkgKeysetTranscript {
                keyset_id: keyset.id,
                aggregate_keys: keyset.aggregate_keys.clone(),
                public_shares: keyset.public_shares.clone(),
            })
            .collect::<Vec<_>>();
        if self.payload.keysets != expected_keysets {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "keyset public material mismatch".to_string(),
            });
        }

        for member_id in public_config.members.keys() {
            let member =
                public_config
                    .members
                    .get(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?;
            let signature =
                self.signatures
                    .get(member_id)
                    .ok_or(FederationError::DkgTranscriptMismatch {
                        reason: format!("missing transcript signature for member {member_id}"),
                    })?;
            signature.verify(self.transcript_hash, member)?;
        }
        for member_id in self.signatures.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }

        Ok(())
    }
}

fn dkg_signature_transcript(transcript_hash: FederationDkgTranscriptHash) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, FEDERATION_DKG_TRANSCRIPT_SIGNATURE_VERSION);
    append_bytes(&mut transcript, &transcript_hash.to_bytes());
    transcript
}

/// Public federation config shared by every member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Stable federation roster ID.
    pub federation_id: FederationId,
    /// Federation threshold settings.
    pub threshold_params: ThresholdParams,
    /// Public member roster keyed by member ID.
    pub members: BTreeMap<MemberId, MemberConfigPublic>,
    /// Federated aggregate keysets.
    pub keysets: Vec<FederatedKeyset>,
    /// Signed public setup transcript for audit and later DKG compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_transcript: Option<FederationDkgTranscript>,
    /// Policy for mint and melt payment observations.
    #[serde(default)]
    pub payment_observation_policy: FederationPaymentObservationPolicy,
}

impl FederationConfig {
    /// Create a federation config and compute its roster ID.
    pub fn new(
        threshold_params: ThresholdParams,
        members: BTreeMap<MemberId, MemberConfigPublic>,
        keysets: Vec<FederatedKeyset>,
    ) -> Result<Self> {
        let federation_id = Self::derive_federation_id(threshold_params, &members)?;
        let config = Self {
            federation_id,
            threshold_params,
            members,
            keysets,
            setup_transcript: None,
            payment_observation_policy: FederationPaymentObservationPolicy::default(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Derive the deterministic federation ID from threshold params and roster.
    pub fn derive_federation_id(
        threshold_params: ThresholdParams,
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> Result<FederationId> {
        threshold_params.validate()?;
        if usize::from(threshold_params.member_count) != members.len() {
            return Err(FederationError::MemberCountMismatch {
                expected: threshold_params.member_count,
                found: members.len(),
            });
        }

        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_ROSTER_HASH_VERSION);
        transcript.extend_from_slice(&threshold_params.member_count.to_be_bytes());
        transcript.extend_from_slice(&threshold_params.signature_threshold.to_be_bytes());
        transcript.extend_from_slice(&threshold_params.consensus_threshold.to_be_bytes());

        for (member_id, member) in members {
            member.validate(*member_id)?;
            transcript.extend_from_slice(&member_id.get().to_be_bytes());
            append_str(&mut transcript, &member.public_mint_url.to_string());
            append_str(&mut transcript, &member.federation_api_url.to_string());
            append_bytes(&mut transcript, &member.identity_public_key.to_bytes());
        }

        Ok(FederationId::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
    }

    /// Validate this config.
    pub fn validate(&self) -> Result<()> {
        let expected_id = Self::derive_federation_id(self.threshold_params, &self.members)?;
        if expected_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: expected_id,
                found: self.federation_id,
            });
        }

        for keyset in &self.keysets {
            keyset.validate(self.threshold_params, &self.members)?;
        }

        if let Some(transcript) = &self.setup_transcript {
            transcript.validate_against_config(self)?;
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;

        Ok(())
    }

    /// Validate that blinded outputs target active v3/BLS aggregate keysets.
    pub fn validate_blinded_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_output(output)?;
        }

        Ok(())
    }

    /// Validate melt change outputs, allowing amount-zero placeholders.
    pub fn validate_blinded_change_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_change_output(output)?;
        }

        Ok(())
    }

    /// Validate one blinded output and return the aggregate keyset it targets.
    pub fn validate_blinded_output(&self, output: &BlindedMessage) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, false)
    }

    /// Validate one melt change output and return the aggregate keyset it targets.
    pub fn validate_blinded_change_output(
        &self,
        output: &BlindedMessage,
    ) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, true)
    }

    fn validate_blinded_output_with_amount_policy(
        &self,
        output: &BlindedMessage,
        allow_zero_amount_placeholder: bool,
    ) -> Result<&FederatedKeyset> {
        if output.keyset_id.get_version() != KeySetVersion::Version02 {
            return Err(FederationError::InvalidFederatedKeysetVersion {
                keyset_id: output.keyset_id,
            });
        }

        let keyset = self
            .keysets
            .iter()
            .find(|keyset| keyset.id == output.keyset_id)
            .ok_or(FederationError::UnknownFederatedKeyset {
                keyset_id: output.keyset_id,
            })?;
        if !keyset.active {
            return Err(FederationError::InactiveFederatedKeyset {
                keyset_id: keyset.id,
            });
        }
        if output.amount != Amount::ZERO || !allow_zero_amount_placeholder {
            if !keyset.aggregate_keys.contains_key(&output.amount) {
                return Err(FederationError::UnknownFederatedKeysetAmount {
                    keyset_id: keyset.id,
                    amount: output.amount,
                });
            }
        }
        if output.amount == Amount::ZERO
            && allow_zero_amount_placeholder
            && keyset.aggregate_keys.is_empty()
        {
            return Err(FederationError::UnknownFederatedKeysetAmount {
                keyset_id: keyset.id,
                amount: output.amount,
            });
        }
        output.blinded_secret.as_bls_g1().map_err(|_| {
            FederationError::InvalidFederationBlindedSecret {
                keyset_id: output.keyset_id,
                amount: output.amount,
            }
        })?;

        Ok(keyset)
    }

    /// Validate keysets advertised by one federation member to a wallet.
    pub fn validate_member_advertised_keysets(
        &self,
        member_id: MemberId,
        advertised_keysets: &[KeySet],
    ) -> Result<()> {
        self.validate()?;
        if !self.members.contains_key(&member_id) {
            return Err(FederationError::UnknownMember { member_id });
        }

        for advertised in advertised_keysets {
            if advertised.id.get_version() != KeySetVersion::Version02 {
                return Err(FederationError::InvalidFederatedKeysetVersion {
                    keyset_id: advertised.id,
                });
            }
            let keyset = self
                .keysets
                .iter()
                .find(|keyset| keyset.id == advertised.id)
                .ok_or(FederationError::UnknownFederatedKeyset {
                    keyset_id: advertised.id,
                })?;
            keyset.validate_advertised_by_member(member_id, advertised)?;
        }

        for keyset in &self.keysets {
            if !advertised_keysets
                .iter()
                .any(|advertised| advertised.id == keyset.id)
            {
                return Err(FederationError::MissingAdvertisedFederatedKeyset {
                    member_id,
                    keyset_id: keyset.id,
                });
            }
        }

        Ok(())
    }

    /// Return the payment verification policy for one observation context.
    #[must_use]
    pub fn payment_verification_policy(
        &self,
        context: FederationPaymentObservationContext,
    ) -> &FederationPaymentVerificationPolicy {
        self.payment_observation_policy.policy_for(context)
    }

    /// Ensure the configured policy allows the fake-wallet single-observation path.
    pub fn ensure_single_payment_observation_can_finalize(
        &self,
        context: FederationPaymentObservationContext,
    ) -> Result<()> {
        let policy = *self.payment_verification_policy(context);
        if policy.permits_single_observation_finality() {
            return Ok(());
        }

        Err(FederationError::PaymentObservationQuorumNotSatisfied {
            context,
            required: policy.required_matching_observations(),
            found: 1,
        })
    }

    /// Return a member by ID.
    #[must_use]
    pub fn member(&self, member_id: MemberId) -> Option<&MemberConfigPublic> {
        self.members.get(&member_id)
    }
}

/// Private federation config stored by one local member.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberConfigPrivate {
    /// Final federation roster hash this private config belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation_id: Option<FederationId>,
    /// Local member ID.
    pub member_id: MemberId,
    /// Private secp256k1 identity key used to authenticate peer traffic.
    pub identity_secret_key: SecretKey,
    /// Local secret BLS shares by keyset ID and amount.
    pub keyset_secret_shares: BTreeMap<Id, BTreeMap<Amount, BlsSecretKeyShare>>,
    /// Hash of the public setup transcript used to derive this member's shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_transcript_hash: Option<FederationDkgTranscriptHash>,
}

impl fmt::Debug for MemberConfigPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberConfigPrivate")
            .field("federation_id", &self.federation_id)
            .field("member_id", &self.member_id)
            .field("keyset_secret_shares", &self.keyset_secret_shares.keys())
            .field("setup_transcript_hash", &self.setup_transcript_hash)
            .finish_non_exhaustive()
    }
}

impl MemberConfigPrivate {
    /// Validate this private config against the public federation config.
    pub fn validate_against_public(&self, public_config: &FederationConfig) -> Result<()> {
        let member =
            public_config
                .member(self.member_id)
                .ok_or(FederationError::MissingMember {
                    member_id: self.member_id,
                })?;

        if !matches!(self.identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(FederationError::InvalidIdentityKeyKind {
                member_id: self.member_id,
            });
        }

        if self.identity_secret_key.public_key() != member.identity_public_key {
            return Err(FederationError::PrivateIdentityMismatch {
                member_id: self.member_id,
            });
        }

        match self.federation_id {
            Some(federation_id) if federation_id != public_config.federation_id => {
                return Err(FederationError::FederationIdMismatch {
                    expected: public_config.federation_id,
                    found: federation_id,
                });
            }
            None if public_config.setup_transcript.is_some() => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config missing federation id".to_string(),
                });
            }
            Some(_) | None => {}
        }

        match (&public_config.setup_transcript, self.setup_transcript_hash) {
            (Some(transcript), Some(hash)) if transcript.transcript_hash != hash => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private setup transcript hash mismatch".to_string(),
                });
            }
            (Some(_), None) => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config missing setup transcript hash".to_string(),
                });
            }
            (None, Some(_)) => {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: "private config has setup transcript hash but public config has no transcript"
                        .to_string(),
                });
            }
            (Some(_), Some(_)) | (None, None) => {}
        }

        for keyset_id in self.keyset_secret_shares.keys() {
            if public_config
                .keysets
                .iter()
                .all(|keyset| keyset.id != *keyset_id)
            {
                return Err(FederationError::UnknownPrivateKeyset {
                    keyset_id: *keyset_id,
                });
            }
        }

        for keyset in &public_config.keysets {
            let secret_shares = self.keyset_secret_shares.get(&keyset.id).ok_or(
                FederationError::MissingPrivateKeyset {
                    keyset_id: keyset.id,
                },
            )?;
            let public_member_shares = keyset.public_shares.get(&self.member_id).ok_or(
                FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id: self.member_id,
                },
            )?;

            for amount in keyset.aggregate_keys.keys() {
                let secret_share =
                    secret_shares
                        .get(amount)
                        .ok_or(FederationError::MissingSecretShare {
                            keyset_id: keyset.id,
                            amount: *amount,
                        })?;
                if secret_share.signer_id().get() != self.member_id.get() {
                    return Err(FederationError::SecretShareSignerMismatch {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        signer_id: secret_share.signer_id().get(),
                    });
                }

                let public_share = public_member_shares.get(amount).ok_or(
                    FederationError::MissingPublicShareAmount {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        amount: *amount,
                    },
                )?;
                if secret_share.public_key_share() != *public_share {
                    return Err(FederationError::PrivateShareMismatch {
                        keyset_id: keyset.id,
                        member_id: self.member_id,
                        amount: *amount,
                    });
                }
            }
        }

        Ok(())
    }
}

fn append_str(transcript: &mut Vec<u8>, value: &str) {
    append_bytes(transcript, value.as_bytes());
}

fn append_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use crate::nuts::nut02::KeySetVersion;
    use cashu::nuts::nut01::{trusted_dealer_keygen, BlsG1PublicKey, BlsSecretKey};

    use super::*;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    fn member(member_id: MemberId) -> MemberConfigPublic {
        let public_mint_url =
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("public mint url");
        let federation_api_url =
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("federation api url");
        MemberConfigPublic {
            member_id,
            public_mint_url,
            federation_api_url,
            identity_public_key: identity_secret(member_id).public_key(),
        }
    }

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn test_roster() -> BTreeMap<MemberId, MemberConfigPublic> {
        [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(|id| (id, member(id)))
            .collect()
    }

    fn test_keyset(
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> (
        FederatedKeyset,
        BTreeMap<MemberId, BTreeMap<Amount, BlsSecretKeyShare>>,
    ) {
        let signer_ids = members
            .keys()
            .map(|member_id| member_id.to_bls_signer_id())
            .collect::<Vec<_>>();
        let amount = Amount::from(1);
        let threshold_keyset =
            trusted_dealer_keygen(&[bls_key(1), bls_key(2)], &signer_ids).expect("keyset");
        let mut aggregate_keys = BTreeMap::new();
        aggregate_keys.insert(amount, threshold_keyset.aggregate_public_key());

        let mut public_shares = BTreeMap::new();
        for share in threshold_keyset.public_shares() {
            public_shares
                .entry(MemberId::new(share.signer_id().get()).expect("member id"))
                .or_insert_with(BTreeMap::new)
                .insert(amount, *share);
        }

        let mut private_shares = BTreeMap::new();
        for share in threshold_keyset.secret_shares() {
            private_shares
                .entry(MemberId::new(share.signer_id().get()).expect("member id"))
                .or_insert_with(BTreeMap::new)
                .insert(amount, share.clone());
        }

        (
            FederatedKeyset::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                aggregate_keys,
                public_shares,
            ),
            private_shares,
        )
    }

    fn dkg_reveal(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> FederationDkgParticipantCommitmentReveal {
        FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            member_id,
            coefficient_commitments: BTreeMap::from([(
                Amount::from(1),
                vec![
                    bls_key(seed).public_key_g2(),
                    bls_key(seed + 10).public_key_g2(),
                ],
            )]),
        }
    }

    fn dkg_reveals(
        public_config: &FederationConfig,
    ) -> BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal> {
        [
            (member_id(1), dkg_reveal(public_config, member_id(1), 11)),
            (member_id(2), dkg_reveal(public_config, member_id(2), 21)),
            (member_id(3), dkg_reveal(public_config, member_id(3), 31)),
        ]
        .into_iter()
        .collect()
    }

    fn dkg_commitments(
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> BTreeMap<MemberId, FederationDkgParticipantCommitment> {
        reveals
            .iter()
            .map(|(member_id, reveal)| {
                (
                    *member_id,
                    FederationDkgParticipantCommitment::from_reveal(reveal)
                        .expect("commitment from reveal"),
                )
            })
            .collect()
    }

    #[test]
    fn member_id_rejects_zero_and_serializes() {
        assert!(matches!(
            MemberId::new(0),
            Err(FederationError::InvalidMemberId(0))
        ));
        let json = serde_json::to_string(&member_id(42)).expect("member json");
        assert_eq!(json, "42");
        assert_eq!(
            serde_json::from_str::<MemberId>(&json).expect("member from json"),
            member_id(42)
        );
    }

    #[test]
    fn threshold_params_validate_bft_defaults() {
        let params = ThresholdParams::with_bft_consensus(5, 3).expect("params");
        assert_eq!(params.consensus_threshold, 4);
        assert!(matches!(
            ThresholdParams::new(5, 4, 3),
            Err(FederationError::InvalidThresholdParams {
                members: 5,
                signature_threshold: 4,
                consensus_threshold: 3
            })
        ));
    }

    #[test]
    fn federation_id_is_deterministic_for_roster() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let id = FederationConfig::derive_federation_id(params, &members).expect("id");
        let config = FederationConfig::new(params, members.clone(), Vec::new()).expect("config");

        assert_eq!(config.federation_id, id);
        assert_eq!(
            serde_json::from_str::<FederationId>(&serde_json::to_string(&id).expect("id json"))
                .expect("id from json"),
            id
        );
    }

    #[test]
    fn federation_config_validates_keyset_public_and_private_shares() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, private_shares) = test_keyset(&members);
        let config =
            FederationConfig::new(params, members.clone(), vec![keyset.clone()]).expect("config");
        assert_eq!(keyset.id.get_version(), KeySetVersion::Version02);
        let config_json = serde_json::to_string(&config).expect("config json");
        assert_eq!(
            serde_json::from_str::<FederationConfig>(&config_json).expect("config from json"),
            config
        );

        for member_id in members.keys() {
            let mut keyset_secret_shares = BTreeMap::new();
            keyset_secret_shares.insert(
                keyset.id,
                private_shares
                    .get(member_id)
                    .expect("member private shares")
                    .clone(),
            );
            let private = MemberConfigPrivate {
                federation_id: None,
                member_id: *member_id,
                identity_secret_key: identity_secret(*member_id),
                keyset_secret_shares,
                setup_transcript_hash: None,
            };
            private
                .validate_against_public(&config)
                .expect("private config");
        }
    }

    #[test]
    fn federation_payment_policy_defaults_to_fake_wallet_for_legacy_configs() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset]).expect("config");
        assert_eq!(
            config.payment_observation_policy,
            FederationPaymentObservationPolicy::fake_wallet_for_testing()
        );
        config
            .ensure_single_payment_observation_can_finalize(
                FederationPaymentObservationContext::MintQuote,
            )
            .expect("fake wallet mint observations can finalize singly");
        config
            .ensure_single_payment_observation_can_finalize(
                FederationPaymentObservationContext::MeltQuote,
            )
            .expect("fake wallet melt observations can finalize singly");

        let mut legacy_json = serde_json::to_value(&config).expect("config json");
        legacy_json
            .as_object_mut()
            .expect("config object")
            .remove("payment_observation_policy");

        let decoded =
            serde_json::from_value::<FederationConfig>(legacy_json).expect("legacy config");
        assert_eq!(
            decoded.payment_observation_policy,
            FederationPaymentObservationPolicy::fake_wallet_for_testing()
        );
        decoded.validate().expect("legacy config validates");
    }

    #[test]
    fn dkg_commitment_round_validates_pre_reveal_hashes() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveals = dkg_reveals(&config);
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&config, commitments).expect("commitment round");

        round
            .validate_reveals(&config, &reveals)
            .expect("reveals match commitments");

        let round_json = serde_json::to_string(&round).expect("round json");
        assert_eq!(
            serde_json::from_str::<FederationDkgCommitmentRound>(&round_json)
                .expect("round from json"),
            round
        );
        let commitment_hash = round
            .commitments
            .get(&member_id(1))
            .expect("member commitment")
            .commitment_hash;
        let hash_json = serde_json::to_string(&commitment_hash).expect("hash json");
        assert_eq!(
            serde_json::from_str::<FederationDkgCommitmentHash>(&hash_json)
                .expect("hash from json"),
            commitment_hash
        );
    }

    #[test]
    fn dkg_commitment_round_rejects_changed_reveal_after_commitment() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let mut reveals = dkg_reveals(&config);
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&config, commitments).expect("commitment round");
        reveals
            .get_mut(&member_id(1))
            .expect("member reveal")
            .coefficient_commitments
            .get_mut(&Amount::from(1))
            .expect("amount commitments")[0] = bls_key(99).public_key_g2();

        assert!(matches!(
            round.validate_reveals(&config, &reveals),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1) && reason == "reveal hash mismatch"
        ));
    }

    #[test]
    fn dkg_commitment_round_rejects_bad_reveal_coefficient_count() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let mut reveals = dkg_reveals(&config);
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&config, commitments).expect("commitment round");
        reveals
            .get_mut(&member_id(1))
            .expect("member reveal")
            .coefficient_commitments
            .get_mut(&Amount::from(1))
            .expect("amount commitments")
            .pop();

        assert!(matches!(
            round.validate_reveals(&config, &reveals),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason.contains("coefficient commitment count mismatch")
        ));
    }

    #[test]
    fn dkg_commitment_round_rejects_missing_or_unknown_participants() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveals = dkg_reveals(&config);
        let mut commitments = dkg_commitments(&reveals);
        commitments.remove(&member_id(2));

        assert!(matches!(
            FederationDkgCommitmentRound::new(&config, commitments),
            Err(FederationError::DkgCommitmentRoundMismatch { reason })
                if reason == "commitment count mismatch"
        ));

        let mut reveals_with_unknown = dkg_reveals(&config);
        reveals_with_unknown.remove(&member_id(2));
        reveals_with_unknown.insert(member_id(99), dkg_reveal(&config, member_id(99), 41));
        let commitments = dkg_commitments(&reveals_with_unknown);
        assert!(matches!(
            FederationDkgCommitmentRound::new(&config, commitments),
            Err(FederationError::UnknownMember { member_id: id }) if id == member_id(99)
        ));
    }

    #[test]
    fn federation_payment_policy_validates_real_backend_quorum() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let mut config = FederationConfig::new(params, members, vec![keyset]).expect("config");

        config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
            params.consensus_threshold,
            FederationPaymentTrustModel::SharedBackendReadOnly,
        );
        config.validate().expect("consensus quorum policy");

        assert!(matches!(
            config.ensure_single_payment_observation_can_finalize(
                FederationPaymentObservationContext::MintQuote
            ),
            Err(FederationError::PaymentObservationQuorumNotSatisfied {
                context: FederationPaymentObservationContext::MintQuote,
                required,
                found: 1,
            }) if required == params.consensus_threshold
        ));

        config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
            params.consensus_threshold - 1,
            FederationPaymentTrustModel::OriginMemberProof,
        );
        assert!(matches!(
            config.validate(),
            Err(FederationError::InvalidPaymentObservationQuorum {
                context: FederationPaymentObservationContext::MintQuote,
                required,
                members: 3,
                consensus_threshold,
            }) if required == params.consensus_threshold - 1
                && consensus_threshold == params.consensus_threshold
        ));

        config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
            params.member_count + 1,
            FederationPaymentTrustModel::ThresholdGateway,
        );
        assert!(matches!(
            config.validate(),
            Err(FederationError::InvalidPaymentObservationQuorum {
                context: FederationPaymentObservationContext::MintQuote,
                required,
                members,
                ..
            }) if required == params.member_count + 1 && members == params.member_count
        ));
    }

    #[test]
    fn federation_payment_policy_evaluates_observation_quorum() {
        let fake = FederationPaymentVerificationPolicy::fake_wallet_for_testing();
        let real = FederationPaymentVerificationPolicy::member_quorum(
            2,
            FederationPaymentTrustModel::SharedBackendReadOnly,
        );
        let single = FederationPaymentObservationQuorum {
            context: FederationPaymentObservationContext::MintQuote,
            required: 2,
            matching_observers: vec![member_id(1)],
            conflicting_observers: Vec::new(),
        };
        fake.evaluate_quorum(&single)
            .expect("fake wallet accepts one matching observer");

        assert!(matches!(
            real.evaluate_quorum(&single),
            Err(FederationError::PaymentObservationQuorumNotSatisfied {
                context: FederationPaymentObservationContext::MintQuote,
                required: 2,
                found: 1,
            })
        ));

        let matching = FederationPaymentObservationQuorum {
            matching_observers: vec![member_id(1), member_id(2)],
            ..single.clone()
        };
        real.evaluate_quorum(&matching)
            .expect("real policy accepts matching quorum");

        let conflicting = FederationPaymentObservationQuorum {
            matching_observers: vec![member_id(1), member_id(2)],
            conflicting_observers: vec![member_id(3)],
            ..single
        };
        assert!(matches!(
            real.evaluate_quorum(&conflicting),
            Err(FederationError::PaymentObservationConflict {
                context: FederationPaymentObservationContext::MintQuote,
                matching: 2,
                conflicting: 1,
            })
        ));
    }

    #[test]
    fn federation_payment_policy_rejects_zero_operator_limits() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let mut config = FederationConfig::new(params, members, vec![keyset]).expect("config");

        config.payment_observation_policy.max_pending_quotes = 0;
        assert!(matches!(
            config.validate(),
            Err(FederationError::InvalidPaymentObservationLimit {
                field: "max_pending_quotes",
                value: 0,
            })
        ));
    }

    #[test]
    fn federation_config_rejects_v2_aggregate_keyset() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (mut keyset, _) = test_keyset(&members);
        keyset.id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );

        assert!(matches!(
            FederationConfig::new(params, members, vec![keyset]),
            Err(FederationError::InvalidFederatedKeysetVersion { .. })
        ));
    }

    #[test]
    fn federation_config_rejects_v1_aggregate_keyset() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (mut keyset, _) = test_keyset(&members);
        keyset.id = Id::v1_from_keys(&keyset.keys());

        assert!(matches!(
            FederationConfig::new(params, members, vec![keyset]),
            Err(FederationError::InvalidFederatedKeysetVersion { .. })
        ));
    }

    #[test]
    fn federation_config_rejects_non_bls_aggregate_keyset_json() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset]).expect("config");
        let mut json = serde_json::to_value(&config).expect("config json");
        let keysets = json
            .get_mut("keysets")
            .and_then(serde_json::Value::as_array_mut)
            .expect("keysets array");
        let aggregate_keys = keysets
            .get_mut(0)
            .and_then(|keyset| keyset.get_mut("aggregate_keys"))
            .and_then(serde_json::Value::as_object_mut)
            .expect("aggregate key map");
        let amount_key = aggregate_keys.keys().next().expect("amount key").to_owned();
        aggregate_keys.insert(
            amount_key,
            serde_json::Value::String(
                "02a8cda4cf448bfce9a9e46e588c06ea1780fcb94e3bbdf3277f42995d403a8b0c".to_string(),
            ),
        );

        assert!(
            serde_json::from_value::<FederationConfig>(json).is_err(),
            "secp-style public key must not deserialize as a BLS aggregate key"
        );
    }

    #[test]
    fn federation_config_allows_zero_amount_change_placeholders() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset.clone()]).expect("config");
        let placeholder = BlindedMessage::new(
            Amount::ZERO,
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"change placeholder")),
        );

        assert!(matches!(
            config.validate_blinded_outputs(&[placeholder.clone()]),
            Err(FederationError::UnknownFederatedKeysetAmount { .. })
        ));
        config
            .validate_blinded_change_outputs(&[placeholder])
            .expect("change placeholder");
    }

    #[test]
    fn federation_config_rejects_non_v3_change_placeholders() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset.clone()]).expect("config");
        let placeholder = BlindedMessage::new(
            Amount::ZERO,
            Id::v2_from_data(
                &keyset.keys(),
                &keyset.unit,
                keyset.input_fee_ppk,
                keyset.final_expiry,
            ),
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"non-v3 change placeholder")),
        );

        assert!(matches!(
            config.validate_blinded_change_outputs(&[placeholder]),
            Err(FederationError::InvalidFederatedKeysetVersion { .. })
        ));
    }

    #[test]
    fn federation_config_rejects_mismatched_share() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (mut keyset, _) = test_keyset(&members);
        let amount = Amount::from(1);
        let wrong_keyset = trusted_dealer_keygen(
            &[bls_key(9), bls_key(10)],
            &[
                member_id(1).to_bls_signer_id(),
                member_id(2).to_bls_signer_id(),
            ],
        )
        .expect("wrong keyset");
        keyset
            .public_shares
            .get_mut(&member_id(1))
            .expect("member shares")
            .insert(amount, wrong_keyset.public_shares()[0]);

        assert!(matches!(
            FederationConfig::new(params, members, vec![keyset]),
            Err(FederationError::AggregateKeyMismatch { amount, .. }) if amount == Amount::from(1)
        ));
    }

    #[test]
    fn federation_config_rejects_wrong_identity_key_kind() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let mut members = test_roster();
        let member_id = member_id(1);
        members
            .get_mut(&member_id)
            .expect("member")
            .identity_public_key = SecretKey::bls_from_reduced_bytes(&[9; 32]).public_key();

        assert!(matches!(
            FederationConfig::new(params, members, Vec::new()),
            Err(FederationError::InvalidIdentityKeyKind { member_id: id }) if id == member_id
        ));
    }
}
