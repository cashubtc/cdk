//! Deterministic federation journal.
//!
//! This in-memory journal models consensus-accepted operation order, duplicate
//! handling, conflict detection, status updates, order digests, and catch-up
//! ranges. Durable storage backends persist the same entry shape.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationError, FederationOperationConflictKey, FederationOperationEnvelope,
    FederationOperationId, MemberId, Result,
};

const FEDERATION_JOURNAL_GENESIS_VERSION: &[u8] = b"cdk-federation-journal-genesis-v1";
const FEDERATION_JOURNAL_ENTRY_VERSION: &[u8] = b"cdk-federation-journal-entry-v1";
const FEDERATION_OPERATION_OUTCOME_HASH_VERSION: &[u8] = b"cdk-federation-operation-outcome-v1";
const FEDERATION_STATE_GENESIS_VERSION: &[u8] = b"cdk-federation-state-genesis-v1";
const FEDERATION_STATE_ENTRY_VERSION: &[u8] = b"cdk-federation-state-entry-v1";
const FEDERATION_MATERIALIZED_STATE_ROW_VERSION: &[u8] =
    b"cdk-federation-materialized-state-row-v1";
const FEDERATION_MATERIALIZED_STATE_ROW_KEY_FINGERPRINT_VERSION: &[u8] =
    b"cdk-federation-materialized-state-row-key-fingerprint-v1";
const FEDERATION_MATERIALIZED_STATE_ROW_FINGERPRINT_VERSION: &[u8] =
    b"cdk-federation-materialized-state-row-fingerprint-v1";

/// Digest of the accepted federation operation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationJournalDigest([u8; 32]);

impl FederationJournalDigest {
    /// Create a journal digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a journal digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidJournalDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a journal digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this journal digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this journal digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Return the empty-journal digest.
    #[must_use]
    pub fn genesis() -> Self {
        Self(Sha256Hash::hash(FEDERATION_JOURNAL_GENESIS_VERSION).to_byte_array())
    }

    fn append(self, index: u64, operation_id: FederationOperationId) -> Self {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_JOURNAL_ENTRY_VERSION);
        append_hash_bytes(&mut transcript, &self.to_bytes());
        transcript.extend_from_slice(&index.to_be_bytes());
        append_hash_bytes(&mut transcript, &operation_id.to_bytes());
        Self(Sha256Hash::hash(&transcript).to_byte_array())
    }
}

impl fmt::Display for FederationJournalDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationJournalDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationJournalDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationJournalDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Stable hash of a consensus-accepted operation outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationOperationOutcomeHash([u8; 32]);

impl FederationOperationOutcomeHash {
    /// Create an outcome hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse an outcome hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidOperationOutcomeHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse an outcome hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this outcome hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this outcome hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn accepted_operation(
        index: u64,
        operation_id: FederationOperationId,
        previous_order_digest: FederationJournalDigest,
        order_digest: FederationJournalDigest,
        state_digest: FederationStateDigest,
    ) -> Self {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_OPERATION_OUTCOME_HASH_VERSION);
        transcript.extend_from_slice(&index.to_be_bytes());
        append_hash_bytes(&mut transcript, &operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &previous_order_digest.to_bytes());
        append_hash_bytes(&mut transcript, &order_digest.to_bytes());
        append_hash_bytes(&mut transcript, &state_digest.to_bytes());
        Self(Sha256Hash::hash(&transcript).to_byte_array())
    }
}

impl fmt::Display for FederationOperationOutcomeHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationOperationOutcomeHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationOperationOutcomeHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationOperationOutcomeHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Digest of consensus-visible federation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationStateDigest([u8; 32]);

impl FederationStateDigest {
    /// Create a state digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create a state digest from an order digest for status fixtures.
    #[must_use]
    pub fn from_order_digest(digest: FederationJournalDigest) -> Self {
        Self(digest.to_bytes())
    }

    /// Parse a state digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidStateDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a state digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this state digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this state digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Return the empty-state digest.
    #[must_use]
    pub fn genesis() -> Self {
        Self(Sha256Hash::hash(FEDERATION_STATE_GENESIS_VERSION).to_byte_array())
    }

    fn append(self, index: u64, operation_id: FederationOperationId) -> Self {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_STATE_ENTRY_VERSION);
        append_hash_bytes(&mut transcript, &self.to_bytes());
        transcript.extend_from_slice(&index.to_be_bytes());
        append_hash_bytes(&mut transcript, &operation_id.to_bytes());
        Self(Sha256Hash::hash(&transcript).to_byte_array())
    }

    fn append_materialized_row(self, row: &FederationMaterializedStateRow) -> Result<Self> {
        let row = serde_json::to_vec(row)?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_MATERIALIZED_STATE_ROW_VERSION);
        append_hash_bytes(&mut transcript, &self.to_bytes());
        append_hash_bytes(&mut transcript, &row);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationStateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationStateDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationStateDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationStateDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// One deterministic row included in a materialized federation state digest.
