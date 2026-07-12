//! Shared federation configuration types.
//!
//! These types describe a static Cashu federation roster and its threshold BLS
//! key material. They are deliberately transport-agnostic so mintd, tests,
//! private REST peers, and future DKG tooling can reuse the same validation
//! rules.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine as _};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

mod consensus;
mod dkg_driver;
mod journal;
mod operation;
mod signing;
mod transport;
mod trusted_dealer;

/// Maximum members admitted into one dealer-free federation DKG ceremony.
///
/// This keeps the all-to-all participant state and public reveal dimensions
/// bounded independently of the broader federation configuration format.
pub const FEDERATION_MAX_DKG_MEMBERS: usize = 64;
/// Maximum denomination keys admitted into one dealer-free DKG keyset.
///
/// Cashu amounts are `u64`; 64 keys cover the complete conventional power-of-two
/// denomination set while bounding DKG work and transport size.
pub const FEDERATION_MAX_DKG_KEYSET_AMOUNTS: usize = 64;
/// Maximum JSON body accepted by one private DKG REST route.
pub const FEDERATION_MAX_DKG_REQUEST_BODY_BYTES: usize = 1024 * 1024;

pub use self::consensus::{
    FederationAlephBftAdapterStatus, FederationAlephBftConsensusAdapter,
    FederationAlephBftFinalizationHandler, FederationAlephBftFinalizedItem,
    FederationAlephBftFinalizedItemStatus, FederationAlephBftFinalizedUnitOutcome,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitBodyHash,
    FederationAlephBftUnitDataProvider, FederationAlephBftUnitPayload,
    FederationConsensusDefaultItem, FederationConsensusItem, FederationConsensusItemAdmission,
    FederationConsensusItemQueue, FederationConsensusModuleFact, FederationConsensusRuntime,
    FederationQuorumVerifiedCatchUp, FederationVerifiedOperationAdmission,
    FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION, FEDERATION_CONSENSUS_ITEM_VERSION,
    FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES, FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
    FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES, FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
};
pub use self::dkg_driver::{
    FederationDkgDriverPhase, FederationDkgDriverPlan, FederationDkgDriverProgress,
    FederationDkgDriverRecords, ValidatedFederationDkgParticipantState,
};
pub use self::journal::{
    FederationJournalAppendOutcome, FederationJournalConsensusEvidence, FederationJournalDigest,
    FederationJournalEntry, FederationJournalTip, FederationMaterializedStateRow,
    FederationMaterializedStateRowFingerprint, FederationMaterializedStateSnapshot,
    FederationMemberStatusKind, FederationMemberStatusSnapshot, FederationOperationOutcomeHash,
    FederationOperationStatus, FederationStateDigest, FederationStateDigestCheckpoint,
    InMemoryFederationJournal,
};
pub use self::operation::{
    FederationKeysetRotationOperation, FederationMeltOperation, FederationMeltQuoteOperation,
    FederationMeltQuotePaymentObservation, FederationMintOperation, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationMintRequest, FederationOperation,
    FederationOperationBalanceAudit, FederationOperationConflictKey, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationPaymentObservation,
    FederationPaymentObservationEvidenceGroup, FederationPaymentObservationEvidenceGroupSummary,
    FederationPaymentObservationQuarantine, FederationPaymentObservationQuarantineReason,
    FederationPaymentObservationQuarantineRecord, FederationPaymentObservationQuorum,
    FederationPaymentObservationRecord, FederationPaymentObservationReport,
    FederationPaymentObservationReportAction, FederationPaymentObservationReportStatus,
    FederationPaymentObservationReportSummary, FederationRequestTranscript,
    FederationRequestTranscriptHash, FederationSwapOperation,
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES, FEDERATION_OPERATION_VERSION,
};
pub use self::signing::{
    FederationBlindSignatureShare, FederationBlindSignatureShareSet, FederationSignatureShareRecord,
};
pub use self::transport::{
    FederationAlephBftActivityRequest, FederationAlephBftActivityResponse,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationAlephBftNetworkRetryRecord, FederationDkgActivationRequest,
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestBodyHash, FederationRestMessageKind, FederationRestSequenceTracker,
    FederationRestSignature, FederationSessionOutcome, FederationSessionOutcomeQuorum,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    FEDERATION_REST_API_VERSION, FEDERATION_SESSION_OUTCOME_VERSION,
};
pub use self::trusted_dealer::{
    generate_trusted_dealer_configs, generate_trusted_dealer_configs_with_options,
    TrustedDealerConfigBundle, TrustedDealerGenerationOptions, TrustedDealerKeysetSpec,
    TrustedDealerMember,
};

use crate::nuts::nut01::{
    aggregate_public_key_commitments, aggregate_public_key_shares, aggregate_secret_key_shares,
    evaluate_public_key_commitments, evaluate_secret_key_coefficients,
    verify_blind_signature_share, BlsAggregatePublicKey, BlsG1PublicKey, BlsPublicKeyShare,
    BlsSecretKey, BlsSecretKeyShare, BlsSignerId,
};
use crate::nuts::nut02::{KeySet, KeySetVersion};
use crate::nuts::nut06::{FederationInfo, FederationProtocolInfo};
use crate::{
    mint_url::MintUrl, Amount, BlindedMessage, CurrencyUnit, Id, Keys, PublicKey, SecretKey,
};

const FEDERATION_ROSTER_HASH_VERSION: &[u8] = b"cdk-federation-roster-v1";
const FEDERATION_CONFIG_DIGEST_VERSION: &[u8] = b"cdk-federation-config-digest-v1";
const FEDERATION_DKG_COMMITMENT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-commitment-v2";
const FEDERATION_DKG_PARTICIPANT_STATE_VERSION: u16 = 2;
const FEDERATION_DKG_PUBLIC_CEREMONY_VERSION: u16 = 1;
const FEDERATION_DKG_TRANSCRIPT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-transcript-v2";
const FEDERATION_DKG_TRANSCRIPT_SIGNATURE_VERSION: &[u8] =
    b"cdk-federation-dkg-transcript-signature-v1";
const FEDERATION_KEYSET_ROTATION_PROOF_VERSION: &[u8] = b"cdk-federation-keyset-rotation-pop-v1";
const FEDERATION_OPERATOR_REDACTED_VALUE: &str = "[REDACTED]";
const FEDERATION_OPERATOR_REDACTED_PATH: &str = "[REDACTED_PATH]";
const FEDERATION_OPERATOR_REDACTED_URL_CREDENTIAL: &str = "redacted";
/// Versioned prefix for federation invite codes.
pub const FEDERATION_INVITE_PREFIX: &str = "cashufedA";
/// Federation invite payload version supported by this build.
pub const FEDERATION_INVITE_VERSION: u16 = 1;

/// Result alias for federation configuration validation.
pub type Result<T> = std::result::Result<T, FederationError>;

/// Redact likely secrets and local paths from federation operator-facing text.
pub fn redact_federation_operator_message(message: &str) -> String {
    message
        .split_whitespace()
        .map(redact_federation_operator_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_federation_operator_token(token: &str) -> String {
    let (prefix, core, suffix) = split_federation_operator_token(token);
    if core.is_empty() {
        return token.to_string();
    }

    format!("{prefix}{}{suffix}", redact_federation_operator_core(core))
}

fn redact_federation_operator_core(core: &str) -> String {
    let redacted_core = redact_federation_operator_value(core);
    if redacted_core != core {
        return redacted_core;
    }

    if let Some((key, separator, value)) = split_federation_operator_key_value(core) {
        if federation_operator_key_is_sensitive(key) {
            return format!("{key}{separator}{FEDERATION_OPERATOR_REDACTED_VALUE}");
        }

        let redacted_value = redact_federation_operator_value(value);
        if redacted_value != value {
            return format!("{key}{separator}{redacted_value}");
        }
    }

    core.to_string()
}

fn redact_federation_operator_value(value: &str) -> String {
    if let Some(redacted_url) = redact_federation_operator_url(value) {
        return redacted_url;
    }
    if looks_like_federation_operator_path(value) {
        return FEDERATION_OPERATOR_REDACTED_PATH.to_string();
    }

    value.to_string()
}

fn redact_federation_operator_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    if url.scheme().eq_ignore_ascii_case("file") {
        return Some(FEDERATION_OPERATOR_REDACTED_PATH.to_string());
    }
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }

    if url
        .set_username(FEDERATION_OPERATOR_REDACTED_URL_CREDENTIAL)
        .is_err()
    {
        return Some(FEDERATION_OPERATOR_REDACTED_VALUE.to_string());
    }
    if url
        .set_password(Some(FEDERATION_OPERATOR_REDACTED_URL_CREDENTIAL))
        .is_err()
    {
        return Some(FEDERATION_OPERATOR_REDACTED_VALUE.to_string());
    }

    Some(url.to_string())
}

