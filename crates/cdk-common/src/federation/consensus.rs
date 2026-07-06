//! Deterministic local consensus boundary.
//!
//! This is not a production quorum protocol. It validates federation identity,
//! appends operations to the accepted journal, and exposes the same
//! status/catch-up/submit payloads used by private REST. Production ordering
//! backends can replace the decision source while keeping this API shape.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{
    FederationConfig, FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalConsensusEvidence, FederationJournalEntry,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationPeerStatus, FederationProtocolVersions, FederationRestMessageKind,
    FederationSessionOutcome, FederationSessionOutcomeQuorum, FederationSignedRequest,
    FederationStateDigest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationJournal, MemberId, Result, ThresholdParams,
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

/// Version of AlephBFT unit payloads carrying CDK consensus items.
pub const FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION: u16 = 1;

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
    /// Federation protocol versions required to interpret this unit.
    pub protocol_versions: FederationProtocolVersions,
    /// Consensus item version required for every item in the body.
    pub consensus_item_version: u16,
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
            protocol_versions: FederationProtocolVersions::current(),
            consensus_item_version: FEDERATION_CONSENSUS_ITEM_VERSION,
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
        self.protocol_versions.validate()?;
        ensure_consensus_item_version(self.consensus_item_version)?;

        for item in &self.items {
            item.ensure_supported_version()?;
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
}

/// Operator-facing status for the AlephBFT adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationAlephBftAdapterStatus {
    /// Consensus backend represented by this status snapshot.
    pub consensus_backend: FederationConsensusBackendKind,
    /// Federation whose operations are being ordered.
    pub federation_id: FederationId,
    /// Local member producing unit payloads.
    pub local_member: MemberId,
    /// Current AlephBFT session accepted by the adapter.
    pub session: u64,
    /// Next local unit index that will be used by the data provider.
    pub next_unit_index: u64,
    /// Next finalized unit index expected by the finalization handler.
    pub next_unit_finalization_index: u64,
    /// Next global finalized consensus-item index.
    pub next_item_index: u64,
    /// Number of local consensus items waiting to be proposed.
    pub queued_item_count: u64,
    /// Number of finalized consensus items retained by the adapter.
    pub finalized_item_count: u64,
    /// Number of accepted operation entries in the adapter journal.
    pub accepted_operation_count: u64,
    /// Highest accepted operation index, if any operation has been accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_accepted_operation_index: Option<u64>,
    /// Number of members allowed to submit finalized units, when roster-bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_member_count: Option<u64>,
}

/// One item that can enter the federation consensus pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationConsensusItem {
    /// A canonical Cashu mint operation envelope.
    Operation(FederationOperationEnvelope),
    /// A module-defined fact that must be ordered with operations.
    ModuleFact(FederationConsensusModuleFact),
    /// A member-signed session outcome.
    SessionSignature(FederationSessionOutcome),
    /// A generic consensus item for future compatibility tests.
    Default(FederationConsensusDefaultItem),
}

impl FederationConsensusItem {
    /// Wrap an operation envelope as a consensus item.
    #[must_use]
    pub fn operation(envelope: FederationOperationEnvelope) -> Self {
        Self::Operation(envelope)
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
            Self::ModuleFact(_) | Self::SessionSignature(_) | Self::Default(_) => Ok(None),
        }
    }

    /// Reject consensus items from unsupported item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        match self {
            Self::Operation(envelope) => envelope.ensure_supported_version(),
            Self::ModuleFact(item) => item.ensure_supported_version(),
            Self::SessionSignature(outcome) => {
                if outcome.version != super::FEDERATION_SESSION_OUTCOME_VERSION {
                    return Err(FederationError::SessionOutcomeVersionMismatch {
                        expected: super::FEDERATION_SESSION_OUTCOME_VERSION,
                        found: outcome.version,
                    });
                }

                Ok(())
            }
            Self::Default(item) => item.ensure_supported_version(),
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
            FederationConsensusItem::ModuleFact(_)
            | FederationConsensusItem::SessionSignature(_)
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
pub struct FederationConsensusItemQueue {
    max_item_bytes: usize,
    pending: VecDeque<FederationConsensusItem>,
    pending_operation_ids: BTreeSet<FederationOperationId>,
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
        }
    }

    /// Return the number of queued items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Return true when the queue has no pending items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Submit an item to the pending queue after size and duplicate checks.
    pub fn submit(
        &mut self,
        item: FederationConsensusItem,
    ) -> Result<FederationConsensusItemAdmission> {
        item.ensure_within_size_limit(self.max_item_bytes)?;
        if let Some(operation_id) = item.operation_id()? {
            if !self.pending_operation_ids.insert(operation_id) {
                return Ok(FederationConsensusItemAdmission::DuplicateOperation);
            }
        }

        self.pending.push_back(item);
        Ok(FederationConsensusItemAdmission::Accepted)
    }

    /// Drain up to `limit` items in FIFO order.
    pub fn drain_batch(&mut self, limit: usize) -> Vec<FederationConsensusItem> {
        let mut batch = Vec::new();
        for _ in 0..limit {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            if let Ok(Some(operation_id)) = item.operation_id() {
                self.pending_operation_ids.remove(&operation_id);
            }
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
            if let Ok(Some(operation_id)) = item.operation_id() {
                self.pending_operation_ids.remove(&operation_id);
            }
            batch_bytes = batch_bytes.saturating_add(item_size);
            batch.push(item);
        }

        Ok(batch)
    }
}

/// Bounded AlephBFT data provider for local pending CDK consensus items.
#[derive(Debug, Clone)]
pub struct FederationAlephBftUnitDataProvider {
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