///
/// The row intentionally stores string fields instead of backend-specific Rust
/// structs. Database adapters can map mint quotes, melt quotes, proof states,
/// and blind signatures into this stable shape without making the digest depend
/// on SQL column order or debug formatting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FederationMaterializedStateRow {
    /// Logical table or state collection name.
    pub table: String,
    /// Stable row key inside the table.
    pub key: String,
    /// Deterministic row fields as string values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

impl FederationMaterializedStateRow {
    /// Create a materialized state row.
    pub fn new<T, K>(table: T, key: K) -> Self
    where
        T: Into<String>,
        K: Into<String>,
    {
        Self {
            table: table.into(),
            key: key.into(),
            fields: BTreeMap::new(),
        }
    }

    /// Add one deterministic field and return the row.
    #[must_use]
    pub fn with_field<N, V>(mut self, name: N, value: V) -> Self
    where
        N: Into<String>,
        V: Into<String>,
    {
        self.insert_field(name, value);
        self
    }

    /// Insert or replace one deterministic field.
    pub fn insert_field<N, V>(&mut self, name: N, value: V)
    where
        N: Into<String>,
        V: Into<String>,
    {
        self.fields.insert(name.into(), value.into());
    }

    /// Return a compact fingerprint for operator-side row diffing.
    ///
    /// The fingerprint exposes the logical table name plus digests of the row
    /// key and the complete row. It intentionally does not expose the key or
    /// field values, because those may contain invoices, payment proofs, proof
    /// identifiers, or blinded messages.
    pub fn fingerprint(&self) -> Result<FederationMaterializedStateRowFingerprint> {
        let mut key_transcript = Vec::new();
        append_hash_bytes(
            &mut key_transcript,
            FEDERATION_MATERIALIZED_STATE_ROW_KEY_FINGERPRINT_VERSION,
        );
        append_hash_bytes(&mut key_transcript, self.table.as_bytes());
        append_hash_bytes(&mut key_transcript, self.key.as_bytes());

        let row = serde_json::to_vec(self)?;
        let mut row_transcript = Vec::new();
        append_hash_bytes(
            &mut row_transcript,
            FEDERATION_MATERIALIZED_STATE_ROW_FINGERPRINT_VERSION,
        );
        append_hash_bytes(&mut row_transcript, &row);

        Ok(FederationMaterializedStateRowFingerprint {
            table: self.table.clone(),
            key_digest: FederationStateDigest::from_bytes(
                Sha256Hash::hash(&key_transcript).to_byte_array(),
            ),
            row_digest: FederationStateDigest::from_bytes(
                Sha256Hash::hash(&row_transcript).to_byte_array(),
            ),
        })
    }
}

/// Redacted fingerprint for one materialized-state row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FederationMaterializedStateRowFingerprint {
    /// Logical table or state collection name.
    pub table: String,
    /// Digest of the row table and key.
    pub key_digest: FederationStateDigest,
    /// Digest of the complete deterministic row.
    pub row_digest: FederationStateDigest,
}

/// Deterministic materialized-state snapshot used for federation convergence checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationMaterializedStateSnapshot {
    rows: Vec<FederationMaterializedStateRow>,
}

impl FederationMaterializedStateSnapshot {
    /// Create an empty materialized-state snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// Create a snapshot from rows. Rows are sorted during digest calculation.
    #[must_use]
    pub fn from_rows<I>(rows: I) -> Self
    where
        I: IntoIterator<Item = FederationMaterializedStateRow>,
    {
        Self {
            rows: rows.into_iter().collect(),
        }
    }

    /// Add one row to the snapshot.
    pub fn push(&mut self, row: FederationMaterializedStateRow) {
        self.rows.push(row);
    }

    /// Return snapshot rows in insertion order.
    #[must_use]
    pub fn rows(&self) -> &[FederationMaterializedStateRow] {
        &self.rows
    }

    /// Consume this snapshot and return its rows.
    #[must_use]
    pub fn into_rows(self) -> Vec<FederationMaterializedStateRow> {
        self.rows
    }

    /// Return compact fingerprints for all rows in deterministic order.
    pub fn row_fingerprints(&self) -> Result<Vec<FederationMaterializedStateRowFingerprint>> {
        let mut rows = self.rows.clone();
        rows.sort();
        rows.iter()
            .map(FederationMaterializedStateRow::fingerprint)
            .collect()
    }

    /// Compute a deterministic digest over all materialized rows.
    ///
    /// Rows are sorted by table, key, and field content before hashing, so
    /// backend iteration order cannot affect the digest. Duplicate rows remain
    /// visible to the digest because every sorted row is appended.
    pub fn digest(&self) -> Result<FederationStateDigest> {
        let mut rows = self.rows.clone();
        rows.sort();
        let mut digest = FederationStateDigest::genesis();
        for row in &rows {
            digest = digest.append_materialized_row(row)?;
        }
        Ok(digest)
    }
}

/// Local application status for an accepted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationOperationStatus {
    /// Operation has been accepted into the consensus order.
    Accepted,
    /// Local member is applying the operation to mint state.
    Applying,
    /// Local member applied the operation successfully.
    Applied,
    /// Local member rejected the operation during deterministic validation.
    Rejected,
    /// Local member failed while applying the operation and needs recovery.
    Failed,
}

