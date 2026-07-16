//! Deterministic local consensus boundary.
//!
//! This is not a production quorum protocol. It validates federation identity,
//! appends operations to the accepted journal, and exposes the same
//! status/catch-up/submit payloads used by private REST. Production ordering
//! backends can replace the decision source while keeping this API shape.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use super::{
    FederationAlephBftRecoveryCursor, FederationApplicationState, FederationCheckpoint,
    FederationCheckpointId, FederationConfig, FederationError, FederationId,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationJournalConsensusEvidence, FederationJournalEntry, FederationMaterializedStateDigest,
    FederationOperationConflictKey, FederationOperationEnvelope, FederationOperationId,
    FederationOperationStatus, FederationPeerStatus, FederationRestMessageKind,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationJournal, MemberId, Result, ThresholdParams,
    VerifiedFederationCheckpointQuorum, VerifiedFederationSuffix,
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES,
};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const DEFAULT_CATCH_UP_LIMIT: u16 = 100;
const MAX_CATCH_UP_LIMIT: u16 = 1000;

/// Version of consensus item payloads.
pub const FEDERATION_CONSENSUS_ITEM_VERSION: u16 = 1;

/// Default maximum canonical size for one consensus item.
pub const FEDERATION_MAX_CONSENSUS_ITEM_BYTES: usize =
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES + 4096;

const FEDERATION_ALEPH_BFT_UNIT_BODY_HASH_VERSION: &[u8] = b"cdk-federation-aleph-bft-unit-body-v1";
const FEDERATION_RECOVERY_BARRIER_ID_VERSION: &[u8] = b"cdk-federation-recovery-barrier-id-v1";

/// Version of AlephBFT unit payloads carrying CDK consensus items.
pub const FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION: u16 = 1;

/// Version of ordered recovery-barrier consensus items.
pub const FEDERATION_RECOVERY_BARRIER_VERSION: u16 = 1;

/// Default maximum number of consensus items in one AlephBFT unit payload.
pub const FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS: usize = 64;

/// Default maximum canonical item-body bytes in one AlephBFT unit payload.
pub const FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES: usize = FEDERATION_MAX_CONSENSUS_ITEM_BYTES * 4;

/// Default maximum canonical bytes for one complete AlephBFT unit payload.
pub const FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES: usize =
    FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES + 16 * 1024;

/// Digest of the ordered consensus-item body in an AlephBFT unit payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationAlephBftUnitBodyHash([u8; 32]);

impl FederationAlephBftUnitBodyHash {
    /// Create a unit body hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a unit body hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidAlephBftUnitBodyHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a unit body hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this unit body hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this unit body hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationAlephBftUnitBodyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationAlephBftUnitBodyHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationAlephBftUnitBodyHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationAlephBftUnitBodyHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Canonical payload carried by an AlephBFT unit.
///
/// The payload deliberately remains CDK-specific: AlephBFT orders units, while
/// the unit body carries already-versioned CDK consensus items.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftUnitPayload {
    /// AlephBFT unit payload version.
    pub version: u16,
    /// Federation whose consensus instance produced the unit.
    pub federation_id: FederationId,
    /// Member that submitted this unit.
    pub submitting_member: MemberId,
    /// Consensus session containing this unit.
    pub session: u64,
    /// Submitting member's unit index within the session.
    pub unit_index: u64,
    /// Operation IDs derived from operation items, in body order.
    pub operation_ids: Vec<FederationOperationId>,
    /// Hash of the ordered canonical item body.
    pub body_hash: FederationAlephBftUnitBodyHash,
    /// Ordered CDK consensus items proposed by this unit.
    pub items: Vec<FederationConsensusItem>,
}

impl FederationAlephBftUnitPayload {
    /// Build a canonical AlephBFT unit payload from ordered consensus items.
    pub fn new(
        federation_id: FederationId,
        submitting_member: MemberId,
        session: u64,
        unit_index: u64,
        items: Vec<FederationConsensusItem>,
    ) -> Result<Self> {
        let operation_ids = collect_operation_ids(&items)?;
        let body_hash = compute_aleph_bft_unit_body_hash(&items)?;
        let payload = Self {
            version: FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION,
            federation_id,
            submitting_member,
            session,
            unit_index,
            operation_ids,
            body_hash,
            items,
        };
        payload.ensure_supported_version()?;
        Ok(payload)
    }

    /// Return canonical JSON bytes with all object keys sorted.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_supported_version()?;
        canonical_json_bytes(self)
    }

    /// Return this unit payload's canonical size.
    pub fn canonical_size(&self) -> Result<usize> {
        Ok(self.canonical_bytes()?.len())
    }

    /// Reject this payload if its canonical encoding exceeds `max_bytes`.
    pub fn ensure_within_size_limit(&self, max_bytes: usize) -> Result<usize> {
        let actual = self.canonical_size()?;
        if actual > max_bytes {
            return Err(FederationError::ConsensusItemTooLarge {
                actual,
                max: max_bytes,
            });
        }

        Ok(actual)
    }

    /// Reject this payload if its metadata or body hash cannot be verified.
    pub fn ensure_supported_version(&self) -> Result<()> {
        if self.version != FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION {
            return Err(FederationError::ProtocolVersionMismatch {
                component: "aleph_bft_unit_payload",
                expected: FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION,
                found: self.version,
            });
        }
        for item in &self.items {
            item.ensure_supported_version()?;
            item.ensure_federation_id(self.federation_id)?;
        }

        let expected_operation_ids = collect_operation_ids(&self.items)?;
        if self.operation_ids != expected_operation_ids {
            return Err(FederationError::AlephBftUnitOperationIdsMismatch);
        }

        let expected_hash = compute_aleph_bft_unit_body_hash(&self.items)?;
        if self.body_hash != expected_hash {
            return Err(FederationError::AlephBftUnitBodyHashMismatch {
                expected: expected_hash.to_hex(),
                found: self.body_hash.to_hex(),
            });
        }

        Ok(())
    }
}

/// Deterministic status for one finalized consensus item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationAlephBftFinalizedItemStatus {
    /// The item should be considered by the deterministic apply loop.
    Accepted,
    /// The item wraps an operation already seen in finalized order.
    DuplicateOperation,
    /// The operation failed durable member-authentication checks.
    RejectedInvalidOperation,
    /// The recovery barrier requested a cursor beyond its own finalized position.
    RejectedInvalidRecoveryBarrier,
}

/// One consensus item after AlephBFT finalized its containing unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAlephBftFinalizedItem {
    /// Global accepted item index assigned by the local finalization handler.
    pub accepted_item_index: u64,
    /// AlephBFT session containing the finalized unit.
    pub session: u64,
    /// Unit position in finalized order within the session.
    pub unit_finalization_index: u64,
    /// Member that submitted the finalized unit.
    pub unit_submitting_member: MemberId,
    /// Submitting member's unit index within the session.
    pub unit_index: u64,
    /// Item index inside the finalized unit body.
    pub item_index_in_unit: u64,
    /// Operation ID when this item wraps an operation envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<FederationOperationId>,
    /// Status to guide deterministic application.
    pub status: FederationAlephBftFinalizedItemStatus,
    /// Hash of the ordered unit body that produced this item.
    pub unit_body_hash: FederationAlephBftUnitBodyHash,
    /// Finalized consensus item body.
    pub item: FederationConsensusItem,
}

/// Result of finalizing one AlephBFT unit payload locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAlephBftFinalizedUnitOutcome {
    /// AlephBFT session containing the finalized unit.
    pub session: u64,
    /// Unit position in finalized order within the session.
    pub unit_finalization_index: u64,
    /// Member that submitted the finalized unit.
    pub unit_submitting_member: MemberId,
    /// Submitting member's unit index within the session.
    pub unit_index: u64,
    /// Hash of the ordered unit body.
    pub unit_body_hash: FederationAlephBftUnitBodyHash,
    /// Ordered consensus items produced from the finalized unit.
    pub items: Vec<FederationAlephBftFinalizedItem>,
}

impl FederationAlephBftFinalizedUnitOutcome {
    /// Return operation IDs that were seen before this finalized unit.
    #[must_use]
    pub fn duplicate_operation_ids(&self) -> Vec<FederationOperationId> {
        self.items
            .iter()
            .filter_map(|item| {
                if item.status == FederationAlephBftFinalizedItemStatus::DuplicateOperation {
                    item.operation_id
                } else {
                    None
                }
            })
            .collect()
    }
}

impl FederationAlephBftFinalizedItem {
    /// Return this finalized fact's canonical encoded size for checkpoint budgeting.
    pub fn canonical_size(&self) -> Result<usize> {
        Ok(canonical_json_bytes(self)?.len())
    }

    /// Return consensus evidence matching this finalized item.
    #[must_use]
    pub fn consensus_evidence(&self) -> FederationJournalConsensusEvidence {
        FederationJournalConsensusEvidence::new(
            self.session,
            self.accepted_item_index,
            self.unit_finalization_index,
            self.unit_submitting_member,
            self.unit_index,
        )
    }

    /// Validate that another retained item is the exact same finalized fact.
    pub fn ensure_matches(&self, other: &Self) -> Result<()> {
        if self.accepted_item_index != other.accepted_item_index
            || self.session != other.session
            || self.unit_finalization_index != other.unit_finalization_index
            || self.unit_submitting_member != other.unit_submitting_member
            || self.unit_index != other.unit_index
            || self.item_index_in_unit != other.item_index_in_unit
            || self.operation_id != other.operation_id
            || self.status != other.status
            || self.unit_body_hash != other.unit_body_hash
            || self.item.canonical_bytes()? != other.item.canonical_bytes()?
        {
            return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                reason: format!(
                    "finalized item mismatch at accepted item index {}",
                    other.accepted_item_index
                ),
            });
        }
        Ok(())
    }
}