    /// Return true when the provider has no queued consensus items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Begin a new AlephBFT session and reset local unit indexing.
    pub fn begin_session(&mut self, session: u64) {
        self.session = session;
        self.next_unit_index = 0;
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

    /// Preview the next non-empty unit payload without draining queued items.
    pub fn preview_next_unit_payload(&self) -> Result<Option<FederationAlephBftUnitPayload>> {
        let mut preview = self.clone();
        preview.next_unit_payload()
    }

    /// Drain the next unit only when it exactly matches a previously previewed payload.
    pub fn drain_next_unit_payload_matching(
        &mut self,
        expected: &FederationAlephBftUnitPayload,
    ) -> Result<FederationAlephBftUnitPayload> {
        expected.ensure_supported_version()?;
        let Some(found) = self.preview_next_unit_payload()? else {
            return Err(FederationError::AlephBftUnitProposalUnavailable);
        };
        ensure_matching_aleph_bft_unit_proposal(expected, &found)?;

        let Some(drained) = self.next_unit_payload()? else {
            return Err(FederationError::AlephBftUnitProposalUnavailable);
        };
        ensure_matching_aleph_bft_unit_proposal(expected, &drained)?;
        Ok(drained)
    }
}

fn ensure_matching_aleph_bft_unit_proposal(
    expected: &FederationAlephBftUnitPayload,
    found: &FederationAlephBftUnitPayload,
) -> Result<()> {
    let expected_bytes = expected.canonical_bytes()?;
    let found_bytes = found.canonical_bytes()?;
    if expected_bytes != found_bytes {
        return Err(FederationError::AlephBftUnitProposalMismatch {
            expected_session: expected.session,
            expected_member: expected.submitting_member,
            expected_unit_index: expected.unit_index,
            expected_body_hash: expected.body_hash.to_hex(),
            found_session: found.session,
            found_member: found.submitting_member,
            found_unit_index: found.unit_index,
            found_body_hash: found.body_hash.to_hex(),
        });
    }

    Ok(())
}

/// Converts verified AlephBFT unit payloads into deterministic accepted items.
#[derive(Debug, Clone)]
pub struct FederationAlephBftFinalizationHandler {
    federation_id: FederationId,
    session: u64,
    next_unit_finalization_index: u64,
    next_item_index: u64,
    max_unit_payload_bytes: usize,
    allowed_members: Option<BTreeSet<MemberId>>,
    finalized_units: BTreeSet<(u64, MemberId, u64)>,
    finalized_operation_ids: BTreeSet<FederationOperationId>,
}

impl FederationAlephBftFinalizationHandler {
    /// Create a finalization handler for session zero with default limits.
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
            finalized_units: BTreeSet::new(),
            finalized_operation_ids: BTreeSet::new(),
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
        handler.allowed_members = Some(allowed_members.into_iter().collect());
        Ok(handler)
    }

    fn recover_from_finalized_items(
        federation_id: FederationId,
        session: u64,
        max_unit_payload_bytes: usize,
        allowed_members: Option<BTreeSet<MemberId>>,
        finalized_items: &[FederationAlephBftFinalizedItem],
    ) -> Result<Self> {
        ensure_non_zero_consensus_limit("max_unit_payload_bytes", max_unit_payload_bytes)?;

        let mut finalized_units = BTreeSet::new();
        let mut finalized_unit_metadata = BTreeMap::new();
        let mut finalized_operation_ids = BTreeSet::new();
        let mut next_unit_finalization_index = 0;
        let mut next_item_index = 0;

        for (expected_index, finalized_item) in finalized_items.iter().enumerate() {
            let expected_index =
                u64::try_from(expected_index).expect("finalized item count fits in u64");
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
            if let Some(allowed_members) = &allowed_members {
                if !allowed_members.contains(&finalized_item.unit_submitting_member) {
                    return Err(FederationError::UnknownMember {
                        member_id: finalized_item.unit_submitting_member,
                    });
                }
            }

            let computed_operation_id = finalized_item.item.operation_id()?;
            if computed_operation_id != finalized_item.operation_id {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: "finalized item operation id does not match item body".to_string(),
                });
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
                    if finalized_operation_ids.insert(operation_id) {
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
                (FederationAlephBftFinalizedItemStatus::Accepted, None) => {}
            }

            let unit_key = (
                finalized_item.session,
                finalized_item.unit_submitting_member,
                finalized_item.unit_index,
            );
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

            if finalized_item.session == session {
                next_unit_finalization_index = next_unit_finalization_index
                    .max(finalized_item.unit_finalization_index.saturating_add(1));
            }
            next_item_index = finalized_item.accepted_item_index.saturating_add(1);
        }

