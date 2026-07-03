//! Append-only journal event types and id generation.
//!
//! The mint keeps current state in mutable tables. To make state transitions
//! auditable and replayable, every creation and every mutation of the tracked
//! entities is also written to an insert-only `journal` table as a serialized
//! [`Event`].
//!
//! An [`Event`] is either a [`Snapshot`] (the full base object captured when an
//! entity is created) or a [`Delta`] (one field-level change captured when an
//! entity is mutated). A journal row is identified by a compound
//! `(entity, record)` key: an [`Entity`] discriminant naming the source table
//! plus that row's primary key. Replaying one row's events in `id` order
//! (snapshot first, then deltas) reconstructs its current state, so the journal
//! doubles as an ordered event stream.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use web_time::{SystemTime, UNIX_EPOCH};

use crate::mint::{IncomingPayment, MeltQuote, MintKeySetInfo, MintQuote};
use crate::nuts::nut07::State;
use crate::nuts::BlindSignature;
use crate::payment::PaymentIdentifier;
use crate::{Amount, MeltQuoteState, Proof};

/// Error serializing or deserializing an [`Event`].
#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    /// The event could not be serialized to CBOR.
    #[error("event serialization failed: {0}")]
    Serialize(String),
    /// The stored bytes could not be decoded into an event.
    #[error("event deserialization failed: {0}")]
    Deserialize(String),
    /// The stored `entity` discriminant does not map to a known [`Entity`].
    #[error("unknown journal entity discriminant: {0}")]
    UnknownEntity(u8),
}

/// The source table a journal row refers to.
///
/// Stored as its `u8` discriminant in the `journal.entity` column; the row's
/// primary key is stored separately in `journal.record`. Together they form the
/// journal's compound `(entity, record)` key, replacing the earlier
/// `"table_name:pk"` string. The discriminants are part of the stored format,
/// so existing variants must keep their values.
#[non_exhaustive]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Entity {
    /// A `mint_quote` row, keyed by its quote id.
    MintQuote = 1,
    /// A `melt_quote` row, keyed by its quote id.
    MeltQuote = 2,
    /// A `proof` row, keyed by its `Y` hex.
    Proof = 3,
    /// A `blind_signature` row, keyed by its blinded-secret hex.
    BlindSignature = 4,
    /// A `keyset` row, keyed by its keyset id.
    Keyset = 5,
}

impl Entity {
    /// The stored discriminant written to the `journal.entity` column.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for Entity {
    type Error = EventLogError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            1 => Entity::MintQuote,
            2 => Entity::MeltQuote,
            3 => Entity::Proof,
            4 => Entity::BlindSignature,
            5 => Entity::Keyset,
            other => return Err(EventLogError::UnknownEntity(other)),
        })
    }
}

/// A single journal event: either a full-object snapshot or a field delta.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Full base object, written when an entity is created. Boxed because a
    /// snapshot is far larger than a delta.
    Snapshot(Box<Snapshot>),
    /// One field-level change, written when an entity is mutated.
    Delta(Delta),
}

/// Full base object captured at creation time.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Snapshot {
    /// A newly created melt quote.
    MeltQuote(MeltQuote),
    /// A newly created mint quote. Its `amount_paid`/`amount_issued` start at
    /// zero and grow through [`Delta::MintQuotePayment`]/[`Delta::MintQuoteIssuance`].
    MintQuote(MintQuote),
    /// A newly created proof. Its initial state is always `Unspent`.
    Proof(Proof),
    /// A blind signature issued by the mint. Immutable once created, so it has
    /// no deltas.
    BlindSignature(BlindSignature),
    /// A newly created keyset.
    Keyset(MintKeySetInfo),
}

/// One writable field's new value.
///
/// Each variant maps to exactly one writable field and carries only that
/// field's new value; the journal's compound `(entity, record)` key identifies
/// which row changed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Delta {
    /// `melt_quote.state`
    MeltQuoteState(MeltQuoteState),
    /// `melt_quote.payment_proof`
    MeltQuotePaymentProof(Option<String>),
    /// `melt_quote.request_lookup_id`
    MeltQuoteRequestLookupId(PaymentIdentifier),
    /// A payment received for a mint quote (appended to `mint_quote.payments`,
    /// increments `amount_paid`).
    MintQuotePayment(IncomingPayment),
    /// An issuance recorded against a mint quote (increments `amount_issued`).
    MintQuoteIssuance(Amount),
    /// `proof.state`
    ProofState(State),
    /// A proof row was removed (tombstone, carries no value).
    ProofRemoved,
    /// `keyset.active`
    KeysetActive(bool),
}

