//! Retention floor of federation consensus history.
//!
//! A member keeps one full scheduled-checkpoint interval of journal entries and
//! finalized items, and always the validity window of its latest scheduled
//! checkpoint. Everything below may be deleted. The floor depends only on the
//! scheduled checkpoint sequence and on finalized items, so members that
//! promoted the same checkpoints compute the same floor. A peer whose
//! catch-up cursor is below every member's floor installs a state transfer.

use serde::{Deserialize, Serialize};

use super::{
    federation_operation_window_start_session, FederationApplicationDigest, FederationCheckpoint,
    FederationCheckpointId, FederationFinalizedItemDigest, FederationJournalDigest,
};

/// Journal entries and finalized items deleted per storage transaction.
pub const FEDERATION_RETENTION_PRUNE_BATCH_ROWS: u64 = 500;

/// First retained position of consensus history and the digests just before it.
///
/// The floor is the scheduled checkpoint before the latest one, or the start
/// of the latest scheduled checkpoint's validity window if that is earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationRetentionFloor {
    /// Scheduled checkpoint before the latest one; its rows are kept.
    pub checkpoint_id: FederationCheckpointId,
    /// First retained journal entry.
    pub journal_start: u64,
    /// First retained finalized item.
    pub item_start: u64,
    /// Session that finalized the item at `item_start`.
    pub item_start_session: u64,
    /// Accepted-order digest before `journal_start`.
    pub journal_base_digest: FederationJournalDigest,
    /// Finalized-item digest before `item_start`.
    pub item_base_digest: FederationFinalizedItemDigest,
    /// Application digest before `journal_start`.
    pub application_base_digest: FederationApplicationDigest,
}

impl FederationRetentionFloor {
    /// Whether journal entries from `next_index` on are still held.
    #[must_use]
    pub fn retains_journal_from(&self, next_index: u64) -> bool {
        next_index >= self.journal_start
    }

    /// Whether finalized items from `next_item_index` on are still held.
    #[must_use]
    pub fn retains_items_from(&self, next_item_index: u64) -> bool {
        next_item_index >= self.item_start
    }

    /// Whether the whole validity window before `checkpoint`, plus the one
    /// older item that proves it complete, is still held.
    #[must_use]
    pub fn retains_window_of(&self, checkpoint: &FederationCheckpoint) -> bool {
        self.item_start == 0
            || self.item_start_session
                < federation_operation_window_start_session(checkpoint.aleph_bft_session)
    }
}
