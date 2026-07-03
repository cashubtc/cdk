//! Traits and types for the mint's append-only transparency event log.
//!
//! See `docs/adr/0001-append-only-transparency-log.md`. This module is
//! intentionally additive and separate from [`super::Database`]/
//! [`super::Transaction`]: appending an entry happens inside the *same*
//! database transaction as the mutation it describes (implemented
//! directly by `cdk-sql-common`, alongside the existing mutation methods,
//! rather than through a new trait method dispatch), while reading the
//! log back — needed from other crates such as the background checkpoint
//! publisher and the audit HTTP endpoints — goes through
//! [`TransparencyLogDatabase`].
//!
//! Tree state (the Merkle peaks) and signed checkpoints are deliberately
//! *not* modeled here: they are small, single-valued or slowly-growing
//! records with no need for relational range queries, so they are stored
//! through the mint's existing generic [`super::KVStoreDatabase`] instead
//! of a bespoke schema.

use async_trait::async_trait;

use super::Error;

/// Entity kinds that participate in the append-only transparency log.
///
/// Only entities that are mutated or deleted after creation need to be
/// logged — insert-only tables are already their own complete history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoggedEntity {
    /// A melt quote's mutable fields (state, payment lookup, fee info).
    MeltQuote,
    /// A proof's state (and its removal on compensation).
    Proof,
    /// A keyset's `active` flag.
    Keyset,
    /// A blind signature's DLEQ fields, filled in after being initially
    /// stored with placeholders.
    BlindSignature,
}

impl LoggedEntity {
    /// The stable string stored in the database's `entity_type` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MeltQuote => "melt_quote",
            Self::Proof => "proof",
            Self::Keyset => "keyset",
            Self::BlindSignature => "blind_signature",
        }
    }
}

impl std::str::FromStr for LoggedEntity {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "melt_quote" => Ok(Self::MeltQuote),
            "proof" => Ok(Self::Proof),
            "keyset" => Ok(Self::Keyset),
            "blind_signature" => Ok(Self::BlindSignature),
            other => Err(Error::Internal(format!(
                "unknown transparency log entity type: {other}"
            ))),
        }
    }
}

/// The kind of mutation a transparency log entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventOp {
    /// A row was created (the entry payload records its initial,
    /// non-secret field values). Logged so that the Merkle tree commits
    /// to a row's *existence*, not just its later transitions — without
    /// it, replay from the log alone is impossible and an operator could
    /// silently invent or vanish rows that were never updated.
    Insert = 0,
    /// A row was updated (its new field values are in the entry payload).
    Update = 1,
    /// A row was deleted (the entry payload records its final state).
    Delete = 2,
}

impl EventOp {
    /// The `i16` stored in the database's `op` column.
    pub fn as_i16(&self) -> i16 {
        *self as i16
    }
}

impl TryFrom<i16> for EventOp {
    type Error = Error;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Insert),
            1 => Ok(Self::Update),
            2 => Ok(Self::Delete),
            other => Err(Error::Internal(format!(
                "unknown transparency log op code: {other}"
            ))),
        }
    }
}

/// One durable, sequenced row of the mint's append-only transparency
/// event log, as persisted by `cdk-sql-common` and read back by the
/// checkpoint publisher and the audit HTTP endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintEventLogEntry {
    /// Zero-based Merkle tree leaf index (NUT-XX's public `seq`), assigned
    /// densely by the single appender via
    /// [`TransparencyLogDatabase::assign_leaf_indices`] — deliberately not
    /// the row's auto-increment id, whose gaps (permanent on Postgres after
    /// a rolled-back transaction) must never affect tree positions.
    pub seq: u64,
    /// Which kind of entity this entry describes.
    pub entity_type: LoggedEntity,
    /// The entity's own identifier (quote id, Y hex, keyset id, blinded
    /// message hex).
    pub entity_id: String,
    /// What kind of mutation this entry records.
    pub op: EventOp,
    /// Canonical encoding of the fields the mutation wrote. Callers on
    /// the write side decide the encoding; callers on the read side
    /// (replay, audit) must agree on the same one.
    pub payload: Vec<u8>,
    /// RFC 6962 leaf hash of this entry, precomputed at insert time from
    /// `(entity_type, entity_id, op, payload, created_time)` — `seq` is
    /// deliberately excluded, see [`Self::leaf_preimage`] — so that neither
    /// the checkpoint publisher nor an external auditor need to re-derive
    /// the exact encoding used to hash it.
    pub leaf_hash: [u8; 32],
    /// Unix timestamp (seconds) the entry was appended at.
    pub created_time: u64,
}

impl MintEventLogEntry {
    /// The canonical byte preimage that gets hashed (via
    /// `cdk_tlog::merkle::leaf_hash`, applied by the caller — this crate
    /// does not depend on the hashing implementation) to produce
    /// `leaf_hash`.
    ///
    /// Deliberately excludes `seq`: RFC 6962 leaves never encode their
    /// own tree position, since the position is implicit from where the
    /// leaf lands in the tree. This also sidesteps a chicken-and-egg
    /// problem on the write path, where `seq` isn't known until the
    /// insert that assigns it has already happened.
    ///
    /// Exposed so external auditors replaying the raw log rows (via
    /// `/v1/audit/entries`, not through this crate) can recompute the
    /// same hash and verify it against a checkpoint's Merkle tree.
    pub fn leaf_preimage(&self) -> Vec<u8> {
        event_leaf_preimage(
            self.entity_type,
            &self.entity_id,
            self.op,
            &self.payload,
            self.created_time,
        )
    }
}