/// Consensus evidence attached to a journal entry accepted from an ordering backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FederationJournalConsensusEvidence {
    /// Consensus session that finalized the item.
    pub session: u64,
    /// Global accepted item index assigned by the ordering backend.
    pub item_index: u64,
    /// Unit position in finalized order within the session.
    pub unit_finalization_index: u64,
    /// Member that submitted the finalized unit carrying this item.
    pub unit_submitting_member: MemberId,
    /// Submitting member's unit index within the session.
    pub unit_index: u64,
}

impl FederationJournalConsensusEvidence {
    /// Create accepted-item consensus evidence.
    #[must_use]
    pub fn new(
        session: u64,
        item_index: u64,
        unit_finalization_index: u64,
        unit_submitting_member: MemberId,
        unit_index: u64,
    ) -> Self {
        Self {
            session,
            item_index,
            unit_finalization_index,
            unit_submitting_member,
            unit_index,
        }
    }
}

/// One accepted federation operation journal entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationJournalEntry {
    /// Zero-based consensus order index.
    pub index: u64,
    /// Stable operation ID.
    pub operation_id: FederationOperationId,
    /// Versioned operation envelope.
    pub envelope: FederationOperationEnvelope,
    /// Quote-scoped conflict key when the operation has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_key: Option<FederationOperationConflictKey>,
    /// All quote-scoped conflict keys touched by the operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_keys: Vec<FederationOperationConflictKey>,
    /// Digest before this operation was appended.
    pub previous_order_digest: FederationJournalDigest,
    /// Digest after this operation was appended.
    pub order_digest: FederationJournalDigest,
    /// Stable hash of the accepted operation outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_outcome_hash: Option<FederationOperationOutcomeHash>,
    /// Consensus-visible state digest after this operation was appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_digest: Option<FederationStateDigest>,
    /// Ordering-backend evidence for this accepted operation, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus_evidence: Option<FederationJournalConsensusEvidence>,
    /// Local application status.
    pub status: FederationOperationStatus,
}

impl FederationJournalEntry {
    /// Return the expected accepted-operation outcome hash for this entry.
    #[must_use]
    pub fn expected_operation_outcome_hash(
        &self,
        state_digest: FederationStateDigest,
    ) -> FederationOperationOutcomeHash {
        FederationOperationOutcomeHash::accepted_operation(
            self.index,
            self.operation_id,
            self.previous_order_digest,
            self.order_digest,
            state_digest,
        )
    }

    fn validate_outcome_metadata(
        &self,
        expected_state_digest: FederationStateDigest,
    ) -> Result<()> {
        if let Some(found) = self.state_digest {
            if found != expected_state_digest {
                return Err(FederationError::JournalStateDigestMismatch {
                    index: self.index,
                    expected: expected_state_digest,
                    found,
                });
            }
        }

        if let Some(found) = self.operation_outcome_hash {
            let expected = self.expected_operation_outcome_hash(expected_state_digest);
            if found != expected {
                return Err(FederationError::JournalOutcomeHashMismatch {
                    index: self.index,
                    expected: expected.to_string(),
                    found: found.to_string(),
                });
            }
        }

        Ok(())
    }
}

/// Durable checkpoint for consensus-visible materialized federation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FederationStateDigestCheckpoint {
    /// Accepted operation index that this checkpoint certifies.
    pub operation_index: u64,
    /// Accepted operation id at `operation_index`.
    pub operation_id: FederationOperationId,
    /// Accepted journal-order digest at `operation_index`.
    pub order_digest: FederationJournalDigest,
    /// Materialized state digest after applying the checkpointed operation.
    pub state_digest: FederationStateDigest,
}

impl FederationStateDigestCheckpoint {
    /// Build a digest checkpoint for an accepted journal entry.
    #[must_use]
    pub fn new(entry: &FederationJournalEntry, state_digest: FederationStateDigest) -> Self {
        Self {
            operation_index: entry.index,
            operation_id: entry.operation_id,
            order_digest: entry.order_digest,
            state_digest,
        }
    }
}

/// Durable local sync status labels for one federation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationMemberStatusKind {
    /// The member is caught up and ready to process public federation requests.
    Healthy,
    /// The member has observed that a peer is ahead.
    Lagging,
    /// The member is actively applying journal entries from a peer.
    CatchingUp,
    /// The member is waiting for signed session-outcome evidence after catch-up.
    VerifyingSessionOutcome,
    /// The member is waiting for materialized state digests to converge.
    MaterializedStateMismatch,
    /// The member halted federation public request handling.
    Halted,
}

impl FederationMemberStatusKind {
    /// Return the stable database/API label for this status.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Lagging => "lagging",
            Self::CatchingUp => "catching_up",
            Self::VerifyingSessionOutcome => "verifying_session_outcome",
            Self::MaterializedStateMismatch => "materialized_state_mismatch",
            Self::Halted => "halted",
        }
    }

    /// Parse a status from its stable database/API label.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "healthy" => Some(Self::Healthy),
            "lagging" => Some(Self::Lagging),
            "catching_up" => Some(Self::CatchingUp),
            "verifying_session_outcome" => Some(Self::VerifyingSessionOutcome),
            "materialized_state_mismatch" => Some(Self::MaterializedStateMismatch),
            "halted" => Some(Self::Halted),
            _ => None,
        }
    }
}