/// Operator-facing status for the consensus runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationConsensusRuntimeStatus {
    /// Federation whose operations are being ordered.
    pub federation_id: FederationId,
    /// Local member producing unit payloads.
    pub local_member: MemberId,
    /// Current AlephBFT session accepted by the runtime.
    pub session: u64,
    /// Next local unit index that will be used by the data provider.
    pub next_unit_index: u64,
    /// Next finalized unit index expected by the finalization handler.
    pub next_unit_finalization_index: u64,
    /// Next global finalized consensus-item index.
    pub next_item_index: u64,
    /// Number of local consensus items waiting to be proposed.
    pub queued_item_count: u64,
    /// Number of drained local consensus items waiting for finality.
    pub pending_finalization_item_count: u64,
    /// Number of finalized consensus items retained by the runtime.
    pub finalized_item_count: u64,
    /// Number of accepted operation entries in the runtime journal.
    pub accepted_operation_count: u64,
    /// Highest accepted operation index, if any operation has been accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_accepted_operation_index: Option<u64>,
    /// Number of members allowed to submit finalized units, when roster-bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_member_count: Option<u64>,
}

/// Persisted local AlephBFT operation that has been accepted into the pending
/// mempool but not yet finalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAlephBftPendingOperationRecord {
    /// Operation identifier derived from the canonical operation envelope.
    pub operation_id: FederationOperationId,
    /// AlephBFT session this local operation is queued for.
    pub session_id: u64,
    /// Canonical operation envelope to replay into the local AlephBFT queue.
    pub envelope: FederationOperationEnvelope,
}

/// Stable generation identifier for one ordered recovery-barrier request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationRecoveryBarrierId([u8; 32]);

impl FederationRecoveryBarrierId {
    /// Create a barrier ID from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a barrier ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| FederationError::CheckpointInvalid {
                reason: format!(
                    "recovery barrier id must be 32 bytes, found {}",
                    bytes.len()
                ),
            })?;
        Ok(Self(bytes))
    }

    /// Parse a barrier ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return the raw barrier ID bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return the barrier ID as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationRecoveryBarrierId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationRecoveryBarrierId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationRecoveryBarrierId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationRecoveryBarrierId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Bounded request to select a recovery checkpoint through AlephBFT order.
///
/// The request commits only to the latest scheduled anchor known by the
/// proposer and the finalized cursor that needed coverage. It never carries a
/// checkpoint boundary, rolling digest, or materialized-state claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationRecoveryBarrier {
    /// Barrier format version.
    pub version: u16,
    /// Federation whose ordered log must select the checkpoint boundary.
    pub federation_id: FederationId,
    /// Latest scheduled checkpoint sequence known by the proposer.
    pub scheduled_checkpoint_sequence: u64,
    /// Latest scheduled checkpoint ID, absent before the first scheduled checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_anchor_id: Option<FederationCheckpointId>,
    /// Finalized cursor that the resulting checkpoint must move beyond.
    pub requested_after_finalized_item_index: u64,
    /// Stable generation derived from the other fields and federation ID.
    pub generation: FederationRecoveryBarrierId,
}

impl FederationRecoveryBarrier {
    /// Construct a canonical recovery-barrier request.
    pub fn new(
        federation_id: FederationId,
        scheduled_checkpoint_sequence: u64,
        scheduled_anchor_id: Option<FederationCheckpointId>,
        requested_after_finalized_item_index: u64,
    ) -> Result<Self> {
        let generation = recovery_barrier_id(
            federation_id,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            requested_after_finalized_item_index,
        )?;
        let barrier = Self {
            version: FEDERATION_RECOVERY_BARRIER_VERSION,
            federation_id,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            requested_after_finalized_item_index,
            generation,
        };
        barrier.ensure_supported_version()?;
        Ok(barrier)
    }

    /// Validate version, anchor shape, and generation binding.
    pub fn ensure_supported_version(&self) -> Result<()> {
        if self.version != FEDERATION_RECOVERY_BARRIER_VERSION {
            return Err(FederationError::ProtocolVersionMismatch {
                component: "recovery_barrier",
                expected: FEDERATION_RECOVERY_BARRIER_VERSION,
                found: self.version,
            });
        }
        if (self.scheduled_checkpoint_sequence == 0) != self.scheduled_anchor_id.is_none() {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery barrier scheduled anchor shape is invalid".to_string(),
            });
        }
        let expected = recovery_barrier_id(
            self.federation_id,
            self.scheduled_checkpoint_sequence,
            self.scheduled_anchor_id,
            self.requested_after_finalized_item_index,
        )?;
        if self.generation != expected {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery barrier generation does not match its request".to_string(),
            });
        }
        Ok(())
    }
}

/// One item that can enter the federation consensus pipeline.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationConsensusItem {
    /// A canonical Cashu mint operation envelope.
    Operation(FederationOperationEnvelope),
    /// A bounded request to select a recovery checkpoint after this ordered unit.
    RecoveryBarrier(FederationRecoveryBarrier),
    /// A module-defined fact that must be ordered with operations.
    ModuleFact(FederationConsensusModuleFact),
    /// A generic consensus item for future compatibility tests.
    Default(FederationConsensusDefaultItem),
}

impl FederationConsensusItem {
    /// Wrap an operation envelope as a consensus item.
    #[must_use]
    pub fn operation(envelope: FederationOperationEnvelope) -> Self {
        Self::Operation(envelope)
    }

    /// Wrap a recovery barrier as a consensus item.
    #[must_use]
    pub fn recovery_barrier(barrier: FederationRecoveryBarrier) -> Self {
        Self::RecoveryBarrier(barrier)
    }

    /// Return canonical JSON bytes with all object keys sorted.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_supported_version()?;
        canonical_json_bytes(self)
    }

    /// Return this item's canonical size.
    pub fn canonical_size(&self) -> Result<usize> {
        Ok(self.canonical_bytes()?.len())
    }

    /// Reject this item if its canonical encoding exceeds `max_bytes`.
    pub fn ensure_within_size_limit(&self, max_bytes: usize) -> Result<usize> {
        self.ensure_supported_version()?;
        if let Self::Operation(envelope) = self {
            envelope.ensure_admissible_size()?;
        }

        let actual = self.canonical_size()?;
        if actual > max_bytes {
            return Err(FederationError::ConsensusItemTooLarge {
                actual,
                max: max_bytes,
            });
        }

        Ok(actual)
    }

    /// Reject this item if its canonical encoding exceeds the default limit.
    pub fn ensure_admissible_size(&self) -> Result<usize> {
        self.ensure_within_size_limit(FEDERATION_MAX_CONSENSUS_ITEM_BYTES)
    }

    /// Return the operation ID when this item wraps an operation envelope.
    pub fn operation_id(&self) -> Result<Option<FederationOperationId>> {
        match self {
            Self::Operation(envelope) => Ok(Some(envelope.operation_id()?)),
            Self::RecoveryBarrier(_) | Self::ModuleFact(_) | Self::Default(_) => Ok(None),
        }
    }

    /// Reject consensus items from unsupported item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        match self {
            Self::Operation(envelope) => envelope.ensure_supported_version(),
            Self::RecoveryBarrier(barrier) => barrier.ensure_supported_version(),
            Self::ModuleFact(item) => item.ensure_supported_version(),
            Self::Default(item) => item.ensure_supported_version(),
        }
    }

    fn ensure_federation_id(&self, federation_id: FederationId) -> Result<()> {
        match self {
            Self::RecoveryBarrier(barrier) if barrier.federation_id != federation_id => {
                Err(FederationError::CheckpointInvalid {
                    reason: "recovery barrier federation does not match its AlephBFT unit"
                        .to_string(),
                })
            }
            Self::Operation(_)
            | Self::RecoveryBarrier(_)
            | Self::ModuleFact(_)
            | Self::Default(_) => Ok(()),
        }
    }
}

/// A member-to-member operation submission that passed signed REST admission checks.
#[derive(Debug, Clone)]
pub struct FederationVerifiedOperationAdmission {
    /// Member that signed the private REST submission.
    pub sender: MemberId,
    /// Sender-local private REST sequence covered by the signature.
    pub sequence: u64,
    /// Operation ID computed from the exact submitted operation envelope.
    pub operation_id: FederationOperationId,
    /// Consensus item preserving the submitted operation envelope.
    pub item: FederationConsensusItem,
}

impl FederationVerifiedOperationAdmission {
    /// Verify a signed private REST operation submission and prepare it for consensus admission.
    pub fn from_signed_submit_request(
        request: &FederationSignedRequest<FederationSubmitOperationRequest>,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<Self> {
        request.verify(public_config, expected_receiver)?;
        if request.kind != FederationRestMessageKind::SubmitOperation {
            return Err(FederationError::RestMessageKindMismatch {
                expected: FederationRestMessageKind::SubmitOperation,
                found: request.kind,
            });
        }

        request.payload.ensure_admissible_size()?;
        let operation_id = request.payload.operation_id()?;
        let item = FederationConsensusItem::operation(request.payload.envelope.clone());
        item.ensure_admissible_size()?;

        Ok(Self {
            sender: request.sender,
            sequence: request.sequence,
            operation_id,
            item,
        })
    }

    /// Return the operation envelope carried by this verified admission.
    #[must_use]
    pub fn operation_envelope(&self) -> &FederationOperationEnvelope {
        match &self.item {
            FederationConsensusItem::Operation(envelope) => envelope,
            FederationConsensusItem::RecoveryBarrier(_)
            | FederationConsensusItem::ModuleFact(_)
            | FederationConsensusItem::Default(_) => {
                unreachable!("verified operation admissions always carry operation items")
            }
        }
    }

    /// Return the sender and sequence covered by the signed REST request.
    #[must_use]
    pub fn sender_sequence(&self) -> (MemberId, u64) {
        (self.sender, self.sequence)
    }
}

/// Module-defined fact to order through consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConsensusModuleFact {
    /// Item format version.
    pub version: u16,
    /// Module namespace.
    pub module: String,
    /// Fact kind within the module namespace.
    pub kind: String,
    /// Canonical JSON payload.
    pub payload: serde_json::Value,
}

impl FederationConsensusModuleFact {
    /// Create a module fact consensus item.
    #[must_use]
    pub fn new<M, K>(module: M, kind: K, payload: serde_json::Value) -> Self
    where
        M: Into<String>,
        K: Into<String>,
    {
        Self {
            version: FEDERATION_CONSENSUS_ITEM_VERSION,
            module: module.into(),
            kind: kind.into(),
            payload,
        }
    }