        Ok(Self {
            federation_id,
            session,
            next_unit_finalization_index,
            next_item_index,
            max_unit_payload_bytes,
            allowed_members,
            finalized_units,
            finalized_operation_ids,
        })
    }

    /// Return the session accepted by this handler.
    #[must_use]
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Return the next unit finalization index within the current session.
    #[must_use]
    pub fn next_unit_finalization_index(&self) -> u64 {
        self.next_unit_finalization_index
    }

    /// Return the next global accepted item index.
    #[must_use]
    pub fn next_item_index(&self) -> u64 {
        self.next_item_index
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
            let status = match operation_id {
                Some(operation_id) => {
                    if self.finalized_operation_ids.insert(operation_id) {
                        FederationAlephBftFinalizedItemStatus::Accepted
                    } else {
                        FederationAlephBftFinalizedItemStatus::DuplicateOperation
                    }
                }
                None => FederationAlephBftFinalizedItemStatus::Accepted,
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

/// AlephBFT-facing adapter boundary for operation submission and finalization.
///
/// This does not run AlephBFT networking. It owns the CDK side of the boundary:
/// wallet/public handlers can enqueue operations, an AlephBFT task can pull
/// canonical unit payloads from the data provider, and finalized units are
/// converted back into deterministic journal responses.
#[derive(Debug, Clone)]
pub struct FederationAlephBftConsensusAdapter {
    data_provider: FederationAlephBftUnitDataProvider,
    finalization_handler: FederationAlephBftFinalizationHandler,
    journal: InMemoryFederationJournal,
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
}

impl FederationAlephBftConsensusAdapter {
    /// Create an AlephBFT adapter boundary with default limits.
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
        }
    }

    /// Create an AlephBFT adapter boundary that only accepts finalized units from known members.
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
        })
    }

    /// Create an AlephBFT adapter boundary with explicit limits.
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
        })
    }

    /// Recover an AlephBFT adapter boundary from retained finalized items.
    pub fn recover_from_finalized_items(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        Self::recover_inner(federation_id, local_member, session, None, finalized_items)
    }

    /// Recover an AlephBFT adapter boundary with a roster-bound finalization guard.
    pub fn recover_for_members_from_finalized_items<I>(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        allowed_members: I,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        Self::recover_inner(
            federation_id,
            local_member,
            session,
            Some(allowed_members.into_iter().collect()),
            finalized_items,
        )
    }

    fn recover_inner(
        federation_id: FederationId,
        local_member: MemberId,
        session: u64,
        allowed_members: Option<BTreeSet<MemberId>>,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self> {
        let finalization_handler =
            FederationAlephBftFinalizationHandler::recover_from_finalized_items(
                federation_id,
                session,
                FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                allowed_members,
                &finalized_items,
            )?;
        let mut journal = InMemoryFederationJournal::new();
        for finalized_item in &finalized_items {
            if let FederationConsensusItem::Operation(envelope) = &finalized_item.item {
                journal.append_with_consensus_evidence(
                    envelope.clone(),
                    finalized_item.consensus_evidence(),
                )?;
            }
        }

        Ok(Self {
            data_provider: FederationAlephBftUnitDataProvider::new(
                federation_id,
                local_member,
                session,
            ),
            finalization_handler,
            journal,
            finalized_items,
        })
    }

    /// Return the backend kind represented by this adapter boundary.
    #[must_use]
    pub fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::AlephBft
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

    /// Return accepted-operation journal state produced from finalized units.
    #[must_use]
    pub fn journal(&self) -> &InMemoryFederationJournal {
        &self.journal
    }

    /// Return finalized consensus items in local finalization order.
    #[must_use]
    pub fn finalized_items(&self) -> &[FederationAlephBftFinalizedItem] {
        &self.finalized_items
    }

    /// Return an operator-facing status snapshot for this adapter.
    #[must_use]
    pub fn status(&self) -> FederationAlephBftAdapterStatus {
        FederationAlephBftAdapterStatus {
            consensus_backend: self.backend_kind(),
            federation_id: self.data_provider.federation_id,
            local_member: self.data_provider.submitting_member,
            session: self.session(),
            next_unit_index: self.next_unit_index(),
            next_unit_finalization_index: self.finalization_handler.next_unit_finalization_index(),
            next_item_index: self.next_item_index(),
            queued_item_count: u64::try_from(self.queued_len()).expect("queue length fits in u64"),
            finalized_item_count: u64::try_from(self.finalized_items.len())
                .expect("finalized item count fits in u64"),
            accepted_operation_count: self.journal.len() as u64,
            latest_accepted_operation_index: self.journal.entries().last().map(|entry| entry.index),
            allowed_member_count: self.finalization_handler.allowed_member_count(),
        }
    }

    /// Return this member's current peer status from finalized operations.
    #[must_use]
    pub fn peer_status(
        &self,
        federation_id: FederationId,
        member_id: MemberId,
    ) -> FederationPeerStatus {
        FederationPeerStatus::from_journal_with_backend(
            federation_id,
            member_id,
            &self.journal,
            FederationConsensusBackendKind::AlephBft,
        )
    }

    /// Begin a later AlephBFT session for both data-provider and finalizer.
    pub fn begin_session(&mut self, session: u64) -> Result<()> {
        self.finalization_handler.begin_session(session)?;
        self.data_provider.begin_session(session);
        Ok(())
    }

    /// Queue one operation envelope for AlephBFT unit construction.
    pub fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.data_provider.submit_operation(envelope)
    }

    /// Pull the next local unit payload for the AlephBFT data provider.
    pub fn next_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        self.data_provider.next_unit_payload()
    }

    /// Preview the next local unit payload without draining it.
    pub fn preview_next_unit_payload(&self) -> Result<Option<FederationAlephBftUnitPayload>> {
        self.data_provider.preview_next_unit_payload()
    }

    /// Drain the next local unit only if it exactly matches a prior preview.
    pub fn drain_next_unit_payload_matching(
        &mut self,
        expected: &FederationAlephBftUnitPayload,
    ) -> Result<FederationAlephBftUnitPayload> {
        self.data_provider
            .drain_next_unit_payload_matching(expected)
    }

    /// Apply one finalized AlephBFT unit to the local accepted-operation journal.
    pub fn finalize_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        let outcome = self.finalization_handler.finalize_next_unit(payload)?;
        let mut responses = Vec::new();

        for finalized_item in outcome.items {
            if let FederationConsensusItem::Operation(envelope) = &finalized_item.item {
                let response = FederationSubmitOperationResponse::from_outcome(
                    self.journal.append_with_consensus_evidence(
                        envelope.clone(),
                        finalized_item.consensus_evidence(),
                    )?,
                );
                responses.push(response);
            }
            self.finalized_items.push(finalized_item);
        }

        Ok(responses)
    }
}

fn ensure_non_zero_consensus_limit(field: &'static str, value: usize) -> Result<()> {
    if value == 0 {
        return Err(FederationError::InvalidConsensusLimit { field, value: 0 });
    }

    Ok(())
}

/// Minimal synchronous consensus engine surface used by federation runtimes.
///
/// The first implementation is the deterministic in-memory engine below. Real
/// networking or BFT finalization engines should preserve this state-machine
/// boundary: submit admissible items, expose accepted order, serve catch-up,
/// replay peer catch-up, and update only local application status.
pub trait FederationConsensusEngine: fmt::Debug + Send {
    /// Return the backend kind used by this consensus engine.
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    /// Return this member's federation ID.
    fn federation_id(&self) -> FederationId;

    /// Return this member's ID.
    fn member_id(&self) -> MemberId;

    /// Return the accepted operation journal.
    fn journal(&self) -> &InMemoryFederationJournal;

    /// Return this member's consensus-visible state digest.
    fn state_digest(&self) -> FederationStateDigest;

    /// Return a mutable accepted operation journal.
    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal;

    /// Submit an already-built operation envelope.
    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse>;

    /// Submit an operation request from a transport boundary.
    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse>;

    /// Return this member's current peer status.
    fn peer_status(&self) -> FederationPeerStatus;

    /// Return a catch-up request for the next local missing operation.
    fn catch_up_request(&self) -> FederationJournalCatchUpRequest;

    /// Return journal entries for a catch-up request.
    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse;

    /// Return journal entries for a checked catch-up request.
    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse>;

    /// Apply a peer catch-up response to the local journal.
    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>>;