impl fmt::Display for FederationMemberStatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Durable local sync status snapshot for one federation member.
///
/// This record is local operator/admin state. It is intentionally excluded from
/// consensus materialized state digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationMemberStatusSnapshot {
    /// Local member whose status is being recorded.
    pub member_id: MemberId,
    /// Local sync status label.
    pub status: FederationMemberStatusKind,
    /// Peer associated with the status, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<MemberId>,
    /// Local operation count recorded for lagging statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_operation_count: Option<u64>,
    /// Peer operation count recorded for lagging statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_operation_count: Option<u64>,
    /// Operation count associated with verification or digest convergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_count: Option<u64>,
    /// Redacted operator-facing status message, when useful for restart/status output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Unix time when the snapshot was recorded.
    pub updated_time: u64,
}

/// Outcome of appending an operation envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "entry", rename_all = "snake_case")]
pub enum FederationJournalAppendOutcome {
    /// The operation was newly appended.
    Appended(FederationJournalEntry),
    /// The operation was already present and was returned idempotently.
    Duplicate(FederationJournalEntry),
}

impl FederationJournalAppendOutcome {
    /// Return the journal entry for either outcome.
    #[must_use]
    pub fn entry(&self) -> &FederationJournalEntry {
        match self {
            Self::Appended(entry) | Self::Duplicate(entry) => entry,
        }
    }
}

/// Deterministic in-memory journal used until durable storage is wired.
#[derive(Debug, Clone)]
pub struct InMemoryFederationJournal {
    entries: Vec<FederationJournalEntry>,
    operation_indices: BTreeMap<FederationOperationId, usize>,
    conflict_keys: BTreeMap<String, FederationOperationId>,
    latest_order_digest: FederationJournalDigest,
}