    /// Reject module facts from unsupported consensus-item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        ensure_consensus_item_version(self.version)
    }
}

/// Generic consensus item for forward-compatible item classes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConsensusDefaultItem {
    /// Item format version.
    pub version: u16,
    /// Item kind.
    pub kind: String,
    /// Canonical JSON payload.
    pub payload: serde_json::Value,
}

impl FederationConsensusDefaultItem {
    /// Create a default consensus item.
    #[must_use]
    pub fn new<K>(kind: K, payload: serde_json::Value) -> Self
    where
        K: Into<String>,
    {
        Self {
            version: FEDERATION_CONSENSUS_ITEM_VERSION,
            kind: kind.into(),
            payload,
        }
    }

    /// Reject default items from unsupported consensus-item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        ensure_consensus_item_version(self.version)
    }
}

fn ensure_consensus_item_version(version: u16) -> Result<()> {
    if version != FEDERATION_CONSENSUS_ITEM_VERSION {
        return Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found: version,
        });
    }

    Ok(())
}

fn recovery_barrier_id(
    federation_id: FederationId,
    scheduled_checkpoint_sequence: u64,
    scheduled_anchor_id: Option<FederationCheckpointId>,
    requested_after_finalized_item_index: u64,
) -> Result<FederationRecoveryBarrierId> {
    if (scheduled_checkpoint_sequence == 0) != scheduled_anchor_id.is_none() {
        return Err(FederationError::CheckpointInvalid {
            reason: "recovery barrier scheduled anchor shape is invalid".to_string(),
        });
    }

    let mut transcript = Vec::new();
    append_len_prefixed_bytes(&mut transcript, FEDERATION_RECOVERY_BARRIER_ID_VERSION);
    transcript.extend_from_slice(&FEDERATION_RECOVERY_BARRIER_VERSION.to_be_bytes());
    append_len_prefixed_bytes(&mut transcript, &federation_id.to_bytes());
    transcript.extend_from_slice(&scheduled_checkpoint_sequence.to_be_bytes());
    match scheduled_anchor_id {
        Some(anchor_id) => {
            transcript.push(1);
            append_len_prefixed_bytes(&mut transcript, &anchor_id.to_bytes());
        }
        None => transcript.push(0),
    }
    transcript.extend_from_slice(&requested_after_finalized_item_index.to_be_bytes());
    Ok(FederationRecoveryBarrierId::from_bytes(
        Sha256Hash::hash(&transcript).to_byte_array(),
    ))
}

fn collect_operation_ids(items: &[FederationConsensusItem]) -> Result<Vec<FederationOperationId>> {
    let mut operation_ids = Vec::new();
    for item in items {
        if let Some(operation_id) = item.operation_id()? {
            operation_ids.push(operation_id);
        }
    }

    Ok(operation_ids)
}

fn compute_aleph_bft_unit_body_hash(
    items: &[FederationConsensusItem],
) -> Result<FederationAlephBftUnitBodyHash> {
    let mut transcript = Vec::new();
    append_len_prefixed_bytes(&mut transcript, FEDERATION_ALEPH_BFT_UNIT_BODY_HASH_VERSION);
    transcript.extend_from_slice(
        &u64::try_from(items.len())
            .expect("consensus item count fits in u64")
            .to_be_bytes(),
    );

    for item in items {
        let canonical = item.canonical_bytes()?;
        append_len_prefixed_bytes(&mut transcript, &canonical);
    }

    Ok(FederationAlephBftUnitBodyHash::from_bytes(
        Sha256Hash::hash(&transcript).to_byte_array(),
    ))
}

fn append_len_prefixed_bytes(transcript: &mut Vec<u8>, bytes: &[u8]) {
    transcript.extend_from_slice(
        &u64::try_from(bytes.len())
            .expect("byte slice length fits in u64")
            .to_be_bytes(),
    );
    transcript.extend_from_slice(bytes);
}

/// Admission result for the pending consensus item queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationConsensusItemAdmission {
    /// The item was queued.
    Accepted,
    /// An operation with the same operation ID is already pending.
    DuplicateOperation,
}

/// Bounded pending-item queue for consensus data providers.
#[derive(Debug, Clone)]
pub(crate) struct FederationConsensusItemQueue {
    max_item_bytes: usize,
    pending: VecDeque<FederationConsensusItem>,
    pending_operation_ids: BTreeSet<FederationOperationId>,
    pending_conflict_keys: HashMap<FederationOperationConflictKey, FederationOperationId>,
}

impl Default for FederationConsensusItemQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationConsensusItemQueue {
    /// Create an empty queue with the default item size limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_item_bytes(FEDERATION_MAX_CONSENSUS_ITEM_BYTES)
    }

    /// Create an empty queue with a custom item size limit.
    #[must_use]
    pub fn with_max_item_bytes(max_item_bytes: usize) -> Self {
        Self {
            max_item_bytes,
            pending: VecDeque::new(),
            pending_operation_ids: BTreeSet::new(),
            pending_conflict_keys: HashMap::new(),
        }
    }

    /// Return the number of queued items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Return whether a recovery barrier is waiting for unit construction.
    #[must_use]
    pub fn has_recovery_barrier(&self) -> bool {
        self.pending
            .iter()
            .any(|item| matches!(item, FederationConsensusItem::RecoveryBarrier(_)))
    }

    /// Return true when the queue has no pending items.
    #[must_use]
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Submit an item to the pending queue after size and duplicate checks.
    pub fn submit(
        &mut self,
        item: FederationConsensusItem,
    ) -> Result<FederationConsensusItemAdmission> {
        item.ensure_within_size_limit(self.max_item_bytes)?;
        let metadata = pending_consensus_item_operation_metadata(&item)?;
        if let Some((operation_id, conflict_keys)) = &metadata {
            if self.pending_operation_ids.contains(operation_id) {
                return Ok(FederationConsensusItemAdmission::DuplicateOperation);
            }
            for conflict_key in conflict_keys {
                if let Some(existing_operation_id) = self.pending_conflict_keys.get(conflict_key) {
                    return Err(FederationError::ConflictingOperation {
                        conflict_key: serde_json::to_string(conflict_key)?,
                        existing_operation_id: existing_operation_id.to_string(),
                        attempted_operation_id: operation_id.to_string(),
                    });
                }
            }
            self.pending_operation_ids.insert(*operation_id);
            for conflict_key in conflict_keys {
                self.pending_conflict_keys
                    .insert(conflict_key.clone(), *operation_id);
            }
        }

        self.pending.push_back(item);
        Ok(FederationConsensusItemAdmission::Accepted)
    }

    /// Drain up to `limit` items in FIFO order.
    #[cfg(test)]
    pub fn drain_batch(&mut self, limit: usize) -> Vec<FederationConsensusItem> {
        let mut batch = Vec::new();
        for _ in 0..limit {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            self.release_pending_operation_metadata(&item);
            batch.push(item);
        }
        batch
    }

    /// Drain up to `max_items` FIFO items without exceeding `max_bytes`.
    ///
    /// If the first queued item is larger than `max_bytes`, the queue is left
    /// unchanged and an error is returned so the caller can apply backpressure
    /// or surface the invalid batching limit.
    pub fn drain_batch_until(
        &mut self,
        max_items: usize,
        max_bytes: usize,
    ) -> Result<Vec<FederationConsensusItem>> {
        let mut batch = Vec::new();
        let mut batch_bytes = 0usize;

        for _ in 0..max_items {
            let Some(item) = self.pending.front() else {
                break;
            };
            let item_size = item.canonical_size()?;
            if item_size > max_bytes {
                if batch.is_empty() {
                    return Err(FederationError::ConsensusItemTooLarge {
                        actual: item_size,
                        max: max_bytes,
                    });
                }
                break;
            }
            if !batch.is_empty() && batch_bytes.saturating_add(item_size) > max_bytes {
                break;
            }

            let Some(item) = self.pending.pop_front() else {
                break;
            };
            self.release_pending_operation_metadata(&item);
            batch_bytes = batch_bytes.saturating_add(item_size);
            batch.push(item);
        }

        Ok(batch)
    }

    fn release_pending_operation_metadata(&mut self, item: &FederationConsensusItem) {
        let Ok(Some((operation_id, conflict_keys))) =
            pending_consensus_item_operation_metadata(item)
        else {
            return;
        };
        self.pending_operation_ids.remove(&operation_id);
        for conflict_key in conflict_keys {
            if self.pending_conflict_keys.get(&conflict_key) == Some(&operation_id) {
                self.pending_conflict_keys.remove(&conflict_key);
            }
        }
    }
}

fn pending_consensus_item_operation_metadata(
    item: &FederationConsensusItem,
) -> Result<Option<(FederationOperationId, Vec<FederationOperationConflictKey>)>> {
    match item {
        FederationConsensusItem::Operation(envelope) => {
            Ok(Some(pending_operation_metadata_from_envelope(envelope)?))
        }
        FederationConsensusItem::RecoveryBarrier(_)
        | FederationConsensusItem::ModuleFact(_)
        | FederationConsensusItem::Default(_) => Ok(None),
    }
}

fn pending_operation_metadata_from_envelope(
    envelope: &FederationOperationEnvelope,
) -> Result<(FederationOperationId, Vec<FederationOperationConflictKey>)> {
    let operation_id = envelope.operation_id()?;
    let conflict_keys = envelope.operation.conflict_keys();
    Ok((operation_id, conflict_keys))
}

/// Bounded AlephBFT data provider for local pending CDK consensus items.
#[derive(Debug, Clone)]
pub(crate) struct FederationAlephBftUnitDataProvider {
    federation_id: FederationId,
    submitting_member: MemberId,
    session: u64,
    next_unit_index: u64,
    max_items_per_unit: usize,
    max_unit_body_bytes: usize,
    queue: FederationConsensusItemQueue,
}

impl FederationAlephBftUnitDataProvider {
    /// Create an empty data provider with default batching limits.
    pub fn new(federation_id: FederationId, submitting_member: MemberId, session: u64) -> Self {
        Self::with_limits(
            federation_id,
            submitting_member,
            session,
            FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
            FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
            FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
        )
        .expect("default AlephBFT data-provider limits are non-zero")
    }