fn looks_like_federation_operator_path(value: &str) -> bool {
    if value.starts_with("~/") || value.starts_with("./") || value.starts_with("../") {
        return true;
    }
    if value.starts_with('/') && !value.starts_with("//") && value.len() > 1 {
        return true;
    }

    let bytes = value.as_bytes();
    bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

fn split_federation_operator_key_value(core: &str) -> Option<(&str, char, &str)> {
    for separator in ['=', ':'] {
        if let Some(index) = core.find(separator) {
            let key = &core[..index];
            let value = &core[index + separator.len_utf8()..];
            if separator == ':' && value.starts_with("//") {
                continue;
            }
            if !value.is_empty() && federation_operator_key_is_structured(key) {
                return Some((key, separator, value));
            }
        }
    }

    None
}

fn federation_operator_key_is_structured(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn federation_operator_key_is_sensitive(key: &str) -> bool {
    let key = key
        .trim_start_matches('-')
        .to_ascii_lowercase()
        .replace('-', "_");
    key.contains("password")
        || key.contains("passwd")
        || key.contains("secret")
        || key.contains("token")
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("mnemonic")
        || key.contains("seed")
        || key.contains("preimage")
        || key.contains("invoice")
        || key.contains("proof")
        || key.contains("private_config")
        || key == "path"
        || key.ends_with("_path")
}

fn split_federation_operator_token(token: &str) -> (&str, &str, &str) {
    let start = token
        .char_indices()
        .find_map(|(index, ch)| {
            if federation_operator_token_prefix(ch) {
                None
            } else {
                Some(index)
            }
        })
        .unwrap_or(token.len());
    if start == token.len() {
        return (token, "", "");
    }

    let end = token
        .char_indices()
        .rev()
        .find_map(|(index, ch)| {
            if index < start || federation_operator_token_suffix(ch) {
                None
            } else {
                Some(index + ch.len_utf8())
            }
        })
        .unwrap_or(start);

    (&token[..start], &token[start..end], &token[end..])
}

fn federation_operator_token_prefix(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'')
}

fn federation_operator_token_suffix(ch: char) -> bool {
    matches!(ch, ')' | ']' | '}' | '>' | '"' | '\'' | ',' | ';')
}

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
    /// A federation config digest must be exactly 32 bytes.
    #[error("Invalid federation config digest size: expected={expected}, found={found}")]
    InvalidFederationConfigDigestSize {
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
    /// A federation operation outcome hash must be exactly 32 bytes.
    #[error("Invalid federation operation outcome hash size: expected={expected}, found={found}")]
    InvalidOperationOutcomeHashSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation request transcript hash must be exactly 32 bytes.
    #[error("Invalid federation request transcript hash size: expected={expected}, found={found}")]
    InvalidRequestTranscriptHashSize {
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
    /// Pending consensus item admission would exceed the configured queue limit.
    #[error("Federation pending consensus mempool full: pending={pending}, max={max}")]
    PendingConsensusMempoolFull {
        /// Number of items already pending.
        pending: usize,
        /// Maximum pending item count.
        max: usize,
    },
    /// Waiting for an accepted operation response would exceed the configured limit.
    #[error("Federation response waiter limit exceeded: waiting={waiting}, max={max}")]
    ResponseWaiterLimitExceeded {
        /// Number of response waiters already registered.
        waiting: usize,
        /// Maximum response waiter count.
        max: usize,
    },
    /// A submitted operation is still pending consensus finalization.
    #[error(
        "Federation operation {operation_id} is pending AlephBFT finalization after {timeout_ms}ms; retry the exact same request"
    )]
    OperationPendingFinalization {
        /// Pending federation operation ID.
        operation_id: FederationOperationId,
        /// Timeout that elapsed before finalization was observed.
        timeout_ms: u64,
    },
    /// A submitted operation was finalized but rejected by consensus validation.
    #[error("Federation operation {operation_id} finalized with terminal status {status}")]
    RejectedOperation {
        /// Rejected federation operation ID.
        operation_id: FederationOperationId,
        /// Terminal journal status.
        status: FederationOperationStatus,
    },
    /// A peer returned a submit-operation response for a different operation.
    #[error("Federation submit-operation response mismatch: expected={expected}, found={found}")]
    SubmitOperationResponseMismatch {
        /// Expected submitted operation ID.
        expected: FederationOperationId,
        /// Operation ID found in the peer response.
        found: FederationOperationId,
    },
    /// A peer acknowledged AlephBFT activity for a different operation.
    #[error("Federation AlephBFT activity response mismatch: expected={expected}, found={found}")]
    AlephBftActivityOperationMismatch {
        /// Expected activity operation ID.
        expected: FederationOperationId,
        /// Operation ID found in the peer response.
        found: FederationOperationId,
    },
    /// A consensus batching or payload limit must be non-zero.
    #[error("Invalid federation consensus limit {field}: value={value}")]
    InvalidConsensusLimit {
        /// Configured limit field.
        field: &'static str,
        /// Invalid value.
        value: u64,
    },
    /// An AlephBFT unit body hash must be exactly 32 bytes.
    #[error("Invalid federation AlephBFT unit body hash size: expected={expected}, found={found}")]
    InvalidAlephBftUnitBodyHashSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// An AlephBFT unit body hash does not match the canonical item body.
    #[error("Federation AlephBFT unit body hash mismatch: expected={expected}, found={found}")]
    AlephBftUnitBodyHashMismatch {
        /// Expected body hash.
        expected: String,
        /// Found body hash.
        found: String,
    },
    /// An AlephBFT unit operation ID list does not match the canonical item body.
    #[error("Federation AlephBFT unit operation ids do not match item body")]
    AlephBftUnitOperationIdsMismatch,
    /// An AlephBFT unit must contain at least one consensus item.
    #[error("Federation AlephBFT unit cannot be empty")]
    EmptyAlephBftUnit,
    /// An AlephBFT unit belongs to a different finalization session.
    #[error("Federation AlephBFT unit session mismatch: expected={expected}, found={found}")]
    AlephBftUnitSessionMismatch {
        /// Expected finalization session.
        expected: u64,
        /// Found payload session.
        found: u64,
    },
    /// An AlephBFT session cannot rotate while local units are awaiting finality.
    #[error(
        "Federation AlephBFT session rotation blocked: current_session={current_session}, requested_session={requested_session}, pending_units={pending_units}, pending_items={pending_items}"
    )]
    PendingAlephBftFinalization {
        /// Session currently accepted by the adapter.
        current_session: u64,
        /// Later session requested by the caller.
        requested_session: u64,
        /// Locally drained units still waiting for finality.
        pending_units: usize,
        /// Consensus items inside locally drained units still waiting for finality.
        pending_items: usize,
    },
    /// An opaque AlephBFT network message could not be decoded.
    #[error("Invalid federation AlephBFT network message: {reason}")]
    InvalidAlephBftNetworkMessage {
        /// Decode rejection reason.
        reason: String,
    },
    /// The same AlephBFT member unit was finalized more than once.
    #[error(
        "Duplicate federation AlephBFT unit: session={session}, member={member_id}, unit_index={unit_index}"
    )]
    DuplicateAlephBftUnit {
        /// Finalization session containing the duplicate unit.
        session: u64,
        /// Member whose unit was duplicated.
        member_id: MemberId,
        /// Member-local unit index.
        unit_index: u64,
    },
    /// Retained AlephBFT finalized-item metadata is not internally consistent.
    #[error("Federation AlephBFT finalized item recovery mismatch: {reason}")]
    AlephBftFinalizedItemRecoveryMismatch {
        /// Human-readable mismatch reason.
        reason: String,
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
    /// Dealer-free DKG needs at least one amount per keyset.
    #[error("Federation DKG keyset for unit {unit} has no amounts")]
    EmptyDkgKeyset {
        /// Unit of the empty keyset.
        unit: CurrencyUnit,
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
    /// A federation member did not advertise federation metadata in mint info.
    #[error("Federation member {member_id} did not advertise federation mint info")]
    MissingFederationMintInfo {
        /// Member whose mint-info response was checked.
        member_id: MemberId,
    },
    /// A federation member advertised a public config digest that does not match the wallet config.
    #[error(
        "Federation member {member_id} advertised config digest mismatch: expected={expected}, found={found}"
    )]
    FederationConfigDigestMismatch {
        /// Member whose mint-info response was checked.
        member_id: MemberId,
        /// Expected public config digest.
        expected: FederationConfigDigest,
        /// Advertised public config digest.
        found: FederationConfigDigest,
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
    /// A local member could not prove possession of the rotation secret share.
    #[error(
        "Federated keyset {keyset_id} proof-of-possession failed for member {member_id} amount {amount}"
    )]
    KeysetProofOfPossessionFailed {
        /// Keyset ID being activated.
        keyset_id: Id,
        /// Member whose local secret share was checked.
        member_id: MemberId,
        /// Amount whose rotation proof failed.
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
    /// Signature-share provenance referenced a different accepted operation.
    #[error("Federation signature-share operation mismatch: expected={expected}, found={found}")]
    SignatureShareOperationMismatch {
        /// Operation ID from the accepted journal entry or provenance.
        expected: FederationOperationId,
        /// Operation ID embedded in the signature share set.
        found: FederationOperationId,
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
    /// Operation-local balance audit found more liabilities than assets.
    #[error(
        "Federation operation balance audit failed for {operation_kind}: assets={assets}, liabilities={liabilities}"
    )]
    NegativeBalanceAudit {
        /// Operation kind being audited.
        operation_kind: FederationOperationKind,
        /// Assets available inside the operation-local audit scope.
        assets: Amount,
        /// Liabilities created inside the operation-local audit scope.
        liabilities: Amount,
    },
    /// Operation-local balance audit overflowed while summing amounts.
    #[error("Federation operation balance audit amount overflow for {operation_kind}")]
    BalanceAuditAmountOverflow {
        /// Operation kind being audited.
        operation_kind: FederationOperationKind,
    },
    /// Operation envelope used an unsupported payload version.
    #[error("Federation operation version mismatch: expected={expected}, found={found}")]
    OperationVersionMismatch {
        /// Expected operation envelope version.
        expected: u16,
        /// Found operation envelope version.
        found: u16,
    },
    /// Consensus item used an unsupported payload version.
    #[error("Federation consensus item version mismatch: expected={expected}, found={found}")]
    ConsensusItemVersionMismatch {
        /// Expected consensus item version.
        expected: u16,
        /// Found consensus item version.
        found: u16,
    },
    /// Federation metadata advertised an unsupported protocol component version.
    #[error(
        "Federation protocol version mismatch for {component}: expected={expected}, found={found}"
    )]
    ProtocolVersionMismatch {
        /// Protocol component being validated.
        component: &'static str,
        /// Expected component version.
        expected: u16,
        /// Found component version.
        found: u16,
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
    /// Catch-up response did not advance the local journal.
    #[error("Federation catch-up made no progress at index {index}")]
    CatchUpNoProgress {
        /// Journal index that still needs a catch-up entry.
        index: u64,
    },
    /// Stored journal conflict metadata did not match the canonical operation.
    #[error("Federation journal conflict metadata mismatch at index {index}")]
    JournalConflictKeysMismatch {
        /// Journal index whose conflict metadata did not match.
        index: u64,
    },
    /// A journal entry that requires consensus evidence did not include it.
    #[error("Federation journal entry is missing consensus evidence at index {index}")]
    JournalConsensusEvidenceMissing {
        /// Journal index whose consensus evidence was missing.
        index: u64,
    },
    /// Stored journal operation outcome hash did not match the accepted entry.
    #[error(
        "Federation journal outcome hash mismatch at index {index}: expected={expected}, found={found}"
    )]
    JournalOutcomeHashMismatch {
        /// Journal index whose outcome hash did not match.
        index: u64,
        /// Expected outcome hash.
        expected: String,
        /// Found outcome hash.
        found: String,
    },
    /// Stored journal state digest did not match the accepted sequence.
    #[error(
        "Federation journal state digest mismatch at index {index}: expected={expected}, found={found}"
    )]
    JournalStateDigestMismatch {
        /// Journal index whose state digest did not match.
        index: u64,
        /// Expected state digest.
        expected: FederationStateDigest,
        /// Found state digest.
        found: FederationStateDigest,
    },
    /// A journal rejection payload appeared on a non-rejected operation.
    #[error("Federation journal rejection state mismatch at index {index}")]
    JournalRejectionStateMismatch {
        /// Journal index whose application state was malformed.
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
    /// DKG ceremony IDs must be exactly 32 bytes.
    #[error("Invalid federation DKG ceremony id size: expected={expected}, found={found}")]
    InvalidDkgCeremonyIdSize {
        /// Required byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A dealer-free DKG roster exceeds the supported resource bound.
    #[error("Federation DKG member limit exceeded: found={found}, max={max}")]
    DkgMemberLimitExceeded {
        /// Configured federation member count.
        found: usize,
        /// Maximum supported dealer-free DKG member count.
        max: usize,
    },
    /// A dealer-free DKG keyset exceeds the supported denomination bound.
    #[error("Federation DKG keyset amount limit exceeded: found={found}, max={max}")]
    DkgKeysetAmountLimitExceeded {
        /// Configured denomination count.
        found: usize,
        /// Maximum supported denomination count.
        max: usize,
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
    /// A federation invite code could not be parsed.
    #[error("Invalid federation invite: {reason}")]
    InvalidFederationInvite {
        /// Human-readable parse or validation failure.
        reason: String,
    },
    /// A federation invite code used an unsupported payload version.
    #[error("Federation invite version mismatch: expected={expected}, found={found}")]
    FederationInviteVersionMismatch {
        /// Expected invite payload version.
        expected: u16,
        /// Found invite payload version.
        found: u16,
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

/// Digest of the complete public federation config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationConfigDigest([u8; 32]);

impl FederationConfigDigest {
    /// Create a federation config digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a federation config digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidFederationConfigDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a federation config digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this federation config digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this federation config digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationConfigDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationConfigDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationConfigDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationConfigDigest {
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
        struct MemberIdVisitor;

        impl de::Visitor<'_> for MemberIdVisitor {
            type Value = MemberId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-zero u16 federation member id")
            }

            fn visit_u16<E>(self, value: u16) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                MemberId::new(value).map_err(E::custom)
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = u16::try_from(value).map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = u16::try_from(value).map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let id = value.parse::<u16>().map_err(E::custom)?;
                MemberId::new(id).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(MemberIdVisitor)
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

/// Federation protocol component versions advertised in public metadata.
///
/// These versions let members and federation-aware wallets fail closed when a
/// config requires operation, consensus, private REST, or session-outcome
/// semantics they do not understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationProtocolVersions {
    /// Canonical operation envelope version.
    pub operation: u16,
    /// Consensus item envelope version.
    pub consensus_item: u16,
    /// Private federation REST API version.
    pub rest_api: u16,
    /// Signed session-outcome payload version.
    pub session_outcome: u16,
}

impl FederationProtocolVersions {
    /// Return the protocol versions supported by this CDK build.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            operation: FEDERATION_OPERATION_VERSION,
            consensus_item: FEDERATION_CONSENSUS_ITEM_VERSION,
            rest_api: FEDERATION_REST_API_VERSION,
            session_outcome: FEDERATION_SESSION_OUTCOME_VERSION,
        }
    }

    /// Validate that every advertised protocol component is supported.
    pub fn validate(&self) -> Result<()> {
        let current = Self::current();
        validate_protocol_component("operation", current.operation, self.operation)?;
        validate_protocol_component(
            "consensus_item",
            current.consensus_item,
            self.consensus_item,
        )?;
        validate_protocol_component("rest_api", current.rest_api, self.rest_api)?;
        validate_protocol_component(
            "session_outcome",
            current.session_outcome,
            self.session_outcome,
        )?;
        Ok(())
    }

    fn to_mint_info(self) -> FederationProtocolInfo {
        FederationProtocolInfo::new(
            self.operation,
            self.consensus_item,
            self.rest_api,
            self.session_outcome,
        )
    }

    fn from_mint_info(info: FederationProtocolInfo) -> Self {
        Self {
            operation: info.operation,
            consensus_item: info.consensus_item,
            rest_api: info.rest_api,
            session_outcome: info.session_outcome,
        }
    }
}

impl Default for FederationProtocolVersions {
    fn default() -> Self {
        Self::current()
    }
}

fn validate_protocol_component(component: &'static str, expected: u16, found: u16) -> Result<()> {
    if expected == found {
        return Ok(());
    }

    Err(FederationError::ProtocolVersionMismatch {
        component,
        expected,
        found,
    })
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
#[serde(deny_unknown_fields)]
pub struct FederationPaymentObservationPolicy {
    /// Incoming mint quote payment verification policy.
    pub mint_quotes: FederationPaymentVerificationPolicy,
    /// Outgoing melt quote payment verification policy.
    pub melt_quotes: FederationPaymentVerificationPolicy,
}

impl FederationPaymentObservationPolicy {
    /// Create the development/test policy where one accepted observation finalizes.
    #[must_use]
    pub fn development_single_observation_for_testing() -> Self {
        Self {
            mint_quotes:
                FederationPaymentVerificationPolicy::development_single_observation_for_testing(),
            melt_quotes:
                FederationPaymentVerificationPolicy::development_single_observation_for_testing(),
        }
    }

    /// Create a real-backend policy requiring matching member observations.
    #[must_use]
    pub fn member_quorum(required: u16) -> Self {
        let policy = FederationPaymentVerificationPolicy::member_quorum(required);
        Self {
            mint_quotes: policy,
            melt_quotes: policy,
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

/// Policy for one class of payment observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationPaymentVerificationPolicy {
    /// Test/development policy where one accepted observation finalizes.
    #[serde(rename = "development_single_observation")]
    DevelopmentSingleObservation,
    /// Real-backend policy requiring matching observations from a member quorum.
    MemberQuorum {
        /// Required number of matching member observations.
        required: u16,
    },
}

impl FederationPaymentVerificationPolicy {
    /// Create the development/test single-observation policy.
    #[must_use]
    pub fn development_single_observation_for_testing() -> Self {
        Self::DevelopmentSingleObservation
    }

    /// Create a real-backend member-quorum policy.
    #[must_use]
    pub fn member_quorum(required: u16) -> Self {
        Self::MemberQuorum { required }
    }

    /// Return true when one accepted observation may finalize quote state.
    #[must_use]
    pub fn permits_single_observation_finality(self) -> bool {
        matches!(self, Self::DevelopmentSingleObservation)
    }

    /// Number of matching observations required by this policy.
    #[must_use]
    pub fn required_matching_observations(self) -> u16 {
        match self {
            Self::DevelopmentSingleObservation => 1,
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
            Self::DevelopmentSingleObservation => Ok(()),
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

/// Public DKG-derived keyset plus one member's local secret BLS shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgMemberKeysetShare {
    /// Federation roster this completed DKG keyset belongs to.
    pub federation_id: FederationId,
    /// Local federation member that owns the secret shares.
    pub member_id: MemberId,
    /// Public aggregate keyset derived from all revealed DKG commitments.
    pub keyset: FederatedKeyset,
    /// Complete public ceremony that produced this keyset.
    pub public_ceremony: FederationDkgPublicCeremony,
    /// Final local secret BLS share per amount for one federation member.
    pub secret_shares: BTreeMap<Amount, BlsSecretKeyShare>,
}

impl FederationDkgMemberKeysetShare {
    /// Hash locally derived durable DKG output without reevaluating public polynomials.
    ///
    /// This is the hot-path cache boundary used while collecting transcript
    /// signatures. Callers must supply records produced locally by
    /// [`FederationDkgCommitmentRound::derive_member_keyset_share`]. Final
    /// startup activation and config export still use
    /// [`FederationDkgFinalizedMemberConfig::from_records`] to independently
    /// rederive and verify all public shares.
    pub fn transcript_hash_for_locally_derived_records(
        setup_public_config: &FederationConfig,
        keyset_shares: &[Self],
    ) -> Result<FederationDkgTranscriptHash> {
        setup_public_config.validate()?;
        if !setup_public_config.keysets.is_empty() || setup_public_config.setup_transcript.is_some()
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "locally derived DKG records require a setup-only public config"
                    .to_string(),
            });
        }
        let local_member = keyset_shares
            .first()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "missing local DKG keyset share before transcript signature".to_string(),
            })?
            .member_id;
        let mut keysets_by_id = BTreeMap::new();
        let mut ceremonies_by_keyset_id = BTreeMap::new();
        for share in keyset_shares {
            if share.federation_id != setup_public_config.federation_id {
                return Err(FederationError::FederationIdMismatch {
                    expected: setup_public_config.federation_id,
                    found: share.federation_id,
                });
            }
            if share.member_id != local_member {
                return Err(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                });
            }
            share
                .public_ceremony
                .validate_against_config(setup_public_config)?;
            share
                .keyset
                .validate_metadata(&setup_public_config.members)?;
            let keyset_spec = &share
                .public_ceremony
                .reveals
                .values()
                .next()
                .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                    reason: "locally derived DKG ceremony has no reveals".to_string(),
                })?
                .keyset_spec;
            let keyset_amounts = share
                .keyset
                .aggregate_keys
                .keys()
                .copied()
                .collect::<BTreeSet<_>>();
            if share.keyset.unit != keyset_spec.unit
                || share.keyset.active != keyset_spec.active
                || share.keyset.input_fee_ppk != keyset_spec.input_fee_ppk
                || share.keyset.final_expiry != keyset_spec.final_expiry
                || keyset_amounts != keyset_spec.amounts
            {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "locally derived DKG keyset policy mismatch for {}",
                        share.keyset.id
                    ),
                });
            }
            match keysets_by_id.insert(share.keyset.id, share.keyset.clone()) {
                Some(existing) if existing != share.keyset => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("conflicting local DKG keyset {}", share.keyset.id),
                    });
                }
                Some(_) | None => {}
            }
            match ceremonies_by_keyset_id.insert(share.keyset.id, share.public_ceremony.clone()) {
                Some(existing) if existing != share.public_ceremony => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting local public DKG ceremony for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let mut final_public_config = setup_public_config.clone();
        final_public_config.keysets = keysets_by_id.into_values().collect();
        final_public_config.setup_transcript = None;
        final_public_config.validate_wallet_runtime_metadata()?;
        let public_ceremonies = ceremonies_by_keyset_id.into_values().collect::<Vec<_>>();
        let payload = FederationDkgTranscript::payload_from_canonical_ceremonies(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
            public_ceremonies,
        );
        FederationDkgTranscriptHash::for_payload(&payload)
    }
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
        self.validate_metadata(members)?;

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

    fn validate_metadata(&self, members: &BTreeMap<MemberId, MemberConfigPublic>) -> Result<()> {
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

        Ok(())
    }

    /// Validate that one member controls the local secret shares for this rotation keyset.
    ///
    /// This is an activation-time proof gate: the member signs a deterministic,
    /// domain-separated challenge for each amount with its local BLS secret share
    /// and immediately verifies the proof against the published public share.
    pub fn validate_member_rotation_proof_of_possession(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
        secret_shares: &BTreeMap<Amount, BlsSecretKeyShare>,
    ) -> Result<()> {
        let public_member_shares =
            self.public_shares
                .get(&member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: self.id,
                    member_id,
                })?;

        for (amount, aggregate_key) in &self.aggregate_keys {
            let secret_share =
                secret_shares
                    .get(amount)
                    .ok_or(FederationError::MissingSecretShare {
                        keyset_id: self.id,
                        amount: *amount,
                    })?;
            if secret_share.signer_id().get() != member_id.get() {
                return Err(FederationError::SecretShareSignerMismatch {
                    keyset_id: self.id,
                    member_id,
                    signer_id: secret_share.signer_id().get(),
                });
            }

            let public_share = public_member_shares.get(amount).ok_or(
                FederationError::MissingPublicShareAmount {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                },
            )?;
            if secret_share.public_key_share() != *public_share {
                return Err(FederationError::PrivateShareMismatch {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                });
            }

            let challenge = self.rotation_proof_challenge(
                federation_id,
                member_id,
                *amount,
                aggregate_key,
                public_share,
            );
            let proof = secret_share.blind_sign_share(&challenge);
            let proof_valid = verify_blind_signature_share(&proof, &challenge, public_share)
                .map_err(|_| FederationError::KeysetProofOfPossessionFailed {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                })?;
            if !proof_valid {
                return Err(FederationError::KeysetProofOfPossessionFailed {
                    keyset_id: self.id,
                    member_id,
                    amount: *amount,
                });
            }
        }

        Ok(())
    }

    fn rotation_proof_challenge(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
        amount: Amount,
        aggregate_key: &BlsAggregatePublicKey,
        public_share: &BlsPublicKeyShare,
    ) -> BlsG1PublicKey {
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_KEYSET_ROTATION_PROOF_VERSION);
        append_bytes(&mut transcript, &federation_id.to_bytes());
        append_str(&mut transcript, &self.id.to_string());
        append_str(&mut transcript, &self.unit.to_string());
        append_bytes(&mut transcript, &self.input_fee_ppk.to_be_bytes());
        match self.final_expiry {
            Some(expiry) => {
                append_bytes(&mut transcript, &[1]);
                append_bytes(&mut transcript, &expiry.to_be_bytes());
            }
            None => append_bytes(&mut transcript, &[0]),
        }
        append_bytes(&mut transcript, &member_id.get().to_be_bytes());
        append_str(&mut transcript, &amount.to_string());
        append_bytes(&mut transcript, &aggregate_key.to_bytes());
        append_bytes(&mut transcript, &public_share.to_bytes());

        BlsG1PublicKey::hash_to_curve(&transcript)
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