impl Default for InMemoryFederationJournal {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryFederationJournal {
    /// Create an empty in-memory journal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            operation_indices: BTreeMap::new(),
            conflict_keys: BTreeMap::new(),
            latest_order_digest: FederationJournalDigest::genesis(),
        }
    }

    /// Number of accepted operations in the journal.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return true when the journal has no accepted operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return the latest order digest.
    #[must_use]
    pub fn latest_order_digest(&self) -> FederationJournalDigest {
        self.latest_order_digest
    }

    /// Return a deterministic digest for consensus-visible state.
    ///
    /// This digest is derived from the accepted operation sequence and
    /// intentionally excludes local application status. Durable DB-state
    /// digests can extend this API without changing operator status payloads.
    #[must_use]
    pub fn state_digest(&self) -> FederationStateDigest {
        let mut digest = FederationStateDigest::genesis();
        for entry in &self.entries {
            digest = digest.append(entry.index, entry.operation_id);
        }
        digest
    }

    /// Return the next journal index that would be appended locally.
    #[must_use]
    pub fn next_index(&self) -> u64 {
        self.entries.len() as u64
    }

    /// Return the digest after the entry immediately before `next_index`.
    #[must_use]
    pub fn order_digest_before(&self, next_index: u64) -> Option<FederationJournalDigest> {
        if next_index == 0 {
            return Some(FederationJournalDigest::genesis());
        }

        self.entries
            .get(next_index.saturating_sub(1) as usize)
            .map(|entry| entry.order_digest)
    }

    /// Return all entries.
    #[must_use]
    pub fn entries(&self) -> &[FederationJournalEntry] {
        &self.entries
    }

    /// Return entries whose index is greater than or equal to `next_index`.
    #[must_use]
    pub fn entries_from(&self, next_index: u64) -> Vec<FederationJournalEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.index >= next_index)
            .cloned()
            .collect()
    }

    /// Rebuild a journal from persisted entries after validating the digest chain.
    pub fn from_entries(entries: Vec<FederationJournalEntry>) -> Result<Self> {
        let mut journal = Self::new();

        for entry in entries {
            let expected_index = journal.next_index();
            if entry.index != expected_index {
                return Err(FederationError::CatchUpIndexMismatch {
                    expected: expected_index,
                    found: entry.index,
                });
            }

            let expected_operation_id = entry.envelope.operation_id()?;
            if expected_operation_id != entry.operation_id {
                return Err(FederationError::CatchUpOperationIdMismatch {
                    index: entry.index,
                    expected: expected_operation_id,
                    found: entry.operation_id,
                });
            }

            if entry.previous_order_digest != journal.latest_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: entry.index,
                    expected: journal.latest_order_digest,
                    found: entry.previous_order_digest,
                });
            }

            let expected_order_digest = journal
                .latest_order_digest
                .append(entry.index, entry.operation_id);
            if entry.order_digest != expected_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: entry.index.saturating_add(1),
                    expected: expected_order_digest,
                    found: entry.order_digest,
                });
            }
            let expected_state_digest =
                journal.state_digest_after_append(entry.index, entry.operation_id);
            entry.validate_outcome_metadata(expected_state_digest)?;

            let conflict_keys = entry.envelope.operation.conflict_keys();
            let conflict_key = conflict_keys.first().cloned();
            if entry.conflict_key != conflict_key || entry.conflict_keys != conflict_keys {
                return Err(FederationError::JournalConflictKeysMismatch { index: entry.index });
            }

            if journal.operation_indices.contains_key(&entry.operation_id) {
                return Err(FederationError::ConflictingOperation {
                    conflict_key: format!("operation_id:{}", entry.operation_id),
                    existing_operation_id: entry.operation_id.to_string(),
                    attempted_operation_id: entry.operation_id.to_string(),
                });
            }

            let conflict_key_labels = conflict_keys
                .iter()
                .map(canonical_json_string)
                .collect::<Result<Vec<_>>>()?;
            for conflict_key_label in &conflict_key_labels {
                if let Some(existing_operation_id) = journal.conflict_keys.get(conflict_key_label) {
                    return Err(FederationError::ConflictingOperation {
                        conflict_key: conflict_key_label.clone(),
                        existing_operation_id: existing_operation_id.to_string(),
                        attempted_operation_id: entry.operation_id.to_string(),
                    });
                }
            }

            journal
                .operation_indices
                .insert(entry.operation_id, entry.index as usize);
            for conflict_key_label in conflict_key_labels {
                journal
                    .conflict_keys
                    .insert(conflict_key_label, entry.operation_id);
            }
            journal.latest_order_digest = entry.order_digest;
            journal.entries.push(entry);
        }

        Ok(journal)
    }

    /// Return an entry by operation ID.
    #[must_use]
    pub fn entry_by_operation_id(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<&FederationJournalEntry> {
        self.operation_indices
            .get(&operation_id)
            .and_then(|index| self.entries.get(*index))
    }

    /// Append an accepted operation envelope or return an idempotent duplicate.
    pub fn append(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationJournalAppendOutcome> {
        self.append_inner(envelope, None)
    }

    /// Append an accepted operation envelope with consensus evidence.
    pub fn append_with_consensus_evidence(
        &mut self,
        envelope: FederationOperationEnvelope,
        evidence: FederationJournalConsensusEvidence,
    ) -> Result<FederationJournalAppendOutcome> {
        self.append_inner(envelope, Some(evidence))
    }

    fn append_inner(
        &mut self,
        envelope: FederationOperationEnvelope,
        consensus_evidence: Option<FederationJournalConsensusEvidence>,
    ) -> Result<FederationJournalAppendOutcome> {
        let operation_id = envelope.operation_id()?;
        if let Some(existing_index) = self.operation_indices.get(&operation_id) {
            let entry = self
                .entries
                .get(*existing_index)
                .expect("operation index points to an entry")
                .clone();
            return Ok(FederationJournalAppendOutcome::Duplicate(entry));
        }

        envelope.operation.balance_audit()?;
        let conflict_keys = envelope.operation.conflict_keys();
        let conflict_key = conflict_keys.first().cloned();
        let conflict_key_labels = conflict_keys
            .iter()
            .map(canonical_json_string)
            .collect::<Result<Vec<_>>>()?;

        for conflict_key_label in &conflict_key_labels {
            if let Some(existing_operation_id) = self.conflict_keys.get(conflict_key_label) {
                return Err(FederationError::ConflictingOperation {
                    conflict_key: conflict_key_label.clone(),
                    existing_operation_id: existing_operation_id.to_string(),
                    attempted_operation_id: operation_id.to_string(),
                });
            }
        }

        let index = self.entries.len() as u64;
        let previous_order_digest = self.latest_order_digest;
        let order_digest = previous_order_digest.append(index, operation_id);
        let state_digest = self.state_digest_after_append(index, operation_id);
        let operation_outcome_hash = FederationOperationOutcomeHash::accepted_operation(
            index,
            operation_id,
            previous_order_digest,
            order_digest,
            state_digest,
        );
        let entry = FederationJournalEntry {
            index,
            operation_id,
            envelope,
            conflict_key,
            conflict_keys,
            previous_order_digest,
            order_digest,
            operation_outcome_hash: Some(operation_outcome_hash),
            state_digest: Some(state_digest),
            consensus_evidence,
            status: FederationOperationStatus::Accepted,
        };

        self.entries.push(entry.clone());
        self.operation_indices.insert(operation_id, index as usize);
        for conflict_key_label in conflict_key_labels {
            self.conflict_keys.insert(conflict_key_label, operation_id);
        }
        self.latest_order_digest = order_digest;

        Ok(FederationJournalAppendOutcome::Appended(entry))
    }

    fn state_digest_after_append(
        &self,
        index: u64,
        operation_id: FederationOperationId,
    ) -> FederationStateDigest {
        self.state_digest().append(index, operation_id)
    }

    /// Validate and append one catch-up entry from a federation peer.
    ///
    /// The peer's local application status is intentionally ignored. Catch-up
    /// synchronizes accepted consensus order; each member applies that order to
    /// its own mint state independently.
    pub fn append_catch_up_entry(
        &mut self,
        peer_entry: &FederationJournalEntry,
    ) -> Result<FederationJournalEntry> {
        let expected_index = self.next_index();
        if peer_entry.index != expected_index {
            return Err(FederationError::CatchUpIndexMismatch {
                expected: expected_index,
                found: peer_entry.index,
            });
        }

        let expected_operation_id = peer_entry.envelope.operation_id()?;
        if expected_operation_id != peer_entry.operation_id {
            return Err(FederationError::CatchUpOperationIdMismatch {
                index: peer_entry.index,
                expected: expected_operation_id,
                found: peer_entry.operation_id,
            });
        }

        if peer_entry.previous_order_digest != self.latest_order_digest {
            return Err(FederationError::CatchUpDigestMismatch {
                index: peer_entry.index,
                expected: self.latest_order_digest,
                found: peer_entry.previous_order_digest,
            });
        }

        let expected_order_digest = self
            .latest_order_digest
            .append(peer_entry.index, peer_entry.operation_id);
        if peer_entry.order_digest != expected_order_digest {
            return Err(FederationError::CatchUpDigestMismatch {
                index: peer_entry.index.saturating_add(1),
                expected: expected_order_digest,
                found: peer_entry.order_digest,
            });
        }

        let expected_state_digest =
            self.state_digest_after_append(peer_entry.index, peer_entry.operation_id);
        peer_entry.validate_outcome_metadata(expected_state_digest)?;

        match self.append_inner(peer_entry.envelope.clone(), peer_entry.consensus_evidence)? {
            FederationJournalAppendOutcome::Appended(entry) => Ok(entry),
            FederationJournalAppendOutcome::Duplicate(entry) => {
                Err(FederationError::CatchUpIndexMismatch {
                    expected: expected_index,
                    found: entry.index,
                })
            }
        }
    }

    /// Update local application status for an accepted operation.
    pub fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        let index = self.operation_indices.get(&operation_id).ok_or_else(|| {
            FederationError::UnknownOperation {
                operation_id: operation_id.to_string(),
            }
        })?;
        let entry = self
            .entries
            .get_mut(*index)
            .expect("operation index points to an entry");
        entry.status = status;
        Ok(entry.clone())
    }
}