impl Event {
    /// The [`Entity`] this event belongs to, derived from its variant.
    ///
    /// Every event maps to exactly one entity, so the writer stores this in the
    /// `journal.entity` column rather than taking it as a separate argument.
    pub fn entity(&self) -> Entity {
        match self {
            Event::Snapshot(snapshot) => snapshot.entity(),
            Event::Delta(delta) => delta.entity(),
        }
    }

    /// Serializes the event for the `journal.event` column.
    ///
    /// Uses JSON, the same encoding the mint already uses to persist these
    /// domain types (quote requests, options, keyset amounts). A binary format
    /// like CBOR is more compact but several cashu types serialize differently
    /// under a non-human-readable serializer and do not round-trip there.
    pub fn to_bytes(&self) -> Result<Vec<u8>, EventLogError> {
        serde_json::to_vec(self).map_err(|e| EventLogError::Serialize(e.to_string()))
    }

    /// Decodes an event from the bytes stored in the `journal.event` column.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EventLogError> {
        serde_json::from_slice(bytes).map_err(|e| EventLogError::Deserialize(e.to_string()))
    }
}

impl Snapshot {
    /// The [`Entity`] this snapshot creates.
    fn entity(&self) -> Entity {
        match self {
            Snapshot::MeltQuote(_) => Entity::MeltQuote,
            Snapshot::MintQuote(_) => Entity::MintQuote,
            Snapshot::Proof(_) => Entity::Proof,
            Snapshot::BlindSignature(_) => Entity::BlindSignature,
            Snapshot::Keyset(_) => Entity::Keyset,
        }
    }
}

impl Delta {
    /// The [`Entity`] whose field this delta changes.
    fn entity(&self) -> Entity {
        match self {
            Delta::MeltQuoteState(_)
            | Delta::MeltQuotePaymentProof(_)
            | Delta::MeltQuoteRequestLookupId(_) => Entity::MeltQuote,
            Delta::MintQuotePayment(_) | Delta::MintQuoteIssuance(_) => Entity::MintQuote,
            Delta::ProofState(_) | Delta::ProofRemoved => Entity::Proof,
            Delta::KeysetActive(_) => Entity::Keyset,
        }
    }
}

// Ergonomic conversions so orchestration sites can write `value.into()` instead
// of the full `Event::Delta(Delta::Variant(value))` / snapshot wrapping.

impl From<Delta> for Event {
    fn from(delta: Delta) -> Self {
        Event::Delta(delta)
    }
}

impl From<Snapshot> for Event {
    fn from(snapshot: Snapshot) -> Self {
        Event::Snapshot(Box::new(snapshot))
    }
}

impl From<MeltQuoteState> for Event {
    fn from(state: MeltQuoteState) -> Self {
        Event::Delta(Delta::MeltQuoteState(state))
    }
}

impl From<State> for Event {
    fn from(state: State) -> Self {
        Event::Delta(Delta::ProofState(state))
    }
}

impl From<PaymentIdentifier> for Event {
    fn from(request_lookup_id: PaymentIdentifier) -> Self {
        Event::Delta(Delta::MeltQuoteRequestLookupId(request_lookup_id))
    }
}

impl From<MeltQuote> for Event {
    fn from(quote: MeltQuote) -> Self {
        Event::Snapshot(Box::new(Snapshot::MeltQuote(quote)))
    }
}

impl From<MintQuote> for Event {
    fn from(quote: MintQuote) -> Self {
        Event::Snapshot(Box::new(Snapshot::MintQuote(quote)))
    }
}

impl From<BlindSignature> for Event {
    fn from(signature: BlindSignature) -> Self {
        Event::Snapshot(Box::new(Snapshot::BlindSignature(signature)))
    }
}

impl From<Proof> for Event {
    fn from(proof: Proof) -> Self {
        Event::Snapshot(Box::new(Snapshot::Proof(proof)))
    }
}

impl From<MintKeySetInfo> for Event {
    fn from(keyset: MintKeySetInfo) -> Self {
        Event::Snapshot(Box::new(Snapshot::Keyset(keyset)))
    }
}

// Snowflake id layout (63 bits, always positive as an i64):
//   bits 62..22 : 41-bit millisecond timestamp since EPOCH_MS
//   bits 21..12 : 10-bit node id
//   bits 11..0  : 12-bit per-millisecond sequence
const SEQ_BITS: u64 = 12;
const NODE_BITS: u64 = 10;
const MAX_SEQ: u64 = (1 << SEQ_BITS) - 1;
const MAX_NODE: u64 = (1 << NODE_BITS) - 1;

/// Custom epoch: 2024-01-01T00:00:00Z in milliseconds. Maximizes the usable
/// range of the 41-bit timestamp.
const EPOCH_MS: u64 = 1_704_067_200_000;