    /// Verify a session-outcome quorum before applying a peer catch-up response.
    fn apply_catch_up_response_with_session_outcome_quorum(
        &mut self,
        response: &FederationJournalCatchUpResponse,
        config: &FederationConfig,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<FederationQuorumVerifiedCatchUp>;

    /// Update local application status for an accepted operation.
    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry>;
}

/// Catch-up entries applied after verifying quorum-signed session outcome evidence.
#[derive(Debug, Clone)]
pub struct FederationQuorumVerifiedCatchUp {
    /// Entries appended to the local accepted-operation journal.
    pub applied_entries: Vec<FederationJournalEntry>,
    /// Verified signer quorum over the exact catch-up response range.
    pub session_outcome_quorum: FederationSessionOutcomeQuorum,
}

/// Runtime-owned consensus engine.
///
/// CDK keeps this small enum instead of exposing the in-memory backend through
/// server state. That lets Axum and storage code depend on the consensus engine
/// contract while future engines can be added behind this wrapper.
#[derive(Debug, Clone)]
pub struct FederationConsensusRuntime {
    backend: FederationConsensusRuntimeBackend,
}

/// Consensus backend selected for ordering federation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FederationConsensusBackendKind {
    /// Deterministic local append-log backend for tests and local smoke.
    #[default]
    InMemory,
    /// Future AlephBFT-backed production ordering backend.
    AlephBft,
}

impl FederationConsensusBackendKind {
    /// Return the canonical config string for this backend.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::AlephBft => "aleph_bft",
        }
    }

    /// Return true when this build can instantiate the backend.
    #[must_use]
    pub fn is_available(self) -> bool {
        matches!(self, Self::InMemory)
    }
}

impl fmt::Display for FederationConsensusBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FederationConsensusBackendKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().replace('-', "_").as_str() {
            "in_memory" | "inmemory" | "memory" => Ok(Self::InMemory),
            "aleph_bft" | "alephbft" => Ok(Self::AlephBft),
            _ => Err(format!(
                "Unknown federation consensus backend: {s}. Valid options: in_memory, aleph_bft"
            )),
        }
    }
}

#[derive(Debug, Clone)]
enum FederationConsensusRuntimeBackend {
    InMemory(InMemoryFederationConsensus),
    AlephBft(FederationAlephBftConsensusAdapter),
}

impl FederationConsensusRuntime {
    /// Create an in-memory runtime for deterministic tests and local use.
    #[must_use]
    pub fn in_memory(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            backend: FederationConsensusRuntimeBackend::InMemory(InMemoryFederationConsensus::new(
                federation_id,
                member_id,
            )),
        }
    }

    /// Create a runtime for the requested backend.
    pub fn with_backend(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_id: MemberId,
    ) -> Result<Self> {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Ok(Self::in_memory(federation_id, member_id))
            }
            FederationConsensusBackendKind::AlephBft => Ok(Self {
                backend: FederationConsensusRuntimeBackend::AlephBft(
                    FederationAlephBftConsensusAdapter::new(federation_id, member_id, 0),
                ),
            }),
        }
    }

    /// Recover an AlephBFT runtime from retained finalized items.
    pub fn aleph_bft_from_finalized_items<I>(
        federation_id: FederationId,
        member_id: MemberId,
        session: u64,
        member_ids: I,
        consensus_threshold: u16,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let allowed_members = validate_adapter_roster(member_ids, consensus_threshold, member_id)?;
        Ok(Self {
            backend: FederationConsensusRuntimeBackend::AlephBft(
                FederationAlephBftConsensusAdapter::recover_for_members_from_finalized_items(
                    federation_id,
                    member_id,
                    session,
                    allowed_members,
                    finalized_items,
                )?,
            ),
        })
    }

    /// Return the backend selected for this runtime.
    #[must_use]
    pub fn backend_kind(&self) -> FederationConsensusBackendKind {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(_) => {
                FederationConsensusBackendKind::InMemory
            }
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                FederationConsensusBackendKind::AlephBft
            }
        }
    }

    /// Return the number of locally queued consensus items, if the backend exposes a queue.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(_) => 0,
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.queued_len(),
        }
    }

    /// Return the number of dispatched consensus items awaiting finalization.
    #[must_use]
    pub fn pending_finalization_len(&self) -> usize {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(_) => 0,
            FederationConsensusRuntimeBackend::AlephBft(_) => 0,
        }
    }

    /// Return queued plus dispatched consensus items awaiting finalization.
    #[must_use]
    pub fn pending_mempool_len(&self) -> usize {
        self.queued_len()
            .saturating_add(self.pending_finalization_len())
    }

    /// Queue one operation for AlephBFT unit construction.
    pub fn submit_aleph_bft_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => {
                adapter.submit_operation(envelope)
            }
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Pull the next local AlephBFT unit payload from the runtime.
    pub fn next_aleph_bft_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.next_unit_payload(),
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Preview the next local AlephBFT unit payload without draining it.
    pub fn preview_next_aleph_bft_unit_payload(
        &self,
    ) -> Result<Option<FederationAlephBftUnitPayload>> {
        match &self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => {
                adapter.preview_next_unit_payload()
            }
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Drain the next AlephBFT unit only if it exactly matches a prior preview.
    pub fn drain_next_aleph_bft_unit_payload_matching(
        &mut self,
        expected: &FederationAlephBftUnitPayload,
    ) -> Result<FederationAlephBftUnitPayload> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => {
                adapter.drain_next_unit_payload_matching(expected)
            }
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Apply one externally finalized AlephBFT unit payload.
    pub fn finalize_aleph_bft_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.finalize_unit(payload),
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Return finalized AlephBFT items held by this runtime.
    pub fn aleph_bft_finalized_items(&self) -> Result<&[FederationAlephBftFinalizedItem]> {
        match &self.backend {
            FederationConsensusRuntimeBackend::AlephBft(adapter) => Ok(adapter.finalized_items()),
            FederationConsensusRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Rebuild an in-memory runtime from persisted journal entries.
    pub fn in_memory_from_entries(
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        Ok(Self {
            backend: FederationConsensusRuntimeBackend::InMemory(
                InMemoryFederationConsensus::from_entries(federation_id, member_id, entries)?,
            ),
        })
    }

    /// Rebuild a runtime for the requested backend from persisted journal entries.
    pub fn from_entries_with_backend(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Self::in_memory_from_entries(federation_id, member_id, entries)
            }
            FederationConsensusBackendKind::AlephBft => {
                if entries.is_empty() {
                    Self::with_backend(backend, federation_id, member_id)
                } else {
                    Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                        reason: "cannot recover AlephBFT runtime from journal entries without finalized items"
                            .to_string(),
                    })
                }
            }
        }
    }
}