/// Unique identifier that domain-separates one dealer-free DKG ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgCeremonyId([u8; 32]);

impl FederationDkgCeremonyId {
    /// Create a ceremony ID from 32 bytes of operator-provided entropy.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a ceremony ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgCeremonyIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a ceremony ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return the ceremony ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return the ceremony ID as lowercase hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationDkgCeremonyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgCeremonyId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgCeremonyId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgCeremonyId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Public keyset policy agreed before a dealer-free DKG ceremony starts.
///
/// Every participant commits to this specification together with its public
/// polynomial commitments. A federation therefore cannot complete a ceremony
/// when members disagree about the unit, fees, expiry, activation state, or
/// supported denominations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgKeysetSpec {
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether the completed keyset should sign new outputs.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    pub final_expiry: Option<u64>,
    /// Denominations for which every participant generates a polynomial.
    pub amounts: BTreeSet<Amount>,
}

impl FederationDkgKeysetSpec {
    /// Create and validate a dealer-free DKG keyset specification.
    pub fn new(
        unit: CurrencyUnit,
        active: bool,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        amounts: BTreeSet<Amount>,
    ) -> Result<Self> {
        let spec = Self {
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            amounts,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Validate that this ceremony creates a bounded, non-empty denomination set.
    pub fn validate(&self) -> Result<()> {
        if self.amounts.is_empty() {
            return Err(FederationError::EmptyDkgKeyset {
                unit: self.unit.clone(),
            });
        }
        if self.amounts.len() > FEDERATION_MAX_DKG_KEYSET_AMOUNTS {
            return Err(FederationError::DkgKeysetAmountLimitExceeded {
                found: self.amounts.len(),
                max: FEDERATION_MAX_DKG_KEYSET_AMOUNTS,
            });
        }
        Ok(())
    }

    /// Validate this keyset together with the bounded dealer-free DKG roster.
    pub fn validate_for_federation(&self, public_config: &FederationConfig) -> Result<()> {
        self.validate()?;
        if public_config.members.len() > FEDERATION_MAX_DKG_MEMBERS {
            return Err(FederationError::DkgMemberLimitExceeded {
                found: public_config.members.len(),
                max: FEDERATION_MAX_DKG_MEMBERS,
            });
        }
        Ok(())
    }
}

/// Durable lifecycle state for one dealer-free DKG ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationDkgCeremonyState {
    /// The ceremony may accept setup messages and advance its driver.
    Active,
    /// The ceremony was explicitly replaced before completion.
    Aborted,
    /// Every configured member confirmed the finalized configuration.
    Complete,
}

impl FederationDkgCeremonyState {
    /// Return the stable operator-facing state label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Aborted => "aborted",
            Self::Complete => "complete",
        }
    }
}

/// Durable operator and driver record for one dealer-free DKG ceremony.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgCeremonyRecord {
    /// Federation roster and threshold this ceremony belongs to.
    pub federation_id: FederationId,
    /// Unique ceremony identifier.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Local member that owns this database record.
    pub member_id: MemberId,
    /// Exact keyset policy committed by the ceremony.
    pub keyset_spec: FederationDkgKeysetSpec,
    /// Current durable lifecycle state.
    pub state: FederationDkgCeremonyState,
    /// Explicit predecessor replaced by this ceremony, when any.
    pub replaces_ceremony_id: Option<FederationDkgCeremonyId>,
    /// Explicit successor that caused this ceremony to abort, when any.
    pub replacement_ceremony_id: Option<FederationDkgCeremonyId>,
}

impl FederationDkgCeremonyRecord {
    /// Create a new active ceremony record.
    #[must_use]
    pub fn active(
        federation_id: FederationId,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
        replaces_ceremony_id: Option<FederationDkgCeremonyId>,
    ) -> Self {
        Self {
            federation_id,
            ceremony_id,
            member_id,
            keyset_spec,
            state: FederationDkgCeremonyState::Active,
            replaces_ceremony_id,
            replacement_ceremony_id: None,
        }
    }

    /// Return the exact aborted successor state for an active record.
    pub fn aborted_by(&self, replacement_ceremony_id: FederationDkgCeremonyId) -> Result<Self> {
        if self.state != FederationDkgCeremonyState::Active
            || self.replacement_ceremony_id.is_some()
            || replacement_ceremony_id == self.ceremony_id
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG ceremony {} cannot transition from {} to aborted by {}",
                    self.ceremony_id,
                    self.state.as_str(),
                    replacement_ceremony_id
                ),
            });
        }
        let mut aborted = self.clone();
        aborted.state = FederationDkgCeremonyState::Aborted;
        aborted.replacement_ceremony_id = Some(replacement_ceremony_id);
        Ok(aborted)
    }

    /// Return the exact completed successor state for an active record.
    pub fn completed(&self) -> Result<Self> {
        if self.state != FederationDkgCeremonyState::Active
            || self.replacement_ceremony_id.is_some()
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG ceremony {} cannot transition from {} to complete",
                    self.ceremony_id,
                    self.state.as_str()
                ),
            });
        }
        let mut completed = self.clone();
        completed.state = FederationDkgCeremonyState::Complete;
        Ok(completed)
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
    /// Unique DKG ceremony this commitment belongs to.
    pub ceremony_id: FederationDkgCeremonyId,
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
            ceremony_id: reveal.ceremony_id,
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
        if reveal.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "ceremony id mismatch".to_string(),
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
    /// Unique DKG ceremony this reveal belongs to.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Member that reveals public polynomial commitments.
    pub member_id: MemberId,
    /// Keyset policy and denomination set committed by this participant.
    pub keyset_spec: FederationDkgKeysetSpec,
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
        self.keyset_spec.validate_for_federation(public_config)?;
        if self.coefficient_commitments.is_empty() {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "missing coefficient commitments".to_string(),
            });
        }

        let committed_amounts = self
            .coefficient_commitments
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if committed_amounts != self.keyset_spec.amounts {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "keyset specification amount set mismatch".to_string(),
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

    /// Verify one private contribution against this participant's revealed polynomial.
    pub fn validate_secret_share_request(
        &self,
        public_config: &FederationConfig,
        request: &FederationDkgSecretShareRequest,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        request.validate_sender_receiver(public_config, self.member_id, request.receiver)?;
        if request.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!(
                    "secret contribution ceremony id mismatch for receiver {}",
                    request.receiver
                ),
            });
        }
        let request_amounts = request
            .secret_shares
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if request_amounts != self.keyset_spec.amounts {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!(
                    "secret contribution amount set mismatch for receiver {}",
                    request.receiver
                ),
            });
        }
        for (amount, secret_share) in &request.secret_shares {
            let commitments = self.coefficient_commitments.get(amount).ok_or(
                FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!("missing public commitments for amount {amount}"),
                },
            )?;
            let expected_share =
                evaluate_public_key_commitments(request.receiver.to_bls_signer_id(), commitments)?;
            if secret_share.public_key_share() != expected_share {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "secret contribution does not match public commitments for receiver {} amount {amount}",
                        request.receiver
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Private, restartable state owned by one dealer-free DKG participant.
///
/// Generation creates one random polynomial per denomination and immediately
/// evaluates it for every configured receiver. Only the public reveal and the
/// already-evaluated per-recipient contributions remain in this state; the
/// polynomial coefficients are dropped and zeroized before generation returns.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgParticipantState {
    version: u16,
    federation_id: FederationId,
    ceremony_id: FederationDkgCeremonyId,
    member_id: MemberId,
    reveal: FederationDkgParticipantCommitmentReveal,
    secret_share_requests: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
}

impl fmt::Debug for FederationDkgParticipantState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FederationDkgParticipantState")
            .field("version", &self.version)
            .field("federation_id", &self.federation_id)
            .field("ceremony_id", &self.ceremony_id)
            .field("member_id", &self.member_id)
            .field("keyset_spec", &self.reveal.keyset_spec)
            .field("receiver_count", &self.secret_share_requests.len())
            .finish_non_exhaustive()
    }
}

impl FederationDkgParticipantState {
    /// Generate private state for one cooperative dealer-free DKG participant.
    pub fn generate(
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Result<Self> {
        let state =
            Self::generate_by_construction(public_config, ceremony_id, member_id, keyset_spec)?;
        state.validate(public_config)?;
        Ok(state)
    }

    fn generate_by_construction(
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Result<Self> {
        public_config.validate()?;
        if !public_config.members.contains_key(&member_id) {
            return Err(FederationError::MissingMember { member_id });
        }
        keyset_spec.validate_for_federation(public_config)?;

        let coefficient_count = usize::from(public_config.threshold_params.signature_threshold);
        let coefficients = keyset_spec
            .amounts
            .iter()
            .map(|amount| {
                let coefficients = (0..coefficient_count)
                    .map(|_| BlsSecretKey::generate())
                    .collect::<Vec<_>>();
                (*amount, coefficients)
            })
            .collect::<BTreeMap<_, _>>();
        let coefficient_commitments = coefficients
            .iter()
            .map(|(amount, coefficients)| {
                (
                    *amount,
                    coefficients
                        .iter()
                        .map(BlsSecretKey::public_key_g2)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let reveal = FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            ceremony_id,
            member_id,
            keyset_spec,
            coefficient_commitments,
        };

        let mut secret_share_requests = BTreeMap::new();
        for receiver in public_config.members.keys().copied() {
            let secret_shares = coefficients
                .iter()
                .map(|(amount, coefficients)| {
                    Ok((
                        *amount,
                        evaluate_secret_key_coefficients(
                            receiver.to_bls_signer_id(),
                            coefficients,
                        )?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            secret_share_requests.insert(
                receiver,
                FederationDkgSecretShareRequest::new(
                    public_config.federation_id,
                    ceremony_id,
                    member_id,
                    receiver,
                    secret_shares,
                ),
            );
        }

        Ok(Self {
            version: FEDERATION_DKG_PARTICIPANT_STATE_VERSION,
            federation_id: public_config.federation_id,
            ceremony_id,
            member_id,
            reveal,
            secret_share_requests,
        })
    }

    /// Validate private participant state before resuming a ceremony.
    pub fn validate(&self, public_config: &FederationConfig) -> Result<()> {
        public_config.validate()?;
        if self.version != FEDERATION_DKG_PARTICIPANT_STATE_VERSION {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!("unsupported participant state version {}", self.version),
            });
        }
        if self.federation_id != public_config.federation_id
            || self.reveal.federation_id != self.federation_id
        {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        if self.reveal.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "participant state ceremony id mismatch".to_string(),
            });
        }
        if self.reveal.member_id != self.member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: self.member_id,
                found: self.reveal.member_id,
            });
        }
        self.reveal.validate_against_config(public_config)?;

        let expected_receivers = public_config
            .members
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let receivers = self
            .secret_share_requests
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if receivers != expected_receivers {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "participant receiver set mismatch".to_string(),
            });
        }

        for (receiver, request) in &self.secret_share_requests {
            if request.receiver != *receiver {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "secret contribution receiver mismatch: expected={receiver}, found={}",
                        request.receiver
                    ),
                });
            }
            self.reveal
                .validate_secret_share_request(public_config, request)?;
        }

        Ok(())
    }

    /// Return this participant's member ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Return the ceremony ID bound to every participant message.
    #[must_use]
    pub fn ceremony_id(&self) -> FederationDkgCeremonyId {
        self.ceremony_id
    }

    /// Return the keyset specification committed by this participant.
    #[must_use]
    pub fn keyset_spec(&self) -> &FederationDkgKeysetSpec {
        &self.reveal.keyset_spec
    }

    /// Build this participant's first-round commitment request.
    pub fn commitment_request(&self) -> Result<FederationDkgCommitmentRequest> {
        Ok(FederationDkgCommitmentRequest::new(
            FederationDkgParticipantCommitment::from_reveal(&self.reveal)?,
        ))
    }

    /// Build this participant's public commitment-reveal request.
    #[must_use]
    pub fn reveal_request(&self) -> FederationDkgRevealRequest {
        FederationDkgRevealRequest::new(self.reveal.clone())
    }

    /// Return the private contribution addressed to one configured receiver.
    pub fn secret_share_request(
        &self,
        receiver: MemberId,
    ) -> Result<FederationDkgSecretShareRequest> {
        self.secret_share_requests
            .get(&receiver)
            .cloned()
            .ok_or(FederationError::MissingMember {
                member_id: receiver,
            })
    }
}

/// DKG commitment round collected before participants reveal public commitments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgCommitmentRound {
    /// Federation roster hash for this setup round.
    pub federation_id: FederationId,
    /// Unique DKG ceremony represented by this round.
    pub ceremony_id: FederationDkgCeremonyId,
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
        let ceremony_id = commitments
            .values()
            .next()
            .ok_or(FederationError::DkgCommitmentRoundMismatch {
                reason: "missing commitments".to_string(),
            })?
            .ceremony_id;
        let round = Self {
            federation_id: public_config.federation_id,
            ceremony_id,
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
            if commitment.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "commitment ceremony id mismatch".to_string(),
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
            if reveal.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "reveal ceremony id mismatch".to_string(),
                });
            }
        }

        let first_member =
            self.participant_order
                .first()
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: "empty participant order".to_string(),
                })?;
        let expected_spec = &reveals
            .get(first_member)
            .ok_or(FederationError::DkgCommitmentRoundMismatch {
                reason: format!("missing reveal for member {first_member}"),
            })?
            .keyset_spec;
        for (member_id, reveal) in reveals {
            if &reveal.keyset_spec != expected_spec {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "keyset specification mismatch".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Derive a federated aggregate keyset from a fully revealed DKG round.
    ///
    /// Every participant contributes a public polynomial commitment for each
    /// amount. The final aggregate mint key for an amount is the sum of all
    /// constant-term commitments, and each member's public share is the sum of
    /// every participant polynomial evaluated at that member ID.
    pub fn derive_federated_keyset(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<FederatedKeyset> {
        self.validate_reveals(public_config, reveals)?;
        let first_member_id =
            self.participant_order
                .first()
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: "empty participant order".to_string(),
                })?;
        let first_reveal =
            reveals
                .get(first_member_id)
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing reveal for member {first_member_id}"),
                })?;
        let keyset_spec = &first_reveal.keyset_spec;

        let mut aggregate_keys = BTreeMap::new();
        let mut public_shares = public_config
            .members
            .keys()
            .map(|member_id| (*member_id, BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();
        for amount in &keyset_spec.amounts {
            let coefficient_count = usize::from(public_config.threshold_params.signature_threshold);
            let aggregate_coefficients = (0..coefficient_count)
                .map(|coefficient_index| {
                    let participant_coefficients = self
                        .participant_order
                        .iter()
                        .map(|participant_id| {
                            let commitments = dkg_reveal_amount_commitments(
                                reveals,
                                *participant_id,
                                *amount,
                            )?;
                            commitments.get(coefficient_index).copied().ok_or(
                                FederationError::DkgCommitmentMismatch {
                                    member_id: *participant_id,
                                    reason: format!(
                                        "missing coefficient commitment {coefficient_index} for amount {amount}"
                                    ),
                                },
                            )
                        })
                        .collect::<Result<Vec<_>>>()?;
                    aggregate_public_key_commitments(&participant_coefficients)
                        .map_err(Into::into)
                })
                .collect::<Result<Vec<_>>>()?;
            aggregate_keys.insert(
                *amount,
                *aggregate_coefficients.first().ok_or(
                    FederationError::DkgCommitmentRoundMismatch {
                        reason: format!("empty aggregate polynomial for amount {amount}"),
                    },
                )?,
            );

            for member_id in public_config.members.keys() {
                let signer_id = member_id.to_bls_signer_id();
                let public_share =
                    evaluate_public_key_commitments(signer_id, &aggregate_coefficients)?;
                public_shares
                    .get_mut(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?
                    .insert(*amount, public_share);
            }
        }

        let keyset = FederatedKeyset::new(
            keyset_spec.unit.clone(),
            keyset_spec.active,
            keyset_spec.input_fee_ppk,
            keyset_spec.final_expiry,
            aggregate_keys,
            public_shares,
        );
        keyset.validate(public_config.threshold_params, &public_config.members)?;
        Ok(keyset)
    }

    /// Derive one member's final local secret shares for a DKG keyset.
    ///
    /// `participant_secret_shares` contains the private same-signer scalar
    /// contribution received from each DKG participant. Each contribution is
    /// checked against that participant's revealed public polynomial
    /// commitments before the final local share is aggregated.
    pub fn derive_member_keyset_share(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
        member_id: MemberId,
        participant_secret_shares: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    ) -> Result<FederationDkgMemberKeysetShare> {
        if !public_config.members.contains_key(&member_id) {
            return Err(FederationError::MissingMember { member_id });
        }
        for (participant_id, request) in participant_secret_shares {
            request.validate_sender_receiver(public_config, *participant_id, member_id)?;
            if request.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *participant_id,
                    reason: "secret contribution belongs to another ceremony".to_string(),
                });
            }
        }
        let keyset = self.derive_federated_keyset(public_config, reveals)?;
        let signer_id = member_id.to_bls_signer_id();
        let public_member_shares =
            keyset
                .public_shares
                .get(&member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id,
                })?;

        let mut secret_shares = BTreeMap::new();
        for amount in keyset.aggregate_keys.keys() {
            let mut participant_shares = Vec::with_capacity(self.participant_order.len());
            for participant_id in &self.participant_order {
                let secret_share = dkg_participant_secret_share(
                    participant_secret_shares,
                    *participant_id,
                    *amount,
                )?;
                if secret_share.signer_id() != signer_id {
                    return Err(FederationError::SecretShareSignerMismatch {
                        keyset_id: keyset.id,
                        member_id,
                        signer_id: secret_share.signer_id().get(),
                    });
                }

                let commitments = dkg_reveal_amount_commitments(reveals, *participant_id, *amount)?;
                let expected_public_share =
                    evaluate_public_key_commitments(signer_id, commitments)?;
                if secret_share.public_key_share() != expected_public_share {
                    return Err(FederationError::DkgCommitmentMismatch {
                        member_id: *participant_id,
                        reason: format!("local secret contribution mismatch for amount {amount}"),
                    });
                }
                participant_shares.push(secret_share.clone());
            }

            let secret_share = aggregate_secret_key_shares(signer_id, &participant_shares)?;
            let public_share = public_member_shares.get(amount).ok_or(
                FederationError::MissingPublicShareAmount {
                    keyset_id: keyset.id,
                    member_id,
                    amount: *amount,
                },
            )?;
            if secret_share.public_key_share() != *public_share {
                return Err(FederationError::PrivateShareMismatch {
                    keyset_id: keyset.id,
                    member_id,
                    amount: *amount,
                });
            }
            secret_shares.insert(*amount, secret_share);
        }

        let public_ceremony =
            FederationDkgPublicCeremony::new(public_config, self.clone(), reveals.clone())?;
        Ok(FederationDkgMemberKeysetShare {
            federation_id: public_config.federation_id,
            member_id,
            keyset,
            public_ceremony,
            secret_shares,
        })
    }
}