    /// Create an empty data provider with explicit batching limits.
    pub fn with_limits(
        federation_id: FederationId,
        submitting_member: MemberId,
        session: u64,
        max_item_bytes: usize,
        max_items_per_unit: usize,
        max_unit_body_bytes: usize,
    ) -> Result<Self> {
        ensure_non_zero_consensus_limit("max_item_bytes", max_item_bytes)?;
        ensure_non_zero_consensus_limit("max_items_per_unit", max_items_per_unit)?;
        ensure_non_zero_consensus_limit("max_unit_body_bytes", max_unit_body_bytes)?;

        Ok(Self {
            federation_id,
            submitting_member,
            session,
            next_unit_index: 0,
            max_items_per_unit,
            max_unit_body_bytes,
            queue: FederationConsensusItemQueue::with_max_item_bytes(max_item_bytes),
        })
    }

    /// Return the session used for newly produced unit payloads.
    #[must_use]
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Return the index that will be used by the next non-empty unit.
    #[must_use]
    pub fn next_unit_index(&self) -> u64 {
        self.next_unit_index
    }

    /// Return the number of queued consensus items.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }

    /// Return whether a recovery barrier is waiting for unit construction.
    #[must_use]
    pub fn has_recovery_barrier(&self) -> bool {
        self.queue.has_recovery_barrier()
    }

    /// Return true when the provider has no queued consensus items.
    #[must_use]
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Begin a new AlephBFT session and reset local unit indexing.
    pub fn begin_session(&mut self, session: u64) {
        self.session = session;
        self.next_unit_index = 0;
    }

    fn advance_next_unit_index_after_finalized_local_unit(
        &mut self,
        payload: &FederationAlephBftUnitPayload,
    ) {
        if payload.session == self.session && payload.submitting_member == self.submitting_member {
            self.next_unit_index = self
                .next_unit_index
                .max(payload.unit_index.saturating_add(1));
        }
    }

    fn with_next_unit_index(mut self, next_unit_index: u64) -> Self {
        self.next_unit_index = next_unit_index;
        self
    }

    /// Queue a consensus item for the next unit payload.
    pub fn submit_item(
        &mut self,
        item: FederationConsensusItem,
    ) -> Result<FederationConsensusItemAdmission> {
        self.queue.submit(item)
    }

    /// Queue an operation envelope for the next unit payload.
    pub fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.submit_item(FederationConsensusItem::operation(envelope))
    }

    /// Return the next non-empty unit payload, or `None` when no item is ready.
    pub fn next_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        let items = self
            .queue
            .drain_batch_until(self.max_items_per_unit, self.max_unit_body_bytes)?;
        if items.is_empty() {
            return Ok(None);
        }

        let payload = FederationAlephBftUnitPayload::new(
            self.federation_id,
            self.submitting_member,
            self.session,
            self.next_unit_index,
            items,
        )?;
        self.next_unit_index = self.next_unit_index.saturating_add(1);
        Ok(Some(payload))
    }
}

/// Converts verified AlephBFT unit payloads into deterministic accepted items.
#[derive(Debug, Clone)]
pub(crate) struct FederationAlephBftFinalizationHandler {
    federation_id: FederationId,
    session: u64,
    next_unit_finalization_index: u64,
    next_item_index: u64,
    max_unit_payload_bytes: usize,
    allowed_members: Option<BTreeSet<MemberId>>,
    operation_config: Option<FederationConfig>,
    finalized_units: BTreeSet<(u64, MemberId, u64)>,
    finalized_operation_ids: BTreeSet<FederationOperationId>,
    next_unit_index_by_member: BTreeMap<MemberId, u64>,
    replay_floor_by_member: BTreeMap<MemberId, u64>,
}

impl FederationAlephBftFinalizationHandler {
    /// Create a finalization handler for session zero with default limits.
    #[cfg(test)]
    pub fn new(federation_id: FederationId) -> Self {
        Self::with_limits(
            federation_id,
            0,
            0,
            FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
        )
        .expect("default AlephBFT finalization limits are non-zero")
    }

    /// Create a finalization handler with explicit session, item index, and limits.
    pub fn with_limits(
        federation_id: FederationId,
        session: u64,
        next_item_index: u64,
        max_unit_payload_bytes: usize,
    ) -> Result<Self> {
        ensure_non_zero_consensus_limit("max_unit_payload_bytes", max_unit_payload_bytes)?;

        Ok(Self {
            federation_id,
            session,
            next_unit_finalization_index: 0,
            next_item_index,
            max_unit_payload_bytes,
            allowed_members: None,
            operation_config: None,
            finalized_units: BTreeSet::new(),
            finalized_operation_ids: BTreeSet::new(),
            next_unit_index_by_member: BTreeMap::new(),
            replay_floor_by_member: BTreeMap::new(),
        })
    }

    /// Create a finalization handler that only accepts units from known members.
    pub fn with_allowed_members<I>(
        federation_id: FederationId,
        session: u64,
        next_item_index: u64,
        max_unit_payload_bytes: usize,
        allowed_members: I,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let mut handler = Self::with_limits(
            federation_id,
            session,
            next_item_index,
            max_unit_payload_bytes,
        )?;
        let allowed_members = allowed_members.into_iter().collect::<BTreeSet<_>>();
        handler.next_unit_index_by_member = allowed_members
            .iter()
            .copied()
            .map(|member_id| (member_id, 0))
            .collect();
        handler.replay_floor_by_member = handler.next_unit_index_by_member.clone();
        handler.allowed_members = Some(allowed_members);
        Ok(handler)
    }

    fn at_checkpoint_base(
        config: &FederationConfig,
        checkpoint: &FederationCheckpoint,
    ) -> Result<Self> {
        checkpoint.validate_against_config(config)?;
        let allowed_members = config.members.keys().copied().collect::<BTreeSet<_>>();
        Ok(Self {
            federation_id: config.federation_id,
            session: checkpoint.aleph_bft_session,
            next_unit_finalization_index: checkpoint.next_unit_finalization_index,
            next_item_index: checkpoint.finalized_next_item_index,
            max_unit_payload_bytes: FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
            allowed_members: Some(allowed_members),
            operation_config: Some(config.clone()),
            finalized_units: BTreeSet::new(),
            finalized_operation_ids: BTreeSet::new(),
            next_unit_index_by_member: checkpoint.next_unit_index_by_member.clone(),
            replay_floor_by_member: checkpoint.next_unit_index_by_member.clone(),
        })
    }