impl FederationConsensusEngine for FederationConsensusRuntime {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        Self::backend_kind(self)
    }

    fn federation_id(&self) -> FederationId {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.federation_id(),
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.status().federation_id,
        }
    }

    fn member_id(&self) -> MemberId {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.member_id(),
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.status().local_member,
        }
    }

    fn journal(&self) -> &InMemoryFederationJournal {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.journal(),
            FederationConsensusRuntimeBackend::AlephBft(adapter) => adapter.journal(),
        }
    }

    fn state_digest(&self) -> FederationStateDigest {
        self.journal().state_digest()
    }

    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.journal_mut(),
            FederationConsensusRuntimeBackend::AlephBft(adapter) => &mut adapter.journal,
        }
    }

    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.submit_envelope(envelope),
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }

    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.submit_operation(request),
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }

    fn peer_status(&self) -> FederationPeerStatus {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine
                .peer_status()
                .with_consensus_backend(self.backend_kind()),
            FederationConsensusRuntimeBackend::AlephBft(adapter) => {
                adapter.peer_status(self.federation_id(), self.member_id())
            }
        }
    }

    fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        FederationJournalCatchUpRequest::new(self.journal().next_index())
            .with_known_order_digest(self.journal().latest_order_digest())
    }

    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.catch_up(request),
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                FederationJournalCatchUpResponse::from_journal(
                    self.federation_id(),
                    self.journal(),
                    request,
                    DEFAULT_CATCH_UP_LIMIT,
                    MAX_CATCH_UP_LIMIT,
                )
            }
        }
    }

    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine.catch_up_checked(request),
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                if let Some(known_order_digest) = request.known_order_digest {
                    if let Some(expected_digest) =
                        self.journal().order_digest_before(request.next_index)
                    {
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
        }
    }

    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => {
                engine.apply_catch_up_response(response)
            }
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }

    fn apply_catch_up_response_with_session_outcome_quorum(
        &mut self,
        response: &FederationJournalCatchUpResponse,
        config: &FederationConfig,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<FederationQuorumVerifiedCatchUp> {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine
                .apply_catch_up_response_with_session_outcome_quorum(
                    response,
                    config,
                    additional_outcomes,
                ),
            FederationConsensusRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }

    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.journal_mut().update_status(operation_id, status)
    }
}

/// Deterministic multi-member in-memory consensus simulator.
///
/// This is a test engine, not a BFT protocol. It lets CDK model the effects
/// that matter for the mint state machine before a real consensus backend is
/// plugged in: proposals can be delayed, duplicated, and delivered in a
/// different order; reachable members must satisfy the configured quorum; and
/// offline members can catch up from a peer journal.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensusNetwork {
    federation_id: FederationId,
    consensus_threshold: u16,
    members: BTreeMap<MemberId, FederationConsensusRuntime>,
    online_members: BTreeSet<MemberId>,
    pending: VecDeque<InMemoryFederationConsensusProposal>,
}

#[derive(Debug, Clone)]
struct InMemoryFederationConsensusProposal {
    sender: MemberId,
    envelope: FederationOperationEnvelope,
}

impl InMemoryFederationConsensusNetwork {
    /// Create a multi-member simulator with every member initially online.
    pub fn new<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let mut members = BTreeMap::new();
        for member_id in member_ids {
            if members
                .insert(
                    member_id,
                    FederationConsensusRuntime::in_memory(federation_id, member_id),
                )
                .is_some()
            {
                return Err(FederationError::DuplicateMember { member_id });
            }
        }

        let member_count = members.len() as u16;
        if consensus_threshold == 0 || consensus_threshold > member_count {
            return Err(FederationError::InvalidThresholdParams {
                members: member_count,
                signature_threshold: consensus_threshold,
                consensus_threshold,
            });
        }