/// Complete public record of one dealer-free DKG ceremony.
///
/// This deliberately excludes private scalar contributions. The commitment
/// round and every reveal are sufficient to audit the commit-before-reveal
/// exchange and deterministically rederive the aggregate keyset and all public
/// member shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgPublicCeremony {
    /// Public ceremony record format version.
    pub version: u16,
    /// Pre-reveal commitment round for every federation member.
    pub commitment_round: FederationDkgCommitmentRound,
    /// Revealed public polynomial commitments for every federation member.
    pub reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
}

impl FederationDkgPublicCeremony {
    /// Build and validate a complete public DKG ceremony record.
    pub fn new(
        public_config: &FederationConfig,
        commitment_round: FederationDkgCommitmentRound,
        reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<Self> {
        let ceremony = Self {
            version: FEDERATION_DKG_PUBLIC_CEREMONY_VERSION,
            commitment_round,
            reveals,
        };
        ceremony.validate_against_config(public_config)?;
        Ok(ceremony)
    }

    /// Return the unique ID that domain-separates this ceremony.
    #[must_use]
    pub fn ceremony_id(&self) -> FederationDkgCeremonyId {
        self.commitment_round.ceremony_id
    }

    /// Validate the full commitment/reveal exchange against a federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.version != FEDERATION_DKG_PUBLIC_CEREMONY_VERSION {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "public ceremony version mismatch: expected={}, found={}",
                    FEDERATION_DKG_PUBLIC_CEREMONY_VERSION, self.version
                ),
            });
        }
        self.commitment_round
            .validate_reveals(public_config, &self.reveals)
    }

    /// Rederive the public aggregate keyset committed by this ceremony.
    pub fn derive_federated_keyset(
        &self,
        public_config: &FederationConfig,
    ) -> Result<FederatedKeyset> {
        self.validate_against_config(public_config)?;
        self.commitment_round
            .derive_federated_keyset(public_config, &self.reveals)
    }
}

fn dkg_reveal_amount_commitments(
    reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    member_id: MemberId,
    amount: Amount,
) -> Result<&[BlsAggregatePublicKey]> {
    let reveal = reveals
        .get(&member_id)
        .ok_or(FederationError::DkgCommitmentRoundMismatch {
            reason: format!("missing reveal for member {member_id}"),
        })?;
    let commitments = reveal.coefficient_commitments.get(&amount).ok_or(
        FederationError::DkgCommitmentMismatch {
            member_id,
            reason: format!("missing coefficient commitments for amount {amount}"),
        },
    )?;
    Ok(commitments)
}

fn dkg_participant_secret_share(
    participant_secret_shares: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    member_id: MemberId,
    amount: Amount,
) -> Result<&BlsSecretKeyShare> {
    let contribution = participant_secret_shares.get(&member_id).ok_or(
        FederationError::DkgCommitmentMismatch {
            member_id,
            reason: "missing local secret contributions".to_string(),
        },
    )?;
    let secret_share =
        contribution
            .secret_shares
            .get(&amount)
            .ok_or(FederationError::DkgCommitmentMismatch {
                member_id,
                reason: format!("missing local secret contribution for amount {amount}"),
            })?;
    Ok(secret_share)
}

/// In-memory DKG setup state collected from authenticated member setup messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgSetupState {
    /// Federation roster hash for this setup state.
    pub federation_id: FederationId,
    /// Ceremony accepted by the first commitment, if setup has started.
    pub ceremony_id: Option<FederationDkgCeremonyId>,
    /// Deterministic participant order.
    pub participant_order: Vec<MemberId>,
    /// Pre-reveal commitment hashes collected by member.
    pub commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
    /// Revealed public polynomial commitments collected by member.
    pub reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
}

impl FederationDkgSetupState {
    /// Create an empty DKG setup state for a public federation config.
    #[must_use]
    pub fn new(public_config: &FederationConfig) -> Self {
        Self {
            federation_id: public_config.federation_id,
            ceremony_id: None,
            participant_order: public_config.members.keys().copied().collect(),
            commitments: BTreeMap::new(),
            reveals: BTreeMap::new(),
        }
    }

    /// Validate this setup state against the federation roster.
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
        if self.commitments.is_empty() && self.ceremony_id.is_some() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "setup state has a ceremony id without commitments".to_string(),
            });
        }
        for (member_id, commitment) in &self.commitments {
            if commitment.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: commitment.member_id,
                });
            }
            if Some(commitment.ceremony_id) != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "setup-state commitment ceremony id mismatch".to_string(),
                });
            }
            commitment.validate_against_config(public_config)?;
        }
        for (member_id, reveal) in &self.reveals {
            if reveal.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: reveal.member_id,
                });
            }
            if Some(reveal.ceremony_id) != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "setup-state reveal ceremony id mismatch".to_string(),
                });
            }
            reveal.validate_against_config(public_config)?;
            let commitment =
                self.commitments
                    .get(member_id)
                    .ok_or(FederationError::DkgCommitmentMismatch {
                        member_id: *member_id,
                        reason: "missing prior commitment".to_string(),
                    })?;
            commitment.validate_reveal(reveal, public_config)?;
        }

        Ok(())
    }

    /// Insert a member commitment.
    ///
    /// Returns `true` when the state changed and `false` for an idempotent duplicate.
    pub fn insert_commitment(
        &mut self,
        public_config: &FederationConfig,
        commitment: FederationDkgParticipantCommitment,
    ) -> Result<bool> {
        self.validate_against_config(public_config)?;
        commitment.validate_against_config(public_config)?;
        if let Some(existing) = self.commitments.get(&commitment.member_id) {
            if existing == &commitment {
                return Ok(false);
            }
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: commitment.member_id,
                reason: "conflicting commitment".to_string(),
            });
        }
        match self.ceremony_id {
            Some(ceremony_id) if ceremony_id != commitment.ceremony_id => {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: commitment.member_id,
                    reason: "commitment belongs to another ceremony".to_string(),
                });
            }
            Some(_) => {}
            None => self.ceremony_id = Some(commitment.ceremony_id),
        }
        self.commitments.insert(commitment.member_id, commitment);
        Ok(true)
    }

    /// Insert a member reveal after its matching commitment is known.
    ///
    /// Returns `true` when the state changed and `false` for an idempotent duplicate.
    pub fn insert_reveal(
        &mut self,
        public_config: &FederationConfig,
        reveal: FederationDkgParticipantCommitmentReveal,
    ) -> Result<bool> {
        self.validate_against_config(public_config)?;
        reveal.validate_against_config(public_config)?;
        if let Some(existing) = self.reveals.get(&reveal.member_id) {
            if existing == &reveal {
                return Ok(false);
            }
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "conflicting reveal".to_string(),
            });
        }
        let commitment = self.commitments.get(&reveal.member_id).ok_or(
            FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "missing prior commitment".to_string(),
            },
        )?;
        if Some(reveal.ceremony_id) != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "reveal belongs to another ceremony".to_string(),
            });
        }
        commitment.validate_reveal(&reveal, public_config)?;
        self.reveals.insert(reveal.member_id, reveal);
        Ok(true)
    }

    /// Return whether all member commitments have been collected.
    #[must_use]
    pub fn commitments_complete(&self, public_config: &FederationConfig) -> bool {
        self.commitments.len() == public_config.members.len()
    }

    /// Return whether all member reveals have been collected.
    #[must_use]
    pub fn reveals_complete(&self, public_config: &FederationConfig) -> bool {
        self.reveals.len() == public_config.members.len()
    }

    /// Return the fully validated round once all commitments and reveals are present.
    pub fn complete_round(
        &self,
        public_config: &FederationConfig,
    ) -> Result<Option<FederationDkgCommitmentRound>> {
        self.validate_against_config(public_config)?;
        if !self.commitments_complete(public_config) || !self.reveals_complete(public_config) {
            return Ok(None);
        }
        let round = FederationDkgCommitmentRound::new(public_config, self.commitments.clone())?;
        round.validate_reveals(public_config, &self.reveals)?;
        Ok(Some(round))
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

    /// Sign a DKG/setup transcript hash with one member identity key.
    pub fn sign(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::sign_with(
            transcript_hash,
            private_config,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    /// Sign a DKG/setup transcript hash without auxiliary randomness.
    pub fn sign_no_aux_rand(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::sign_with(
            transcript_hash,
            private_config,
            FederationSchnorrSigningMode::NoAuxRand,
        )
    }

    fn sign_with(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
        mode: FederationSchnorrSigningMode,
    ) -> Result<Self> {
        let transcript = dkg_signature_transcript(transcript_hash);
        let signature = match mode {
            FederationSchnorrSigningMode::RandomAux => {
                private_config.identity_secret_key.sign(&transcript)?
            }
            FederationSchnorrSigningMode::NoAuxRand => private_config
                .identity_secret_key
                .sign_no_aux_rand(&transcript)?,
        };
        Ok(Self(signature.serialize()))
    }

    /// Verify this signature against one member identity public key.
    pub fn verify(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederationSchnorrSigningMode {
    RandomAux,
    NoAuxRand,
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
    /// Dealer-free Pedersen DKG setup with a complete public ceremony record.
    PedersenDkg,
}

/// Public keyset material committed by a DKG/setup transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgKeysetTranscript {
    /// Federated keyset ID.
    pub keyset_id: Id,
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether the keyset signs new outputs.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    pub final_expiry: Option<u64>,
    /// Aggregate BLS public key per amount.
    pub aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
    /// Public BLS shares per member and amount.
    pub public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
}

impl FederationDkgKeysetTranscript {
    fn federated_keyset(&self) -> FederatedKeyset {
        FederatedKeyset {
            id: self.keyset_id,
            unit: self.unit.clone(),
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
            final_expiry: self.final_expiry,
            aggregate_keys: self.aggregate_keys.clone(),
            public_shares: self.public_shares.clone(),
        }
    }

    fn matches_immutable_keyset_material(&self, keyset: &FederatedKeyset) -> bool {
        self.keyset_id == keyset.id
            && self.unit == keyset.unit
            && self.input_fee_ppk == keyset.input_fee_ppk
            && self.final_expiry == keyset.final_expiry
            && self.aggregate_keys == keyset.aggregate_keys
            && self.public_shares == keyset.public_shares
    }
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
    /// Complete public ceremonies, one per Pedersen-DKG keyset in keyset order.
    pub ceremonies: Vec<FederationDkgPublicCeremony>,
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
    fn payload_from_config(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<FederationDkgTranscriptPayload> {
        let ceremonies = Self::canonical_ceremonies(setup_method, public_config, ceremonies)?;
        Ok(Self::payload_from_canonical_ceremonies(
            setup_method,
            public_config,
            ceremonies,
        ))
    }

    fn payload_from_canonical_ceremonies(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: Vec<FederationDkgPublicCeremony>,
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
                    unit: keyset.unit.clone(),
                    active: keyset.active,
                    input_fee_ppk: keyset.input_fee_ppk,
                    final_expiry: keyset.final_expiry,
                    aggregate_keys: keyset.aggregate_keys.clone(),
                    public_shares: keyset.public_shares.clone(),
                })
                .collect(),
            ceremonies,
        }
    }

    /// Derive the transcript hash that members must sign for this setup method and config.
    ///
    /// Trusted-dealer setup requires an empty ceremony slice. Pedersen DKG
    /// requires exactly one complete, keyset-matching ceremony per public keyset.
    pub fn hash_for_config(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<FederationDkgTranscriptHash> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
        FederationDkgTranscriptHash::for_payload(&payload)
    }

    /// Create a transcript from externally collected member signatures.
    ///
    /// The ceremony requirements are the same as [`Self::hash_for_config`].
    pub fn from_signatures(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        signatures: BTreeMap<MemberId, FederationDkgTranscriptSignature>,
    ) -> Result<Self> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
        let transcript_hash = FederationDkgTranscriptHash::for_payload(&payload)?;
        let transcript = Self {
            payload,
            transcript_hash,
            signatures,
        };
        transcript.validate_against_config(public_config)?;
        Ok(transcript)
    }

    /// Create a signed transcript from already-generated trusted-dealer material.
    pub fn trusted_dealer(
        public_config: &FederationConfig,
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::TrustedDealer,
            public_config,
            &[],
            private_configs,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    /// Create a trusted-dealer transcript with reproducible no-aux-rand signatures.
    pub fn trusted_dealer_no_aux_rand(
        public_config: &FederationConfig,
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::TrustedDealer,
            public_config,
            &[],
            private_configs,
            FederationSchnorrSigningMode::NoAuxRand,
        )
    }

    /// Create a signed transcript from dealer-free Pedersen DKG public material.
    pub fn pedersen_dkg(
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::PedersenDkg,
            public_config,
            ceremonies,
            private_configs,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    fn signed_setup_with_mode(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
        mode: FederationSchnorrSigningMode,
    ) -> Result<Self> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
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
                FederationDkgTranscriptSignature::sign_with(transcript_hash, private_config, mode)?,
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

    fn canonical_ceremonies(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<Vec<FederationDkgPublicCeremony>> {
        match setup_method {
            FederationDkgSetupMethod::TrustedDealer => {
                if !ceremonies.is_empty() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: "trusted-dealer transcript contains DKG ceremonies".to_string(),
                    });
                }
                Ok(Vec::new())
            }
            FederationDkgSetupMethod::PedersenDkg => {
                if ceremonies.is_empty() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: "Pedersen DKG transcript is missing public ceremonies".to_string(),
                    });
                }
                if ceremonies.len() != public_config.keysets.len() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "Pedersen DKG ceremony count mismatch: expected={}, found={}",
                            public_config.keysets.len(),
                            ceremonies.len()
                        ),
                    });
                }

                let mut ceremony_ids = BTreeSet::new();
                let mut ceremonies_by_keyset = BTreeMap::new();
                for ceremony in ceremonies {
                    if !ceremony_ids.insert(ceremony.ceremony_id()) {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!("duplicate DKG ceremony id {}", ceremony.ceremony_id()),
                        });
                    }
                    let keyset = ceremony.derive_federated_keyset(public_config)?;
                    let keyset_id = keyset.id;
                    if ceremonies_by_keyset
                        .insert(keyset_id, (keyset, ceremony.clone()))
                        .is_some()
                    {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!("duplicate DKG ceremony for keyset {keyset_id}"),
                        });
                    }
                }

                let mut canonical = Vec::with_capacity(public_config.keysets.len());
                for expected_keyset in &public_config.keysets {
                    let (derived_keyset, ceremony) = ceremonies_by_keyset
                        .remove(&expected_keyset.id)
                        .ok_or(FederationError::DkgTranscriptMismatch {
                            reason: format!(
                                "missing public DKG ceremony for keyset {}",
                                expected_keyset.id
                            ),
                        })?;
                    if &derived_keyset != expected_keyset {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!(
                                "public DKG ceremony does not derive keyset {}",
                                expected_keyset.id
                            ),
                        });
                    }
                    canonical.push(ceremony);
                }
                if let Some(keyset_id) = ceremonies_by_keyset.keys().next() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("unexpected public DKG ceremony for keyset {keyset_id}"),
                    });
                }
                Ok(canonical)
            }
        }
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
                unit: keyset.unit.clone(),
                active: keyset.active,
                input_fee_ppk: keyset.input_fee_ppk,
                final_expiry: keyset.final_expiry,
                aggregate_keys: keyset.aggregate_keys.clone(),
                public_shares: keyset.public_shares.clone(),
            })
            .collect::<Vec<_>>();
        if self.payload.keysets != expected_keysets {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "keyset public material mismatch".to_string(),
            });
        }
        let canonical_ceremonies = Self::canonical_ceremonies(
            self.payload.setup_method,
            public_config,
            &self.payload.ceremonies,
        )?;
        if self.payload.ceremonies != canonical_ceremonies {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "public DKG ceremonies are not in keyset order".to_string(),
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

    /// Validate this historical setup transcript against a later runtime config.
    ///
    /// The signed payload is verified exactly against the setup-time keyset
    /// snapshot. Consensus key rotations may subsequently change activation
    /// flags and append keysets, but may not remove or mutate any immutable
    /// setup key material.
    pub fn validate_setup_lineage_against_config(
        &self,
        public_config: &FederationConfig,
    ) -> Result<()> {
        let mut setup_config = public_config.clone();
        setup_config.keysets = self
            .payload
            .keysets
            .iter()
            .map(FederationDkgKeysetTranscript::federated_keyset)
            .collect();
        setup_config.setup_transcript = None;
        setup_config.validate()?;
        self.validate_against_config(&setup_config)?;

        for setup_keyset in &self.payload.keysets {
            let current_keyset = public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == setup_keyset.keyset_id)
                .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "setup keyset {} is missing from current config",
                        setup_keyset.keyset_id
                    ),
                })?;
            if !setup_keyset.matches_immutable_keyset_material(current_keyset) {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "setup keyset {} public material changed",
                        setup_keyset.keyset_id
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Final public and local-private member config material assembled from DKG records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgFinalizedMemberConfig {
    /// Public federation config with DKG-derived keysets and setup transcript.
    pub public_config: FederationConfig,
    /// Local private member config with DKG-derived secret shares and transcript binding.
    pub private_config: MemberConfigPrivate,
}

impl FederationDkgFinalizedMemberConfig {
    /// Assemble final member config material from durable DKG completion records.
    pub fn from_records(
        setup_public_config: &FederationConfig,
        local_private_config: &MemberConfigPrivate,
        keyset_shares: &[FederationDkgMemberKeysetShare],
        transcript_signature_records: &[FederationDkgTranscriptSignatureRequest],
    ) -> Result<Self> {
        setup_public_config.validate()?;
        let local_member = local_private_config.member_id;
        if !setup_public_config.members.contains_key(&local_member) {
            return Err(FederationError::MissingMember {
                member_id: local_member,
            });
        }

        let mut keysets_by_id = BTreeMap::new();
        let mut ceremonies_by_keyset_id = BTreeMap::new();
        let mut secret_shares_by_keyset = BTreeMap::new();
        for share in keyset_shares {
            if share.federation_id != setup_public_config.federation_id {
                return Err(FederationError::FederationIdMismatch {
                    expected: setup_public_config.federation_id,
                    found: share.federation_id,
                });
            }
            if share.member_id != local_member {
                return Err(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                });
            }
            let ceremony_keyset = share
                .public_ceremony
                .derive_federated_keyset(setup_public_config)?;
            if ceremony_keyset != share.keyset {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "public DKG ceremony does not match local keyset share {}",
                        share.keyset.id
                    ),
                });
            }
            match keysets_by_id.insert(share.keyset.id, share.keyset.clone()) {
                Some(existing) if existing != share.keyset => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("conflicting DKG keyset {}", share.keyset.id),
                    });
                }
                Some(_) | None => {}
            }
            match ceremonies_by_keyset_id.insert(share.keyset.id, share.public_ceremony.clone()) {
                Some(existing) if existing != share.public_ceremony => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting public DKG ceremony for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
            match secret_shares_by_keyset.insert(share.keyset.id, share.secret_shares.clone()) {
                Some(existing) if existing != share.secret_shares => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting local DKG secret shares for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }
        if keysets_by_id.is_empty() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "missing DKG keyset shares".to_string(),
            });
        }

        let mut public_config = setup_public_config.clone();
        public_config.keysets = keysets_by_id.into_values().collect();
        public_config.setup_transcript = None;
        public_config.validate()?;
        let public_ceremonies = ceremonies_by_keyset_id.into_values().collect::<Vec<_>>();

        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &public_config,
            &public_ceremonies,
        )?;
        let mut transcript_signatures = BTreeMap::new();
        for signature_record in transcript_signature_records {
            if signature_record.transcript_hash != transcript_hash {
                continue;
            }
            signature_record.validate_sender(&public_config, signature_record.signer)?;
            match transcript_signatures.insert(signature_record.signer, signature_record.signature)
            {
                Some(existing) if existing != signature_record.signature => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting transcript signature for member {}",
                            signature_record.signer
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let transcript = FederationDkgTranscript::from_signatures(
            FederationDkgSetupMethod::PedersenDkg,
            &public_config,
            &public_ceremonies,
            transcript_signatures,
        )?;
        public_config.setup_transcript = Some(transcript);
        public_config.validate()?;

        let mut private_config = local_private_config.clone();
        private_config.federation_id = Some(public_config.federation_id);
        private_config.setup_transcript_hash = Some(transcript_hash);
        private_config.keyset_secret_shares = secret_shares_by_keyset;
        private_config.validate_against_public(&public_config)?;

        Ok(Self {
            public_config,
            private_config,
        })
    }

    /// Require every roster member to confirm every ceremony in this final config.
    pub fn validate_activation_records(
        &self,
        setup_public_config: &FederationConfig,
        activation_records: &[FederationDkgActivationRequest],
    ) -> Result<()> {
        self.public_config.validate()?;
        self.private_config
            .validate_against_public(&self.public_config)?;
        let transcript = self
            .public_config
            .setup_transcript
            .as_ref()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "DKG activation requires a complete setup transcript".to_string(),
            })?;
        transcript.validate_setup_lineage_against_config(&self.public_config)?;
        let mut activation_public_config = self.public_config.clone();
        activation_public_config.keysets = transcript
            .payload
            .keysets
            .iter()
            .map(FederationDkgKeysetTranscript::federated_keyset)
            .collect();
        activation_public_config.setup_transcript = Some(transcript.clone());
        activation_public_config.validate()?;
        transcript.validate_against_config(&activation_public_config)?;
        let ceremony_ids = transcript
            .payload
            .ceremonies
            .iter()
            .map(FederationDkgPublicCeremony::ceremony_id)
            .collect::<BTreeSet<_>>();
        if ceremony_ids.is_empty() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG activation transcript has no dealer-free ceremonies".to_string(),
            });
        }

        let mut confirmations = BTreeMap::new();
        for activation in activation_records
            .iter()
            .filter(|activation| ceremony_ids.contains(&activation.ceremony_id))
        {
            activation.validate_sender(
                setup_public_config,
                &activation_public_config,
                activation.signer,
            )?;
            let coordinate = (activation.ceremony_id, activation.signer);
            match confirmations.insert(coordinate, *activation) {
                Some(existing) if existing != *activation => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting DKG activation for member {} ceremony {}",
                            activation.signer, activation.ceremony_id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        for ceremony_id in ceremony_ids {
            for member_id in self.public_config.members.keys() {
                if !confirmations.contains_key(&(ceremony_id, *member_id)) {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "missing DKG activation for member {member_id} ceremony {ceremony_id}"
                        ),
                    });
                }
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
    /// Protocol component versions required by this public config.
    pub protocol_versions: FederationProtocolVersions,
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
    pub payment_observation_policy: FederationPaymentObservationPolicy,
}