/// See [`MintEventLogEntry::leaf_preimage`].
pub fn event_leaf_preimage(
    entity_type: LoggedEntity,
    entity_id: &str,
    op: EventOp,
    payload: &[u8],
    created_time: u64,
) -> Vec<u8> {
    let entity_type = entity_type.as_str().as_bytes();
    let entity_id = entity_id.as_bytes();
    let mut buf = Vec::with_capacity(entity_type.len() + entity_id.len() + payload.len() + 10);
    buf.extend_from_slice(entity_type);
    buf.push(0);
    buf.extend_from_slice(entity_id);
    buf.push(0);
    buf.push(op.as_i16() as u8);
    buf.extend_from_slice(&created_time.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Read-only access to the mint's append-only transparency event log.
///
/// There is deliberately no transactional write trait here: appending is
/// implemented by `cdk-sql-common` directly inside each mutation method
/// (`update_melt_quote_state`, `update_proofs_state`, etc.), on the same
/// connection and in the same transaction as the mutation itself, so that
/// a rolled-back mutation can never leave behind an orphaned log entry.
#[async_trait]
pub trait TransparencyLogDatabase {
    /// Transparency Log Database Error
    type Err: Into<Error> + From<Error>;

    /// Assigns dense, zero-based leaf indices to up to `max` not yet
    /// indexed committed rows, in row-insertion (`seq`) order, continuing
    /// from the highest index already assigned. Returns the newly indexed
    /// entries, ordered by leaf index.
    ///
    /// This is how a committed row becomes part of the Merkle tree's leaf
    /// order: the row's auto-increment id is *not* its tree position
    /// (auto-increment gaps are permanent on Postgres after a rollback and
    /// must never stall or shift the tree). Exactly one appender task per
    /// mint may call this — see the ADR's single-sequencer discussion.
    ///
    /// Crash-safe: assignment is durable in the event log table itself, so
    /// an appender that crashes between assigning and folding re-reads the
    /// already-indexed-but-unfolded suffix via
    /// [`Self::get_event_log_range`] on restart.
    async fn assign_leaf_indices(&self, max: u64) -> Result<Vec<MintEventLogEntry>, Self::Err>;

    /// Reads indexed log entries with leaf index in `[start, end)`,
    /// ordered by leaf index. Rows not yet indexed by
    /// [`Self::assign_leaf_indices`] are not visible here.
    async fn get_event_log_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<MintEventLogEntry>, Self::Err>;
}

/// Type alias for a shared handle to the mint's transparency log reader.
pub type DynTransparencyLogDatabase =
    std::sync::Arc<dyn TransparencyLogDatabase<Err = Error> + Send + Sync>;

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn logged_entity_round_trips_through_str() {
        for entity in [
            LoggedEntity::MeltQuote,
            LoggedEntity::Proof,
            LoggedEntity::Keyset,
            LoggedEntity::BlindSignature,
        ] {
            assert_eq!(LoggedEntity::from_str(entity.as_str()).unwrap(), entity);
        }
    }

    #[test]
    fn unknown_logged_entity_errors() {
        assert!(LoggedEntity::from_str("not_a_real_entity").is_err());
    }

    #[test]
    fn event_op_round_trips_through_i16() {
        for op in [EventOp::Insert, EventOp::Update, EventOp::Delete] {
            assert_eq!(EventOp::try_from(op.as_i16()).unwrap(), op);
        }
    }

    #[test]
    fn unknown_event_op_errors() {
        assert!(EventOp::try_from(99).is_err());
    }

    #[test]
    fn leaf_preimage_excludes_seq_but_depends_on_everything_else() {
        let base = event_leaf_preimage(
            LoggedEntity::Proof,
            "y-hex",
            EventOp::Update,
            b"payload",
            100,
        );

        // seq isn't a parameter at all, so two entries differing only in
        // seq necessarily produce the same preimage.
        assert_eq!(
            base,
            event_leaf_preimage(
                LoggedEntity::Proof,
                "y-hex",
                EventOp::Update,
                b"payload",
                100
            )
        );

        // Every other field must change the preimage.
        assert_ne!(
            base,
            event_leaf_preimage(
                LoggedEntity::Keyset,
                "y-hex",
                EventOp::Update,
                b"payload",
                100
            )
        );
        assert_ne!(
            base,
            event_leaf_preimage(
                LoggedEntity::Proof,
                "other",
                EventOp::Update,
                b"payload",
                100
            )
        );
        assert_ne!(
            base,
            event_leaf_preimage(
                LoggedEntity::Proof,
                "y-hex",
                EventOp::Delete,
                b"payload",
                100
            )
        );
        assert_ne!(
            base,
            event_leaf_preimage(LoggedEntity::Proof, "y-hex", EventOp::Update, b"other", 100)
        );
        assert_ne!(
            base,
            event_leaf_preimage(
                LoggedEntity::Proof,
                "y-hex",
                EventOp::Update,
                b"payload",
                101
            )
        );
    }
}
