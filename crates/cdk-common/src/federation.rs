//! Shared federation types and public facade.
//!
//! Domain modules keep configuration, DKG, consensus, journal, operation,
//! signing, and transport ownership separate while this facade preserves the
//! established `cdk_common::federation` API.

use thiserror::Error;

mod address_allocation;
mod chain_observation;
mod config;
mod consensus;
mod dkg;
mod dkg_driver;
mod frost_config;
mod frost_dkg;
mod journal;
mod operation;
mod recovery;
mod signing;
mod transaction_proposal;
mod transport;
mod trusted_dealer;

/// Minimum number of members in a federation.
///
/// A one-node deployment has no remote consensus participant and must use
/// standalone mint mode instead of advertising an inoperable federation.
pub const FEDERATION_MIN_MEMBERS: u16 = 2;
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
/// Maximum JSON body accepted by one private FROST signing REST route.
pub const FEDERATION_MAX_FROST_SIGNING_REQUEST_BODY_BYTES: usize = 512 * 1024;

pub use self::address_allocation::{
    FederationAddressAllocation, FederationAddressAllocationOperation,
    FederationAddressAllocationOutcome, FederationAddressAllocationScope,
    FederationAddressAllocationState, FederationAddressKeychain, FederationAddressWalletState,
    FEDERATION_MAX_DERIVATION_INDEX,
};
pub use self::chain_observation::{
    FederationAllocatedAddressRef, FederationChainObservation, FederationChainObservationEvidence,
    FederationChainObservationOutcome, FederationChainObservationState,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationChainSourceSummary,
    FederationDepositConfirmation, FederationDepositObservation, FederationDepositReorgObservation,
    FederationOnchainQuotePayment, FederationWalletUtxo, FederationWalletUtxoState,
};
pub use self::config::{
    FederatedKeyset, FederationConfig, FederationConfigDigest, FederationConfigSnapshot,
    FederationId, FederationInvite, FederationPaymentObservationContext,
    FederationPaymentObservationPolicy, FederationPaymentVerificationPolicy, MemberConfigPrivate,
    MemberConfigPublic, MemberId, ThresholdParams,
};
pub use self::consensus::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftFinalizedUnitOutcome, FederationAlephBftPendingOperationRecord,
    FederationAlephBftUnitBodyHash, FederationAlephBftUnitPayload, FederationConsensusDefaultItem,
    FederationConsensusItem, FederationConsensusItemAdmission, FederationConsensusModuleFact,
    FederationConsensusRuntime, FederationConsensusRuntimeStatus, FederationQuorumVerifiedCatchUp,
    FederationVerifiedOperationAdmission, FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION,
    FEDERATION_CONSENSUS_ITEM_VERSION, FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
    FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS, FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
    FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
};
pub use self::dkg::{
    FederationDkgCeremonyId, FederationDkgCeremonyRecord, FederationDkgCeremonyState,
    FederationDkgCommitmentHash, FederationDkgCommitmentRound, FederationDkgFinalizedMemberConfig,
    FederationDkgKeysetSpec, FederationDkgKeysetTranscript, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgPublicCeremony, FederationDkgSetupMethod,
    FederationDkgSetupState, FederationDkgTranscript, FederationDkgTranscriptHash,
    FederationDkgTranscriptPayload, FederationDkgTranscriptSignature,
};
pub use self::dkg_driver::{
    FederationDkgDriverPhase, FederationDkgDriverPlan, FederationDkgDriverProgress,
    FederationDkgDriverRecords, ValidatedFederationDkgParticipantState,
};
pub use self::frost_config::{
    FederationCdkOnchainFrostAuthorizationAdapter, FederationFrostActiveEpochs,
    FederationFrostApplicationConfig, FederationFrostApplicationId,
    FederationFrostConsumerRegistration, FederationFrostEpochState,
    FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig,
    FederationFrostSealedStorageMetadata, FederationFrostWalletConfig,
    FederationOnchainConfirmationPolicy,
};
pub use self::frost_dkg::{
    FederationDualDkgResult, FederationFrostDkgConfirmationRequest, FederationFrostDkgContext,
    FederationFrostDkgLocalPhase, FederationFrostDkgLocalState, FederationFrostDkgRound1Request,
    FederationFrostDkgRound2Record, FederationFrostDkgRound2Request, FederationFrostRootRecord,
};
pub use self::journal::{
    FederationApplicationState, FederationJournalAppendOutcome, FederationJournalConsensusEvidence,
    FederationJournalDigest, FederationJournalEntry, FederationJournalTip,
    FederationMaterializedStateDigest, FederationMaterializedStateRow,
    FederationMaterializedStateRowFingerprint, FederationMaterializedStateSnapshot,
    FederationOperationStatus, InMemoryFederationJournal,
};
pub use self::operation::{
    FederationKeysetRotationOperation, FederationMeltOperation, FederationMeltQuoteMetadata,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOperation, FederationOperationAuthorization, FederationOperationBalanceAudit,
    FederationOperationConflictKey, FederationOperationEnvelope, FederationOperationId,
    FederationOperationKind, FederationOperationSignature, FederationPaymentObservation,
    FederationPaymentObservationEvidenceGroup, FederationPaymentObservationEvidenceGroupSummary,
    FederationPaymentObservationQuarantine, FederationPaymentObservationQuarantineReason,
    FederationPaymentObservationQuarantineRecord, FederationPaymentObservationQuorum,
    FederationPaymentObservationRecord, FederationPaymentObservationReport,
    FederationPaymentObservationReportAction, FederationPaymentObservationReportStatus,
    FederationPaymentObservationReportSummary, FederationSwapOperation,
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES, FEDERATION_OPERATION_VERSION,
};
pub use self::recovery::{
    FederationRecoveryAction, FederationRecoveryActionCount, FederationRecoveryPhase,
    FederationWalletRecoveryStatus,
};
pub use self::signing::{
    FederationBlindSignatureShare, FederationBlindSignatureShareSet, FederationSignatureShareRecord,
};
pub use self::transaction_proposal::{
    FederationAcceptedTransaction, FederationAcceptedTransactionState,
    FederationCdkOnchainFrostAuthorization, FederationFrostInputSignatureAudit,
    FederationFrostSigningAuthorizationReference, FederationOnchainBatchId,
    FederationOnchainMeltPaymentReceipt, FederationOnchainPaymentBatch,
    FederationOnchainPaymentIntent, FederationOnchainPaymentIntentId,
    FederationOnchainPaymentIntentRecord, FederationOnchainPaymentIntentState,
    FederationSignedTransaction, FederationTaprootSighashType,
    FederationTransactionBroadcastIntent, FederationTransactionBroadcastObservation,
    FederationTransactionBroadcastObservationRecord, FederationTransactionBroadcastResult,
    FederationTransactionBroadcastState, FederationTransactionChangeOutput,
    FederationTransactionInput, FederationTransactionPolicyAudit, FederationTransactionProposal,
    FederationTransactionProposalId, FederationTransactionRecipientOutput,
    FederationTransactionScope, FederationTransactionSettlement,
    FederationTransactionSettlementPolicy, FederationTransactionState,
    FederationTransactionStateOutcome, FEDERATION_MAX_ONCHAIN_BATCH_INTENTS,
    FEDERATION_MAX_ONCHAIN_PROPOSAL_BYTES, FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS,
    FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS, FEDERATION_MAX_ONCHAIN_PSBT_CONSENSUS_BYTES,
    FEDERATION_MAX_ONCHAIN_TRANSACTION_WEIGHT,
};
pub use self::transport::{
    FederationAlephBftActivityRequest, FederationAlephBftActivityResponse,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationCatchUpCertificate, FederationCatchUpCertificateQuorum, FederationCatchUpOperation,
    FederationDkgActivationRequest, FederationDkgCommitmentRequest, FederationDkgReadinessRequest,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationFrostAggregateResultNotification,
    FederationFrostCommitmentRequest, FederationFrostCommitmentResponse,
    FederationFrostSignatureShareResponse, FederationFrostSigningAbortRequest,
    FederationFrostSigningAck, FederationFrostSigningPackageRequest,
    FederationInvalidOperationAlertSummary, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationLocalStatusResponse, FederationOnchainWalletStatus,
    FederationPeerStatus, FederationPeerStatusRequest, FederationRestBodyHash,
    FederationRestMessageKind, FederationRestSequenceTracker, FederationRestSignature,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    FEDERATION_CATCH_UP_CERTIFICATE_VERSION, FEDERATION_REST_API_VERSION,
};
pub use self::trusted_dealer::{
    generate_trusted_dealer_configs, generate_trusted_dealer_configs_with_options,
    TrustedDealerConfigBundle, TrustedDealerGenerationOptions, TrustedDealerKeysetSpec,
    TrustedDealerMember,
};