impl FederationConfig {
    /// Create a federation config with consensus-threshold payment quorum and compute its ID.
    ///
    /// Development setup must replace the payment policy explicitly before signing the config.
    pub fn new(
        threshold_params: ThresholdParams,
        members: BTreeMap<MemberId, MemberConfigPublic>,
        keysets: Vec<FederatedKeyset>,
    ) -> Result<Self> {
        let federation_id = Self::derive_federation_id(threshold_params, &members)?;
        let config = Self {
            federation_id,
            protocol_versions: FederationProtocolVersions::current(),
            threshold_params,
            members,
            keysets,
            setup_transcript: None,
            payment_observation_policy: FederationPaymentObservationPolicy::member_quorum(
                threshold_params.consensus_threshold,
            ),
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
        self.protocol_versions.validate()?;

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
            transcript.validate_setup_lineage_against_config(self)?;
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;

        Ok(())
    }

    /// Validate config metadata needed by an already-imported wallet runtime.
    ///
    /// This keeps cheap structural, version, roster, keyset ID, member-share
    /// presence, and payment-policy checks, but skips expensive threshold BLS
    /// aggregate-key interpolation and setup-transcript proof validation. New
    /// external federation configs should still use [`Self::validate`] at
    /// import or mint startup.
    pub fn validate_wallet_runtime_metadata(&self) -> Result<()> {
        self.protocol_versions.validate()?;

        let expected_id = Self::derive_federation_id(self.threshold_params, &self.members)?;
        if expected_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: expected_id,
                found: self.federation_id,
            });
        }

        for keyset in &self.keysets {
            keyset.validate_metadata(&self.members)?;
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;

        Ok(())
    }

    /// Compute a digest of the complete public federation config.
    pub fn config_digest(&self) -> Result<FederationConfigDigest> {
        self.validate()?;
        self.config_digest_for_validated_config()
    }

    /// Compute a digest of an already-validated public federation config.
    ///
    /// This avoids repeating threshold BLS key interpolation on hot paths where
    /// the caller has already validated the config during construction or
    /// registry loading.
    pub fn config_digest_for_validated_config(&self) -> Result<FederationConfigDigest> {
        let config = serde_json::to_vec(self)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_CONFIG_DIGEST_VERSION);
        append_bytes(&mut transcript, &config);
        Ok(FederationConfigDigest::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
    }

    /// Build the mint-info federation advertisement expected from a member.
    pub fn mint_info_for_member(&self, member_id: MemberId) -> Result<FederationInfo> {
        let config_digest = self.config_digest()?;
        self.mint_info_for_member_with_config_digest(member_id, config_digest)
    }

    /// Build a member advertisement with the digest of an already-validated config.
    pub fn mint_info_for_member_with_config_digest(
        &self,
        member_id: MemberId,
        config_digest: FederationConfigDigest,
    ) -> Result<FederationInfo> {
        if !self.members.contains_key(&member_id) {
            return Err(FederationError::UnknownMember { member_id });
        }

        Ok(FederationInfo::new(
            self.federation_id.to_string(),
            member_id.get(),
            config_digest.to_string(),
            self.protocol_versions.to_mint_info(),
        ))
    }

    /// Validate a member's mint-info federation advertisement.
    pub fn validate_member_mint_info(
        &self,
        expected_member_id: MemberId,
        info: &FederationInfo,
    ) -> Result<()> {
        let expected_digest = self.config_digest()?;
        self.validate_member_mint_info_with_config_digest(expected_member_id, info, expected_digest)
    }

    /// Validate a member's mint-info federation advertisement using a known config digest.
    ///
    /// Callers should use this after validating the config and computing its
    /// digest once for the surrounding operation.
    pub fn validate_member_mint_info_with_config_digest(
        &self,
        expected_member_id: MemberId,
        info: &FederationInfo,
        expected_digest: FederationConfigDigest,
    ) -> Result<()> {
        if !self.members.contains_key(&expected_member_id) {
            return Err(FederationError::UnknownMember {
                member_id: expected_member_id,
            });
        }

        let found_member_id = MemberId::new(info.member_id)?;
        if found_member_id != expected_member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: expected_member_id,
                found: found_member_id,
            });
        }

        let found_federation_id = FederationId::from_str(&info.federation_id)?;
        if found_federation_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: self.federation_id,
                found: found_federation_id,
            });
        }

        let found_digest = FederationConfigDigest::from_str(&info.config_digest)?;
        if found_digest != expected_digest {
            return Err(FederationError::FederationConfigDigestMismatch {
                member_id: expected_member_id,
                expected: expected_digest,
                found: found_digest,
            });
        }

        let advertised_versions =
            FederationProtocolVersions::from_mint_info(info.protocol_versions);
        validate_protocol_component(
            "operation",
            self.protocol_versions.operation,
            advertised_versions.operation,
        )?;
        validate_protocol_component(
            "consensus_item",
            self.protocol_versions.consensus_item,
            advertised_versions.consensus_item,
        )?;
        validate_protocol_component(
            "rest_api",
            self.protocol_versions.rest_api,
            advertised_versions.rest_api,
        )?;
        validate_protocol_component(
            "session_outcome",
            self.protocol_versions.session_outcome,
            advertised_versions.session_outcome,
        )?;

        Ok(())
    }

    /// Return a validated config view after applying an accepted keyset rotation.
    pub fn apply_keyset_rotation(
        &self,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<Self> {
        operation.validate_against_config(self)?;

        let mut rotated = self.clone();
        for keyset in &mut rotated.keysets {
            if keyset.unit == operation.keyset.unit {
                keyset.active = false;
            }
        }

        match rotated
            .keysets
            .iter()
            .position(|keyset| keyset.id == operation.keyset.id)
        {
            Some(index) => rotated.keysets[index] = operation.keyset.clone(),
            None => rotated.keysets.push(operation.keyset.clone()),
        }

        rotated.validate()?;
        Ok(rotated)
    }

    /// Validate that blinded outputs target active v3/BLS aggregate keysets.
    pub fn validate_blinded_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_output(output)?;
        }

        Ok(())
    }

    /// Validate melt change outputs, allowing zero-amount change outputs.
    pub fn validate_blinded_change_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_change_output(output)?;
        }

        Ok(())
    }

    /// Validate restore outputs without requiring the keyset to still be active.
    pub fn validate_blinded_restore_outputs(&self, outputs: &[BlindedMessage]) -> Result<()> {
        for output in outputs {
            self.validate_blinded_restore_output(output)?;
        }

        Ok(())
    }

    /// Validate one blinded output and return the aggregate keyset it targets.
    pub fn validate_blinded_output(&self, output: &BlindedMessage) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, false, true)
    }

    /// Validate one melt change output and return the aggregate keyset it targets.
    pub fn validate_blinded_change_output(
        &self,
        output: &BlindedMessage,
    ) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, true, true)
    }

    /// Validate one restore output and return the aggregate keyset it targets.
    pub fn validate_blinded_restore_output(
        &self,
        output: &BlindedMessage,
    ) -> Result<&FederatedKeyset> {
        self.validate_blinded_output_with_amount_policy(output, true, false)
    }

    fn validate_blinded_output_with_amount_policy(
        &self,
        output: &BlindedMessage,
        allow_zero_amount_change_output: bool,
        require_active_keyset: bool,
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
        if require_active_keyset && !keyset.active {
            return Err(FederationError::InactiveFederatedKeyset {
                keyset_id: keyset.id,
            });
        }
        if output.amount != Amount::ZERO || !allow_zero_amount_change_output {
            if !keyset.aggregate_keys.contains_key(&output.amount) {
                return Err(FederationError::UnknownFederatedKeysetAmount {
                    keyset_id: keyset.id,
                    amount: output.amount,
                });
            }
        }
        if output.amount == Amount::ZERO
            && allow_zero_amount_change_output
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
    /// Validate identity and transcript metadata against the public federation config.
    pub(crate) fn validate_metadata_against_public(
        &self,
        public_config: &FederationConfig,
    ) -> Result<()> {
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

        Ok(())
    }

    /// Validate this private config against the public federation config.
    pub fn validate_against_public(&self, public_config: &FederationConfig) -> Result<()> {
        self.validate_metadata_against_public(public_config)?;

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

/// Portable wallet bootstrap artifact for a static federation.
///
/// The invite intentionally contains only the public federation config. Wallets
/// derive member HTTP connectors from the public mint URLs already committed to
/// that config, then validate aggregate keysets against live member responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationInvite {
    /// Invite payload version.
    pub version: u16,
    /// Public federation config shared by every member.
    pub config: FederationConfig,
}

impl FederationInvite {
    /// Build a validated federation invite from a public config.
    pub fn new(config: FederationConfig) -> Result<Self> {
        let invite = Self {
            version: FEDERATION_INVITE_VERSION,
            config,
        };
        invite.validate()?;
        Ok(invite)
    }

    /// Return the public federation config carried by this invite.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.config
    }

    /// Validate the invite payload and its public config.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_INVITE_VERSION {
            return Err(FederationError::FederationInviteVersionMismatch {
                expected: FEDERATION_INVITE_VERSION,
                found: self.version,
            });
        }
        self.config.validate()
    }

    /// Encode this invite as a versioned, URL-safe invite code.
    pub fn to_invite_code(&self) -> Result<String> {
        self.validate()?;
        let json =
            serde_json::to_vec(self).map_err(|err| FederationError::InvalidFederationInvite {
                reason: err.to_string(),
            })?;
        let encoded = general_purpose::URL_SAFE_NO_PAD.encode(json);
        Ok(format!("{FEDERATION_INVITE_PREFIX}{encoded}"))
    }
}