        Ok(Self {
            federation_id,
            consensus_threshold,
            online_members: members.keys().copied().collect(),
            members,
            pending: VecDeque::new(),
        })
    }

    /// Return the number of queued proposals.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Return the member IDs known to this simulator.
    pub fn member_ids(&self) -> impl Iterator<Item = MemberId> + '_ {
        self.members.keys().copied()
    }

    /// Mark a member online or offline.
    pub fn set_online(&mut self, member_id: MemberId, online: bool) -> Result<()> {
        self.ensure_member(member_id)?;
        if online {
            self.online_members.insert(member_id);
        } else {
            self.online_members.remove(&member_id);
        }
        Ok(())
    }

    /// Queue a proposal from a known online member.
    pub fn submit(
        &mut self,
        sender: MemberId,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationOperationId> {
        self.ensure_member(sender)?;
        self.ensure_online(sender)?;
        self.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        let operation_id = envelope.operation_id()?;
        self.pending
            .push_back(InMemoryFederationConsensusProposal { sender, envelope });
        Ok(operation_id)
    }

    /// Duplicate the next queued proposal at the back of the queue.
    ///
    /// Returns false when the queue is empty.
    pub fn duplicate_next_proposal(&mut self) -> bool {
        let Some(proposal) = self.pending.front().cloned() else {
            return false;
        };
        self.pending.push_back(proposal);
        true
    }

    /// Move the next queued proposal to the back of the queue.
    ///
    /// Returns false when the queue is empty.
    pub fn delay_next_proposal(&mut self) -> bool {
        let Some(proposal) = self.pending.pop_front() else {
            return false;
        };
        self.pending.push_back(proposal);
        true
    }

    /// Deliver one queued proposal to all currently online members.
    pub fn deliver_next(&mut self) -> Result<Option<FederationSubmitOperationResponse>> {
        self.ensure_quorum_reachable()?;
        let Some(proposal) = self.pending.pop_front() else {
            return Ok(None);
        };
        self.ensure_online(proposal.sender)?;

        let mut next_members = self.members.clone();
        let mut response = None;
        for member_id in &self.online_members {
            let member = next_members
                .get_mut(member_id)
                .ok_or(FederationError::UnknownMember {
                    member_id: *member_id,
                })?;
            let member_response = member.submit_envelope(proposal.envelope.clone())?;
            response.get_or_insert(member_response);
        }
        self.members = next_members;

        Ok(response)
    }

    /// Deliver all queued proposals that can be finalized with the current quorum.
    pub fn deliver_all(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        let mut responses = Vec::new();
        while self.pending_len() > 0 {
            if let Some(response) = self.deliver_next()? {
                responses.push(response);
            }
        }
        Ok(responses)
    }

    /// Catch one member up from another member's accepted journal.
    pub fn catch_up_member_from_peer(
        &mut self,
        member_id: MemberId,
        peer_id: MemberId,
    ) -> Result<Vec<FederationJournalEntry>> {
        let mut member = self.member(member_id)?.clone();
        let peer = self.member(peer_id)?.clone();
        let peer_status = peer.peer_status();
        let mut applied = Vec::new();

        while member.peer_status().operation_count < peer_status.operation_count {
            let before = member.peer_status().operation_count;
            let request = member.catch_up_request();
            let response = peer.catch_up_checked(&request)?;
            let next_applied = member.apply_catch_up_response(&response)?;
            if member.peer_status().operation_count == before {
                return Err(FederationError::CatchUpNoProgress { index: before });
            }
            applied.extend(next_applied);
        }

        self.members.insert(member_id, member);
        Ok(applied)
    }

    /// Return a member's current peer status.
    pub fn member_status(&self, member_id: MemberId) -> Result<FederationPeerStatus> {
        Ok(self.member(member_id)?.peer_status())
    }

    /// Return a member's accepted operation IDs in order.
    pub fn accepted_operation_ids(
        &self,
        member_id: MemberId,
    ) -> Result<Vec<FederationOperationId>> {
        Ok(self
            .member(member_id)?
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect())
    }

    /// Return true when every member has the same accepted sequence and state digest.
    #[must_use]
    pub fn all_members_converged(&self) -> bool {
        let mut members = self.members.values();
        let Some(first) = members.next() else {
            return true;
        };
        let first_ids: Vec<_> = first
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect();
        let first_state_digest = first.state_digest();
        members.all(|member| {
            member.state_digest() == first_state_digest
                && member
                    .journal()
                    .entries()
                    .iter()
                    .map(|entry| entry.operation_id)
                    .eq(first_ids.iter().copied())
        })
    }

    fn member(&self, member_id: MemberId) -> Result<&FederationConsensusRuntime> {
        self.members
            .get(&member_id)
            .ok_or(FederationError::UnknownMember { member_id })
    }

    fn ensure_member(&self, member_id: MemberId) -> Result<()> {
        self.member(member_id).map(|_| ())
    }

    fn ensure_online(&self, member_id: MemberId) -> Result<()> {
        if !self.online_members.contains(&member_id) {
            return Err(FederationError::ConsensusQuorumUnavailable {
                reachable: self.online_members.len() as u16,
                required: self.consensus_threshold,
            });
        }
        Ok(())
    }

    fn ensure_quorum_reachable(&self) -> Result<()> {
        let reachable = self.online_members.len() as u16;
        if reachable < self.consensus_threshold {
            return Err(FederationError::ConsensusQuorumUnavailable {
                reachable,
                required: self.consensus_threshold,
            });
        }
        Ok(())
    }

    fn ensure_federation(&self, found: FederationId) -> Result<()> {
        if found != self.federation_id {
            return Err(FederationError::OperationFederationMismatch {
                expected: self.federation_id,
                found,
            });
        }
        Ok(())
    }
}

/// Backend-neutral operation adapter used by consensus ordering engines.
///
/// The in-memory implementation dispatches into the deterministic simulator. A
/// production BFT backend should implement the same admission, dispatch, and
/// finalized-response boundary without changing public mint handlers.
pub trait FederationConsensusBackendAdapter: fmt::Debug + Send {
    /// Return the backend kind used by this adapter.
    fn backend_kind(&self) -> FederationConsensusBackendKind;

    /// Return the number of locally queued consensus items.
    fn queued_len(&self) -> usize;

    /// Return the number of items dispatched but not yet finalized.
    fn pending_finalization_len(&self) -> usize;

    /// Queue one operation envelope for later backend dispatch.
    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission>;

    /// Dispatch queued operation items to the backend without finalizing them.
    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize>;

    /// Deliver every dispatched operation finalized by the backend.
    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>>;

    /// Dispatch queued operations and return finalized responses.
    fn dispatch_and_finalize(
        &mut self,
        limit: usize,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.dispatch_queued_operations(limit)?;
        self.finalize_dispatched_operations()
    }
}

/// Runtime-selected consensus backend adapter.
#[derive(Debug, Clone)]
pub struct FederationConsensusBackendAdapterRuntime {
    backend: FederationConsensusBackendAdapterRuntimeBackend,
}

#[derive(Debug, Clone)]
enum FederationConsensusBackendAdapterRuntimeBackend {
    InMemory(InMemoryFederationConsensusAdapter),
    AlephBft(FederationAlephBftConsensusAdapter),
}