    fn reconstruct_retained_items(
        federation_id: FederationId,
        session: u64,
        max_unit_payload_bytes: usize,
        allowed_members: Option<BTreeSet<MemberId>>,
        operation_config: Option<FederationConfig>,
        checkpoint: Option<&FederationCheckpoint>,
        finalized_items: &[FederationAlephBftFinalizedItem],
    ) -> Result<Self> {
        ensure_non_zero_consensus_limit("max_unit_payload_bytes", max_unit_payload_bytes)?;

        let mut finalized_units = BTreeSet::new();
        let mut finalized_unit_metadata = BTreeMap::new();
        let mut finalized_unit_items = BTreeMap::new();
        let mut finalized_operation_ids = BTreeSet::new();
        let mut last_unit_key = None;
        let mut closed_unit_keys = BTreeSet::new();
        let mut next_item_index_by_unit = BTreeMap::new();
        let mut next_unit_finalization_index_by_session = checkpoint
            .map(|checkpoint| {
                BTreeMap::from([(
                    checkpoint.aleph_bft_session,
                    checkpoint.next_unit_finalization_index,
                )])
            })
            .unwrap_or_default();
        let mut next_unit_finalization_index = checkpoint.map_or(0, |checkpoint| {
            if checkpoint.aleph_bft_session == session {
                checkpoint.next_unit_finalization_index
            } else {
                0
            }
        });
        let base_next_item_index =
            checkpoint.map_or(0, |checkpoint| checkpoint.finalized_next_item_index);
        let mut next_item_index = base_next_item_index;
        let mut last_seen_session = checkpoint.map(|checkpoint| checkpoint.aleph_bft_session);
        let mut next_unit_index_by_member = match checkpoint {
            Some(checkpoint) if checkpoint.aleph_bft_session == session => {
                checkpoint.next_unit_index_by_member.clone()
            }
            _ => allowed_members
                .iter()
                .flatten()
                .copied()
                .map(|member_id| (member_id, 0))
                .collect::<BTreeMap<_, _>>(),
        };
        let replay_floor_by_member = next_unit_index_by_member.clone();

        for (expected_index, finalized_item) in finalized_items.iter().enumerate() {
            let expected_index = base_next_item_index.saturating_add(
                u64::try_from(expected_index).expect("finalized item count fits in u64"),
            );
            if finalized_item.accepted_item_index != expected_index {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: format!(
                        "non-contiguous accepted item index: expected {expected_index}, found {}",
                        finalized_item.accepted_item_index
                    ),
                });
            }
            if finalized_item.session > session {
                return Err(FederationError::AlephBftUnitSessionMismatch {
                    expected: session,
                    found: finalized_item.session,
                });
            }
            if let Some(previous_session) = last_seen_session {
                if finalized_item.session < previous_session {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: format!(
                            "finalized item session moved backwards: previous {previous_session}, found {}",
                            finalized_item.session
                        ),
                    });
                }
            }
            last_seen_session = Some(finalized_item.session);
            if let Some(allowed_members) = &allowed_members {
                if !allowed_members.contains(&finalized_item.unit_submitting_member) {
                    return Err(FederationError::UnknownMember {
                        member_id: finalized_item.unit_submitting_member,
                    });
                }
            }
            finalized_item.item.ensure_supported_version()?;
            finalized_item.item.ensure_federation_id(federation_id)?;

            let computed_operation_id = finalized_item.item.operation_id()?;
            if computed_operation_id != finalized_item.operation_id {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: "finalized item operation id does not match item body".to_string(),
                });
            }

            if let (Some(operation_config), FederationConsensusItem::Operation(envelope)) =
                (&operation_config, &finalized_item.item)
            {
                let authorization_valid = envelope.verify_authorization(operation_config).is_ok();
                let marked_rejected = finalized_item.status
                    == FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation;
                if authorization_valid == marked_rejected {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "finalized operation authentication does not match its status"
                            .to_string(),
                    });
                }
            }
            if let FederationConsensusItem::RecoveryBarrier(barrier) = &finalized_item.item {
                let barrier_valid = barrier.requested_after_finalized_item_index
                    <= finalized_item.accepted_item_index;
                let marked_rejected = finalized_item.status
                    == FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier;
                if barrier_valid == marked_rejected {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "recovery barrier cursor does not match its status".to_string(),
                    });
                }
            }

            match (finalized_item.status, finalized_item.operation_id) {
                (FederationAlephBftFinalizedItemStatus::Accepted, Some(operation_id)) => {
                    if !finalized_operation_ids.insert(operation_id) {
                        return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                            reason: "accepted finalized item repeats an operation id".to_string(),
                        });
                    }
                }
                (FederationAlephBftFinalizedItemStatus::DuplicateOperation, Some(operation_id)) => {
                    if finalized_operation_ids.insert(operation_id) && checkpoint.is_none() {
                        return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                            reason: "duplicate operation finalized before its accepted item"
                                .to_string(),
                        });
                    }
                }
                (FederationAlephBftFinalizedItemStatus::DuplicateOperation, None) => {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "non-operation finalized item cannot be a duplicate operation"
                            .to_string(),
                    });
                }
                (FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation, Some(_)) => {}
                (FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation, None) => {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "non-operation finalized item cannot be an invalid operation"
                            .to_string(),
                    });
                }
                (FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier, None) => {}
                (
                    FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier,
                    Some(_),
                ) => {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "operation item cannot be an invalid recovery barrier".to_string(),
                    });
                }
                (FederationAlephBftFinalizedItemStatus::Accepted, None) => {}
            }

            let unit_key = (
                finalized_item.session,
                finalized_item.unit_submitting_member,
                finalized_item.unit_index,
            );
            if checkpoint.is_some_and(|checkpoint| {
                finalized_item.session == checkpoint.aleph_bft_session
                    && checkpoint
                        .next_unit_index_by_member
                        .get(&finalized_item.unit_submitting_member)
                        .is_some_and(|floor| finalized_item.unit_index < *floor)
            }) {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: "finalized suffix replays a pre-checkpoint unit".to_string(),
                });
            }
            if last_unit_key != Some(unit_key) {
                if let Some(previous_unit_key) = last_unit_key {
                    closed_unit_keys.insert(previous_unit_key);
                }
                if closed_unit_keys.contains(&unit_key) {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "finalized unit items are not contiguous in accepted item order"
                            .to_string(),
                    });
                }
                last_unit_key = Some(unit_key);
            }

            let expected_item_index = next_item_index_by_unit.entry(unit_key).or_insert(0);
            if finalized_item.item_index_in_unit != *expected_item_index {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: format!(
                        "non-contiguous item index in finalized unit accepted order: expected {}, found {}",
                        *expected_item_index, finalized_item.item_index_in_unit
                    ),
                });
            }
            *expected_item_index = (*expected_item_index).saturating_add(1);

            let is_new_unit = !finalized_unit_metadata.contains_key(&unit_key);
            if is_new_unit {
                let expected_unit_finalization_index = next_unit_finalization_index_by_session
                    .entry(finalized_item.session)
                    .or_insert(0);
                if finalized_item.unit_finalization_index != *expected_unit_finalization_index {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: format!(
                            "non-contiguous unit finalization index: expected {}, found {}",
                            *expected_unit_finalization_index,
                            finalized_item.unit_finalization_index
                        ),
                    });
                }
                *expected_unit_finalization_index =
                    (*expected_unit_finalization_index).saturating_add(1);
            }

            let unit_metadata = (
                finalized_item.unit_finalization_index,
                finalized_item.unit_body_hash,
            );
            match finalized_unit_metadata.get(&unit_key) {
                Some(existing) if *existing != unit_metadata => {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "finalized unit metadata changed within retained items".to_string(),
                    });
                }
                Some(_) => {}
                None => {
                    finalized_unit_metadata.insert(unit_key, unit_metadata);
                    finalized_units.insert(unit_key);
                }
            }
            finalized_unit_items
                .entry(unit_key)
                .or_insert_with(Vec::new)
                .push((
                    finalized_item.item_index_in_unit,
                    finalized_item.item.clone(),
                ));

            if finalized_item.session == session {
                next_unit_finalization_index = next_unit_finalization_index
                    .max(finalized_item.unit_finalization_index.saturating_add(1));
                let next_unit_index = finalized_item.unit_index.saturating_add(1);
                next_unit_index_by_member
                    .entry(finalized_item.unit_submitting_member)
                    .and_modify(|current| *current = (*current).max(next_unit_index))
                    .or_insert(next_unit_index);
            }
            next_item_index = finalized_item.accepted_item_index.saturating_add(1);
        }

        for (unit_key, mut indexed_items) in finalized_unit_items {
            indexed_items.sort_by_key(|(item_index, _)| *item_index);
            for (expected_index, (item_index, _)) in indexed_items.iter().enumerate() {
                let expected_index =
                    u64::try_from(expected_index).expect("unit item count fits in u64");
                if *item_index != expected_index {
                    return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: format!(
                            "non-contiguous item index in finalized unit: expected {expected_index}, found {item_index}"
                        ),
                    });
                }
            }
            let items = indexed_items
                .into_iter()
                .map(|(_, item)| item)
                .collect::<Vec<_>>();
            let expected_body_hash = compute_aleph_bft_unit_body_hash(&items)?;
            let Some((_, found_body_hash)) = finalized_unit_metadata.get(&unit_key) else {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: "missing finalized unit metadata for retained items".to_string(),
                });
            };
            if *found_body_hash != expected_body_hash {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: "finalized unit body hash does not match retained item body"
                        .to_string(),
                });
            }
        }

        if last_seen_session.is_some_and(|last_seen| last_seen < session) {
            next_unit_finalization_index = 0;
            for next_unit_index in next_unit_index_by_member.values_mut() {
                *next_unit_index = 0;
            }
        }

        Ok(Self {
            federation_id,
            session,
            next_unit_finalization_index,
            next_item_index,
            max_unit_payload_bytes,
            allowed_members,
            operation_config,
            finalized_units,
            finalized_operation_ids,
            replay_floor_by_member,
            next_unit_index_by_member,
        })
    }

    /// Return the session accepted by this handler.
    #[must_use]
    #[cfg(test)]
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Return the next unit finalization index within the current session.
    #[must_use]
    pub fn next_unit_finalization_index(&self) -> u64 {
        self.next_unit_finalization_index
    }

    /// Return the next unit index expected from each member in the active session.
    #[must_use]
    pub fn next_unit_index_by_member(&self) -> &BTreeMap<MemberId, u64> {
        &self.next_unit_index_by_member
    }

    /// Return the next global accepted item index.
    #[must_use]
    pub fn next_item_index(&self) -> u64 {
        self.next_item_index
    }

    fn register_finalized_operation_id(&mut self, operation_id: FederationOperationId) {
        self.finalized_operation_ids.insert(operation_id);
    }

    /// Return the count of allowed unit submitters when this handler is roster-bound.
    #[must_use]
    pub fn allowed_member_count(&self) -> Option<u64> {
        self.allowed_members
            .as_ref()
            .map(|members| u64::try_from(members.len()).expect("member count fits in u64"))
    }

    /// Begin a later session and reset the per-session unit finalization index.
    pub fn begin_session(&mut self, session: u64) -> Result<()> {
        if session <= self.session {
            return Err(FederationError::AlephBftUnitSessionMismatch {
                expected: self.session.saturating_add(1),
                found: session,
            });
        }

        self.session = session;
        self.next_unit_finalization_index = 0;
        for next_unit_index in self.next_unit_index_by_member.values_mut() {
            *next_unit_index = 0;
        }
        for replay_floor in self.replay_floor_by_member.values_mut() {
            *replay_floor = 0;
        }
        Ok(())
    }

    /// Convert one finalized AlephBFT unit into accepted consensus items.
    pub fn finalize_next_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<FederationAlephBftFinalizedUnitOutcome> {
        payload.ensure_within_size_limit(self.max_unit_payload_bytes)?;
        if payload.federation_id != self.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: self.federation_id,
                found: payload.federation_id,
            });
        }
        if payload.session != self.session {
            return Err(FederationError::AlephBftUnitSessionMismatch {
                expected: self.session,
                found: payload.session,
            });
        }
        if let Some(allowed_members) = &self.allowed_members {
            if !allowed_members.contains(&payload.submitting_member) {
                return Err(FederationError::UnknownMember {
                    member_id: payload.submitting_member,
                });
            }
        }
        if self
            .replay_floor_by_member
            .get(&payload.submitting_member)
            .is_some_and(|floor| payload.unit_index < *floor)
        {
            return Err(FederationError::DuplicateAlephBftUnit {
                session: payload.session,
                member_id: payload.submitting_member,
                unit_index: payload.unit_index,
            });
        }
        if payload.items.is_empty() {
            return Err(FederationError::EmptyAlephBftUnit);
        }

        let unit_key = (
            payload.session,
            payload.submitting_member,
            payload.unit_index,
        );
        if !self.finalized_units.insert(unit_key) {
            return Err(FederationError::DuplicateAlephBftUnit {
                session: payload.session,
                member_id: payload.submitting_member,
                unit_index: payload.unit_index,
            });
        }

        let unit_finalization_index = self.next_unit_finalization_index;
        let unit_submitting_member = payload.submitting_member;
        let unit_index = payload.unit_index;
        let unit_body_hash = payload.body_hash;
        let mut items = Vec::with_capacity(payload.items.len());

        for (item_index_in_unit, item) in payload.items.into_iter().enumerate() {
            let operation_id = item.operation_id()?;
            let status = match (&item, operation_id) {
                (FederationConsensusItem::RecoveryBarrier(barrier), None)
                    if barrier.requested_after_finalized_item_index > self.next_item_index =>
                {
                    FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier
                }
                (FederationConsensusItem::Operation(envelope), Some(_))
                    if self
                        .operation_config
                        .as_ref()
                        .is_some_and(|config| envelope.verify_authorization(config).is_err()) =>
                {
                    FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
                }
                (_, Some(operation_id)) => {
                    if self.finalized_operation_ids.insert(operation_id) {
                        FederationAlephBftFinalizedItemStatus::Accepted
                    } else {
                        FederationAlephBftFinalizedItemStatus::DuplicateOperation
                    }
                }
                (_, None) => FederationAlephBftFinalizedItemStatus::Accepted,
            };
            items.push(FederationAlephBftFinalizedItem {
                accepted_item_index: self.next_item_index,
                session: self.session,
                unit_finalization_index,
                unit_submitting_member,
                unit_index,
                item_index_in_unit: u64::try_from(item_index_in_unit)
                    .expect("item index fits in u64"),
                operation_id,
                status,
                unit_body_hash,
                item,
            });
            self.next_item_index = self.next_item_index.saturating_add(1);
        }
        self.next_unit_finalization_index = self.next_unit_finalization_index.saturating_add(1);
        let next_unit_index = unit_index.saturating_add(1);
        self.next_unit_index_by_member
            .entry(unit_submitting_member)
            .and_modify(|current| *current = (*current).max(next_unit_index))
            .or_insert(next_unit_index);

        Ok(FederationAlephBftFinalizedUnitOutcome {
            session: self.session,
            unit_finalization_index,
            unit_submitting_member,
            unit_index,
            unit_body_hash,
            items,
        })
    }
}