fn canonical_json_string<T>(value: &T) -> Result<String>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_string(&value)?)
}

fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::{
        Amount, BlindedMessage, CurrencyUnit, Id, MeltQuoteCustomRequest, MintQuoteBolt11Request,
        MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse, MintRequest, PaymentMethod,
        PublicKey,
    };

    fn federation_id() -> super::super::FederationId {
        super::super::FederationId::from_bytes([9; 32])
    }

    fn keyset_id() -> Id {
        Id::from_str("00bfa73302d12ffd").expect("keyset id")
    }

    fn output(amount: u64, hex: &str) -> BlindedMessage {
        BlindedMessage::new(
            Amount::from(amount),
            keyset_id(),
            PublicKey::from_hex(hex).expect("public key"),
        )
    }

    fn mint_envelope(quote: &str, output_hex: &str) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::Mint(super::super::FederationMintOperation {
                request: super::super::FederationMintRequest::Single(MintRequest {
                    quote: quote.to_string(),
                    outputs: vec![output(1, output_hex)],
                    signature: None,
                }),
            }),
        )
    }

    fn mint_quote_envelope(amount: u64) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MintQuote(
                super::super::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                        amount: Amount::from(amount),
                        unit: CurrencyUnit::Sat,
                        description: None,
                        pubkey: None,
                    }),
                    response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                        quote: "quote-1".to_string(),
                        request: "invoice-quote-1".to_string(),
                        amount: Some(Amount::from(amount)),
                        unit: Some(CurrencyUnit::Sat),
                        state: crate::MintQuoteState::Unpaid,
                        expiry: None,
                        pubkey: None,
                    }),
                },
            ),
        )
    }

    #[test]
    fn journal_appends_operations_with_monotonic_indices_and_digest() {
        let first = mint_quote_envelope(1);
        let second = FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MeltQuote(
                super::super::FederationMeltQuoteOperation {
                    quote: "melt-quote".to_string(),
                    request: crate::MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
                        method: "custom".to_string(),
                        request: "payreq".to_string(),
                        unit: CurrencyUnit::Sat,
                        extra: serde_json::Value::Null,
                    }),
                },
            ),
        );
        let mut journal = InMemoryFederationJournal::new();
        let genesis = journal.latest_order_digest();
        let first_entry = journal.append(first).expect("append first").entry().clone();
        let second_entry = journal
            .append(second)
            .expect("append second")
            .entry()
            .clone();

        assert_eq!(first_entry.index, 0);
        assert_eq!(first_entry.previous_order_digest, genesis);
        assert_eq!(second_entry.index, 1);
        assert_eq!(second_entry.previous_order_digest, first_entry.order_digest);
        assert_eq!(journal.latest_order_digest(), second_entry.order_digest);
        assert_eq!(
            journal.entries_from(1)[0].operation_id,
            second_entry.operation_id
        );
    }

    #[test]
    fn journal_records_outcome_hash_and_state_digest_for_new_entries() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append")
            .entry()
            .clone();

        let state_digest = entry.state_digest.expect("state digest");
        let outcome_hash = entry.operation_outcome_hash.expect("outcome hash");

        assert_eq!(state_digest, journal.state_digest());
        assert_eq!(
            outcome_hash,
            entry.expected_operation_outcome_hash(state_digest)
        );
        InMemoryFederationJournal::from_entries(vec![entry]).expect("metadata replays");
    }

    #[test]
    fn journal_rejects_tampered_outcome_hash_and_state_digest_on_replay() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append")
            .entry()
            .clone();

        let mut tampered_outcome_hash = entry.clone();
        tampered_outcome_hash.operation_outcome_hash =
            Some(FederationOperationOutcomeHash::from_bytes([42; 32]));
        assert!(matches!(
            InMemoryFederationJournal::from_entries(vec![tampered_outcome_hash]),
            Err(FederationError::JournalOutcomeHashMismatch { .. })
        ));

        let mut tampered_state_digest = entry;
        tampered_state_digest.state_digest = Some(FederationStateDigest::from_bytes([24; 32]));
        assert!(matches!(
            InMemoryFederationJournal::from_entries(vec![tampered_state_digest]),
            Err(FederationError::JournalStateDigestMismatch { .. })
        ));
    }

    #[test]
    fn journal_rejects_conflicts_and_accepts_duplicates() {
        let mut journal = InMemoryFederationJournal::new();
        let first = mint_envelope(
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let duplicate = first.clone();
        let conflict = mint_envelope(
            "quote-1",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let first_entry = journal.append(first).expect("append first").entry().clone();
        let duplicate_entry = journal
            .append(duplicate)
            .expect("append duplicate")
            .entry()
            .clone();

        assert_eq!(journal.len(), 1);
        assert_eq!(duplicate_entry.operation_id, first_entry.operation_id);
        assert!(matches!(
            journal.append(conflict),
            Err(FederationError::ConflictingOperation { .. })
        ));
        assert_eq!(journal.len(), 1);
    }

    #[test]
    fn journal_updates_status_without_changing_order_digest() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append")
            .entry()
            .clone();
        let digest = journal.latest_order_digest();
        let state_digest = journal.state_digest();
        let updated = journal
            .update_status(entry.operation_id, FederationOperationStatus::Applied)
            .expect("status update");

        assert_eq!(updated.status, FederationOperationStatus::Applied);
        assert_eq!(journal.latest_order_digest(), digest);
        assert_eq!(journal.state_digest(), state_digest);
        assert!(matches!(
            journal.update_status(
                FederationOperationId::from_bytes([42; 32]),
                FederationOperationStatus::Applied
            ),
            Err(FederationError::UnknownOperation { .. })
        ));
    }

    #[test]
    fn journal_state_digest_matches_for_same_accepted_log() {
        let mut first = InMemoryFederationJournal::new();
        let mut second = InMemoryFederationJournal::new();
        first.append(mint_quote_envelope(1)).expect("first quote");
        first
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("first mint");
        second.append(mint_quote_envelope(1)).expect("second quote");
        second
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("second mint");

        assert_eq!(first.state_digest(), second.state_digest());
        assert_ne!(first.state_digest(), FederationStateDigest::genesis());
    }

    #[test]
    fn materialized_state_digest_is_order_independent() {
        let first = FederationMaterializedStateSnapshot::from_rows([
            FederationMaterializedStateRow::new("proofs", "proof-y-1")
                .with_field("amount", "1")
                .with_field("state", "SPENT"),
            FederationMaterializedStateRow::new("mint_quotes", "quote-1")
                .with_field("amount", "1")
                .with_field("state", "ISSUED"),
        ]);
        let second = FederationMaterializedStateSnapshot::from_rows([
            FederationMaterializedStateRow::new("mint_quotes", "quote-1")
                .with_field("state", "ISSUED")
                .with_field("amount", "1"),
            FederationMaterializedStateRow::new("proofs", "proof-y-1")
                .with_field("state", "SPENT")
                .with_field("amount", "1"),
        ]);

        assert_eq!(
            first.digest().expect("first digest"),
            second.digest().expect("second digest")
        );
        assert_ne!(
            first.digest().expect("first digest"),
            FederationStateDigest::genesis()
        );
    }

    #[test]
    fn materialized_state_row_fingerprints_are_redacted_and_ordered() {
        let snapshot = FederationMaterializedStateSnapshot::from_rows([
            FederationMaterializedStateRow::new("proof_states", "secret-proof-y")
                .with_field("state", "SPENT"),
            FederationMaterializedStateRow::new("mint_quotes", "secret-quote-id")
                .with_field("state", "ISSUED"),
        ]);

        let fingerprints = snapshot
            .row_fingerprints()
            .expect("materialized row fingerprints");
        let json = serde_json::to_string(&fingerprints).expect("fingerprint json");

        assert_eq!(fingerprints.len(), 2);
        assert_eq!(fingerprints[0].table, "mint_quotes");
        assert_eq!(fingerprints[1].table, "proof_states");
        assert!(!json.contains("secret-proof-y"));
        assert!(!json.contains("secret-quote-id"));
    }

    #[test]
    fn materialized_state_row_fingerprint_changes_when_field_changes() {
        let pending = FederationMaterializedStateRow::new("proof_states", "proof-y-1")
            .with_field("state", "PENDING")
            .fingerprint()
            .expect("pending fingerprint");
        let spent = FederationMaterializedStateRow::new("proof_states", "proof-y-1")
            .with_field("state", "SPENT")
            .fingerprint()
            .expect("spent fingerprint");

        assert_eq!(pending.table, spent.table);
        assert_eq!(pending.key_digest, spent.key_digest);
        assert_ne!(pending.row_digest, spent.row_digest);
    }

    #[test]
    fn materialized_state_digest_changes_when_row_field_changes() {
        let pending =
            FederationMaterializedStateSnapshot::from_rows([FederationMaterializedStateRow::new(
                "proofs",
                "proof-y-1",
            )
            .with_field("amount", "1")
            .with_field("state", "PENDING")]);
        let spent =
            FederationMaterializedStateSnapshot::from_rows([FederationMaterializedStateRow::new(
                "proofs",
                "proof-y-1",
            )
            .with_field("amount", "1")
            .with_field("state", "SPENT")]);

        assert_ne!(
            pending.digest().expect("pending digest"),
            spent.digest().expect("spent digest")
        );
    }

    #[test]
    fn materialized_state_digest_is_duplicate_sensitive() {
        let one =
            FederationMaterializedStateSnapshot::from_rows([FederationMaterializedStateRow::new(
                "blind_signatures",
                "b-1",
            )
            .with_field("amount", "1")]);
        let duplicated = FederationMaterializedStateSnapshot::from_rows([
            FederationMaterializedStateRow::new("blind_signatures", "b-1")
                .with_field("amount", "1"),
            FederationMaterializedStateRow::new("blind_signatures", "b-1")
                .with_field("amount", "1"),
        ]);

        assert_ne!(
            one.digest().expect("one digest"),
            duplicated.digest().expect("duplicated digest")
        );
    }

    #[test]
    fn journal_digest_serializes_as_hex() {
        let digest = FederationJournalDigest::genesis();
        let json = serde_json::to_string(&digest).expect("digest json");

        assert_eq!(json.len(), 66);
        assert_eq!(
            serde_json::from_str::<FederationJournalDigest>(&json).expect("digest from json"),
            digest
        );
        assert_eq!(
            FederationJournalDigest::from_slice(&digest.to_bytes()).expect("digest from bytes"),
            digest
        );
    }

    #[test]
    fn state_digest_serializes_as_hex() {
        let digest = FederationStateDigest::genesis();
        let json = serde_json::to_string(&digest).expect("digest json");

        assert_eq!(json.len(), 66);
        assert_eq!(
            serde_json::from_str::<FederationStateDigest>(&json).expect("digest from json"),
            digest
        );
        assert_eq!(
            FederationStateDigest::from_slice(&digest.to_bytes()).expect("digest from bytes"),
            digest
        );
    }

    #[test]
    fn journal_allows_same_quote_payment_state_only_when_identical() {
        let observation = FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MeltQuotePayment(
                super::super::FederationMeltQuotePaymentObservation {
                    quote: "melt-quote".to_string(),
                    observer: super::super::MemberId::new(1).expect("member id"),
                    method: PaymentMethod::Custom("custom".to_string()),
                    state: crate::MeltQuoteState::Paid,
                    amount: Amount::from(10),
                    fee_reserve: Amount::from(1),
                    payment_id: None,
                    payment_proof: Some("preimage".to_string()),
                    failure_reason: None,
                    observed_time: None,
                },
            ),
        );
        let mut changed_observation = observation.clone();
        if let super::super::FederationOperation::MeltQuotePayment(data) =
            &mut changed_observation.operation
        {
            data.payment_proof = Some("different-preimage".to_string());
        }

        let mut journal = InMemoryFederationJournal::new();
        journal.append(observation).expect("append observation");

        assert!(matches!(
            journal.append(changed_observation),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn journal_allows_same_quote_payment_state_from_different_observers() {
        let observation = FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MeltQuotePayment(
                super::super::FederationMeltQuotePaymentObservation {
                    quote: "melt-quote".to_string(),
                    observer: super::super::MemberId::new(1).expect("member id"),
                    method: PaymentMethod::Custom("custom".to_string()),
                    state: crate::MeltQuoteState::Paid,
                    amount: Amount::from(10),
                    fee_reserve: Amount::from(1),
                    payment_id: None,
                    payment_proof: Some("preimage".to_string()),
                    failure_reason: None,
                    observed_time: None,
                },
            ),
        );
        let mut second_observer = observation.clone();
        if let super::super::FederationOperation::MeltQuotePayment(data) =
            &mut second_observer.operation
        {
            data.observer = super::super::MemberId::new(2).expect("member id");
        }

        let mut journal = InMemoryFederationJournal::new();
        journal
            .append(observation)
            .expect("append first observation");
        journal
            .append(second_observer)
            .expect("append second observer");

        assert_eq!(journal.entries().len(), 2);
    }
}