/// Lock-free Snowflake generator.
///
/// `state` packs the last-used `(timestamp_ms << SEQ_BITS) | sequence`; a CAS
/// loop advances it. `node` is a fixed per-process identifier.
struct Snowflake {
    state: AtomicU64,
    node: AtomicU64,
}

impl Snowflake {
    const fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            node: AtomicU64::new(0),
        }
    }

    fn set_node(&self, node_id: u16) {
        self.node
            .store(u64::from(node_id) & MAX_NODE, Ordering::Relaxed);
    }

    /// Milliseconds since [`EPOCH_MS`]. A clock reading before the unix epoch
    /// is clamped to 0 rather than panicking.
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            .saturating_sub(EPOCH_MS)
    }

    fn next_id(&self) -> i64 {
        loop {
            let now = self.now_ms();
            let prev = self.state.load(Ordering::Acquire);
            let last_ms = prev >> SEQ_BITS;
            let last_seq = prev & MAX_SEQ;

            let (ms, seq) = if now > last_ms {
                (now, 0)
            } else if last_seq < MAX_SEQ {
                // Same millisecond (or clock went backwards): bump sequence.
                (last_ms, last_seq + 1)
            } else {
                // Sequence exhausted for this millisecond: borrow the next one.
                (last_ms + 1, 0)
            };

            let next = (ms << SEQ_BITS) | seq;
            if self
                .state
                .compare_exchange_weak(prev, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let node = self.node.load(Ordering::Relaxed);
                return (((ms << NODE_BITS) | node) << SEQ_BITS | seq) as i64;
            }
        }
    }
}

static GENERATOR: Snowflake = Snowflake::new();

/// Generates a monotonic, time-sortable, node-unique Snowflake id.
///
/// Ids are unique across concurrent writers within a process and, when
/// [`init_event_id_generator`] is given a distinct node id per mint instance,
/// across instances too.
pub fn generate_id() -> i64 {
    GENERATOR.next_id()
}

/// Sets the 10-bit node id used by [`generate_id`]. Call once at mint startup;
/// the node id defaults to `0` if never set.
pub fn init_event_id_generator(node_id: u16) {
    GENERATOR.set_node(node_id);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn delta_round_trip() {
        let deltas = [
            Delta::MeltQuoteState(MeltQuoteState::Paid),
            Delta::MeltQuotePaymentProof(Some("preimage".to_string())),
            Delta::MeltQuoteRequestLookupId(PaymentIdentifier::Label("lbl".to_string())),
            Delta::MintQuoteIssuance(Amount::from(42)),
            Delta::ProofState(State::Spent),
            Delta::ProofRemoved,
            Delta::KeysetActive(true),
        ];

        for delta in deltas {
            let event = Event::Delta(delta);
            let bytes = event.to_bytes().expect("serialize");
            let decoded = Event::from_bytes(&bytes).expect("deserialize");
            assert_eq!(event, decoded);
        }
    }

    #[test]
    fn entity_discriminant_round_trips() {
        for entity in [
            Entity::MintQuote,
            Entity::MeltQuote,
            Entity::Proof,
            Entity::BlindSignature,
            Entity::Keyset,
        ] {
            assert_eq!(Entity::try_from(entity.as_u8()).expect("known"), entity);
        }
        assert!(matches!(
            Entity::try_from(0),
            Err(EventLogError::UnknownEntity(0))
        ));
    }

    #[test]
    fn events_map_to_expected_entity() {
        assert_eq!(
            Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Paid)).entity(),
            Entity::MeltQuote
        );
        assert_eq!(
            Event::Delta(Delta::MintQuoteIssuance(Amount::from(1))).entity(),
            Entity::MintQuote
        );
        assert_eq!(Event::Delta(Delta::ProofRemoved).entity(), Entity::Proof);
        assert_eq!(
            Event::Delta(Delta::KeysetActive(true)).entity(),
            Entity::Keyset
        );
    }

    #[test]
    fn ids_are_monotonic_and_unique() {
        let mut seen = HashSet::new();
        let mut last = i64::MIN;
        for _ in 0..10_000 {
            let id = generate_id();
            assert!(id > last, "ids must be strictly increasing");
            assert!(seen.insert(id), "ids must be unique");
            last = id;
        }
    }

    #[test]
    fn ids_are_unique_across_threads() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..5_000).map(|_| generate_id()).collect::<Vec<_>>()))
            .collect();

        let mut seen = HashSet::new();
        for handle in handles {
            for id in handle.join().expect("thread panicked") {
                assert!(seen.insert(id), "ids must be unique across threads");
            }
        }
    }
}