/// Authenticated root from which a verified consensus suffix is reconstructed.
#[derive(Debug, Clone, Copy)]
pub enum FederationConsensusRecoveryBase<'a> {
    /// Recover from empty federation history.
    Genesis,
    /// Recover after a quorum-authenticated checkpoint.
    Checkpoint(&'a VerifiedFederationCheckpointQuorum),
}

/// AlephBFT consensus runtime for operation submission and finalization.
///
/// This does not run AlephBFT networking. It owns the CDK side of the boundary:
/// wallet/public handlers can enqueue operations, an AlephBFT task can pull
/// canonical unit payloads from the data provider, and finalized units are
/// converted back into deterministic journal responses.
#[derive(Debug, Clone)]
pub struct FederationConsensusRuntime {
    data_provider: FederationAlephBftUnitDataProvider,
    finalization_handler: FederationAlephBftFinalizationHandler,
    journal: InMemoryFederationJournal,
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
    pending_finalization_units:
        BTreeMap<FederationAlephBftPendingUnitKey, Vec<FederationConsensusItem>>,
    pending_finalization_operation_ids: BTreeSet<FederationOperationId>,
    pending_finalization_conflict_keys:
        HashMap<FederationOperationConflictKey, FederationOperationId>,
}

type FederationAlephBftPendingUnitKey = (u64, MemberId, u64, FederationAlephBftUnitBodyHash);