impl FederationConsensusBackendAdapterRuntime {
    /// Create an in-memory backend adapter runtime.
    pub fn in_memory<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        Ok(Self {
            backend: FederationConsensusBackendAdapterRuntimeBackend::InMemory(
                InMemoryFederationConsensusAdapter::new(
                    federation_id,
                    member_ids,
                    consensus_threshold,
                    local_member,
                )?,
            ),
        })
    }

    /// Create an AlephBFT adapter runtime boundary for a configured roster.
    pub fn aleph_bft<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
        session: u64,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let allowed_members =
            validate_adapter_roster(member_ids, consensus_threshold, local_member)?;
        Ok(Self {
            backend: FederationConsensusBackendAdapterRuntimeBackend::AlephBft(
                FederationAlephBftConsensusAdapter::for_members(
                    federation_id,
                    local_member,
                    session,
                    allowed_members,
                )?,
            ),
        })
    }

    /// Recover an AlephBFT adapter runtime from retained finalized items.
    pub fn aleph_bft_from_finalized_items<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
        session: u64,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let allowed_members =
            validate_adapter_roster(member_ids, consensus_threshold, local_member)?;
        Ok(Self {
            backend: FederationConsensusBackendAdapterRuntimeBackend::AlephBft(
                FederationAlephBftConsensusAdapter::recover_for_members_from_finalized_items(
                    federation_id,
                    local_member,
                    session,
                    allowed_members,
                    finalized_items,
                )?,
            ),
        })
    }

    /// Create an adapter runtime for the selected backend.
    pub fn with_backend<I>(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Self::in_memory(federation_id, member_ids, consensus_threshold, local_member)
            }
            FederationConsensusBackendKind::AlephBft => Self::aleph_bft(
                federation_id,
                member_ids,
                consensus_threshold,
                local_member,
                0,
            ),
        }
    }

    /// Pull the next local AlephBFT unit payload from the selected backend.
    pub fn next_aleph_bft_unit_payload(&mut self) -> Result<Option<FederationAlephBftUnitPayload>> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.next_unit_payload()
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Preview the next local AlephBFT unit payload from the selected backend without draining it.
    pub fn preview_next_aleph_bft_unit_payload(
        &self,
    ) -> Result<Option<FederationAlephBftUnitPayload>> {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.preview_next_unit_payload()
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Drain the next AlephBFT unit only if it exactly matches a prior preview.
    pub fn drain_next_aleph_bft_unit_payload_matching(
        &mut self,
        expected: &FederationAlephBftUnitPayload,
    ) -> Result<FederationAlephBftUnitPayload> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.drain_next_unit_payload_matching(expected)
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Apply one externally finalized AlephBFT unit payload.
    pub fn finalize_aleph_bft_unit(
        &mut self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.finalize_unit(payload)
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Return finalized AlephBFT items when this runtime uses the AlephBFT adapter.
    pub fn aleph_bft_finalized_items(&self) -> Result<&[FederationAlephBftFinalizedItem]> {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                Ok(adapter.finalized_items())
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }

    /// Return AlephBFT adapter status when this runtime uses the AlephBFT adapter.
    pub fn aleph_bft_status(&self) -> Result<FederationAlephBftAdapterStatus> {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                Ok(adapter.status())
            }
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(_) => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: FederationConsensusBackendKind::AlephBft
                        .as_str()
                        .to_string(),
                })
            }
        }
    }
}

fn validate_adapter_roster<I>(
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

fn requires_external_finalization_error() -> FederationError {
    FederationError::ConsensusBackendRequiresExternalFinalization {
        backend: FederationConsensusBackendKind::AlephBft
            .as_str()
            .to_string(),
    }
}

impl FederationConsensusBackendAdapter for FederationConsensusBackendAdapterRuntime {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.backend_kind()
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.backend_kind()
            }
        }
    }

    fn queued_len(&self) -> usize {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.queued_len()
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.queued_len()
            }
        }
    }

    fn pending_finalization_len(&self) -> usize {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.pending_finalization_len()
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(_) => 0,
        }
    }

    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.submit_operation(envelope)
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(adapter) => {
                adapter.submit_operation(envelope)
            }
        }
    }

    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.dispatch_queued_operations(limit)
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }

    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => {
                adapter.finalize_dispatched_operations()
            }
            FederationConsensusBackendAdapterRuntimeBackend::AlephBft(_) => {
                Err(requires_external_finalization_error())
            }
        }
    }
}

/// Operation-focused adapter between the pending consensus item queue and the
/// deterministic multi-member simulator.
///
/// This in-memory adapter keeps public submit paths, pending consensus item
/// admission, and finalized operation delivery connected through one narrow
/// surface while production BFT backends are added behind the same trait.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensusAdapter {
    local_member: MemberId,
    queue: FederationConsensusItemQueue,
    network: InMemoryFederationConsensusNetwork,
}

impl InMemoryFederationConsensusAdapter {
    /// Create an adapter with an empty pending item queue.
    pub fn new<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let network = InMemoryFederationConsensusNetwork::new(
            federation_id,
            member_ids,
            consensus_threshold,
        )?;
        network.ensure_member(local_member)?;
        Ok(Self {
            local_member,
            queue: FederationConsensusItemQueue::new(),
            network,
        })
    }

    /// Return the number of locally queued consensus items.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }

    /// Return the number of proposals already dispatched to the simulator.
    #[must_use]
    pub fn network_pending_len(&self) -> usize {
        self.network.pending_len()
    }

    /// Mark a simulated member online or offline.
    pub fn set_online(&mut self, member_id: MemberId, online: bool) -> Result<()> {
        self.network.set_online(member_id, online)
    }

    /// Queue one operation envelope for later backend dispatch.
    pub fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.network.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        self.queue
            .submit(FederationConsensusItem::operation(envelope))
    }

    /// Dispatch queued operation items to the simulator without finalizing them.
    pub fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        self.network.ensure_online(self.local_member)?;
        let mut dispatched = 0;
        for item in self.queue.drain_batch(limit) {
            match item {
                FederationConsensusItem::Operation(envelope) => {
                    self.network.submit(self.local_member, envelope)?;
                    dispatched += 1;
                }
                FederationConsensusItem::ModuleFact(_)
                | FederationConsensusItem::SessionSignature(_)
                | FederationConsensusItem::Default(_) => {}
            }
        }

        Ok(dispatched)
    }

    /// Deliver every dispatched operation that can be finalized by quorum.
    pub fn finalize_dispatched_operations(
        &mut self,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.network.deliver_all()
    }

    /// Dispatch queued operation items and then deliver every finalized result.
    pub fn dispatch_and_finalize(
        &mut self,
        limit: usize,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.dispatch_queued_operations(limit)?;
        self.finalize_dispatched_operations()
    }

    /// Return one member's accepted operation IDs in order.
    pub fn accepted_operation_ids(
        &self,
        member_id: MemberId,
    ) -> Result<Vec<FederationOperationId>> {
        self.network.accepted_operation_ids(member_id)
    }

    /// Return true when all simulated members agree on order and state digest.
    #[must_use]
    pub fn all_members_converged(&self) -> bool {
        self.network.all_members_converged()
    }
}

impl FederationConsensusBackendAdapter for InMemoryFederationConsensusAdapter {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    fn queued_len(&self) -> usize {
        Self::queued_len(self)
    }

    fn pending_finalization_len(&self) -> usize {
        Self::network_pending_len(self)
    }

    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        Self::submit_operation(self, envelope)
    }

    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        Self::dispatch_queued_operations(self, limit)
    }

    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        Self::finalize_dispatched_operations(self)
    }
}

