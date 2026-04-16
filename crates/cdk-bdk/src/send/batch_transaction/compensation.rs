//! Compensation helpers for SendBatch
//!
//! Pre-broadcast batches (Built, Signed) can be compensated by reverting linked
//! intents to Pending, and deleting the batch record.
//!
//! Post-broadcast batches are never compensated -- they are reconciled
//! through the confirmation flow.

use super::state::{Built, Signed};
use super::SendBatch;
use crate::error::Error;
use crate::send::payment_intent::state::Pending;
use crate::send::payment_intent::SendIntent;
use crate::storage::BdkStorage;

async fn compensate_pre_broadcast_batch<S>(
    batch: SendBatch<S>,
    storage: &BdkStorage,
) -> Result<Vec<SendIntent<Pending>>, Error> {
    let mut reverted = Vec::new();
    for intent in batch.intents {
        reverted.push(intent.revert_to_pending(storage).await?);
    }
    storage.delete_send_batch(&batch.batch_id).await?;
    Ok(reverted)
}

impl SendBatch<Built> {
    /// Compensate a built batch: revert intents, delete batch.
    pub async fn compensate(self, storage: &BdkStorage) -> Result<Vec<SendIntent<Pending>>, Error> {
        compensate_pre_broadcast_batch(self, storage).await
    }
}

impl SendBatch<Signed> {
    /// Compensate a signed batch: revert intents, delete batch.
    pub async fn compensate(self, storage: &BdkStorage) -> Result<Vec<SendIntent<Pending>>, Error> {
        compensate_pre_broadcast_batch(self, storage).await
    }
}