impl FederationConsensusRuntime {
    /// Create an AlephBFT runtime with default limits.
    ///
    /// # Panics
    ///
    /// Panics only if the compile-time default finalization limits are zero.
    #[must_use]
    pub fn new(federation_id: FederationId, local_member: MemberId, session: u64) -> Self {
        Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                federation_id,
                local_member,
                session,
            ),
            finalization_handler: FederationAlephBftFinalizationHandler::with_limits(
                federation_id,
                session,
                0,
                FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
            )
            .expect("default AlephBFT finalization limits are non-zero"),
            journal: InMemoryFederationJournal::new(),
            finalized_items: Vec::new(),
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        }
    }

    /// Create a runtime that only accepts finalized units from known members.
    pub fn for_members<I>(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        allowed_members: I,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                federation_id,
                local_member,
                session,
            ),
            finalization_handler: FederationAlephBftFinalizationHandler::with_allowed_members(
                federation_id,
                session,
                0,
                FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                allowed_members,
            )?,
            journal: InMemoryFederationJournal::new(),
            finalized_items: Vec::new(),
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        })
    }

    /// Create an AlephBFT runtime with explicit limits.
    pub fn with_limits(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        max_item_bytes: usize,
        max_items_per_unit: usize,
        max_unit_body_bytes: usize,
        max_unit_payload_bytes: usize,
    ) -> Result<Self> {
        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::with_limits(
                federation_id,
                local_member,
                session,
                max_item_bytes,
                max_items_per_unit,
                max_unit_body_bytes,
            )?,
            finalization_handler: FederationAlephBftFinalizationHandler::with_limits(
                federation_id,
                session,
                0,
                max_unit_payload_bytes,
            )?,
            journal: InMemoryFederationJournal::new(),
            finalized_items: Vec::new(),
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        })
    }

    /// Raw-history reconstruction used only by crate-local fault-injection tests.
    #[doc(hidden)]
    pub fn recover_raw_for_test(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        allowed_members: Option<BTreeSet<MemberId>>,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        Self::reconstruct_raw_test_history(
            federation_id,
            local_member,
            session,
            allowed_members,
            None,
            finalized_items,
        )
    }

    fn reconstruct_authenticated_test_history(
        public_config: FederationConfig,
        local_member: MemberId,
        session: u64,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        public_config.validate()?;
        let allowed_members = public_config.members.keys().copied().collect();
        Self::reconstruct_raw_test_history(
            public_config.federation_id,
            local_member,
            session,
            Some(allowed_members),
            Some(public_config),
            finalized_items,
        )
    }

    fn from_verified_checkpoint_base(
        public_config: FederationConfig,
        local_member: MemberId,
        checkpoint: &FederationCheckpoint,
    ) -> Result<Self> {
        checkpoint.validate_against_config(&public_config)?;
        let next_local_unit_index = checkpoint
            .next_unit_index_by_member
            .get(&local_member)
            .copied()
            .ok_or(FederationError::UnknownMember {
                member_id: local_member,
            })?;
        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                public_config.federation_id,
                local_member,
                checkpoint.aleph_bft_session,
            )
            .with_next_unit_index(next_local_unit_index),
            finalization_handler: FederationAlephBftFinalizationHandler::at_checkpoint_base(
                &public_config,
                checkpoint,
            )?,
            journal: InMemoryFederationJournal::from_base(
                checkpoint.journal_next_index,
                checkpoint.journal_order_digest,
            ),
            finalized_items: Vec::new(),
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn recover_checkpoint_suffix_for_test_inner(
        public_config: FederationConfig,
        local_member: MemberId,
        session: u64,
        checkpoint: &FederationCheckpoint,
        journal_entries: Vec<FederationJournalEntry>,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        checkpoint.validate_against_config(&public_config)?;
        let allowed_members = public_config
            .members
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let finalization_handler =
            FederationAlephBftFinalizationHandler::reconstruct_retained_items(
                public_config.federation_id,
                session,
                FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                Some(allowed_members),
                Some(public_config.clone()),
                Some(checkpoint),
                &finalized_items,
            )?;
        let next_local_unit_index = finalization_handler
            .next_unit_index_by_member()
            .get(&local_member)
            .copied()
            .ok_or(FederationError::UnknownMember {
                member_id: local_member,
            })?;
        let journal = InMemoryFederationJournal::from_base_and_entries(
            checkpoint.journal_next_index,
            checkpoint.journal_order_digest,
            journal_entries,
        )?;
        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                public_config.federation_id,
                local_member,
                session,
            )
            .with_next_unit_index(next_local_unit_index),
            finalization_handler,
            journal,
            finalized_items,
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        })
    }

    fn reconstruct_raw_test_history(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        allowed_members: Option<BTreeSet<MemberId>>,
        operation_config: Option<FederationConfig>,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        let finalization_handler =
            FederationAlephBftFinalizationHandler::reconstruct_retained_items(
                federation_id,
                session,
                FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                allowed_members,
                operation_config,
                None,
                &finalized_items,
            )?;
        let mut journal = InMemoryFederationJournal::new();
        for finalized_item in &finalized_items {
            if finalized_item.status
                != FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
            {
                if let FederationConsensusItem::Operation(envelope) = &finalized_item.item {
                    journal.append_finalized_with_consensus_evidence(
                        envelope.clone(),
                        finalized_item.consensus_evidence(),
                    )?;
                }
            }
        }

        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                federation_id,
                local_member,
                session,
            )
            .with_next_unit_index(next_local_unit_index_after_recovery(
                local_member,
                session,
                &finalized_items,
            )),
            finalization_handler,
            journal,
            finalized_items,
            pending_finalization_units: BTreeMap::new(),
            pending_finalization_operation_ids: BTreeSet::new(),
            pending_finalization_conflict_keys: HashMap::new(),
        })
    }

    /// Return the current AlephBFT session.
    #[must_use]
    pub fn session(&self) -> u64 {
        self.data_provider.session()
    }

    /// Return the number of locally queued consensus items.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.data_provider.queued_len()
    }

    /// Return the number of drained local consensus items awaiting finalization.
    #[must_use]
    pub fn pending_finalization_len(&self) -> usize {
        self.pending_finalization_units.values().map(Vec::len).sum()
    }

    /// Return whether a recovery barrier is queued or awaiting finality.
    #[must_use]
    pub fn has_pending_recovery_barrier(&self) -> bool {
        self.data_provider.has_recovery_barrier()
            || self.pending_finalization_units.values().any(|items| {
                items
                    .iter()
                    .any(|item| matches!(item, FederationConsensusItem::RecoveryBarrier(_)))
            })
    }

    /// Return the next unit index that the local data provider will use.
    #[must_use]
    pub fn next_unit_index(&self) -> u64 {
        self.data_provider.next_unit_index()
    }

    /// Return the next global finalized item index.
    #[must_use]
    pub fn next_item_index(&self) -> u64 {
        self.finalization_handler.next_item_index()
    }

    /// Return the next unit index expected from each configured member.
    #[must_use]
    pub fn next_unit_index_by_member(&self) -> &BTreeMap<MemberId, u64> {
        self.finalization_handler.next_unit_index_by_member()
    }

    /// Return accepted-operation journal state produced from finalized units.
    #[must_use]
    pub fn journal(&self) -> &InMemoryFederationJournal {
        &self.journal
    }

    /// Register one durable pre-checkpoint operation for live idempotency and conflict checks.
    pub fn register_historical_journal_entry(
        &mut self,
        entry: FederationJournalEntry,
    ) -> Result<()> {
        let operation_id = entry.operation_id;
        self.journal.register_historical_entry(entry)?;
        self.finalization_handler
            .register_finalized_operation_id(operation_id);
        Ok(())
    }

    /// Return finalized consensus items in local finalization order.
    #[must_use]
    pub fn finalized_items(&self) -> &[FederationAlephBftFinalizedItem] {
        &self.finalized_items
    }

    /// Return an operator-facing status snapshot for this runtime.
    ///
    /// # Panics
    ///
    /// Panics if an in-memory item count cannot be represented as `u64`.
    #[must_use]
    pub fn status(&self) -> FederationConsensusRuntimeStatus {
        FederationConsensusRuntimeStatus {
            federation_id: self.data_provider.federation_id,
            local_member: self.data_provider.submitting_member,
            session: self.session(),
            next_unit_index: self.next_unit_index(),
            next_unit_finalization_index: self.finalization_handler.next_unit_finalization_index(),
            next_item_index: self.next_item_index(),
            queued_item_count: u64::try_from(self.queued_len()).expect("queue length fits in u64"),
            pending_finalization_item_count: u64::try_from(self.pending_finalization_len())
                .expect("pending finalization item count fits in u64"),
            finalized_item_count: self.next_item_index(),
            accepted_operation_count: self.journal.operation_count(),
            latest_accepted_operation_index: self.journal.next_index().checked_sub(1),
            allowed_member_count: self.finalization_handler.allowed_member_count(),
        }
    }

    /// Return this member's current peer status from finalized operations.
    #[must_use]
    pub fn peer_status_for(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
    ) -> FederationPeerStatus {
        FederationPeerStatus::from_journal(federation_id, member_id, &self.journal)
            .with_aleph_bft_next_item_index(self.next_item_index())
            .with_aleph_bft_session(self.session())
    }

    /// Begin a later AlephBFT session for both data-provider and finalizer.
    pub fn begin_session(&mut self, session: u64) -> Result<()> {
        if !self.pending_finalization_units.is_empty() {
            return Err(FederationError::PendingAlephBftFinalization {
                current_session: self.session(),
                requested_session: session,
                pending_units: self.pending_finalization_units.len(),
                pending_items: self.pending_finalization_len(),
            });
        }

        self.finalization_handler.begin_session(session)?;
        self.data_provider.begin_session(session);
        Ok(())
    }

    /// Begin a later AlephBFT session and requeue local items from units that
    /// were dispatched but not finalized before the previous session ended.
    pub fn begin_session_requeue_pending_finalization(&mut self, session: u64) -> Result<usize> {
        let pending_items = self
            .pending_finalization_units
            .values()
            .flat_map(|items| items.iter().cloned())
            .collect::<Vec<_>>();

        self.finalization_handler.begin_session(session)?;
        self.data_provider.begin_session(session);
        self.pending_finalization_units.clear();
        self.pending_finalization_operation_ids.clear();
        self.pending_finalization_conflict_keys.clear();

        let mut requeued = 0usize;
        for item in pending_items {
            if self.data_provider.submit_item(item)? == FederationConsensusItemAdmission::Accepted {
                requeued = requeued.saturating_add(1);
            }
        }

        Ok(requeued)
    }

    /// Queue one operation envelope for AlephBFT unit construction.
    pub fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        let (operation_id, conflict_keys) = pending_operation_metadata_from_envelope(&envelope)?;
        if self
            .pending_finalization_operation_ids
            .contains(&operation_id)
        {
            return Ok(FederationConsensusItemAdmission::DuplicateOperation);
        }
        for conflict_key in &conflict_keys {
            if let Some(existing_operation_id) =
                self.pending_finalization_conflict_keys.get(conflict_key)
            {
                return Err(FederationError::ConflictingOperation {
                    conflict_key: serde_json::to_string(conflict_key)?,
                    existing_operation_id: existing_operation_id.to_string(),
                    attempted_operation_id: operation_id.to_string(),
                });
            }
        }

        self.data_provider.submit_operation(envelope)
    }

    /// Queue one recovery barrier when none is already queued or awaiting finality.
    pub fn submit_recovery_barrier(&mut self, barrier: FederationRecoveryBarrier) -> Result<bool> {
        barrier.ensure_supported_version()?;
        if barrier.federation_id != self.data_provider.federation_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery barrier federation does not match consensus runtime".to_string(),
            });
        }
        if self.has_pending_recovery_barrier() {
            return Ok(false);
        }

        let admission = self
            .data_provider
            .submit_item(FederationConsensusItem::recovery_barrier(barrier))?;
        Ok(admission == FederationConsensusItemAdmission::Accepted)
    }

    /// Pull the next local unit payload for the AlephBFT data provider.
    pub fn next_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        let Some(payload) = self.data_provider.next_unit_payload()? else {
            return Ok(None);
        };
        self.remember_pending_finalization_unit(&payload)?;
        Ok(Some(payload))
    }

    /// Apply one finalized AlephBFT unit to the local accepted-operation journal.
    pub fn finalize_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        let pending_payload = payload.clone();
        let outcome = self.finalization_handler.finalize_next_unit(payload)?;
        self.data_provider
            .advance_next_unit_index_after_finalized_local_unit(&pending_payload);
        let mut responses = Vec::new();

        for finalized_item in outcome.items {
            if finalized_item.status
                != FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
            {
                if let FederationConsensusItem::Operation(envelope) = &finalized_item.item {
                    let response = FederationSubmitOperationResponse::from_outcome(
                        self.journal.append_finalized_with_consensus_evidence(
                            envelope.clone(),
                            finalized_item.consensus_evidence(),
                        )?,
                    );
                    responses.push(response);
                }
            }
            self.finalized_items.push(finalized_item);
        }
        self.release_pending_finalization_unit(&pending_payload);
        self.release_pending_finalization_decisions(&pending_payload.items);

        Ok(responses)
    }

    fn remember_pending_finalization_unit(
        &mut self,
        payload: &FederationAlephBftUnitPayload,
    ) -> Result<()> {
        let mut operation_ids = BTreeSet::new();
        let mut conflict_keys = HashMap::new();
        for item in &payload.items {
            let Some((operation_id, item_conflict_keys)) =
                pending_consensus_item_operation_metadata(item)?
            else {
                continue;
            };
            operation_ids.insert(operation_id);
            for conflict_key in item_conflict_keys {
                if let Some(existing_operation_id) = self
                    .pending_finalization_conflict_keys
                    .get(&conflict_key)
                    .or_else(|| conflict_keys.get(&conflict_key))
                {
                    return Err(FederationError::ConflictingOperation {
                        conflict_key: serde_json::to_string(&conflict_key)?,
                        existing_operation_id: existing_operation_id.to_string(),
                        attempted_operation_id: operation_id.to_string(),
                    });
                }
                conflict_keys.insert(conflict_key, operation_id);
            }
        }

        self.pending_finalization_units
            .insert(aleph_bft_pending_unit_key(payload), payload.items.clone());
        for operation_id in operation_ids {
            self.pending_finalization_operation_ids.insert(operation_id);
        }
        for (conflict_key, operation_id) in conflict_keys {
            self.pending_finalization_conflict_keys
                .insert(conflict_key, operation_id);
        }

        Ok(())
    }

    fn release_pending_finalization_unit(&mut self, payload: &FederationAlephBftUnitPayload) {
        let Some(items) = self
            .pending_finalization_units
            .remove(&aleph_bft_pending_unit_key(payload))
        else {
            return;
        };
        for item in &items {
            self.release_pending_finalization_item_metadata(item);
        }
    }

    fn release_pending_finalization_decisions(&mut self, items: &[FederationConsensusItem]) {
        let mut operation_ids = BTreeSet::new();
        let mut conflict_keys = HashSet::new();
        for item in items {
            let Ok(Some((operation_id, item_conflict_keys))) =
                pending_consensus_item_operation_metadata(item)
            else {
                continue;
            };
            operation_ids.insert(operation_id);
            conflict_keys.extend(item_conflict_keys);
        }
        if operation_ids.is_empty() && conflict_keys.is_empty() {
            return;
        }

        let pending_unit_keys = self
            .pending_finalization_units
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for unit_key in pending_unit_keys {
            let Some(unit_items) = self.pending_finalization_units.remove(&unit_key) else {
                continue;
            };
            let mut retained_items = Vec::new();
            for item in unit_items {
                if pending_finalization_item_matches_decision(&item, &operation_ids, &conflict_keys)
                {
                    self.release_pending_finalization_item_metadata(&item);
                } else {
                    retained_items.push(item);
                }
            }
            if !retained_items.is_empty() {
                self.pending_finalization_units
                    .insert(unit_key, retained_items);
            }
        }
    }

    fn release_pending_finalization_item_metadata(&mut self, item: &FederationConsensusItem) {
        let Ok(Some((operation_id, conflict_keys))) =
            pending_consensus_item_operation_metadata(item)
        else {
            return;
        };
        self.pending_finalization_operation_ids
            .remove(&operation_id);
        for conflict_key in conflict_keys {
            if self.pending_finalization_conflict_keys.get(&conflict_key) == Some(&operation_id) {
                self.pending_finalization_conflict_keys
                    .remove(&conflict_key);
            }
        }
    }
}

fn next_local_unit_index_after_recovery(
    local_member: MemberId,
    session: u64,
    finalized_items: &[FederationAlephBftFinalizedItem],
) -> u64 {
    finalized_items
        .iter()
        .filter(|item| item.session == session && item.unit_submitting_member == local_member)
        .map(|item| item.unit_index.saturating_add(1))
        .max()
        .unwrap_or(0)
}

fn pending_finalization_item_matches_decision(
    item: &FederationConsensusItem,
    operation_ids: &BTreeSet<FederationOperationId>,
    conflict_keys: &HashSet<FederationOperationConflictKey>,
) -> bool {
    let Ok(Some((operation_id, item_conflict_keys))) =
        pending_consensus_item_operation_metadata(item)
    else {
        return false;
    };

    operation_ids.contains(&operation_id)
        || item_conflict_keys
            .iter()
            .any(|conflict_key| conflict_keys.contains(conflict_key))
}