impl fmt::Display for FederationInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let invite_code = self.to_invite_code().map_err(|_| fmt::Error)?;
        f.write_str(&invite_code)
    }
}

impl FromStr for FederationInvite {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        let payload = value
            .strip_prefix(FEDERATION_INVITE_PREFIX)
            .ok_or_else(|| FederationError::InvalidFederationInvite {
                reason: "unsupported invite prefix".to_string(),
            })?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config)
            .decode(payload)
            .map_err(|err| FederationError::InvalidFederationInvite {
                reason: format!("payload is not URL-safe base64: {err}"),
            })?;
        let invite = serde_json::from_slice::<Self>(&decoded).map_err(|err| {
            FederationError::InvalidFederationInvite {
                reason: err.to_string(),
            }
        })?;
        invite.validate()?;
        Ok(invite)
    }
}

/// Durable public federation config snapshot.
///
/// This snapshot is local member startup state. It stores the public federation
/// config and a digest so a member does not accidentally reuse a database with
/// a different federation roster, keyset set, policy, or protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationConfigSnapshot {
    /// Federation ID derived from the static roster.
    pub federation_id: FederationId,
    /// Digest over the complete public config.
    pub config_digest: FederationConfigDigest,
    /// Public config shared by federation members.
    pub config: FederationConfig,
}

impl FederationConfigSnapshot {
    /// Build a validated durable snapshot from a public federation config.
    pub fn new(config: FederationConfig) -> Result<Self> {
        let config_digest = config.config_digest()?;
        Ok(Self {
            federation_id: config.federation_id,
            config_digest,
            config,
        })
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
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;

    use crate::nuts::nut02::KeySetVersion;
    use cashu::nuts::nut01::{
        evaluate_secret_key_coefficients, trusted_dealer_keygen, BlsG1PublicKey, BlsSecretKey,
    };

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

    fn dkg_ceremony_id() -> FederationDkgCeremonyId {
        FederationDkgCeremonyId::from_bytes([77; 32])
    }

    fn other_dkg_ceremony_id() -> FederationDkgCeremonyId {
        FederationDkgCeremonyId::from_bytes([78; 32])
    }

    #[test]
    fn federation_operator_message_redacts_sensitive_fragments() {
        let message = concat!(
            "peer 2 digest mismatch at operation count 4 ",
            "password=hunter2 token=sync-token ",
            "private_config_path=/home/example/fed/private.json ",
            "database_url=postgresql://status:db-secret@db.example.invalid/cdk ",
            "file=file:///home/example/cdk/config.toml ",
            "relative=../member/private.json ",
            "route=https://fed.example.invalid/federation/v1/local-status ",
            "(secret=quoted-secret),"
        );

        let redacted = redact_federation_operator_message(message);

        assert!(redacted.contains("digest mismatch"));
        assert!(redacted.contains("password=[REDACTED]"));
        assert!(redacted.contains("token=[REDACTED]"));
        assert!(redacted.contains("private_config_path=[REDACTED]"));
        assert!(
            redacted.contains("database_url=postgresql://redacted:redacted@db.example.invalid/cdk")
        );
        assert!(redacted.contains("file=[REDACTED_PATH]"));
        assert!(redacted.contains("relative=[REDACTED_PATH]"));
        assert!(redacted.contains("route=https://fed.example.invalid/federation/v1/local-status"));
        assert!(redacted.contains("(secret=[REDACTED]),"));
        for sensitive in [
            "hunter2",
            "sync-token",
            "/home/example",
            "db-secret",
            "file://",
            "../member",
            "quoted-secret",
        ] {
            assert!(!redacted.contains(sensitive));
        }
    }

    fn dkg_amount_coefficients(seed: u8) -> BTreeMap<Amount, Vec<BlsSecretKey>> {
        BTreeMap::from([(Amount::from(1), vec![bls_key(seed), bls_key(seed + 10)])])
    }

    fn test_roster() -> BTreeMap<MemberId, MemberConfigPublic> {
        test_roster_with_count(3)
    }

    fn test_roster_with_count(count: u16) -> BTreeMap<MemberId, MemberConfigPublic> {
        (1..=count)
            .map(member_id)
            .map(|id| (id, member(id)))
            .collect()
    }

    fn test_keyset(
        members: &BTreeMap<MemberId, MemberConfigPublic>,
    ) -> (
        FederatedKeyset,
        BTreeMap<MemberId, BTreeMap<Amount, BlsSecretKeyShare>>,
    ) {
        test_keyset_with_seed(members, 1)
    }

    fn test_keyset_with_seed(
        members: &BTreeMap<MemberId, MemberConfigPublic>,
        seed: u8,
    ) -> (
        FederatedKeyset,
        BTreeMap<MemberId, BTreeMap<Amount, BlsSecretKeyShare>>,
    ) {
        let signer_ids = members
            .keys()
            .map(|member_id| member_id.to_bls_signer_id())
            .collect::<Vec<_>>();
        let amount = Amount::from(1);
        let threshold_keyset = trusted_dealer_keygen(
            &[bls_key(seed), bls_key(seed.saturating_add(1))],
            &signer_ids,
        )
        .expect("keyset");
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

    fn test_public_federation_config() -> FederationConfig {
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let params = ThresholdParams::new(3, 2, 3).expect("threshold params");
        FederationConfig::new(params, members, vec![keyset]).expect("public config")
    }

    fn raw_invite_code(invite: &FederationInvite) -> String {
        let json = serde_json::to_vec(invite).expect("invite json");
        let encoded = general_purpose::URL_SAFE_NO_PAD.encode(json);
        format!("{FEDERATION_INVITE_PREFIX}{encoded}")
    }

    #[test]
    fn federation_invite_roundtrip_preserves_public_config() {
        let config = test_public_federation_config();
        let invite = FederationInvite::new(config.clone()).expect("invite");
        let invite_code = invite.to_invite_code().expect("invite code");

        assert!(invite_code.starts_with(FEDERATION_INVITE_PREFIX));
        assert_eq!(invite_code, invite.to_string());

        let parsed = FederationInvite::from_str(&invite_code).expect("parse invite");
        assert_eq!(parsed.version, FEDERATION_INVITE_VERSION);
        assert_eq!(parsed.public_config(), &config);
    }

    #[test]
    fn federation_mint_info_advertisement_validates_config_identity() {
        let config = test_public_federation_config();
        let member_id = member_id(2);
        let info = config
            .mint_info_for_member(member_id)
            .expect("mint-info federation advertisement");

        assert_eq!(info.federation_id, config.federation_id.to_string());
        assert_eq!(info.member_id, member_id.get());
        assert_eq!(
            info.config_digest,
            config.config_digest().expect("config digest").to_string()
        );

        config
            .validate_member_mint_info(member_id, &info)
            .expect("member mint-info advertisement");
    }

    #[test]
    fn federation_mint_info_accepts_a_precomputed_validated_digest() {
        let config = test_public_federation_config();
        config.validate().expect("valid federation config");
        let digest = config
            .config_digest_for_validated_config()
            .expect("precomputed config digest");
        let info = config
            .mint_info_for_member_with_config_digest(member_id(2), digest)
            .expect("mint-info federation advertisement");

        assert_eq!(info.config_digest, digest.to_string());
        config
            .validate_member_mint_info_with_config_digest(member_id(2), &info, digest)
            .expect("member mint-info advertisement");
    }

    #[test]
    fn federation_mint_info_validation_rejects_wrong_member_id() {
        let config = test_public_federation_config();
        let mut info = config
            .mint_info_for_member(member_id(2))
            .expect("mint-info federation advertisement");
        info.member_id = member_id(3).get();

        assert!(matches!(
            config.validate_member_mint_info(member_id(2), &info),
            Err(FederationError::MemberIdMismatch {
                expected,
                found,
            }) if expected == member_id(2) && found == member_id(3)
        ));
    }

    #[test]
    fn federation_mint_info_validation_rejects_wrong_federation_id() {
        let config = test_public_federation_config();
        let mut info = config
            .mint_info_for_member(member_id(2))
            .expect("mint-info federation advertisement");
        info.federation_id = FederationId::from_bytes([42u8; 32]).to_string();

        assert!(matches!(
            config.validate_member_mint_info(member_id(2), &info),
            Err(FederationError::FederationIdMismatch { .. })
        ));
    }

    #[test]
    fn federation_mint_info_validation_rejects_wrong_config_digest() {
        let config = test_public_federation_config();
        let mut info = config
            .mint_info_for_member(member_id(2))
            .expect("mint-info federation advertisement");
        info.config_digest = FederationConfigDigest::from_bytes([42u8; 32]).to_string();

        assert!(matches!(
            config.validate_member_mint_info(member_id(2), &info),
            Err(FederationError::FederationConfigDigestMismatch {
                member_id: actual_member_id,
                ..
            }) if actual_member_id == member_id(2)
        ));
    }

    #[test]
    fn federation_mint_info_validation_rejects_unsupported_protocol_version() {
        let config = test_public_federation_config();
        let mut info = config
            .mint_info_for_member(member_id(2))
            .expect("mint-info federation advertisement");
        info.protocol_versions.operation = info.protocol_versions.operation.saturating_add(1);

        assert!(matches!(
            config.validate_member_mint_info(member_id(2), &info),
            Err(FederationError::ProtocolVersionMismatch {
                component,
                expected,
                found,
            }) if component == "operation"
                && expected == FederationProtocolVersions::current().operation
                && found == FederationProtocolVersions::current().operation.saturating_add(1)
        ));
    }

    #[test]
    fn federation_invite_rejects_bad_prefix() {
        let err = FederationInvite::from_str("cashuAunsupported").expect_err("bad prefix");

        assert!(matches!(
            err,
            FederationError::InvalidFederationInvite { .. }
        ));
    }

    #[test]
    fn federation_invite_rejects_unknown_payload_version() {
        let config = test_public_federation_config();
        let mut invite = FederationInvite::new(config).expect("invite");
        invite.version = FEDERATION_INVITE_VERSION.saturating_add(1);
        let invite_code = raw_invite_code(&invite);

        assert!(matches!(
            FederationInvite::from_str(&invite_code),
            Err(FederationError::FederationInviteVersionMismatch {
                expected: FEDERATION_INVITE_VERSION,
                found,
            }) if found == FEDERATION_INVITE_VERSION.saturating_add(1)
        ));
    }

    #[test]
    fn federation_invite_rejects_missing_payload_version() {
        let config = test_public_federation_config();
        let invite = FederationInvite::new(config).expect("invite");
        let mut json = serde_json::to_value(invite).expect("invite json");
        json.as_object_mut()
            .expect("invite object")
            .remove("version");
        let encoded = general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json).expect("invite json bytes"));
        let invite_code = format!("{FEDERATION_INVITE_PREFIX}{encoded}");

        assert!(matches!(
            FederationInvite::from_str(&invite_code),
            Err(FederationError::InvalidFederationInvite { .. })
        ));
    }

    #[test]
    fn federation_invite_rejects_tampered_public_config() {
        let config = test_public_federation_config();
        let mut invite = FederationInvite::new(config).expect("invite");
        invite.config.federation_id = FederationId::from_bytes([42u8; 32]);
        let invite_code = raw_invite_code(&invite);

        assert!(matches!(
            FederationInvite::from_str(&invite_code),
            Err(FederationError::FederationIdMismatch { .. })
        ));
    }

    fn dkg_reveal(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> FederationDkgParticipantCommitmentReveal {
        FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            ceremony_id: dkg_ceremony_id(),
            member_id,
            keyset_spec: FederationDkgKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeSet::from([Amount::from(1)]),
            )
            .expect("DKG keyset spec"),
            coefficient_commitments: dkg_amount_coefficients(seed)
                .into_iter()
                .map(|(amount, coefficients)| {
                    (
                        amount,
                        coefficients
                            .iter()
                            .map(BlsSecretKey::public_key_g2)
                            .collect(),
                    )
                })
                .collect(),
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

    fn assert_protocol_version_mismatch(
        config: FederationConfig,
        component: &'static str,
        expected: u16,
        found: u16,
    ) {
        assert!(matches!(
            config.validate(),
            Err(FederationError::ProtocolVersionMismatch {
                component: actual_component,
                expected: actual_expected,
                found: actual_found,
            }) if actual_component == component
                && actual_expected == expected
                && actual_found == found
        ));
    }

    fn dkg_member_secret_contributions(
        public_config: &FederationConfig,
        local_member_id: MemberId,
    ) -> BTreeMap<MemberId, FederationDkgSecretShareRequest> {
        let signer_id = local_member_id.to_bls_signer_id();
        [
            (member_id(1), dkg_amount_coefficients(11)),
            (member_id(2), dkg_amount_coefficients(21)),
            (member_id(3), dkg_amount_coefficients(31)),
        ]
        .into_iter()
        .map(|(participant_id, amount_coefficients)| {
            let shares = amount_coefficients
                .into_iter()
                .map(|(amount, coefficients)| {
                    (
                        amount,
                        evaluate_secret_key_coefficients(signer_id, &coefficients)
                            .expect("secret contribution"),
                    )
                })
                .collect();
            (
                participant_id,
                FederationDkgSecretShareRequest::new(
                    public_config.federation_id,
                    dkg_ceremony_id(),
                    participant_id,
                    local_member_id,
                    shares,
                ),
            )
        })
        .collect()
    }

    fn dkg_transcript_private_configs(
        public_config: &FederationConfig,
    ) -> BTreeMap<MemberId, MemberConfigPrivate> {
        public_config
            .members
            .keys()
            .map(|member_id| {
                (
                    *member_id,
                    MemberConfigPrivate {
                        federation_id: Some(public_config.federation_id),
                        member_id: *member_id,
                        identity_secret_key: identity_secret(*member_id),
                        keyset_secret_shares: BTreeMap::new(),
                        setup_transcript_hash: None,
                    },
                )
            })
            .collect()
    }

    fn dkg_transcript_signature_records(
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Vec<FederationDkgTranscriptSignatureRequest> {
        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            public_config,
            ceremonies,
        )
        .expect("transcript hash");
        private_configs
            .values()
            .map(|private_config| {
                FederationDkgTranscriptSignatureRequest::new(
                    public_config.federation_id,
                    private_config.member_id,
                    transcript_hash,
                    FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
                        .expect("transcript signature"),
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
    fn member_id_deserializes_from_decimal_string_and_json_map_keys() {
        assert_eq!(
            serde_json::from_str::<MemberId>(r#""42""#).expect("member from string json"),
            member_id(42)
        );
        let members = serde_json::from_str::<BTreeMap<MemberId, String>>(r#"{"42":"online"}"#)
            .expect("member map from json");

        assert_eq!(members.get(&member_id(42)), Some(&"online".to_string()));
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
    fn federation_config_advertises_current_protocol_versions() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");

        assert_eq!(
            config.protocol_versions,
            FederationProtocolVersions::current()
        );

        let json = serde_json::to_value(&config).expect("config json");
        let versions = json
            .get("protocol_versions")
            .and_then(serde_json::Value::as_object)
            .expect("protocol versions object");
        assert_eq!(
            versions
                .get("operation")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(FEDERATION_OPERATION_VERSION))
        );
        assert_eq!(
            versions
                .get("consensus_item")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(FEDERATION_CONSENSUS_ITEM_VERSION))
        );
        assert_eq!(
            versions.get("rest_api").and_then(serde_json::Value::as_u64),
            Some(u64::from(FEDERATION_REST_API_VERSION))
        );
        assert_eq!(
            versions
                .get("session_outcome")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(FEDERATION_SESSION_OUTCOME_VERSION))
        );
    }

    #[test]
    fn federation_config_rejects_missing_protocol_versions() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");
        let mut json = serde_json::to_value(&config).expect("config json");
        json.as_object_mut()
            .expect("config object")
            .remove("protocol_versions");

        assert!(
            serde_json::from_value::<FederationConfig>(json).is_err(),
            "federation configs must explicitly advertise protocol versions"
        );
    }

    #[test]
    fn federation_protocol_versions_reject_missing_components() {
        assert!(
            serde_json::from_value::<FederationProtocolVersions>(serde_json::json!({
                "operation": FEDERATION_OPERATION_VERSION
            }))
            .is_err(),
            "partial federation protocol version metadata must not be accepted"
        );
    }

    #[test]
    fn federation_config_rejects_unsupported_protocol_versions() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");

        let mut operation = config.clone();
        let expected = operation.protocol_versions.operation;
        let found = expected.saturating_add(1);
        operation.protocol_versions.operation = found;
        assert_protocol_version_mismatch(operation, "operation", expected, found);

        let mut consensus_item = config.clone();
        let expected = consensus_item.protocol_versions.consensus_item;
        let found = expected.saturating_add(1);
        consensus_item.protocol_versions.consensus_item = found;
        assert_protocol_version_mismatch(consensus_item, "consensus_item", expected, found);

        let mut rest_api = config.clone();
        let expected = rest_api.protocol_versions.rest_api;
        let found = expected.saturating_add(1);
        rest_api.protocol_versions.rest_api = found;
        assert_protocol_version_mismatch(rest_api, "rest_api", expected, found);

        let mut session_outcome = config;
        let expected = session_outcome.protocol_versions.session_outcome;
        let found = expected.saturating_add(1);
        session_outcome.protocol_versions.session_outcome = found;
        assert_protocol_version_mismatch(session_outcome, "session_outcome", expected, found);
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
    fn federation_config_uses_safe_payment_policy_and_rejects_missing_policy() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset]).expect("config");
        assert_eq!(
            config.payment_observation_policy,
            FederationPaymentObservationPolicy::member_quorum(params.consensus_threshold)
        );
        for context in [
            FederationPaymentObservationContext::MintQuote,
            FederationPaymentObservationContext::MeltQuote,
        ] {
            let policy = *config.payment_verification_policy(context);
            assert!(!policy.permits_single_observation_finality());
            assert_eq!(
                policy.required_matching_observations(),
                params.consensus_threshold
            );
        }

        let mut json = serde_json::to_value(&config).expect("config json");
        json.as_object_mut()
            .expect("config object")
            .remove("payment_observation_policy");

        assert!(
            serde_json::from_value::<FederationConfig>(json).is_err(),
            "federation configs must explicitly declare payment-observation policy"
        );
    }