/// Deterministic local consensus driver used before real quorum networking.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensus {
    federation_id: FederationId,
    member_id: MemberId,
    journal: InMemoryFederationJournal,
}

impl InMemoryFederationConsensus {
    /// Create an empty local consensus driver for one federation member.
    #[must_use]
    pub fn new(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            federation_id,
            member_id,
            journal: InMemoryFederationJournal::new(),
        }
    }

    /// Rebuild local consensus from persisted journal entries.
    pub fn from_entries(
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        let journal = InMemoryFederationJournal::from_entries(entries)?;
        let consensus = Self {
            federation_id,
            member_id,
            journal,
        };

        for entry in consensus.journal.entries() {
            consensus.ensure_federation(entry.envelope.federation_id)?;
        }

        Ok(consensus)
    }

    /// Return this member's federation ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.federation_id
    }

    /// Return this member's ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Return the underlying journal.
    #[must_use]
    pub fn journal(&self) -> &InMemoryFederationJournal {
        &self.journal
    }

    /// Return a mutable reference to the underlying journal.
    pub fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        &mut self.journal
    }

    /// Submit an already-built operation envelope.
    pub fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        self.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        let outcome = self.journal.append(envelope)?;
        Ok(FederationSubmitOperationResponse::from_outcome(outcome))
    }

    /// Submit an operation request from the private REST transport.
    pub fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        self.submit_envelope(request.envelope)
    }

    /// Return this member's current peer status.
    #[must_use]
    pub fn peer_status(&self) -> FederationPeerStatus {
        FederationPeerStatus::from_journal_with_backend(
            self.federation_id,
            self.member_id,
            &self.journal,
            FederationConsensusBackendKind::InMemory,
        )
    }

    /// Return a catch-up request for the next local missing operation.
    #[must_use]
    pub fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        FederationJournalCatchUpRequest::new(self.journal.next_index())
            .with_known_order_digest(self.journal.latest_order_digest())
    }

    /// Return journal entries for a catch-up request.
    #[must_use]
    pub fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        FederationJournalCatchUpResponse::from_journal(
            self.federation_id,
            &self.journal,
            request,
            DEFAULT_CATCH_UP_LIMIT,
            MAX_CATCH_UP_LIMIT,
        )
    }

    /// Return journal entries for a catch-up request after checking the caller's digest.
    pub fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        if let Some(known_order_digest) = request.known_order_digest {
            if let Some(expected_digest) = self.journal.order_digest_before(request.next_index) {
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

    /// Apply a peer catch-up response to the local journal.
    ///
    /// This only replays the accepted operation order. Local application status
    /// is not copied from the peer, because each member must apply entries to
    /// its own mint database and recovery state independently.
    pub fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        if response.federation_id != self.federation_id {
            return Err(FederationError::CatchUpFederationMismatch {
                expected: self.federation_id,
                found: response.federation_id,
            });
        }

        let expected_from_index = self.journal.next_index();
        if response.from_index != expected_from_index {
            return Err(FederationError::CatchUpIndexMismatch {
                expected: expected_from_index,
                found: response.from_index,
            });
        }
        if response.entries.is_empty() && !response.complete {
            return Err(FederationError::CatchUpNoProgress {
                index: expected_from_index,
            });
        }

        let mut applied = Vec::with_capacity(response.entries.len());
        for entry in &response.entries {
            self.ensure_federation(entry.envelope.federation_id)?;
            applied.push(self.journal.append_catch_up_entry(entry)?);
        }

        if response.complete && response.latest_order_digest != self.journal.latest_order_digest() {
            return Err(FederationError::CatchUpDigestMismatch {
                index: self.journal.next_index(),
                expected: self.journal.latest_order_digest(),
                found: response.latest_order_digest,
            });
        }

        Ok(applied)
    }

    /// Verify a session-outcome quorum before applying a peer catch-up response.
    pub fn apply_catch_up_response_with_session_outcome_quorum(
        &mut self,
        response: &FederationJournalCatchUpResponse,
        config: &FederationConfig,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<FederationQuorumVerifiedCatchUp> {
        if config.federation_id != self.federation_id {
            return Err(FederationError::CatchUpFederationMismatch {
                expected: self.federation_id,
                found: config.federation_id,
            });
        }

        let session_outcome_quorum =
            response.verify_session_outcome_quorum(config, additional_outcomes)?;
        let applied_entries = self.apply_catch_up_response(response)?;

        Ok(FederationQuorumVerifiedCatchUp {
            applied_entries,
            session_outcome_quorum,
        })
    }

    /// Update local application status for an accepted operation.
    pub fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.journal.update_status(operation_id, status)
    }

    fn ensure_federation(&self, found: FederationId) -> Result<()> {
        if found != self.federation_id {
            return Err(FederationError::OperationFederationMismatch {
                expected: self.federation_id,
                found,
            });
        }

        Ok(())
    }
}

impl FederationConsensusEngine for InMemoryFederationConsensus {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    fn federation_id(&self) -> FederationId {
        Self::federation_id(self)
    }

    fn member_id(&self) -> MemberId {
        Self::member_id(self)
    }

    fn journal(&self) -> &InMemoryFederationJournal {
        Self::journal(self)
    }

    fn state_digest(&self) -> FederationStateDigest {
        self.journal.state_digest()
    }

    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        Self::journal_mut(self)
    }

    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        Self::submit_envelope(self, envelope)
    }

    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        Self::submit_operation(self, request)
    }

    fn peer_status(&self) -> FederationPeerStatus {
        Self::peer_status(self)
    }

    fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        Self::catch_up_request(self)
    }

    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        Self::catch_up(self, request)
    }

    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        Self::catch_up_checked(self, request)
    }

    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        Self::apply_catch_up_response(self, response)
    }

    fn apply_catch_up_response_with_session_outcome_quorum(
        &mut self,
        response: &FederationJournalCatchUpResponse,
        config: &FederationConfig,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<FederationQuorumVerifiedCatchUp> {
        Self::apply_catch_up_response_with_session_outcome_quorum(
            self,
            response,
            config,
            additional_outcomes,
        )
    }

    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        Self::update_status(self, operation_id, status)
    }
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