fn aleph_bft_pending_unit_key(
    payload: &FederationAlephBftUnitPayload,
) -> FederationAlephBftPendingUnitKey {
    (
        payload.session,
        payload.submitting_member,
        payload.unit_index,
        payload.body_hash,
    )
}

fn ensure_non_zero_consensus_limit(field: &'static str, value: usize) -> Result<()> {
    if value == 0 {
        return Err(FederationError::InvalidConsensusLimit { field, value: 0 });
    }

    Ok(())
}

impl FederationConsensusRuntime {
    /// Create an authenticated live runtime at empty history.
    pub fn authenticated_aleph_bft(
        public_config: &FederationConfig,
        member_id: MemberId,
        session: u64,
    ) -> Result<Self> {
        Self::from_recovery_base(
            public_config,
            member_id,
            session,
            FederationConsensusRecoveryBase::Genesis,
        )
    }

    pub(crate) fn from_recovery_base(
        public_config: &FederationConfig,
        member_id: MemberId,
        active_session: u64,
        base: FederationConsensusRecoveryBase<'_>,
    ) -> Result<Self> {
        public_config.validate()?;
        let allowed_members = validate_runtime_roster(
            public_config.members.keys().copied(),
            public_config.threshold_params.consensus_threshold,
            member_id,
        )?;
        match base {
            FederationConsensusRecoveryBase::Genesis => {
                let mut finalization_handler =
                    FederationAlephBftFinalizationHandler::with_allowed_members(
                        public_config.federation_id,
                        active_session,
                        0,
                        FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                        allowed_members,
                    )?;
                finalization_handler.operation_config = Some(public_config.clone());
                Ok(Self {
                    data_provider: FederationAlephBftUnitDataProvider::new(
                        public_config.federation_id,
                        member_id,
                        active_session,
                    ),
                    finalization_handler,
                    journal: InMemoryFederationJournal::new(),
                    finalized_items: Vec::new(),
                    pending_finalization_units: BTreeMap::new(),
                    pending_finalization_operation_ids: BTreeSet::new(),
                    pending_finalization_conflict_keys: HashMap::new(),
                })
            }
            FederationConsensusRecoveryBase::Checkpoint(checkpoint) => {
                let checkpoint = checkpoint.checkpoint();
                if active_session < checkpoint.aleph_bft_session {
                    return Err(FederationError::AlephBftUnitSessionMismatch {
                        expected: checkpoint.aleph_bft_session,
                        found: active_session,
                    });
                }
                Self::from_verified_checkpoint_base(public_config.clone(), member_id, checkpoint)
            }
        }
    }

    /// Recover a runtime from an exact suffix proof returned by the shared verifier.
    #[must_use]
    pub fn recover_verified_suffix(verified_suffix: VerifiedFederationSuffix) -> Self {
        verified_suffix.into_runtime()
    }

    /// Construct an unverified checkpoint base for cross-crate test fixtures.
    ///
    /// Production recovery must use [`Self::recover_verified_suffix`].
    #[doc(hidden)]
    pub fn checkpoint_base_for_test(
        public_config: &FederationConfig,
        member_id: MemberId,
        checkpoint: &FederationCheckpoint,
    ) -> Result<Self> {
        validate_runtime_roster(
            public_config.members.keys().copied(),
            public_config.threshold_params.consensus_threshold,
            member_id,
        )?;
        Self::from_verified_checkpoint_base(public_config.clone(), member_id, checkpoint)
    }

    /// Reconstruct an unverified checkpoint suffix for crate-local tests.
    #[cfg(test)]
    pub(crate) fn recover_checkpoint_suffix_for_test(
        public_config: &FederationConfig,
        member_id: MemberId,
        session: u64,
        checkpoint: &FederationCheckpoint,
        journal_entries: Vec<FederationJournalEntry>,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        validate_runtime_roster(
            public_config.members.keys().copied(),
            public_config.threshold_params.consensus_threshold,
            member_id,
        )?;
        Self::recover_checkpoint_suffix_for_test_inner(
            public_config.clone(),
            member_id,
            session,
            checkpoint,
            journal_entries,
            finalized_items,
        )
    }

    /// Reconstruct raw genesis history for cross-crate test fixtures.
    ///
    /// Production recovery must use [`Self::recover_verified_suffix`].
    #[doc(hidden)]
    pub fn recover_finalized_items_for_test(
        public_config: &FederationConfig,
        member_id: MemberId,
        session: u64,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        validate_runtime_roster(
            public_config.members.keys().copied(),
            public_config.threshold_params.consensus_threshold,
            member_id,
        )?;
        Self::reconstruct_authenticated_test_history(
            public_config.clone(),
            member_id,
            session,
            finalized_items,
        )
    }

    /// Return queued plus dispatched consensus items awaiting finalization.
    #[must_use]
    pub fn pending_mempool_len(&self) -> usize {
        self.queued_len()
            .saturating_add(self.pending_finalization_len())
    }

    /// Begin a later AlephBFT session for this runtime.
    pub fn begin_aleph_bft_session(&mut self, session: u64) -> Result<()> {
        self.begin_session(session)
    }

    /// Begin a later AlephBFT session, requeuing local units that were not
    /// finalized before the previous session ended.
    pub fn begin_aleph_bft_session_requeue_pending_finalization(
        &mut self,
        session: u64,
    ) -> Result<usize> {
        self.begin_session_requeue_pending_finalization(session)
    }

    /// Queue one operation for AlephBFT unit construction.
    pub fn submit_aleph_bft_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.submit_operation(envelope)
    }

    /// Queue one bounded recovery barrier for AlephBFT ordering.
    pub fn submit_aleph_bft_recovery_barrier(
        &mut self,
        barrier: FederationRecoveryBarrier,
    ) -> Result<bool> {
        self.submit_recovery_barrier(barrier)
    }

    /// Return whether a recovery barrier is queued or awaiting finality.
    #[must_use]
    pub fn has_pending_aleph_bft_recovery_barrier(&self) -> bool {
        self.has_pending_recovery_barrier()
    }

    /// Pull the next local AlephBFT unit payload from the runtime.
    pub fn next_aleph_bft_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        self.next_unit_payload()
    }

    /// Apply one externally finalized AlephBFT unit payload.
    pub fn finalize_aleph_bft_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.finalize_unit(payload)
    }

    /// Return finalized AlephBFT items held by this runtime.
    pub fn aleph_bft_finalized_items(&self) -> Result<&[FederationAlephBftFinalizedItem]> {
        Ok(self.finalized_items())
    }

    /// Return the current AlephBFT runtime status.
    pub fn aleph_bft_status(&self) -> Result<FederationConsensusRuntimeStatus> {
        Ok(self.status())
    }

    /// Return the complete-unit recovery cursor represented by this runtime.
    pub fn aleph_bft_recovery_cursor(&self) -> Result<FederationAlephBftRecoveryCursor> {
        let status = self.aleph_bft_status()?;
        Ok(FederationAlephBftRecoveryCursor {
            session: status.session,
            next_unit_finalization_index: status.next_unit_finalization_index,
            next_unit_index_by_member: self.next_unit_index_by_member().clone(),
        })
    }
}

impl FederationConsensusRuntime {
    /// Return this member's federation ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.status().federation_id
    }

    /// Return this member's ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.status().local_member
    }

    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        &mut self.journal
    }

    /// Return this member's current peer status.
    #[must_use]
    pub fn peer_status(&self) -> FederationPeerStatus {
        self.peer_status_for(self.federation_id(), self.member_id())
    }

    /// Return a catch-up request for the next local finalized item.
    #[must_use]
    pub fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        FederationJournalCatchUpRequest::new(self.journal().next_index())
            .with_known_order_digest(self.journal().latest_order_digest())
            .with_aleph_bft_next_item_index(self.next_item_index())
    }

    /// Return journal entries for a catch-up request.
    #[must_use]
    pub fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        FederationJournalCatchUpResponse::from_journal(
            self.federation_id(),
            self.journal(),
            FederationMaterializedStateDigest::genesis(),
            request,
            DEFAULT_CATCH_UP_LIMIT,
            MAX_CATCH_UP_LIMIT,
        )
    }

    /// Return journal entries after checking the request's known digest.
    pub fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        if let Some(known_order_digest) = request.known_order_digest {
            if let Some(expected_digest) = self.journal().order_digest_before(request.next_index) {
                if expected_digest != known_order_digest {
                    return Err(FederationError::CatchUpDigestMismatch {
                        index: request.next_index,
                        expected: expected_digest,
                        found: known_order_digest,
                    });
                }
            }
        }

        Ok(self.catch_up(request))
    }

    /// Update local application status for an ordered operation.
    pub fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.journal_mut().update_status(operation_id, status)
    }

    /// Update local post-consensus application state.
    pub fn update_application_state(
        &mut self,
        operation_id: FederationOperationId,
        application: FederationApplicationState,
    ) -> Result<FederationJournalEntry> {
        self.journal_mut()
            .update_application_state(operation_id, application)
    }
}

fn validate_runtime_roster<I>(
    member_ids: I,
    consensus_threshold: u16,
    local_member: MemberId,
) -> Result<BTreeSet<MemberId>>
where
    I: IntoIterator<Item = MemberId>,
{
    let mut members = BTreeSet::new();
    for member_id in member_ids {
        if !members.insert(member_id) {
            return Err(FederationError::DuplicateMember { member_id });
        }
    }

    let member_count =
        u16::try_from(members.len()).map_err(|_| FederationError::InvalidThresholdParams {
            members: u16::MAX,
            signature_threshold: consensus_threshold,
            consensus_threshold,
        })?;
    ThresholdParams::new(member_count, consensus_threshold, consensus_threshold)?;
    if !members.contains(&local_member) {
        return Err(FederationError::UnknownMember {
            member_id: local_member,
        });
    }

    Ok(members)
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    let mut value = serde_json::to_value(value)?;
    sort_json_value(&mut value);
    Ok(serde_json::to_vec(&value)?)
}

fn sort_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.sort_keys();
            for value in map.values_mut() {
                sort_json_value(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sort_json_value(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests;