    #[test]
    fn federation_payment_policy_rejects_missing_contexts() {
        assert!(
            serde_json::from_value::<FederationPaymentObservationPolicy>(serde_json::json!({
                "mint_quotes": { "type": "development_single_observation" }
            }))
            .is_err(),
            "payment-observation policy must explicitly cover mint and melt"
        );
    }

    #[test]
    fn federation_payment_policy_rejects_removed_operator_limits() {
        assert!(
            serde_json::from_value::<FederationPaymentObservationPolicy>(serde_json::json!({
                "mint_quotes": { "type": "development_single_observation" },
                "melt_quotes": { "type": "development_single_observation" },
                "max_pending_quotes": 10_000
            }))
            .is_err(),
            "undeployed federation configs must not retain removed operator limits"
        );
    }

    #[test]
    fn federation_payment_policy_rejects_fake_wallet_alias() {
        let json = serde_json::json!({
            "mint_quotes": { "type": "fake_wallet" },
            "melt_quotes": { "type": "fake_wallet" }
        });

        assert!(
            serde_json::from_value::<FederationPaymentObservationPolicy>(json).is_err(),
            "federation configs must use the explicit development_single_observation policy"
        );
    }

    #[test]
    fn federation_payment_policy_rejects_removed_trust_model_field() {
        let json = serde_json::json!({
            "type": "member_quorum",
            "required": 2,
            "trust_model": "shared_backend_read_only"
        });

        assert!(
            serde_json::from_value::<FederationPaymentVerificationPolicy>(json).is_err(),
            "undeployed federation configs must not retain removed trust-model fields"
        );
    }

    #[test]
    fn production_federation_modules_do_not_import_fakewallet_backend() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let production_modules = [
            "crates/cdk-common/src/federation.rs",
            "crates/cdk-common/src/federation/consensus.rs",
            "crates/cdk-common/src/federation/journal.rs",
            "crates/cdk-common/src/federation/operation.rs",
            "crates/cdk-common/src/federation/signing.rs",
            "crates/cdk-common/src/federation/transport.rs",
            "crates/cdk-common/src/federation/trusted_dealer.rs",
            "crates/cdk/src/mint/federation.rs",
            "crates/cdk/src/wallet/federation.rs",
            "crates/cdk-axum/src/custom_handlers.rs",
            "crates/cdk-axum/src/federation.rs",
        ];
        let forbidden = ["cdk_fake_wallet", "FakeWallet"];

        let mut violations = Vec::new();
        for module in production_modules {
            let path = repo_root.join(module);
            let source = fs::read_to_string(&path).expect("federation source file");
            let production_source = match source.find("#[cfg(test)]") {
                Some(test_start) => &source[..test_start],
                None => source.as_str(),
            };

            for token in forbidden {
                if production_source.contains(token) {
                    violations.push(format!("{module} contains `{token}` outside tests"));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "production federation modules must stay payment-backend agnostic:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn federation_manifests_keep_fakewallet_dev_or_backend_scoped() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");

        for manifest in [
            "crates/cashu/Cargo.toml",
            "crates/cdk-common/Cargo.toml",
            "crates/cdk-http-client/Cargo.toml",
        ] {
            let contents = fs::read_to_string(repo_root.join(manifest)).expect("manifest");
            assert!(
                !contents.contains("cdk-fake-wallet"),
                "{manifest} must not depend on the fakewallet backend"
            );
        }

        for manifest in ["crates/cdk/Cargo.toml", "crates/cdk-axum/Cargo.toml"] {
            let contents = fs::read_to_string(repo_root.join(manifest)).expect("manifest");
            let hits = manifest_lines_for_dependency(&contents, "cdk-fake-wallet");
            assert!(
                !hits.is_empty(),
                "{manifest} should keep fakewallet test coverage explicit"
            );
            assert!(
                hits.iter()
                    .all(|(section, _line)| section == "dev-dependencies"),
                "{manifest} must keep cdk-fake-wallet under [dev-dependencies]: {hits:?}"
            );
        }

        let mintd_manifest = "crates/cdk-mintd/Cargo.toml";
        let contents = fs::read_to_string(repo_root.join(mintd_manifest)).expect("mintd manifest");
        let hits = manifest_lines_for_dependency(&contents, "cdk-fake-wallet");
        assert!(
            hits.iter().any(|(section, line)| {
                section == "features" && line.trim() == "fakewallet = [\"dep:cdk-fake-wallet\"]"
            }),
            "{mintd_manifest} must expose fakewallet only as an explicit backend feature"
        );
        assert!(
            hits.iter().any(|(section, line)| {
                section == "dependencies"
                    && line.contains("cdk-fake-wallet")
                    && line.contains("optional = true")
            }),
            "{mintd_manifest} must keep cdk-fake-wallet optional"
        );
        assert!(
            hits.iter().all(|(section, _line)| {
                matches!(section.as_str(), "features" | "dependencies")
            }),
            "{mintd_manifest} has unexpected fakewallet manifest entries: {hits:?}"
        );
    }

    fn manifest_lines_for_dependency(manifest: &str, needle: &str) -> Vec<(String, String)> {
        let mut section = String::new();
        let mut hits = Vec::new();

        for line in manifest.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                section = trimmed.trim_matches(['[', ']']).to_string();
                continue;
            }

            if trimmed.contains(needle) {
                hits.push((section.clone(), trimmed.to_string()));
            }
        }

        hits
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
    fn dkg_ceremony_id_roundtrips_and_rejects_wrong_length() {
        let ceremony_id = dkg_ceremony_id();
        let json = serde_json::to_string(&ceremony_id).expect("ceremony id json");
        assert_eq!(
            serde_json::from_str::<FederationDkgCeremonyId>(&json).expect("ceremony id"),
            ceremony_id
        );
        assert_eq!(
            FederationDkgCeremonyId::from_str(&ceremony_id.to_string())
                .expect("ceremony id from string"),
            ceremony_id
        );
        assert!(matches!(
            FederationDkgCeremonyId::from_slice(&[1; 31]),
            Err(FederationError::InvalidDkgCeremonyIdSize {
                expected: 32,
                found: 31
            })
        ));
    }

    #[test]
    fn dkg_keyset_spec_rejects_more_than_u64_denominations() {
        let amounts = (1..=FEDERATION_MAX_DKG_KEYSET_AMOUNTS + 1)
            .map(|amount| Amount::from(amount as u64))
            .collect();

        assert!(matches!(
            FederationDkgKeysetSpec::new(CurrencyUnit::Sat, true, 0, None, amounts),
            Err(FederationError::DkgKeysetAmountLimitExceeded { found, max })
                if found == FEDERATION_MAX_DKG_KEYSET_AMOUNTS + 1
                    && max == FEDERATION_MAX_DKG_KEYSET_AMOUNTS
        ));
    }

    #[test]
    fn dkg_keyset_spec_rejects_oversized_federation_roster() {
        let member_count = (FEDERATION_MAX_DKG_MEMBERS + 1) as u16;
        let params = ThresholdParams::with_bft_consensus(member_count, 44).expect("params");
        let config =
            FederationConfig::new(params, test_roster_with_count(member_count), Vec::new())
                .expect("oversized public config remains valid outside dealer-free DKG");
        let spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("bounded keyset spec");

        assert!(matches!(
            spec.validate_for_federation(&config),
            Err(FederationError::DkgMemberLimitExceeded { found, max })
                if found == FEDERATION_MAX_DKG_MEMBERS + 1
                    && max == FEDERATION_MAX_DKG_MEMBERS
        ));
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
            round.derive_federated_keyset(&config, &reveals),
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
    fn dkg_setup_state_collects_commitments_and_reveals_idempotently() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveals = dkg_reveals(&config);
        let commitments = dkg_commitments(&reveals);
        let mut state = FederationDkgSetupState::new(&config);

        assert!(state
            .complete_round(&config)
            .expect("partial round")
            .is_none());
        for commitment in commitments.values() {
            assert!(state
                .insert_commitment(&config, *commitment)
                .expect("insert commitment"));
            assert!(!state
                .insert_commitment(&config, *commitment)
                .expect("duplicate commitment"));
        }
        assert!(state.commitments_complete(&config));
        assert!(state
            .complete_round(&config)
            .expect("partial round")
            .is_none());

        for reveal in reveals.values() {
            assert!(state
                .insert_reveal(&config, reveal.clone())
                .expect("insert reveal"));
            assert!(!state
                .insert_reveal(&config, reveal.clone())
                .expect("duplicate reveal"));
        }

        assert!(state.reveals_complete(&config));
        let round = state
            .complete_round(&config)
            .expect("complete round")
            .expect("round complete");
        round
            .validate_reveals(&config, &reveals)
            .expect("reveals validate");
        let state_json = serde_json::to_string(&state).expect("state json");
        assert_eq!(
            serde_json::from_str::<FederationDkgSetupState>(&state_json).expect("state from json"),
            state
        );
    }

    #[test]
    fn dkg_setup_state_rejects_conflicting_commitment_or_reveal() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveals = dkg_reveals(&config);
        let mut state = FederationDkgSetupState::new(&config);
        let first_reveal = reveals.get(&member_id(1)).expect("first reveal").clone();
        let first_commitment =
            FederationDkgParticipantCommitment::from_reveal(&first_reveal).expect("commitment");
        let conflicting_reveal = dkg_reveal(&config, member_id(1), 91);
        let conflicting_commitment =
            FederationDkgParticipantCommitment::from_reveal(&conflicting_reveal)
                .expect("conflicting commitment");

