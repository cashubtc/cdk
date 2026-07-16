//! Shared crash-safe staging pipeline for signed send transactions.
//!
//! Both the batch send path and the Payjoin send path must commit a signed
//! transaction with the same durable-write ordering, and `recovery.rs` replays
//! interrupted runs against that same ordering:
//!
//! 1. Persist the `Signed` batch record (tx bytes + per-intent assignments)
//!    before moving any intent forward, so every later crash is recoverable
//!    from the signed transaction instead of reverting into a new batch.
//! 2. Transition the member intents to `Batched`.
//! 3. Persist the `Broadcast` batch record before the network send.
//! 4. Transition the member intents to `AwaitingConfirmation`.
//! 5. Broadcast the transaction.
//!
//! Keep this ordering in one place; do not duplicate it at call sites.

use std::collections::HashMap;

use bdk_wallet::bitcoin::{consensus, OutPoint, Transaction, Txid};
use uuid::Uuid;

use crate::chain::BroadcastOutcome;
use crate::error::Error;
use crate::send::batch_transaction::record::{
    BatchOutputAssignment, SendBatchRecord, SendBatchState,
};
use crate::send::batch_transaction::{state as batch_state, SendBatch};
use crate::send::payment_intent::{state as intent_state, SendIntent};
use crate::CdkBdk;

/// A send intent in a state that can still be assigned into a batch once the
/// `Signed` batch record is durable.
pub(crate) enum StageableSendIntent {
    Claimed(SendIntent<intent_state::BatchClaimed>),
    Payjoin(SendIntent<intent_state::PayjoinNegotiating>),
}

impl StageableSendIntent {
    async fn assign_to_batch(
        self,
        storage: &crate::storage::BdkStorage,
        batch_id: Uuid,
    ) -> Result<SendIntent<intent_state::Batched>, Error> {
        match self {
            // A claimed intent already carries its batch id from the claim.
            Self::Claimed(intent) => intent.assign_to_batch(storage).await,
            Self::Payjoin(intent) => intent.assign_to_batch(storage, batch_id).await,
        }
    }
}

/// Outcome of the staging pipeline once the `Signed` batch record is durable.
///
/// From that point on, no failure is returned as `Err`: the batch can always
/// be completed by `recover_send_batches`, and callers decide how to surface
/// the pending state to their own callers.
#[must_use]
pub(crate) enum StagedBroadcastOutcome {
    /// The batch and its intents are durable and the network accepted the
    /// transaction (or already knew it).
    Broadcast,
    /// The batch is durable but a later step failed; recovery will finish
    /// promoting the batch and rebroadcast the transaction.
    PendingRecovery(Error),
}