use crate::{Amount, CurrencyUnit, Id};

const FEDERATION_ROSTER_HASH_VERSION: &[u8] = b"cdk-federation-roster-v1";
const FEDERATION_CONFIG_DIGEST_VERSION: &[u8] = b"cdk-federation-config-digest-v1";
const FEDERATION_DKG_COMMITMENT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-commitment-v1";
const FEDERATION_DKG_PARTICIPANT_STATE_VERSION: u16 = 1;
const FEDERATION_DKG_PUBLIC_CEREMONY_VERSION: u16 = 1;
const FEDERATION_DKG_TRANSCRIPT_HASH_VERSION: &[u8] = b"cdk-federation-dkg-transcript-v1";
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
/// Wallet-facing federation protocol version advertised by configs and members.
pub const FEDERATION_WALLET_PROTOCOL_VERSION: u16 = 1;

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
    /// Canonical address allocation state or operation is malformed.
    #[error("Federation address allocation is invalid: {reason}")]
    AddressAllocationInvalid {
        /// Stable validation reason without local data.
        reason: String,
    },
    /// One quote was already bound to a different descriptor branch.
    #[error("Federation address allocation conflicts with the existing mapping for quote {quote}")]
    AddressAllocationConflict {
        /// Conflicting quote/allocation key.
        quote: String,
    },
    /// A descriptor branch exhausted its non-hardened BIP32 range.
    #[error("Federation {keychain} address derivation index is exhausted")]
    AddressDerivationExhausted {
        /// Exhausted external or internal branch.
        keychain: FederationAddressKeychain,
    },
    /// Canonical member chain evidence is malformed or unauthorized.
    #[error("Federation chain observation is invalid: {reason}")]
    ChainObservationInvalid {
        /// Stable validation reason without member-local source details.
        reason: String,
    },
    /// Durable federation wallet UTXO state violates a safety invariant.
    #[error("Federation chain observation state is invalid: {reason}")]
    ChainObservationStateInvalid {
        /// Stable validation reason without member-local source details.
        reason: String,
    },
    /// Canonical on-chain intent, batch, or transaction proposal is malformed.
    #[error("Federation on-chain transaction proposal is invalid: {reason}")]
    TransactionProposalInvalid {
        /// Stable validation reason without proposer-local details.
        reason: String,
    },
    /// A public or private FROST wallet configuration failed validation.
    #[error("Invalid federation FROST wallet configuration: {reason}")]
    FrostConfigurationInvalid {
        /// Stable validation reason without secret material.
        reason: String,
    },
    /// An application-aware FROST signing transport message failed validation.
    #[error("Federation FROST signing request is invalid: {reason}")]
    FrostSigningInvalid {
        /// Stable validation reason without nonce or local storage material.
        reason: String,
    },
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
    /// A federation journal digest must be exactly 32 bytes.
    #[error("Invalid federation journal digest size: expected={expected}, found={found}")]
    InvalidJournalDigestSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation materialized-state digest must be exactly 32 bytes.
    #[error(
        "Invalid federation materialized-state digest size: expected={expected}, found={found}"
    )]
    InvalidMaterializedStateDigestSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// Member IDs are non-zero u16 values.
    #[error("Invalid federation member id: {0}")]
    InvalidMemberId(u16),
    /// Federation mode requires at least two consensus participants.
    #[error(
        "Federation requires at least {minimum} members; found {found}; use standalone mint mode for a one-node deployment"
    )]
    InsufficientMembers {
        /// Configured federation member count.
        found: u16,
        /// Minimum supported federation member count.
        minimum: u16,
    },
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
    /// A required federation runtime component is not active.
    #[error("Federation runtime component is unavailable: {component}")]
    RuntimeUnavailable {
        /// Stable name of the unavailable runtime component.
        component: &'static str,
    },
    /// The member is behind a peer and cannot serve authoritative requests.
    #[error(
        "Federation member is lagging behind peer {peer}: local_operations={local_operation_count}, peer_operations={peer_operation_count}"
    )]
    NodeLagging {
        /// Peer whose order is ahead of the local member.
        peer: MemberId,
        /// Local durable operation count.
        local_operation_count: u64,
        /// Peer's advertised durable operation count.
        peer_operation_count: u64,
    },
    /// The member is applying history obtained from a peer.
    #[error("Federation member is catching up from peer {peer}")]
    NodeCatchingUp {
        /// Peer supplying catch-up history.
        peer: MemberId,
    },
    /// The member is waiting for quorum-certified catch-up evidence.
    #[error(
        "Federation member is verifying quorum catch-up certificates after catch-up from peer {peer}: operation_count={operation_count}"
    )]
    NodeVerifyingCatchUpCertificate {
        /// Peer that supplied the candidate history.
        peer: MemberId,
        /// Candidate durable operation count.
        operation_count: u64,
    },
    /// The member's materialized state has not converged with its ordered history.
    #[error(
        "Federation member is waiting for materialized state digest convergence with peer {peer}: operation_count={operation_count}"
    )]
    NodeMaterializedStateMismatch {
        /// Peer whose materialized digest differs.
        peer: MemberId,
        /// Durable operation count at which the mismatch was observed.
        operation_count: u64,
    },
    /// The member is halted and cannot serve authoritative requests.
    #[error("Federation member is halted: {reason}")]
    NodeHalted {
        /// Redacted operator-facing halt reason.
        reason: String,
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
    /// The configured consensus threshold does not satisfy AlephBFT's BFT quorum rule.
    #[error("Invalid AlephBFT consensus threshold: expected={expected}, found={found}")]
    InvalidAlephBftConsensusThreshold {
        /// Threshold required by the AlephBFT roster size.
        expected: u16,
        /// Threshold configured by the federation.
        found: u16,
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
    /// A peer status omitted the durable AlephBFT finalized-item cursor.
    #[error("Federation member {member_id} AlephBFT status is missing its finalized-item cursor")]
    AlephBftFinalizedCursorMissing {
        /// Member whose status omitted the cursor.
        member_id: MemberId,
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
    /// A live-session rotation was requested while operations remained pending.
    #[error(
        "Federation AlephBFT session rotation blocked: session={session}, pending_items={pending_items}"
    )]
    AlephBftSessionRotationBlocked {
        /// Session that cannot yet rotate.
        session: u64,
        /// Consensus items still awaiting finalization.
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
    /// A consensus operation did not carry durable member authorization.
    #[error("Federation operation is missing member authorization")]
    OperationAuthorizationMissing,
    /// A federation operation signature must be exactly 64 bytes.
    #[error("Invalid federation operation signature size: expected={expected}, found={found}")]
    InvalidOperationSignatureSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        found: usize,
    },
    /// A federation operation signature could not be decoded as Schnorr.
    #[error("Invalid federation operation signature encoding")]
    InvalidOperationSignature,
    /// A federation operation had an invalid member identity signature.
    #[error("Federation operation signature verification failed for member {member_id}")]
    OperationSignatureVerificationFailed {
        /// Author whose signature was checked.
        member_id: MemberId,
    },
    /// An operation's claimed member identity did not match its authenticated author.
    #[error(
        "Federation operation author mismatch: author={author}, claimed_member={claimed_member}"
    )]
    OperationAuthorMismatch {
        /// Member that signed the operation.
        author: MemberId,
        /// Member identity claimed inside the operation payload.
        claimed_member: MemberId,
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
    /// Signed catch-up certificate used an unsupported payload version.
    #[error(
        "Federation catch-up certificate version mismatch: expected={expected}, found={found}"
    )]
    CatchUpCertificateVersionMismatch {
        /// Expected catch-up certificate version.
        expected: u16,
        /// Found catch-up certificate version.
        found: u16,
    },
    /// Signed catch-up certificate did not match the catch-up response it accompanied.
    #[error("Federation catch-up certificate mismatch: {reason}")]
    CatchUpCertificateMismatch {
        /// Human-readable mismatch reason.
        reason: String,
    },
    /// Signed catch-up certificate had an invalid member identity signature.
    #[error(
        "Federation catch-up certificate signature verification failed for member {member_id}"
    )]
    CatchUpCertificateSignatureVerificationFailed {
        /// Member whose signature was checked.
        member_id: MemberId,
    },
    /// Not enough distinct members signed matching catch-up certificates.
    #[error(
        "Federation catch-up certificate quorum not reached: required={required}, found={found}"
    )]
    CatchUpCertificateQuorumNotReached {
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

fn append_str(transcript: &mut Vec<u8>, value: &str) {
    append_bytes(transcript, value.as_bytes());
}

fn append_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;

    use bitcoin::base64::engine::general_purpose;
    use bitcoin::base64::Engine as _;
    use cashu::nuts::nut01::{
        evaluate_secret_key_coefficients, trusted_dealer_keygen, BlsG1PublicKey, BlsSecretKey,
        BlsSecretKeyShare,
    };

    use crate::nuts::nut02::KeySetVersion;
    use crate::{mint_url::MintUrl, BlindedMessage, PublicKey, SecretKey};

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
        info.wallet_protocol_version = info.wallet_protocol_version.saturating_add(1);

        assert!(matches!(
            config.validate_member_mint_info(member_id(2), &info),
            Err(FederationError::ProtocolVersionMismatch {
                component,
                expected,
                found,
            }) if component == "wallet"
                && expected == FEDERATION_WALLET_PROTOCOL_VERSION
                && found == FEDERATION_WALLET_PROTOCOL_VERSION.saturating_add(1)
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
            ThresholdParams::with_bft_consensus(1, 1),
            Err(FederationError::InsufficientMembers {
                found: 1,
                minimum: FEDERATION_MIN_MEMBERS
            })
        ));
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
    fn federation_config_advertises_only_current_wallet_protocol_version() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");

        assert_eq!(
            config.wallet_protocol_version,
            FEDERATION_WALLET_PROTOCOL_VERSION
        );

        let json = serde_json::to_value(&config).expect("config json");
        assert_eq!(
            json.get("wallet_protocol_version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(FEDERATION_WALLET_PROTOCOL_VERSION))
        );
        assert!(json.get("protocol_versions").is_none());
    }

    #[test]
    fn federation_config_rejects_removed_member_protocol_fields() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");

        for field in ["protocol_versions", "consensus_item_version"] {
            let mut json = serde_json::to_value(&config).expect("config json");
            json[field] = serde_json::json!({ "operation": 1 });
            serde_json::from_value::<FederationConfig>(json)
                .expect_err("removed member protocol metadata must fail closed");
        }
    }

    #[test]
    fn federation_config_rejects_missing_wallet_protocol_version() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");
        let mut json = serde_json::to_value(&config).expect("config json");
        json.as_object_mut()
            .expect("config object")
            .remove("wallet_protocol_version");

        assert!(
            serde_json::from_value::<FederationConfig>(json).is_err(),
            "federation configs must explicitly advertise the wallet protocol version"
        );
    }

    #[test]
    fn federation_config_rejects_unsupported_wallet_protocol_version() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let mut config = FederationConfig::new(params, test_roster(), Vec::new()).expect("config");
        let expected = config.wallet_protocol_version;
        let found = expected.saturating_add(1);
        config.wallet_protocol_version = found;
        assert_protocol_version_mismatch(config, "wallet", expected, found);
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
    fn federation_config_rejects_missing_keyset_fee() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
        let (keyset, _) = test_keyset(&members);
        let config = FederationConfig::new(params, members, vec![keyset]).expect("config");
        let mut json = serde_json::to_value(&config).expect("config json");
        json["keysets"][0]
            .as_object_mut()
            .expect("keyset object")
            .remove("input_fee_ppk");

        assert!(serde_json::from_value::<FederationConfig>(json).is_err());
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
        assert_eq!(
            commitment_hash.to_hex(),
            "0cbfd40e1de0dc973b5e9bc8fc2ac50529181b4c814c1db3f14491be4222a6ae"
        );
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
    fn dkg_participant_state_accepts_only_current_v1_shape() {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        let members = test_roster();
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

        let state = FederationDkgParticipantState::generate(
            &setup_config,
            dkg_ceremony_id(),
            member_id(1),
            keyset_spec.clone(),
        )
        .expect("participant state");
        let state_json = serde_json::to_value(&state).expect("participant state json");
        assert_eq!(state_json["version"], serde_json::json!(1));
        for version in [0, 2, u16::MAX] {
            let mut unsupported = state_json.clone();
            unsupported["version"] = serde_json::json!(version);
            let unsupported = serde_json::from_value::<FederationDkgParticipantState>(unsupported)
                .expect("complete non-v1 state decodes before semantic validation");
            unsupported
                .validate(&setup_config)
                .expect_err("non-v1 participant state must fail closed");
        }
        let mut unknown = state_json;
        unknown["unknown_state_field"] = serde_json::json!(true);
        serde_json::from_value::<FederationDkgParticipantState>(unknown)
            .expect_err("unknown participant state fields must fail closed");
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
            transcript.transcript_hash.to_hex(),
            "5ed12527c0fd385dc5d3fb5d275429fdeeaeff416f0ea8dc32a3c1f2bd49cb88"
        );
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