        state
            .insert_commitment(&config, first_commitment)
            .expect("insert commitment");
        assert!(matches!(
            state.insert_commitment(&config, conflicting_commitment),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1) && reason == "conflicting commitment"
        ));

        state
            .insert_reveal(&config, first_reveal)
            .expect("insert reveal");
        assert!(matches!(
            state.insert_reveal(&config, conflicting_reveal),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1) && reason == "conflicting reveal"
        ));
    }

    #[test]
    fn dkg_setup_state_rejects_cross_ceremony_commitment_without_mutation() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let first_reveal = dkg_reveal(&config, member_id(1), 11);
        let first_commitment =
            FederationDkgParticipantCommitment::from_reveal(&first_reveal).expect("commitment");
        let mut other_reveal = dkg_reveal(&config, member_id(2), 21);
        other_reveal.ceremony_id = other_dkg_ceremony_id();
        let other_commitment = FederationDkgParticipantCommitment::from_reveal(&other_reveal)
            .expect("other ceremony commitment");
        let mut state = FederationDkgSetupState::new(&config);
        state
            .insert_commitment(&config, first_commitment)
            .expect("first commitment");

        assert!(matches!(
            state.insert_commitment(&config, other_commitment),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(2) && reason == "commitment belongs to another ceremony"
        ));
        assert_eq!(state.ceremony_id, Some(dkg_ceremony_id()));
        assert_eq!(state.commitments.len(), 1);
        assert!(!state.commitments.contains_key(&member_id(2)));
    }

    #[test]
    fn dkg_setup_state_rejects_reveal_without_prior_commitment() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveal = dkg_reveal(&config, member_id(1), 11);
        let mut state = FederationDkgSetupState::new(&config);

        assert!(matches!(
            state.insert_reveal(&config, reveal),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1) && reason == "missing prior commitment"
        ));
    }

    #[test]
    fn dealer_free_dkg_participants_derive_same_five_member_keyset_after_reload() {
        let params = ThresholdParams::with_bft_consensus(5, 3).expect("params");
        let members = test_roster_with_count(5);
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let keyset_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            7,
            Some(1_900_000_000),
            [Amount::from(1), Amount::from(2), Amount::from(4)]
                .into_iter()
                .collect(),
        )
        .expect("keyset spec");

        let participants = members
            .keys()
            .copied()
            .map(|participant_id| {
                let state = FederationDkgParticipantState::generate(
                    &setup_config,
                    dkg_ceremony_id(),
                    participant_id,
                    keyset_spec.clone(),
                )
                .expect("participant state");
                let encoded = serde_json::to_vec(&state).expect("participant state json");
                let reloaded = serde_json::from_slice::<FederationDkgParticipantState>(&encoded)
                    .expect("reloaded participant state");
                reloaded
                    .validate(&setup_config)
                    .expect("reloaded state validates");
                assert_eq!(reloaded.member_id(), participant_id);
                assert_eq!(reloaded.ceremony_id(), dkg_ceremony_id());
                assert_eq!(reloaded.keyset_spec(), &keyset_spec);
                (participant_id, reloaded)
            })
            .collect::<BTreeMap<_, _>>();

        let first_state = participants.get(&member_id(1)).expect("first participant");
        let first_request = first_state
            .secret_share_request(member_id(1))
            .expect("self contribution");
        let first_secret_hex = crate::util::hex::encode(
            first_request
                .secret_shares
                .values()
                .next()
                .expect("first secret contribution")
                .to_secret_bytes(),
        );
        assert!(!format!("{first_state:?}").contains(&first_secret_hex));

        let reveals = participants
            .iter()
            .map(|(participant_id, state)| (*participant_id, state.reveal_request().reveal))
            .collect::<BTreeMap<_, _>>();
        let commitments = participants
            .iter()
            .map(|(participant_id, state)| {
                (
                    *participant_id,
                    state
                        .commitment_request()
                        .expect("participant commitment")
                        .commitment,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let round =
            FederationDkgCommitmentRound::new(&setup_config, commitments).expect("DKG round");
        round
            .validate_reveals(&setup_config, &reveals)
            .expect("participant reveals");

        let mut expected_keyset = None;
        for receiver in members.keys().copied() {
            let participant_secret_shares = participants
                .iter()
                .map(|(participant_id, state)| {
                    (
                        *participant_id,
                        state
                            .secret_share_request(receiver)
                            .expect("receiver contribution"),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let material = round
                .derive_member_keyset_share(
                    &setup_config,
                    &reveals,
                    receiver,
                    &participant_secret_shares,
                )
                .expect("derived member keyset share");
            assert_eq!(material.secret_shares.len(), keyset_spec.amounts.len());
            match &expected_keyset {
                Some(expected) => assert_eq!(&material.keyset, expected),
                None => expected_keyset = Some(material.keyset),
            }
        }

        let keyset = expected_keyset.expect("derived keyset");
        assert_eq!(keyset.unit, keyset_spec.unit);
        assert_eq!(keyset.input_fee_ppk, keyset_spec.input_fee_ppk);
        assert_eq!(keyset.final_expiry, keyset_spec.final_expiry);
        keyset.validate(params, &members).expect("keyset validates");
    }

    #[test]
    fn dealer_free_dkg_participant_reload_rejects_tampered_secret_contribution() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let setup_config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("setup config");
        let keyset_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("keyset spec");
        let mut state = FederationDkgParticipantState::generate(
            &setup_config,
            dkg_ceremony_id(),
            member_id(1),
            keyset_spec,
        )
        .expect("participant state");
        state
            .secret_share_requests
            .get_mut(&member_id(2))
            .expect("receiver contribution")
            .secret_shares
            .insert(
                Amount::from(1),
                evaluate_secret_key_coefficients(
                    member_id(2).to_bls_signer_id(),
                    &[bls_key(91), bls_key(92)],
                )
                .expect("tampered share"),
            );

        assert!(matches!(
            state.validate(&setup_config),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason.contains("does not match public commitments")
        ));
    }

    #[test]
    fn dealer_free_dkg_rejects_participant_keyset_policy_mismatch() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let setup_config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("setup config");
        let base_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("keyset spec");
        let mut participants = setup_config
            .members
            .keys()
            .copied()
            .map(|participant_id| {
                (
                    participant_id,
                    FederationDkgParticipantState::generate(
                        &setup_config,
                        dkg_ceremony_id(),
                        participant_id,
                        base_spec.clone(),
                    )
                    .expect("participant state"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        participants
            .get_mut(&member_id(2))
            .expect("second participant")
            .reveal
            .keyset_spec
            .input_fee_ppk = 1;

        let reveals = participants
            .iter()
            .map(|(participant_id, state)| (*participant_id, state.reveal_request().reveal))
            .collect::<BTreeMap<_, _>>();
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&setup_config, commitments).expect("DKG round");

        assert!(matches!(
            round.validate_reveals(&setup_config, &reveals),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(2) && reason == "keyset specification mismatch"
        ));
    }

    #[test]
    fn dkg_commitment_round_derives_keyset_and_pedersen_transcript() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let reveals = dkg_reveals(&setup_config);
        let commitments = dkg_commitments(&reveals);
        let round = FederationDkgCommitmentRound::new(&setup_config, commitments).expect("round");

        let keyset = round
            .derive_federated_keyset(&setup_config, &reveals)
            .expect("derived federated keyset");
        assert_eq!(keyset.id.get_version(), KeySetVersion::Version02);
        assert_eq!(keyset.aggregate_keys.len(), 1);
        assert_eq!(keyset.public_shares.len(), members.len());
        keyset
            .validate(params, &members)
            .expect("derived keyset validates");

        let mut public_config =
            FederationConfig::new(params, members, vec![keyset.clone()]).expect("public config");
        let public_ceremony =
            FederationDkgPublicCeremony::new(&setup_config, round, reveals).expect("ceremony");
        let transcript = FederationDkgTranscript::pedersen_dkg(
            &public_config,
            std::slice::from_ref(&public_ceremony),
            &dkg_transcript_private_configs(&public_config),
        )
        .expect("pedersen DKG transcript");
        assert_eq!(
            transcript.payload.setup_method,
            FederationDkgSetupMethod::PedersenDkg
        );
        assert_eq!(transcript.payload.keysets.len(), 1);
        assert_eq!(transcript.payload.keysets[0].keyset_id, keyset.id);
        assert_eq!(transcript.payload.keysets[0].unit, keyset.unit);
        assert_eq!(transcript.payload.ceremonies, vec![public_ceremony]);
        assert_eq!(
            transcript.payload.ceremonies[0].ceremony_id(),
            dkg_ceremony_id()
        );
        assert_eq!(transcript.payload.keysets[0].active, keyset.active);
        assert_eq!(
            transcript.payload.keysets[0].input_fee_ppk,
            keyset.input_fee_ppk
        );
        assert_eq!(
            transcript.payload.keysets[0].final_expiry,
            keyset.final_expiry
        );
        transcript
            .validate_against_config(&public_config)
            .expect("transcript validates");

        public_config.setup_transcript = Some(transcript);
        public_config.validate().expect("config with transcript");
    }

    #[test]
    fn dkg_pedersen_transcript_binds_complete_public_ceremony() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let reveals = dkg_reveals(&setup_config);
        let commitments = dkg_commitments(&reveals);
        let round = FederationDkgCommitmentRound::new(&setup_config, commitments).expect("round");
        let keyset = round
            .derive_federated_keyset(&setup_config, &reveals)
            .expect("keyset");
        let public_config =
            FederationConfig::new(params, members, vec![keyset.clone()]).expect("public config");
        let public_ceremony =
            FederationDkgPublicCeremony::new(&setup_config, round, reveals.clone())
                .expect("public ceremony");
        let private_configs = dkg_transcript_private_configs(&public_config);
        let transcript = FederationDkgTranscript::pedersen_dkg(
            &public_config,
            std::slice::from_ref(&public_ceremony),
            &private_configs,
        )
        .expect("pedersen transcript");

        assert!(matches!(
            FederationDkgTranscript::hash_for_config(
                FederationDkgSetupMethod::PedersenDkg,
                &public_config,
                &[],
            ),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "Pedersen DKG transcript is missing public ceremonies"
        ));

        let mut relabeled_reveals = reveals;
        for reveal in relabeled_reveals.values_mut() {
            reveal.ceremony_id = other_dkg_ceremony_id();
        }
        let relabeled_commitments = dkg_commitments(&relabeled_reveals);
        let relabeled_round =
            FederationDkgCommitmentRound::new(&setup_config, relabeled_commitments)
                .expect("relabeled round");
        let relabeled_ceremony =
            FederationDkgPublicCeremony::new(&setup_config, relabeled_round, relabeled_reveals)
                .expect("relabeled ceremony");
        assert_eq!(
            relabeled_ceremony
                .derive_federated_keyset(&setup_config)
                .expect("relabeled keyset"),
            keyset
        );
        let relabeled_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &public_config,
            std::slice::from_ref(&relabeled_ceremony),
        )
        .expect("relabeled transcript hash");
        assert_ne!(relabeled_hash, transcript.transcript_hash);
        assert!(matches!(
            FederationDkgTranscript::from_signatures(
                FederationDkgSetupMethod::PedersenDkg,
                &public_config,
                std::slice::from_ref(&relabeled_ceremony),
                transcript.signatures.clone(),
            ),
            Err(FederationError::DkgTranscriptSignatureVerificationFailed { .. })
        ));

        let mut tampered = transcript;
        tampered.payload.ceremonies[0]
            .reveals
            .get_mut(&member_id(1))
            .expect("first reveal")
            .coefficient_commitments
            .get_mut(&Amount::from(1))
            .expect("amount commitments")[0] = bls_key(99).public_key_g2();
        assert!(matches!(
            tampered.validate_against_config(&public_config),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason == "transcript hash does not match payload"
        ));
    }

    #[test]
    fn dkg_commitment_round_derives_member_keyset_share() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let reveals = dkg_reveals(&setup_config);
        let commitments = dkg_commitments(&reveals);
        let round = FederationDkgCommitmentRound::new(&setup_config, commitments).expect("round");
        let local_member = member_id(2);
        let material = round
            .derive_member_keyset_share(
                &setup_config,
                &reveals,
                local_member,
                &dkg_member_secret_contributions(&setup_config, local_member),
            )
            .expect("local member keyset share");

        assert_eq!(material.keyset.id.get_version(), KeySetVersion::Version02);
        assert_eq!(material.secret_shares.len(), 1);
        let public_config = FederationConfig::new(params, members, vec![material.keyset.clone()])
            .expect("public config");
        let private_config = MemberConfigPrivate {
            federation_id: None,
            member_id: local_member,
            identity_secret_key: identity_secret(local_member),
            keyset_secret_shares: BTreeMap::from([(material.keyset.id, material.secret_shares)]),
            setup_transcript_hash: None,
        };
        private_config
            .validate_against_public(&public_config)
            .expect("derived private config validates");
    }

    #[test]
    fn dkg_finalized_member_config_assembles_from_durable_records() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let reveals = dkg_reveals(&setup_config);
        let commitments = dkg_commitments(&reveals);
        let round = FederationDkgCommitmentRound::new(&setup_config, commitments).expect("round");
        let local_member = member_id(2);
        let keyset_share = round
            .derive_member_keyset_share(
                &setup_config,
                &reveals,
                local_member,
                &dkg_member_secret_contributions(&setup_config, local_member),
            )
            .expect("local member keyset share");
        let mut public_config =
            FederationConfig::new(params, members, vec![keyset_share.keyset.clone()])
                .expect("public config");
        let private_configs = dkg_transcript_private_configs(&public_config);
        let transcript_signatures = dkg_transcript_signature_records(
            &public_config,
            std::slice::from_ref(&keyset_share.public_ceremony),
            &private_configs,
        );
        let finalized = FederationDkgFinalizedMemberConfig::from_records(
            &setup_config,
            private_configs
                .get(&local_member)
                .expect("local private config"),
            &[keyset_share.clone()],
            &transcript_signatures,
        )
        .expect("finalized member config");
        let transcript = finalized
            .public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript");

        assert_eq!(finalized.public_config.keysets, vec![keyset_share.keyset]);
        assert_eq!(
            finalized.private_config.federation_id,
            Some(finalized.public_config.federation_id)
        );
        assert_eq!(
            finalized.private_config.setup_transcript_hash,
            Some(transcript.transcript_hash)
        );
        finalized
            .private_config
            .validate_against_public(&finalized.public_config)
            .expect("finalized private validates");

        public_config.setup_transcript = Some(transcript.clone());
        assert_eq!(finalized.public_config, public_config);
    }

    #[test]
    fn dkg_finalized_member_config_rejects_missing_transcript_signature() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let setup_config =
            FederationConfig::new(params, members.clone(), Vec::new()).expect("setup config");
        let reveals = dkg_reveals(&setup_config);
        let commitments = dkg_commitments(&reveals);
        let round = FederationDkgCommitmentRound::new(&setup_config, commitments).expect("round");
        let local_member = member_id(2);
        let keyset_share = round
            .derive_member_keyset_share(
                &setup_config,
                &reveals,
                local_member,
                &dkg_member_secret_contributions(&setup_config, local_member),
            )
            .expect("local member keyset share");
        let public_config =
            FederationConfig::new(params, members, vec![keyset_share.keyset.clone()])
                .expect("public config");
        let private_configs = dkg_transcript_private_configs(&public_config);
        let mut transcript_signatures = dkg_transcript_signature_records(
            &public_config,
            std::slice::from_ref(&keyset_share.public_ceremony),
            &private_configs,
        );
        transcript_signatures.retain(|signature| signature.signer != member_id(3));

        assert!(matches!(
            FederationDkgFinalizedMemberConfig::from_records(
                &setup_config,
                private_configs
                    .get(&local_member)
                    .expect("local private config"),
                &[keyset_share],
                &transcript_signatures,
            ),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason.contains("missing transcript signature for member 3")
        ));
    }

    #[test]
    fn dkg_member_keyset_share_rejects_bad_secret_contributions() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let reveals = dkg_reveals(&config);
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&config, commitments).expect("commitment round");
        let local_member = member_id(2);
        let mut contributions = dkg_member_secret_contributions(&config, local_member);
        contributions
            .get_mut(&member_id(1))
            .expect("participant one")
            .secret_shares
            .insert(
                Amount::from(1),
                evaluate_secret_key_coefficients(
                    local_member.to_bls_signer_id(),
                    &[bls_key(91), bls_key(92)],
                )
                .expect("bad secret contribution"),
            );

        assert!(matches!(
            round.derive_member_keyset_share(
                &config,
                &reveals,
                local_member,
                &contributions,
            ),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason.contains("local secret contribution mismatch")
        ));

        let mut wrong_ceremony = dkg_member_secret_contributions(&config, local_member);
        wrong_ceremony
            .get_mut(&member_id(1))
            .expect("participant one")
            .ceremony_id = other_dkg_ceremony_id();
        assert!(matches!(
            round.derive_member_keyset_share(
                &config,
                &reveals,
                local_member,
                &wrong_ceremony,
            ),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason == "secret contribution belongs to another ceremony"
        ));

        let mut wrong_signer = dkg_member_secret_contributions(&config, local_member);
        let mut wrong_signer_coefficients = dkg_amount_coefficients(11);
        let wrong_signer_amount_coefficients = wrong_signer_coefficients
            .remove(&Amount::from(1))
            .expect("amount coefficients");
        wrong_signer
            .get_mut(&member_id(1))
            .expect("participant one")
            .secret_shares
            .insert(
                Amount::from(1),
                evaluate_secret_key_coefficients(
                    member_id(3).to_bls_signer_id(),
                    &wrong_signer_amount_coefficients,
                )
                .expect("wrong-signer contribution"),
            );

        assert!(matches!(
            round.derive_member_keyset_share(
                &config,
                &reveals,
                local_member,
                &wrong_signer,
            ),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason.contains("secret contribution signer mismatch")
        ));
    }

    #[test]
    fn dkg_reveal_rejects_keyset_spec_amount_mismatch() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config =
            FederationConfig::new(params, test_roster(), Vec::new()).expect("public config");
        let mut reveals = dkg_reveals(&config);
        reveals
            .get_mut(&member_id(1))
            .expect("first reveal")
            .coefficient_commitments
            .insert(
                Amount::from(2),
                vec![bls_key(41).public_key_g2(), bls_key(42).public_key_g2()],
            );
        let commitments = dkg_commitments(&reveals);
        let round =
            FederationDkgCommitmentRound::new(&config, commitments).expect("commitment round");

        assert!(matches!(
            round.derive_federated_keyset(&config, &reveals),
            Err(FederationError::DkgCommitmentMismatch { member_id: id, reason })
                if id == member_id(1)
                    && reason == "keyset specification amount set mismatch"
        ));
    }

    #[test]
    fn federation_payment_policy_validates_real_backend_quorum() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let mut config = FederationConfig::new(params, members, vec![keyset]).expect("config");

        config.validate().expect("consensus quorum policy");
        let mint_policy =
            *config.payment_verification_policy(FederationPaymentObservationContext::MintQuote);
        assert!(!mint_policy.permits_single_observation_finality());
        assert_eq!(
            mint_policy.required_matching_observations(),
            params.consensus_threshold
        );

        config.payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(params.consensus_threshold - 1);
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

        config.payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(params.member_count + 1);
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
        let development =
            FederationPaymentVerificationPolicy::development_single_observation_for_testing();
        let real = FederationPaymentVerificationPolicy::member_quorum(2);
        let single = FederationPaymentObservationQuorum {
            context: FederationPaymentObservationContext::MintQuote,
            required: 2,
            matching_observers: vec![member_id(1)],
            conflicting_observers: Vec::new(),
        };
        development
            .evaluate_quorum(&single)
            .expect("development policy accepts one matching observer");

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
    fn federated_keyset_validates_member_rotation_proof_of_possession() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, private_shares) = test_keyset_with_seed(&members, 7);
        let config = FederationConfig::new(params, members, vec![keyset.clone()]).expect("config");
        let member_id = member_id(1);
        let member_shares = private_shares
            .get(&member_id)
            .expect("member private shares");

        keyset
            .validate_member_rotation_proof_of_possession(
                config.federation_id,
                member_id,
                member_shares,
            )
            .expect("member proves rotation share possession");
    }

    #[test]
    fn federated_keyset_rejects_wrong_member_rotation_secret_share() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset_with_seed(&members, 7);
        let (_, wrong_private_shares) = test_keyset_with_seed(&members, 9);
        let config = FederationConfig::new(params, members, vec![keyset.clone()]).expect("config");
        let member_id = member_id(1);
        let member_shares = wrong_private_shares
            .get(&member_id)
            .expect("wrong member private shares");

        assert!(matches!(
            keyset.validate_member_rotation_proof_of_possession(
                config.federation_id,
                member_id,
                member_shares,
            ),
            Err(FederationError::PrivateShareMismatch { .. })
        ));
    }

    #[test]
    fn federation_config_applies_keyset_rotation_to_active_outputs() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (old_keyset, _) = test_keyset_with_seed(&members, 1);
        let (new_keyset, _) = test_keyset_with_seed(&members, 7);
        let config =
            FederationConfig::new(params, members, vec![old_keyset.clone()]).expect("config");
        let old_output = BlindedMessage::new(
            Amount::from(1),
            old_keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"old rotation output")),
        );
        let new_output = BlindedMessage::new(
            Amount::from(1),
            new_keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"new rotation output")),
        );

        config
            .validate_blinded_outputs(std::slice::from_ref(&old_output))
            .expect("old keyset active before rotation");
        let rotated = config
            .apply_keyset_rotation(&FederationKeysetRotationOperation {
                keyset: new_keyset.clone(),
            })
            .expect("apply keyset rotation");

        assert_eq!(rotated.keysets.len(), 2);
        assert!(
            !rotated
                .keysets
                .iter()
                .find(|keyset| keyset.id == old_keyset.id)
                .expect("old keyset")
                .active
        );
        assert!(
            rotated
                .keysets
                .iter()
                .find(|keyset| keyset.id == new_keyset.id)
                .expect("new keyset")
                .active
        );
        assert!(matches!(
            rotated.validate_blinded_outputs(std::slice::from_ref(&old_output)),
            Err(FederationError::InactiveFederatedKeyset { .. })
        ));
        rotated
            .validate_blinded_restore_outputs(&[old_output])
            .expect("old keyset remains restorable after rotation");
        rotated
            .validate_blinded_outputs(&[new_output])
            .expect("new keyset active after rotation");
    }

    #[test]
    fn federation_config_rejects_inactive_keyset_rotation_target() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (old_keyset, _) = test_keyset_with_seed(&members, 1);
        let (mut new_keyset, _) = test_keyset_with_seed(&members, 7);
        new_keyset.active = false;
        let config = FederationConfig::new(params, members, vec![old_keyset]).expect("config");

        assert!(matches!(
            config.apply_keyset_rotation(&FederationKeysetRotationOperation { keyset: new_keyset }),
            Err(FederationError::InactiveFederatedKeyset { .. })
        ));
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