impl CdkBdk {
    /// Durably stage a signed send transaction and broadcast it, following the
    /// crash-safety ordering documented at the top of this module.
    ///
    /// The transaction must already be applied to the BDK wallet graph as
    /// unconfirmed. Failure to persist the `Signed` record is the only `Err`
    /// path; it is compensated by evicting the transaction from the wallet
    /// graph again, and leaves every intent in its pre-staging state so the
    /// caller's retry loop can attempt staging again.
    pub(crate) async fn stage_and_broadcast_signed_send_batch(
        &self,
        batch_id: Uuid,
        tx: &Transaction,
        assignments: Vec<BatchOutputAssignment>,
        fee_sat: u64,
        intents: Vec<StageableSendIntent>,
        planning_guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<StagedBroadcastOutcome, Error> {
        let tx_bytes = consensus::serialize(tx);
        let txid = tx.compute_txid();

        if let Err(err) = self
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat,
                },
            })
            .await
        {
            // Nothing is durable yet: revert the wallet graph so the tx's
            // inputs aren't stranded in an orphaned unconfirmed tx.
            if let Err(evict_err) = self.evict_unstaged_send_tx(txid).await {
                tracing::error!(
                    %batch_id,
                    %txid,
                    error = %evict_err,
                    "Could not evict unstaged send tx after Signed batch persistence failure"
                );
            }
            return Err(err);
        }

        // BDK now knows the selected inputs are spent and the Signed record is
        // durable enough for startup recovery. Later intent promotion and all
        // network I/O must not serialize otherwise-independent transaction
        // planning.
        drop(planning_guard);

        // The Signed record is durable. If assigning a member fails here the
        // intent keeps its pre-staging state and recovery repairs it from the
        // batch's assignments; never mark intents failed past this point.
        let mut batched = Vec::with_capacity(intents.len());
        for intent in intents {
            batched.push(intent.assign_to_batch(&self.storage, batch_id).await?);
        }

        Ok(self
            .promote_signed_batch_and_broadcast(
                batch_id,
                tx,
                tx_bytes,
                assignments,
                fee_sat,
                batched,
            )
            .await)
    }

    /// Promote a durable `Signed` batch to `Broadcast`, transition its member
    /// intents to `AwaitingConfirmation`, then broadcast the transaction.
    ///
    /// Shared tail of the staging pipeline, also used by recovery to replay a
    /// `Signed` batch found after a crash. Never returns `Err`: the `Signed`
    /// record is already durable, so every failure is reported as
    /// [`StagedBroadcastOutcome::PendingRecovery`] and retried by recovery.
    pub(crate) async fn promote_signed_batch_and_broadcast(
        &self,
        batch_id: Uuid,
        tx: &Transaction,
        tx_bytes: Vec<u8>,
        assignments: Vec<BatchOutputAssignment>,
        fee_sat: u64,
        intents: Vec<SendIntent<intent_state::Batched>>,
    ) -> StagedBroadcastOutcome {
        let txid = tx.compute_txid();
        let txid_str = txid.to_string();

        let signed_batch = SendBatch::<batch_state::Signed>::reconstruct(batch_id, intents);
        let broadcast_result = match signed_batch
            .mark_broadcast(
                &self.storage,
                txid_str.clone(),
                tx_bytes,
                assignments.clone(),
                fee_sat,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    %batch_id,
                    %txid,
                    error = %err,
                    "Signed send batch is durable but could not be marked broadcast"
                );
                return StagedBroadcastOutcome::PendingRecovery(err);
            }
        };

        // Pair each intent with its assignment via intent_id rather than
        // positional index, so any future reordering of either list is safe.
        let assignment_by_intent: HashMap<Uuid, &BatchOutputAssignment> =
            assignments.iter().map(|a| (a.intent_id, a)).collect();

        for intent in broadcast_result.intents {
            let intent_id = intent.intent_id;
            let Some(assignment) = assignment_by_intent.get(&intent_id) else {
                tracing::error!(
                    %batch_id,
                    %intent_id,
                    "Send batch intent has no output assignment"
                );
                return StagedBroadcastOutcome::PendingRecovery(Error::BatchAssignmentMissing {
                    batch_id,
                    intent_id,
                });
            };
            let outpoint = OutPoint::new(txid, assignment.vout).to_string();
            if let Err(err) = intent
                .mark_broadcast(
                    &self.storage,
                    txid_str.clone(),
                    outpoint,
                    assignment.fee_contribution_sat,
                )
                .await
            {
                tracing::warn!(
                    %batch_id,
                    %intent_id,
                    %txid,
                    error = %err,
                    "Send batch is durable but an intent could not be marked awaiting confirmation"
                );
                return StagedBroadcastOutcome::PendingRecovery(err);
            }
        }

        match self.broadcast_transaction_internal(tx.clone()).await {
            Ok(BroadcastOutcome::Accepted) => {}
            Ok(BroadcastOutcome::AlreadyKnown) => {
                tracing::info!(
                    "Batch {} txid {} was already known to backend",
                    batch_id,
                    txid
                );
            }
            Err(failure) => {
                self.log_broadcast_failure(
                    "Broadcast failed after durable staging",
                    batch_id,
                    &txid_str,
                    &failure,
                );
                return StagedBroadcastOutcome::PendingRecovery(Error::Wallet(format!(
                    "Broadcast failed after signed batch persistence: {}",
                    failure.message
                )));
            }
        }

        StagedBroadcastOutcome::Broadcast
    }

    /// Evict a transaction that was applied to the BDK wallet graph but never
    /// reached a durable batch record, so its inputs return to the spendable
    /// set.
    pub(crate) async fn evict_unstaged_send_tx(&self, txid: Txid) -> Result<(), Error> {
        let evict_time = crate::util::unix_now().saturating_add(1);
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db
            .wallet
            .apply_evicted_txs([(txid, evict_time)]);
        wallet_with_db.persist().map_err(Error::Database)?;
        Ok(())
    }
}
