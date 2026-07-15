//! Deterministic federation journal.
//!
//! This in-memory journal models consensus-accepted operation order, duplicate
//! handling, conflict detection, status updates, order digests, and catch-up
//! ranges. Durable storage backends persist the same entry shape.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, HashMap};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationError, FederationOperation, FederationOperationConflictKey,
    FederationOperationEnvelope, FederationOperationId, MemberId, Result,
};
use crate::error::ErrorResponse;

const FEDERATION_JOURNAL_GENESIS_VERSION: &[u8] = b"cdk-federation-journal-genesis-v1";
const FEDERATION_JOURNAL_ENTRY_VERSION: &[u8] = b"cdk-federation-journal-entry-v1";
const FEDERATION_MATERIALIZED_STATE_GENESIS_VERSION: &[u8] = b"cdk-federation-state-genesis-v1";
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

    pub(crate) fn append(self, index: u64, operation_id: FederationOperationId) -> Self {
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

/// Digest of the deterministic materialized federation database state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationMaterializedStateDigest([u8; 32]);

impl FederationMaterializedStateDigest {
    /// Create a materialized-state digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a materialized-state digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidMaterializedStateDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a materialized-state digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this materialized-state digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this materialized-state digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Return the empty materialized-state digest.
    #[must_use]
    pub fn genesis() -> Self {
        Self(Sha256Hash::hash(FEDERATION_MATERIALIZED_STATE_GENESIS_VERSION).to_byte_array())
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

impl fmt::Display for FederationMaterializedStateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationMaterializedStateDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationMaterializedStateDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationMaterializedStateDigest {
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
            key_digest: FederationMaterializedStateDigest::from_bytes(
                Sha256Hash::hash(&key_transcript).to_byte_array(),
            ),
            row_digest: FederationMaterializedStateDigest::from_bytes(
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
    pub key_digest: FederationMaterializedStateDigest,
    /// Digest of the complete deterministic row.
    pub row_digest: FederationMaterializedStateDigest,
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
    pub fn digest(&self) -> Result<FederationMaterializedStateDigest> {
        let mut rows = self.rows.clone();
        rows.sort();
        let mut digest = FederationMaterializedStateDigest::genesis();
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

impl FederationOperationStatus {
    /// Stable lowercase label for operator output and error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for FederationOperationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Legal post-consensus application state for one ordered operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationApplicationState {
    /// Operation is accepted and waiting for local application.
    Accepted {},
    /// Operation is currently being applied locally.
    Applying {},
    /// Operation was applied successfully.
    Applied {},
    /// Operation failed for a retryable or ambiguous reason.
    Failed {},
    /// Consensus ordered a conflicting operation that cannot be applied locally.
    ConsensusRejected {},
    /// Operation was deterministically rejected with its standard Cashu error.
    Rejected {
        /// Standard Cashu response returned on replay and wallet fanout.
        response: ErrorResponse,
    },
}

impl FederationApplicationState {
    /// Construct a legal state from its durable SQL columns.
    pub fn from_parts(
        index: u64,
        status: FederationOperationStatus,
        rejection: Option<ErrorResponse>,
    ) -> Result<Self> {
        match (status, rejection) {
            (FederationOperationStatus::Accepted, None) => Ok(Self::Accepted {}),
            (FederationOperationStatus::Applying, None) => Ok(Self::Applying {}),
            (FederationOperationStatus::Applied, None) => Ok(Self::Applied {}),
            (FederationOperationStatus::Failed, None) => Ok(Self::Failed {}),
            (FederationOperationStatus::Rejected, None) => Ok(Self::ConsensusRejected {}),
            (FederationOperationStatus::Rejected, Some(response)) => {
                Ok(Self::Rejected { response })
            }
            _ => Err(FederationError::JournalRejectionStateMismatch { index }),
        }
    }

    /// Construct a state that carries no deterministic business rejection.
    #[must_use]
    pub fn from_status(status: FederationOperationStatus) -> Self {
        match status {
            FederationOperationStatus::Accepted => Self::Accepted {},
            FederationOperationStatus::Applying => Self::Applying {},
            FederationOperationStatus::Applied => Self::Applied {},
            FederationOperationStatus::Rejected => Self::ConsensusRejected {},
            FederationOperationStatus::Failed => Self::Failed {},
        }
    }

    /// Stable status label represented by this state.
    #[must_use]
    pub fn status(&self) -> FederationOperationStatus {
        match self {
            Self::Accepted {} => FederationOperationStatus::Accepted,
            Self::Applying {} => FederationOperationStatus::Applying,
            Self::Applied {} => FederationOperationStatus::Applied,
            Self::Failed {} => FederationOperationStatus::Failed,
            Self::ConsensusRejected {} | Self::Rejected { .. } => {
                FederationOperationStatus::Rejected
            }
        }
    }

    /// Standard Cashu rejection carried by this state, when rejected.
    #[must_use]
    pub fn rejection(&self) -> Option<&ErrorResponse> {
        match self {
            Self::Rejected { response } => Some(response),
            _ => None,
        }
    }
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
    /// All quote-scoped conflict keys touched by the operation.
    pub conflict_keys: Vec<FederationOperationConflictKey>,
    /// Digest before this operation was appended.
    pub previous_order_digest: FederationJournalDigest,
    /// Digest after this operation was appended.
    pub order_digest: FederationJournalDigest,
    /// Ordering-backend evidence for this accepted operation, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus_evidence: Option<FederationJournalConsensusEvidence>,
    /// Mutable post-consensus application state, excluded from accepted-order hashing.
    pub application: FederationApplicationState,
}

/// Scalar metadata for the latest durable federation journal entry.
///
/// Storage backends expose this separately so peer order checks do not need to
/// load and deserialize the full accepted operation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationJournalTip {
    /// Zero-based index of the latest accepted operation.
    pub index: u64,
    /// Accepted-order digest after the latest operation.
    pub order_digest: FederationJournalDigest,
    /// Local application status of the latest accepted operation.
    pub status: FederationOperationStatus,
}

impl FederationJournalEntry {
    /// Local application status.
    #[must_use]
    pub fn status(&self) -> FederationOperationStatus {
        self.application.status()
    }

    /// Standard Cashu deterministic rejection, when present.
    #[must_use]
    pub fn rejection(&self) -> Option<&ErrorResponse> {
        self.application.rejection()
    }
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
    conflict_keys: HashMap<FederationOperationConflictKey, FederationOperationId>,
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
            conflict_keys: HashMap::new(),
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

            let conflict_keys = entry.envelope.operation.conflict_keys();
            if entry.conflict_keys != conflict_keys {
                return Err(FederationError::JournalConflictKeysMismatch { index: entry.index });
            }

            if journal.operation_indices.contains_key(&entry.operation_id) {
                return Err(FederationError::ConflictingOperation {
                    conflict_key: format!("operation_id:{}", entry.operation_id),
                    existing_operation_id: entry.operation_id.to_string(),
                    attempted_operation_id: entry.operation_id.to_string(),
                });
            }

            for conflict_key in &conflict_keys {
                if application_reserves_conflict_keys(&entry.application) {
                    if let Some(existing_operation_id) = journal.conflict_keys.get(conflict_key) {
                        let finalized_mint_conflict = entry.consensus_evidence.is_some()
                            && matches!(&entry.envelope.operation, FederationOperation::Mint(_));
                        if !finalized_mint_conflict {
                            return Err(FederationError::ConflictingOperation {
                                conflict_key: serde_json::to_string(conflict_key)?,
                                existing_operation_id: existing_operation_id.to_string(),
                                attempted_operation_id: entry.operation_id.to_string(),
                            });
                        }
                    }
                } else if !journal.conflict_keys.contains_key(conflict_key) {
                    return Err(FederationError::ConflictingOperation {
                        conflict_key: serde_json::to_string(conflict_key)?,
                        existing_operation_id: "none".to_string(),
                        attempted_operation_id: entry.operation_id.to_string(),
                    });
                }
            }

            journal
                .operation_indices
                .insert(entry.operation_id, entry.index as usize);
            if application_reserves_conflict_keys(&entry.application) {
                for conflict_key in conflict_keys {
                    journal
                        .conflict_keys
                        .entry(conflict_key)
                        .or_insert(entry.operation_id);
                }
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

    /// Append one finalized operation envelope with consensus evidence.
    ///
    /// Unlike public admission, finalized ordering must be replayable even when
    /// a later operation conflicts with a previously accepted one. Such
    /// conflicts are persisted as deterministic rejected entries instead of
    /// aborting journal replay.
    pub fn append_finalized_with_consensus_evidence(
        &mut self,
        envelope: FederationOperationEnvelope,
        evidence: FederationJournalConsensusEvidence,
    ) -> Result<FederationJournalAppendOutcome> {
        self.append_finalized_inner(envelope, evidence)
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

        for conflict_key in &conflict_keys {
            if let Some(existing_operation_id) = self.conflict_keys.get(conflict_key) {
                return Err(FederationError::ConflictingOperation {
                    conflict_key: serde_json::to_string(conflict_key)?,
                    existing_operation_id: existing_operation_id.to_string(),
                    attempted_operation_id: operation_id.to_string(),
                });
            }
        }

        let entry = self.push_entry(
            envelope,
            operation_id,
            conflict_keys,
            consensus_evidence,
            FederationOperationStatus::Accepted,
        );

        Ok(FederationJournalAppendOutcome::Appended(entry))
    }

    fn append_finalized_inner(
        &mut self,
        envelope: FederationOperationEnvelope,
        consensus_evidence: FederationJournalConsensusEvidence,
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
        let has_conflict = conflict_keys
            .iter()
            .any(|conflict_key| self.conflict_keys.contains_key(conflict_key));
        let mint_operation = matches!(&envelope.operation, FederationOperation::Mint(_));
        let status = if has_conflict && !mint_operation {
            FederationOperationStatus::Rejected
        } else {
            FederationOperationStatus::Accepted
        };

        let entry = self.push_entry(
            envelope,
            operation_id,
            conflict_keys,
            Some(consensus_evidence),
            status,
        );

        Ok(FederationJournalAppendOutcome::Appended(entry))
    }

    fn push_entry(
        &mut self,
        envelope: FederationOperationEnvelope,
        operation_id: FederationOperationId,
        conflict_keys: Vec<FederationOperationConflictKey>,
        consensus_evidence: Option<FederationJournalConsensusEvidence>,
        status: FederationOperationStatus,
    ) -> FederationJournalEntry {
        let index = self.entries.len() as u64;
        let previous_order_digest = self.latest_order_digest;
        let order_digest = previous_order_digest.append(index, operation_id);
        let application = FederationApplicationState::from_status(status);
        let entry = FederationJournalEntry {
            index,
            operation_id,
            envelope,
            conflict_keys: conflict_keys.clone(),
            previous_order_digest,
            order_digest,
            consensus_evidence,
            application: application.clone(),
        };

        self.entries.push(entry.clone());
        self.operation_indices.insert(operation_id, index as usize);
        if application_reserves_conflict_keys(&application) {
            for conflict_key in conflict_keys {
                self.conflict_keys
                    .entry(conflict_key)
                    .or_insert(operation_id);
            }
        }
        self.latest_order_digest = order_digest;
        entry
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

        let outcome = match (&peer_entry.application, peer_entry.consensus_evidence) {
            (FederationApplicationState::ConsensusRejected {}, None) => {
                return Err(FederationError::JournalConsensusEvidenceMissing {
                    index: peer_entry.index,
                });
            }
            (_, Some(consensus_evidence)) => {
                self.append_finalized_inner(peer_entry.envelope.clone(), consensus_evidence)?
            }
            (_, None) => self.append_inner(peer_entry.envelope.clone(), None)?,
        };

        match outcome {
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
        self.update_application_state(
            operation_id,
            FederationApplicationState::from_status(status),
        )
    }

    /// Update local post-consensus application state.
    ///
    /// # Panics
    ///
    /// Panics only if the journal's validated operation index points outside
    /// its entry collection.
    pub fn update_application_state(
        &mut self,
        operation_id: FederationOperationId,
        application: FederationApplicationState,
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
        entry.application = application;
        Ok(entry.clone())
    }
}

fn application_reserves_conflict_keys(application: &FederationApplicationState) -> bool {
    !matches!(
        application,
        FederationApplicationState::ConsensusRejected {}
    )
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

    fn member_id() -> super::super::MemberId {
        super::super::MemberId::new(1).expect("member id")
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
                    request_lookup_id: crate::payment::PaymentIdentifier::CustomId(
                        "quote-1".to_string(),
                    ),
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
                        method: PaymentMethod::BOLT11,
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
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
        let melt_quote_request = crate::MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
            method: "custom".to_string(),
            request: "payreq".to_string(),
            unit: CurrencyUnit::Sat,
            amount: None,
            extra: serde_json::Value::Null,
        });
        let second = FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MeltQuote(
                super::super::operation::test_melt_quote_operation(
                    "melt-quote",
                    melt_quote_request,
                ),
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
    fn journal_entry_json_requires_current_consensus_metadata() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append")
            .entry()
            .clone();

        for field in ["conflict_keys", "previous_order_digest", "order_digest"] {
            let mut value = serde_json::to_value(&entry).expect("journal entry json");
            value
                .as_object_mut()
                .expect("journal entry object")
                .remove(field);
            assert!(serde_json::from_value::<FederationJournalEntry>(value).is_err());
        }
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
    fn journal_records_finalized_mint_conflicts_for_application() {
        let mut journal = InMemoryFederationJournal::new();
        let first = mint_envelope(
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let conflict = mint_envelope(
            "quote-1",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let first_entry = journal
            .append_finalized_with_consensus_evidence(
                first,
                FederationJournalConsensusEvidence::new(0, 0, 0, member_id(), 0),
            )
            .expect("append first finalized")
            .entry()
            .clone();
        let conflicting_entry = journal
            .append_finalized_with_consensus_evidence(
                conflict.clone(),
                FederationJournalConsensusEvidence::new(0, 1, 1, member_id(), 1),
            )
            .expect("append finalized conflict")
            .entry()
            .clone();

        assert_eq!(journal.len(), 2);
        assert_eq!(first_entry.status(), FederationOperationStatus::Accepted);
        assert_eq!(
            conflicting_entry.status(),
            FederationOperationStatus::Accepted
        );
        assert_eq!(conflicting_entry.index, 1);
        let rejection = ErrorResponse::from(crate::Error::IssuedQuote);
        let rejected_entry = journal
            .update_application_state(
                conflicting_entry.operation_id,
                FederationApplicationState::Rejected {
                    response: rejection.clone(),
                },
            )
            .expect("persist mint application rejection");
        assert_eq!(rejected_entry.rejection(), Some(&rejection));
        assert_eq!(
            journal
                .append_finalized_with_consensus_evidence(
                    conflict,
                    FederationJournalConsensusEvidence::new(0, 2, 2, member_id(), 2),
                )
                .expect("duplicate rejected conflict")
                .entry()
                .index,
            rejected_entry.index
        );

        let rebuilt = InMemoryFederationJournal::from_entries(journal.entries().to_vec())
            .expect("rebuild journal with rejected conflict");
        assert_eq!(rebuilt.len(), 2);
        assert_eq!(
            rebuilt.entries()[1].status(),
            FederationOperationStatus::Rejected
        );
        assert_eq!(rebuilt.entries()[1].application, rejected_entry.application);
    }

    #[test]
    fn journal_keeps_non_mint_finalized_conflicts_rejected() {
        let mut journal = InMemoryFederationJournal::new();
        journal
            .append_finalized_with_consensus_evidence(
                mint_quote_envelope(1),
                FederationJournalConsensusEvidence::new(0, 0, 0, member_id(), 0),
            )
            .expect("append first finalized quote");
        let conflicting = journal
            .append_finalized_with_consensus_evidence(
                mint_quote_envelope(2),
                FederationJournalConsensusEvidence::new(0, 1, 1, member_id(), 1),
            )
            .expect("append conflicting finalized quote")
            .entry()
            .clone();

        assert_eq!(conflicting.status(), FederationOperationStatus::Rejected);
        assert_eq!(
            conflicting.application,
            FederationApplicationState::ConsensusRejected {}
        );
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
        let updated = journal
            .update_status(entry.operation_id, FederationOperationStatus::Applied)
            .expect("status update");

        assert_eq!(updated.status(), FederationOperationStatus::Applied);
        assert_eq!(journal.latest_order_digest(), digest);
        assert!(matches!(
            journal.update_status(
                FederationOperationId::from_bytes([42; 32]),
                FederationOperationStatus::Applied
            ),
            Err(FederationError::UnknownOperation { .. })
        ));
        assert!(matches!(
            FederationApplicationState::from_parts(
                entry.index,
                FederationOperationStatus::Applied,
                Some(ErrorResponse::from(crate::Error::TokenAlreadySpent)),
            ),
            Err(FederationError::JournalRejectionStateMismatch { .. })
        ));
    }

    #[test]
    fn business_rejection_keeps_consensus_conflict_reservation_after_replay() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append accepted operation")
            .entry()
            .clone();
        journal
            .update_application_state(
                entry.operation_id,
                FederationApplicationState::Rejected {
                    response: ErrorResponse::from(crate::Error::IssuedQuote),
                },
            )
            .expect("record business rejection");

        assert!(matches!(
            journal.append(mint_quote_envelope(2)),
            Err(FederationError::ConflictingOperation { .. })
        ));

        let mut replayed = InMemoryFederationJournal::from_entries(journal.entries().to_vec())
            .expect("replay business rejection");
        assert!(matches!(
            replayed.append(mint_quote_envelope(2)),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn application_state_serializes_every_legal_state_and_rejects_invalid_shapes() {
        let states = [
            FederationApplicationState::Accepted {},
            FederationApplicationState::Applying {},
            FederationApplicationState::Applied {},
            FederationApplicationState::Failed {},
            FederationApplicationState::ConsensusRejected {},
            FederationApplicationState::Rejected {
                response: ErrorResponse::from(crate::Error::TokenAlreadySpent),
            },
        ];

        for state in states {
            let encoded = serde_json::to_string(&state).expect("serialize application state");
            let decoded: FederationApplicationState =
                serde_json::from_str(&encoded).expect("deserialize application state");
            assert_eq!(decoded, state);
        }

        assert!(serde_json::from_str::<FederationApplicationState>(
            r#"{"state":"applied","response":{"code":11001,"detail":"spent"}}"#,
        )
        .is_err());
        assert!(
            serde_json::from_str::<FederationApplicationState>(r#"{"state":"rejected"}"#,).is_err()
        );
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
            FederationMaterializedStateDigest::genesis()
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
    fn order_and_materialized_state_digest_golden_vectors() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope(1))
            .expect("append vector operation")
            .entry()
            .clone();
        let snapshot = FederationMaterializedStateSnapshot::from_rows([
            FederationMaterializedStateRow::new("mint_quotes", "quote-1")
                .with_field("amount", "1")
                .with_field("state", "ISSUED"),
            FederationMaterializedStateRow::new("proofs", "proof-y-1")
                .with_field("amount", "1")
                .with_field("state", "SPENT"),
        ]);

        assert_eq!(
            FederationJournalDigest::genesis().to_hex(),
            "98719e56ebdf856720bdabe55f489300b0f8495dff738701b045c389a7f03c4e"
        );
        assert_eq!(
            entry.operation_id.to_string(),
            "a2b58bcf783e9936b317f1207a437795972134eb708499b2b3cf97d1b510b117"
        );
        assert_eq!(
            entry.order_digest.to_hex(),
            "20222f79401c3e696d4d06fdbeb393e43c22c56a629f8878616e6afc7016a1fa"
        );
        assert_eq!(
            FederationMaterializedStateDigest::genesis().to_hex(),
            "3bdc17a9cd55a76f38868a285dd337cadd9044db164ab2ec06e5c396e37dac16"
        );
        assert_eq!(
            snapshot.digest().expect("materialized digest").to_hex(),
            "05bb07bd3d72a521967c221d62f97e0cb6917926864c1f14b373386049501330"
        );
    }

    #[test]
    fn materialized_state_digest_serializes_as_hex() {
        let digest = FederationMaterializedStateDigest::genesis();
        let json = serde_json::to_string(&digest).expect("digest json");

        assert_eq!(json.len(), 66);
        assert_eq!(
            serde_json::from_str::<FederationMaterializedStateDigest>(&json)
                .expect("digest from json"),
            digest
        );
        assert_eq!(
            FederationMaterializedStateDigest::from_slice(&digest.to_bytes())
                .expect("digest from bytes"),
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
