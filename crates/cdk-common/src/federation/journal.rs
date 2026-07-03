//! Deterministic federation journal scaffold.
//!
//! This in-memory journal is intentionally small: it proves the data model for
//! consensus-accepted operation order, duplicate handling, conflict detection,
//! status updates, order digests, and catch-up ranges before storage backends
//! persist the same entry shape.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationError, FederationOperationConflictKey, FederationOperationEnvelope,
    FederationOperationId, Result,
};

const FEDERATION_JOURNAL_GENESIS_VERSION: &[u8] = b"cdk-federation-journal-genesis-v1";
const FEDERATION_JOURNAL_ENTRY_VERSION: &[u8] = b"cdk-federation-journal-entry-v1";

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
    /// Local application status.
    pub status: FederationOperationStatus,
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
        let operation_id = envelope.operation_id()?;
        if let Some(existing_index) = self.operation_indices.get(&operation_id) {
            let entry = self
                .entries
                .get(*existing_index)
                .expect("operation index points to an entry")
                .clone();
            return Ok(FederationJournalAppendOutcome::Duplicate(entry));
        }

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
        let entry = FederationJournalEntry {
            index,
            operation_id,
            envelope,
            conflict_key,
            conflict_keys,
            previous_order_digest,
            order_digest,
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

        match self.append(peer_entry.envelope.clone())? {
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
        let updated = journal
            .update_status(entry.operation_id, FederationOperationStatus::Applied)
            .expect("status update");

        assert_eq!(updated.status, FederationOperationStatus::Applied);
        assert_eq!(journal.latest_order_digest(), digest);
        assert!(matches!(
            journal.update_status(
                FederationOperationId::from_bytes([42; 32]),
                FederationOperationStatus::Applied
            ),
            Err(FederationError::UnknownOperation { .. })
        ));
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
    fn journal_allows_same_quote_payment_state_only_when_identical() {
        let observation = FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MeltQuotePayment(
                super::super::FederationMeltQuotePaymentObservation {
                    quote: "melt-quote".to_string(),
                    method: PaymentMethod::Custom("custom".to_string()),
                    state: crate::MeltQuoteState::Paid,
                    amount: Amount::from(10),
                    fee_reserve: Amount::from(1),
                    payment_proof: Some("preimage".to_string()),
                    failure_reason: None,
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
}
