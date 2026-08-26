use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use bdk_wallet::bitcoin::{Address, OutPoint, Transaction};
use uuid::Uuid;

use crate::chain::BroadcastOutcome;
use crate::error::Error;
use crate::send::batch_transaction::record::{BatchOutputAssignment, SendBatchState};
use crate::send::payment_intent::record::{SendIntentRecord, SendIntentState};
use crate::send::payment_intent::{self, SendIntentAny};
use crate::storage::BdkStorage;
use crate::CdkBdk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchIntentRelation {
    Valid,
    MissingIntent,
    IntentReferencesDifferentBatch,
    IntentAlreadyAdvanced,
}

fn batch_intent_ids(batch_state: &SendBatchState) -> Vec<Uuid> {
    match batch_state {
        SendBatchState::Built { intent_ids, .. } => intent_ids.clone(),
        SendBatchState::Signed { assignments, .. }
        | SendBatchState::Cancelled { assignments, .. }
        | SendBatchState::Broadcast { assignments, .. } => {
            assignments.iter().map(|a| a.intent_id).collect()
        }
    }
}

async fn load_batch_intents(
    storage: &BdkStorage,
    intent_ids: &[Uuid],
) -> Result<Vec<SendIntentRecord>, Error> {
    let mut records = Vec::new();

    for intent_id in intent_ids {
        if let Some(record) = storage.get_send_intent(intent_id).await? {
            records.push(record);
        }
    }

    Ok(records)
}

impl CdkBdk {
    async fn cancel_invalid_signed_send_batch(
        &self,
        batch_id: Uuid,
        tx_bytes: &[u8],
        assignments: &[BatchOutputAssignment],
        fee_sat: u64,
    ) -> Result<(), Error> {
        tracing::error!(%batch_id, "Cancelling Signed batch with invalid assignments");
        let evict_at = crate::util::unix_now().saturating_add(1);
        if !self
            .cancel_signed_send_batch(batch_id, tx_bytes, assignments, fee_sat, evict_at)
            .await?
        {
            tracing::info!(
                %batch_id,
                "Signed batch changed concurrently before invalid-assignment cancellation"
            );
        }
        Ok(())
    }

    /// Durably cancel a signed batch, then complete its wallet and intent
    /// compensation. Returns `false` if another worker changed the batch from
    /// `Signed` before cancellation won the compare-and-set.
    pub(crate) async fn cancel_signed_send_batch(
        &self,
        batch_id: Uuid,
        tx_bytes: &[u8],
        assignments: &[BatchOutputAssignment],
        fee_sat: u64,
        evict_at: u64,
    ) -> Result<bool, Error> {
        let signed_state = SendBatchState::Signed {
            tx_bytes: tx_bytes.to_vec(),
            assignments: assignments.to_vec(),
            fee_sat,
        };
        let cancelled_state = SendBatchState::Cancelled {
            tx_bytes: tx_bytes.to_vec(),
            assignments: assignments.to_vec(),
            fee_sat,
            evict_at,
        };

        if self
            .storage
            .transition_send_batch(&batch_id, &signed_state, &cancelled_state)
            .await?
        {
            self.finish_cancelled_send_batch(batch_id, &cancelled_state)
                .await?;
            return Ok(true);
        }

        // Another recovery worker may already have won the cancellation.
        // Help finish its durable state rather than leaving compensation for
        // another process restart.
        let Some(current) = self.storage.get_send_batch(&batch_id).await? else {
            return Ok(true);
        };
        if matches!(current.state, SendBatchState::Cancelled { .. }) {
            self.finish_cancelled_send_batch(batch_id, &current.state)
                .await?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Finish a durable cancellation idempotently.
    pub(crate) async fn finish_cancelled_send_batch(
        &self,
        batch_id: Uuid,
        cancelled_state: &SendBatchState,
    ) -> Result<(), Error> {
        let SendBatchState::Cancelled {
            tx_bytes,
            assignments,
            evict_at,
            ..
        } = cancelled_state
        else {
            return Err(Error::SendBatchStateConflict {
                batch_id,
                expected: "Cancelled",
            });
        };

        let tx = bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(tx_bytes)
            .map_err(|err| Error::Wallet(err.to_string()))?;
        let txid = tx.compute_txid();
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let last_seen = match wallet_with_db.wallet.get_tx(txid) {
            Some(wallet_tx) => match wallet_tx.chain_position {
                bdk_wallet::chain::ChainPosition::Confirmed { .. } => {
                    return Err(Error::Wallet(format!(
                        "Cannot cancel confirmed send-batch transaction {txid}"
                    )));
                }
                bdk_wallet::chain::ChainPosition::Unconfirmed { last_seen, .. } => last_seen,
            },
            None => None,
        };
        let mut effective_evict_at = (*evict_at).max(crate::util::unix_now());
        for timestamp in [
            last_seen,
            wallet_with_db.wallet.tx_graph().get_last_evicted(txid),
        ]
        .into_iter()
        .flatten()
        {
            effective_evict_at =
                effective_evict_at.max(timestamp.checked_add(1).ok_or_else(|| {
                    Error::Wallet(format!(
                        "Cannot evict send-batch transaction {txid} after maximum timestamp"
                    ))
                })?);
        }
        wallet_with_db
            .wallet
            .apply_evicted_txs([(txid, effective_evict_at)]);
        if wallet_with_db.wallet.get_tx(txid).is_some() {
            return Err(Error::Wallet(format!(
                "Send-batch transaction {txid} remained canonical after eviction"
            )));
        }
        wallet_with_db.persist().map_err(Error::Database)?;
        drop(wallet_with_db);

        for assignment in assignments {
            let Some(record) = self.storage.get_send_intent(&assignment.intent_id).await? else {
                continue;
            };
            if assignment.attempt_id.is_nil() || record.attempt_id != assignment.attempt_id {
                tracing::warn!(
                    batch_id = %batch_id,
                    intent_id = %assignment.intent_id,
                    assignment_attempt_id = %assignment.attempt_id,
                    current_attempt_id = %record.attempt_id,
                    "Skipping cancellation compensation for an unbound or replacement send attempt"
                );
                continue;
            }
            if let SendIntentAny::Batched(intent) = payment_intent::from_record(&record) {
                if intent.state.batch_id == batch_id {
                    intent.revert_to_pending(&self.storage).await?;
                }
            }
        }

        let deleted = self
            .storage
            .delete_send_batch_if_state(&batch_id, cancelled_state)
            .await?;
        if !deleted {
            tracing::info!(
                batch_id = %batch_id,
                "Cancelled batch was already cleaned up by another worker"
            );
        }

        Ok(())
    }

    /// Retry every durable cancellation without letting one broken batch
    /// prevent later batches from making progress.
    pub(crate) async fn resume_cancelled_send_batches(&self) -> Result<(), Error> {
        let batches = self.storage.get_all_send_batches().await?;

        for batch in batches {
            if !matches!(batch.state, SendBatchState::Cancelled { .. }) {
                continue;
            }

            if let Err(err) = self
                .finish_cancelled_send_batch(batch.batch_id, &batch.state)
                .await
            {
                tracing::warn!(
                    batch_id = %batch.batch_id,
                    error = %err,
                    "Could not finish durable send-batch cancellation; retrying next cycle"
                );
            }
        }

        Ok(())
    }

    async fn apply_recovered_send_tx(
        &self,
        batch_id: Uuid,
        context: &str,
        tx: &Transaction,
    ) -> bool {
        let txid = tx.compute_txid();
        let mut wallet_with_db = self.wallet_with_db.lock().await;

        if let Err(err) =
            crate::send::service::reserve_unconfirmed_tx(&mut wallet_with_db.wallet, tx)
        {
            tracing::error!(
                batch_id = %batch_id,
                txid = %txid,
                "{context}: could not reserve recovered transaction: {err}"
            );
            return false;
        }

        if let Err(err) = wallet_with_db.persist() {
            tracing::warn!(
                batch_id = %batch_id,
                txid = %txid,
                "{context}: could not persist BDK wallet after applying recovered tx: {err}"
            );
        }
        true
    }

    /// Fence and reserve a recovered Broadcast transaction in one local wallet
    /// critical section. The durable validation immediately precedes the BDK
    /// mutation, so no local batch can select the inputs between those steps.
    async fn prepare_recovered_broadcast_tx(
        &self,
        batch_id: Uuid,
        state: &SendBatchState,
    ) -> Result<Option<Transaction>, Error> {
        let reservation = crate::send::service::decode_broadcast_reservation(&batch_id, state);
        let eligible = self.prepare_broadcast_batch(batch_id, state).await?;
        let Some((_reservation_batch_id, _txid, reservation_tx)) = reservation else {
            return Ok(None);
        };
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        if let Err(err) = crate::send::service::reserve_unconfirmed_tx(
            &mut wallet_with_db.wallet,
            &reservation_tx,
        ) {
            tracing::error!(
                %batch_id,
                txid = %reservation_tx.compute_txid(),
                error = %err,
                "Broadcast recovery could not reserve fenced transaction"
            );
            return Ok(None);
        }
        if let Err(err) = wallet_with_db.persist() {
            tracing::warn!(
                %batch_id,
                txid = %reservation_tx.compute_txid(),
                error = %err,
                "Could not persist BDK wallet after reserving recovered Broadcast transaction"
            );
        }
        Ok(eligible.map(|(_, tx)| tx))
    }

    /// Check durable Broadcast evidence immediately before a Signed batch is
    /// promoted. A transaction already claiming any member intent wins the
    /// safety fence, regardless of whether that Broadcast record is eligible
    /// for network rebroadcast.
    async fn signed_batch_has_broadcast_conflict(
        &self,
        signed_batch_id: Uuid,
        assignments: &[BatchOutputAssignment],
    ) -> Result<bool, Error> {
        let intent_ids: HashSet<_> = assignments
            .iter()
            .map(|assignment| assignment.intent_id)
            .collect();
        let batches = self.storage.get_all_send_batches().await?;
        Ok(batches.into_iter().any(|batch| {
            batch.batch_id != signed_batch_id
                && matches!(
                    batch.state,
                    SendBatchState::Broadcast { assignments, .. }
                        if assignments
                            .iter()
                            .any(|assignment| intent_ids.contains(&assignment.intent_id))
                )
        }))
    }

    fn log_batch_recovery_invariants(
        &self,
        batch_id: Uuid,
        batch_state: &SendBatchState,
        intent_records: &[SendIntentRecord],
    ) {
        let expected_ids = batch_intent_ids(batch_state);
        let expected_set: HashSet<_> = expected_ids.iter().copied().collect();
        let mut found_ids = HashSet::new();
        let mut saw_batched = false;
        let mut saw_awaiting = false;
        let mut saw_pending = false;

        for record in intent_records {
            found_ids.insert(record.intent_id);

            match &record.state {
                SendIntentState::Pending { .. } => {
                    saw_pending = true;
                    tracing::warn!(
                        batch_id = %batch_id,
                        intent_id = %record.intent_id,
                        "Recovery found batch member stored as Pending"
                    );
                }
                SendIntentState::Batched {
                    batch_id: intent_batch_id,
                    ..
                } => {
                    saw_batched = true;
                    if *intent_batch_id != batch_id {
                        tracing::warn!(
                            batch_id = %batch_id,
                            intent_id = %record.intent_id,
                            intent_batch_id = %intent_batch_id,
                            "Recovery found batch member referencing a different batch"
                        );
                    }
                }
                SendIntentState::AwaitingConfirmation {
                    batch_id: intent_batch_id,
                    ..
                } => {
                    saw_awaiting = true;
                    if *intent_batch_id != batch_id {
                        tracing::warn!(
                            batch_id = %batch_id,
                            intent_id = %record.intent_id,
                            intent_batch_id = %intent_batch_id,
                            "Recovery found advanced batch member referencing a different batch"
                        );
                    }
                }
                SendIntentState::Failed { .. } => {}
            }
        }

        for missing_id in expected_set.difference(&found_ids) {
            tracing::warn!(
                batch_id = %batch_id,
                intent_id = %missing_id,
                "Recovery found batch referencing a missing intent"
            );
        }

        if (saw_batched && saw_awaiting) || (saw_pending && (saw_batched || saw_awaiting)) {
            tracing::warn!(
                batch_id = %batch_id,
                saw_pending,
                saw_batched,
                saw_awaiting,
                "Recovery found mixed intent states within one batch"
            );
        }
    }

    fn classify_batch_intent_relation(
        &self,
        batch_id: Uuid,
        record: Option<&SendIntentRecord>,
    ) -> BatchIntentRelation {
        match record {
            None => BatchIntentRelation::MissingIntent,
            Some(record) => match &record.state {
                SendIntentState::Batched {
                    batch_id: intent_batch_id,
                    ..
                } => {
                    if *intent_batch_id == batch_id {
                        BatchIntentRelation::Valid
                    } else {
                        BatchIntentRelation::IntentReferencesDifferentBatch
                    }
                }
                SendIntentState::AwaitingConfirmation { .. } => {
                    BatchIntentRelation::IntentAlreadyAdvanced
                }
                SendIntentState::Pending { .. } | SendIntentState::Failed { .. } => {
                    BatchIntentRelation::IntentReferencesDifferentBatch
                }
            },
        }
    }

    pub(crate) async fn recover_send_saga(&self) -> Result<(), Error> {
        tracing::info!("Recovering send saga...");

        // Phase 1: Compensate pre-broadcast batches
        let batches = self.storage.get_all_send_batches().await?;
        for batch_record in batches {
            let batch_ids = batch_intent_ids(&batch_record.state);
            let batch_records = load_batch_intents(&self.storage, &batch_ids).await?;
            self.log_batch_recovery_invariants(
                batch_record.batch_id,
                &batch_record.state,
                &batch_records,
            );

            match batch_record.state {
                SendBatchState::Built {
                    psbt_bytes: _,
                    intent_ids,
                } => {
                    tracing::info!(
                        "Compensating pre-broadcast batch {} during recovery",
                        batch_record.batch_id
                    );

                    let mut batched_intents = Vec::new();
                    for id in intent_ids {
                        let record = self.storage.get_send_intent(&id).await?;
                        match self
                            .classify_batch_intent_relation(batch_record.batch_id, record.as_ref())
                        {
                            BatchIntentRelation::Valid => {
                                if let Some(record) = record {
                                    if let SendIntentAny::Batched(intent) =
                                        payment_intent::from_record(&record)
                                    {
                                        batched_intents.push(intent);
                                    }
                                }
                            }
                            BatchIntentRelation::MissingIntent => {
                                tracing::warn!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Skipping compensation for missing batch member"
                                );
                            }
                            BatchIntentRelation::IntentReferencesDifferentBatch => {
                                tracing::warn!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Skipping compensation for batch member with mismatched batch reference"
                                );
                            }
                            BatchIntentRelation::IntentAlreadyAdvanced => {
                                tracing::warn!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Skipping compensation for batch member already advanced past Batched"
                                );
                            }
                        }
                    }

                    let batch = crate::send::batch_transaction::SendBatch::<
                        crate::send::batch_transaction::state::Built,
                    >::reconstruct(
                        batch_record.batch_id, batched_intents
                    );

                    if let Err(e) = batch.compensate(&self.storage).await {
                        tracing::error!(
                            "Failed to compensate batch {} during recovery: {}",
                            batch_record.batch_id,
                            e
                        );
                    }
                }
                SendBatchState::Signed {
                    tx_bytes,
                    assignments,
                    fee_sat,
                } => {
                    let tx =
                        match bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(&tx_bytes)
                        {
                            Ok(tx) => tx,
                            Err(err) => {
                                tracing::error!(
                                    "Failed to deserialize signed batch {} during recovery: {}",
                                    batch_record.batch_id,
                                    err
                                );
                                continue;
                            }
                        };

                    let unique_intent_ids: HashSet<_> = assignments
                        .iter()
                        .map(|assignment| assignment.intent_id)
                        .collect();
                    let unique_vouts: HashSet<_> = assignments
                        .iter()
                        .map(|assignment| assignment.vout)
                        .collect();
                    let allocated_fee = assignments.iter().try_fold(0_u64, |total, assignment| {
                        total.checked_add(assignment.fee_contribution_sat)
                    });
                    if assignments.is_empty()
                        || unique_intent_ids.len() != assignments.len()
                        || unique_vouts.len() != assignments.len()
                        || allocated_fee != Some(fee_sat)
                    {
                        self.cancel_invalid_signed_send_batch(
                            batch_record.batch_id,
                            &tx_bytes,
                            &assignments,
                            fee_sat,
                        )
                        .await?;
                        continue;
                    }

                    if self
                        .signed_batch_has_broadcast_conflict(batch_record.batch_id, &assignments)
                        .await?
                    {
                        tracing::error!(
                            batch_id = %batch_record.batch_id,
                            "Cancelling Signed batch because another durable Broadcast batch references one of its intents"
                        );
                        self.cancel_invalid_signed_send_batch(
                            batch_record.batch_id,
                            &tx_bytes,
                            &assignments,
                            fee_sat,
                        )
                        .await?;
                        continue;
                    }

                    let mut assignments_match_intents = true;
                    for assignment in &assignments {
                        let Some(record) =
                            self.storage.get_send_intent(&assignment.intent_id).await?
                        else {
                            assignments_match_intents = false;
                            break;
                        };
                        let state_matches = match &record.state {
                            SendIntentState::Pending { .. } => true,
                            SendIntentState::Batched { batch_id, .. } => {
                                *batch_id == batch_record.batch_id
                            }
                            SendIntentState::AwaitingConfirmation { .. }
                            | SendIntentState::Failed { .. } => false,
                        };
                        let output = usize::try_from(assignment.vout)
                            .ok()
                            .and_then(|vout| tx.output.get(vout));
                        let address = Address::from_str(&record.address)
                            .ok()
                            .and_then(|address| address.require_network(self.network).ok());
                        if assignment.attempt_id.is_nil()
                            || record.attempt_id != assignment.attempt_id
                            || !state_matches
                            || assignment.fee_contribution_sat > record.max_fee_amount_sat
                            || output.is_none_or(|output| {
                                output.value.to_sat() != record.amount_sat
                                    || address.as_ref().is_none_or(|address| {
                                        output.script_pubkey != address.script_pubkey()
                                    })
                            })
                        {
                            assignments_match_intents = false;
                            break;
                        }
                    }
                    if !assignments_match_intents {
                        self.cancel_invalid_signed_send_batch(
                            batch_record.batch_id,
                            &tx_bytes,
                            &assignments,
                            fee_sat,
                        )
                        .await?;
                        continue;
                    }

                    let expected_intent_count = assignments.len();
                    let mut batched_intents = Vec::new();
                    let mut abort_recovery = false;

                    for assignment in &assignments {
                        let id = assignment.intent_id;
                        let record = self.storage.get_send_intent(&id).await?;
                        match record {
                            Some(record)
                                if assignment.attempt_id.is_nil()
                                    || record.attempt_id != assignment.attempt_id =>
                            {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    assignment_attempt_id = %assignment.attempt_id,
                                    current_attempt_id = %record.attempt_id,
                                    "Signed batch recovery aborted because a member is unbound or a replacement attempt"
                                );
                                abort_recovery = true;
                                break;
                            }
                            Some(record) => match payment_intent::from_record(&record) {
                                SendIntentAny::Batched(intent)
                                    if intent.state.batch_id == batch_record.batch_id =>
                                {
                                    batched_intents.push(intent);
                                }
                                SendIntentAny::Pending(intent) => {
                                    tracing::warn!(
                                        batch_id = %batch_record.batch_id,
                                        intent_id = %id,
                                        "Repairing Signed batch member still stored as Pending"
                                    );
                                    match intent
                                        .assign_to_batch(&self.storage, batch_record.batch_id)
                                        .await
                                    {
                                        Ok(intent) => batched_intents.push(intent),
                                        Err(err) => {
                                            tracing::error!(
                                                batch_id = %batch_record.batch_id,
                                                intent_id = %id,
                                                error = %err,
                                                "Signed batch recovery aborted because Pending member could not be assigned"
                                            );
                                            abort_recovery = true;
                                            break;
                                        }
                                    }
                                }
                                SendIntentAny::Batched(intent) => {
                                    tracing::error!(
                                        batch_id = %batch_record.batch_id,
                                        intent_id = %id,
                                        intent_batch_id = %intent.state.batch_id,
                                        "Signed batch recovery aborted because a member references a different batch"
                                    );
                                    abort_recovery = true;
                                    break;
                                }
                                SendIntentAny::AwaitingConfirmation(_) => {
                                    tracing::error!(
                                        batch_id = %batch_record.batch_id,
                                        intent_id = %id,
                                        "Signed batch recovery aborted because a member is already advanced"
                                    );
                                    abort_recovery = true;
                                    break;
                                }
                                SendIntentAny::Failed => {
                                    tracing::error!(
                                        batch_id = %batch_record.batch_id,
                                        intent_id = %id,
                                        "Signed batch recovery aborted because a member is failed"
                                    );
                                    abort_recovery = true;
                                    break;
                                }
                            },
                            None => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member is missing"
                                );
                                abort_recovery = true;
                                break;
                            }
                        }
                    }

                    if abort_recovery || batched_intents.len() != expected_intent_count {
                        tracing::error!(
                            "Cancelling signed batch {} because not all members are recoverable",
                            batch_record.batch_id
                        );
                        let evict_at = crate::util::unix_now().saturating_add(1);
                        if !self
                            .cancel_signed_send_batch(
                                batch_record.batch_id,
                                &tx_bytes,
                                &assignments,
                                fee_sat,
                                evict_at,
                            )
                            .await?
                        {
                            tracing::info!(
                                batch_id = %batch_record.batch_id,
                                "Signed batch changed concurrently before recovery cancellation"
                            );
                        }
                        continue;
                    }

                    let txid = tx.compute_txid();
                    let txid_str = txid.to_string();

                    let signed_batch = crate::send::batch_transaction::SendBatch::<
                        crate::send::batch_transaction::state::Signed,
                    >::reconstruct(
                        batch_record.batch_id, batched_intents
                    );

                    // Re-read immediately before promotion, after any Pending
                    // member repairs, so a concurrent durable Broadcast cannot
                    // be followed by a second network transaction.
                    if self
                        .signed_batch_has_broadcast_conflict(batch_record.batch_id, &assignments)
                        .await?
                    {
                        tracing::error!(
                            batch_id = %batch_record.batch_id,
                            "Cancelling Signed batch after a concurrent Broadcast claim"
                        );
                        self.cancel_signed_send_batch(
                            batch_record.batch_id,
                            &tx_bytes,
                            &assignments,
                            fee_sat,
                            crate::util::unix_now().saturating_add(1),
                        )
                        .await?;
                        continue;
                    }

                    let broadcast_result = match signed_batch
                        .mark_broadcast(
                            &self.storage,
                            txid_str.clone(),
                            tx_bytes.clone(),
                            assignments.clone(),
                            fee_sat,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            tracing::error!(
                                "Failed to promote signed batch {} to Broadcast during recovery: {}",
                                batch_record.batch_id,
                                err
                            );
                            continue;
                        }
                    };

                    // Pair intents with their assignments by intent_id rather
                    // than positional index to avoid any hidden coupling.
                    let assignment_by_intent: HashMap<Uuid, &BatchOutputAssignment> =
                        assignments.iter().map(|a| (a.intent_id, a)).collect();

                    let mut all_intents_transitioned = true;
                    for intent in broadcast_result.intents {
                        let intent_id = intent.intent_id;
                        let Some(assignment) = assignment_by_intent.get(&intent_id) else {
                            tracing::error!(
                                batch_id = %batch_record.batch_id,
                                intent_id = %intent_id,
                                "Signed batch intent has no output assignment during recovery"
                            );
                            all_intents_transitioned = false;
                            break;
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
                            tracing::error!(
                                "Failed to transition signed batch intent {} to AwaitingConfirmation during recovery: {}",
                                intent_id,
                                err
                            );
                            all_intents_transitioned = false;
                            break;
                        }
                    }

                    if !all_intents_transitioned {
                        tracing::error!(
                            "Signed batch {} recovery aborted before broadcast because not all intents transitioned",
                            batch_record.batch_id
                        );
                        continue;
                    }

                    tracing::info!(
                        "Recovering signed batch {} by promoting to Broadcast and broadcasting transaction {}",
                        batch_record.batch_id,
                        txid_str
                    );

                    if !self
                        .apply_recovered_send_tx(
                            batch_record.batch_id,
                            "Signed batch recovery",
                            &tx,
                        )
                        .await
                    {
                        continue;
                    }

                    match self.broadcast_transaction_internal(tx).await {
                        Ok(BroadcastOutcome::Accepted) => {}
                        Ok(BroadcastOutcome::AlreadyKnown) => {
                            tracing::info!(
                                "Recovered signed batch {} txid {} was already known to backend",
                                batch_record.batch_id,
                                txid_str
                            );
                        }
                        Err(failure) => {
                            self.log_broadcast_failure(
                                "Signed batch recovery broadcast failed",
                                batch_record.batch_id,
                                &txid_str,
                                &failure,
                            );
                        }
                    }
                }
                state @ SendBatchState::Cancelled { .. } => {
                    if let Err(err) = self
                        .finish_cancelled_send_batch(batch_record.batch_id, &state)
                        .await
                    {
                        tracing::warn!(
                            batch_id = %batch_record.batch_id,
                            error = %err,
                            "Could not finish durable send-batch cancellation during recovery; continuing"
                        );
                        continue;
                    }
                }
                state @ SendBatchState::Broadcast { .. } => {
                    // A durable Broadcast marker is necessary but not sufficient
                    // authority for network I/O. Repair exact Batched members and
                    // fence corrupt, failed, or replacement attempts first.
                    let Some(tx) = self
                        .prepare_recovered_broadcast_tx(batch_record.batch_id, &state)
                        .await?
                    else {
                        continue;
                    };
                    let txid = tx.compute_txid();
                    tracing::info!("Re-broadcasting batch {} during recovery", txid);

                    match self.broadcast_transaction_internal(tx).await {
                        Ok(BroadcastOutcome::Accepted) => {}
                        Ok(BroadcastOutcome::AlreadyKnown) => {
                            tracing::info!("Recovery rebroadcast tx {} already known", txid);
                        }
                        Err(failure) => {
                            self.log_broadcast_failure(
                                "Broadcast batch recovery rebroadcast failed",
                                batch_record.batch_id,
                                &txid.to_string(),
                                &failure,
                            );
                        }
                    }
                }
            }
        }

        // Phase 2: Reconcile orphaned intents
        let persisted_intents = self.storage.get_all_send_intents().await?;
        let batches = self.storage.get_all_send_batches().await?;

        for persisted in persisted_intents {
            match payment_intent::from_record(&persisted) {
                SendIntentAny::Pending(_) => {}
                SendIntentAny::Failed => {}
                SendIntentAny::Batched(intent) => {
                    let intent_id = intent.intent_id;
                    let batch = batches.iter().find(|b| b.batch_id == intent.state.batch_id);
                    if let Some(batch) = batch {
                        let batch_intent_ids = batch_intent_ids(&batch.state);
                        if !batch_intent_ids.contains(&intent_id) {
                            if matches!(batch.state, SendBatchState::Broadcast { .. }) {
                                tracing::error!(
                                    batch_id = %batch.batch_id,
                                    intent_id = %intent_id,
                                    "Retaining mismatched Broadcast intent as recovery evidence"
                                );
                                continue;
                            }
                            tracing::warn!(
                                batch_id = %batch.batch_id,
                                intent_id = %intent_id,
                                "Intent references batch that does not list it; reverting to Pending"
                            );
                            if let Err(e) = intent.revert_to_pending(&self.storage).await {
                                tracing::error!(
                                    "Failed to revert mismatched intent {} during recovery: {}",
                                    intent_id,
                                    e
                                );
                            }
                            continue;
                        }

                        if matches!(batch.state, SendBatchState::Broadcast { .. }) {
                            // Phase 1 repairs only a fully valid Broadcast batch
                            // before network I/O. A Batched member still present
                            // here was fenced; do not rewind it into eligibility.
                            tracing::error!(
                                batch_id = %batch.batch_id,
                                intent_id = %intent_id,
                                "Leaving fenced Broadcast member unchanged for operator recovery"
                            );
                        }
                    } else {
                        tracing::info!(
                            "Orphaned batched intent {}, reverting to Pending",
                            intent_id
                        );
                        if let Err(e) = intent.revert_to_pending(&self.storage).await {
                            tracing::error!(
                                "Failed to revert orphaned intent {} during recovery: {}",
                                intent_id,
                                e
                            );
                        }
                    }
                }
                SendIntentAny::AwaitingConfirmation(intent) => {
                    let batch_id = intent.state.batch_id;
                    let batch = batches.iter().find(|b| b.batch_id == batch_id);

                    let orphan_reason = match batch {
                        None => Some("missing_batch"),
                        Some(batch)
                            if !batch_intent_ids(&batch.state).contains(&intent.intent_id) =>
                        {
                            Some("batch_does_not_list_intent")
                        }
                        Some(_) => None,
                    };

                    // Drive orphan intents forward using their persisted
                    // txid/outpoint/fee; warn otherwise.
                    if let Some(reason) = orphan_reason {
                        self.try_finalize_orphan_awaiting_intent(intent, batch_id, reason)
                            .await;
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn recover_receive_saga(&self) -> Result<(), Error> {
        tracing::info!("Recovering receive saga...");
        self.scan_for_new_payments().await
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::hashes::Hash as _;
    use bdk_wallet::bitcoin::transaction::Version;
    use bdk_wallet::bitcoin::{
        consensus, Amount as BtcAmount, OutPoint, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    };
    use bdk_wallet::KeychainKind;

    use super::*;
    use crate::send::batch_transaction::record::{
        BatchOutputAssignment, SendBatchRecord, SendBatchState,
    };
    use crate::send::payment_intent::SendIntent;
    use crate::testutil::{store_test_signed_batch, GatedKvStore, PausePoint, ReadPath};
    use crate::types::{PaymentMetadata, PaymentTier};

    const TEST_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    /// Build a `CdkBdk` test instance with a bogus Esplora URL. The sync
    /// loop is never started, so the unreachable URL is harmless; the
    /// BDK wallet is empty, which means `txid_has_required_confirmations`
    /// always returns `false` for any txid we ask about.
    async fn build_test_instance() -> CdkBdk {
        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");
        build_test_instance_with_kv(Arc::new(kv)).await
    }

    /// Build a `CdkBdk` test instance backed by the given KV store.
    async fn build_test_instance_with_kv(
        kv: Arc<dyn cdk_common::database::KVStore<Err = cdk_common::database::Error> + Send + Sync>,
    ) -> CdkBdk {
        crate::testutil::build_test_backend(kv, None).await
    }

    fn awaiting_intent(intent_id: Uuid, batch_id: Uuid, quote_id: &str) -> SendIntentRecord {
        SendIntentRecord {
            intent_id,
            attempt_id: intent_id,
            quote_id: quote_id.to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 25_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id,
                txid: TEST_TXID.to_string(),
                outpoint: format!("{TEST_TXID}:0"),
                fee_contribution_sat: 500,
                created_at: 1_700_000_000,
            },
        }
    }

    fn pending_intent(intent_id: Uuid, quote_id: &str) -> SendIntentRecord {
        SendIntentRecord {
            intent_id,
            attempt_id: intent_id,
            quote_id: quote_id.to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 25_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
        }
    }

    async fn wallet_relevant_send_tx_bytes(backend: &CdkBdk) -> Vec<u8> {
        let recipient_script = Address::from_str("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080")
            .expect("valid test address")
            .require_network(backend.network)
            .expect("test address matches backend network")
            .script_pubkey();
        let mut wallet_with_db = backend.wallet_with_db.lock().await;
        let funding_script = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .script_pubkey();
        let funding_tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: BtcAmount::from_sat(30_000),
                script_pubkey: funding_script,
            }],
        };
        let funding_outpoint = OutPoint::new(funding_tx.compute_txid(), 0);
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(funding_tx, crate::util::unix_now())]);
        wallet_with_db.persist().expect("persist funding tx");
        drop(wallet_with_db);

        let send_tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: funding_outpoint,
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: BtcAmount::from_sat(25_000),
                script_pubkey: recipient_script,
            }],
        };

        consensus::serialize(&send_tx)
    }

    async fn assert_still_awaiting(backend: &CdkBdk, intent_id: Uuid) {
        let fetched = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get_send_intent")
            .expect("intent still present");
        assert!(
            matches!(fetched.state, SendIntentState::AwaitingConfirmation { .. }),
            "intent should remain in AwaitingConfirmation, got {:?}",
            fetched.state
        );
        assert!(
            backend
                .storage
                .get_finalized_intent(&intent_id)
                .await
                .expect("get_finalized_intent")
                .is_none(),
            "no tombstone should exist for an unconfirmed orphan intent"
        );
    }

    async fn assert_wallet_knows_tx(backend: &CdkBdk, txid: &str) {
        let parsed_txid = bdk_wallet::bitcoin::Txid::from_str(txid).expect("test txid must parse");
        let wallet_with_db = backend.wallet_with_db.lock().await;
        assert!(
            wallet_with_db.wallet.get_tx(parsed_txid).is_some(),
            "recovered transaction must be applied to the BDK wallet graph"
        );
    }

    async fn assert_wallet_does_not_know_tx(backend: &CdkBdk, txid: &str) {
        let parsed_txid = bdk_wallet::bitcoin::Txid::from_str(txid).expect("test txid must parse");
        let wallet_with_db = backend.wallet_with_db.lock().await;
        assert!(
            wallet_with_db.wallet.get_tx(parsed_txid).is_none(),
            "cancelled transaction must be evicted from the BDK wallet graph"
        );
    }

    /// An `AwaitingConfirmation` intent whose batch record has been
    /// deleted but whose persisted txid is not known to the wallet must
    /// remain in `AwaitingConfirmation` after recovery — not silently
    /// finalized, not reverted, not crashed. The confirmation sync loop
    /// will finalize it later if the tx confirms.
    #[tokio::test]
    async fn test_recover_send_saga_missing_batch_leaves_intent_awaiting() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();

        backend
            .storage
            .create_send_intent_if_absent(&awaiting_intent(
                intent_id,
                batch_id,
                "quote-missing-batch",
            ))
            .await
            .expect("store awaiting intent");

        // Intentionally do not store any batch record for `batch_id`.

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        assert_still_awaiting(&backend, intent_id).await;
    }

    /// An `AwaitingConfirmation` intent that references a batch which
    /// exists but does not list the intent in its assignments is also an
    /// orphan. With the tx unknown to the wallet, recovery must warn
    /// and leave the intent in place.
    #[tokio::test]
    async fn test_recover_send_saga_batch_not_listing_intent_leaves_intent_awaiting() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let other_intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();

        backend
            .storage
            .create_send_intent_if_absent(&awaiting_intent(
                intent_id,
                batch_id,
                "quote-batch-missing-intent",
            ))
            .await
            .expect("store awaiting intent");

        // Batch exists but lists a different intent id.
        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Broadcast {
                txid: TEST_TXID.to_string(),
                tx_bytes: vec![0x01],
                assignments: vec![BatchOutputAssignment {
                    intent_id: other_intent_id,
                    attempt_id: other_intent_id,
                    vout: 0,
                    fee_contribution_sat: 500,
                }],
                fee_sat: 500,
            },
        };
        backend
            .storage
            .store_send_batch(&batch)
            .await
            .expect("store batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        assert_still_awaiting(&backend, intent_id).await;
    }

    /// Control test: an `AwaitingConfirmation` intent whose batch record
    /// exists and lists it is not an orphan. Recovery must leave it
    /// alone (same terminal state) regardless of confirmation status.
    #[tokio::test]
    async fn test_recover_send_saga_valid_batch_listing_intent_is_untouched() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();

        backend
            .storage
            .create_send_intent_if_absent(&awaiting_intent(intent_id, batch_id, "quote-valid"))
            .await
            .expect("store awaiting intent");

        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Broadcast {
                txid: TEST_TXID.to_string(),
                tx_bytes: vec![0x01],
                assignments: vec![BatchOutputAssignment {
                    intent_id,
                    attempt_id: intent_id,
                    vout: 0,
                    fee_contribution_sat: 500,
                }],
                fee_sat: 500,
            },
        };
        backend
            .storage
            .store_send_batch(&batch)
            .await
            .expect("store batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        assert_still_awaiting(&backend, intent_id).await;
    }

    /// If the process crashes after the signed transaction is persisted but
    /// before the linked intents are moved out of Pending, recovery must bind
    /// those intents to the signed batch and continue toward rebroadcast. It
    /// must not revert them into a fresh batch or fail/unlock them.
    #[tokio::test]
    async fn test_recover_send_saga_signed_batch_repairs_pending_member() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");
        let txid = tx.compute_txid().to_string();

        backend
            .storage
            .create_send_intent_if_absent(&pending_intent(intent_id, "quote-signed-pending"))
            .await
            .expect("store pending intent");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        let intent = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent still present");
        assert!(matches!(
            intent.state,
            SendIntentState::AwaitingConfirmation {
                batch_id: stored_batch_id,
                txid: ref stored_txid,
                ..
            } if stored_batch_id == batch_id && stored_txid == &txid
        ));

        let batch = backend
            .storage
            .get_send_batch(&batch_id)
            .await
            .expect("get batch")
            .expect("batch still present");
        assert!(matches!(batch.state, SendBatchState::Broadcast { .. }));
        assert_wallet_knows_tx(&backend, &txid).await;
    }

    #[tokio::test]
    async fn signed_recovery_cancels_assignment_to_wrong_output() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let valid_tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let mut tx: Transaction = consensus::deserialize(&valid_tx_bytes).expect("valid tx");
        tx.output[0].script_pubkey = Default::default();
        let tx_bytes = consensus::serialize(&tx);

        backend
            .storage
            .create_send_intent_if_absent(&pending_intent(intent_id, "quote-wrong-output"))
            .await
            .expect("store pending intent");
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        backend
            .recover_send_saga()
            .await
            .expect("invalid assignment should be compensated");

        let intent = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("read intent")
            .expect("intent remains active");
        assert!(matches!(intent.state, SendIntentState::Pending { .. }));
        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("read batch")
                .is_none(),
            "invalid Signed batch should be cancelled and removed"
        );
    }

    /// A Signed batch belongs to the attempt that produced its transaction,
    /// not merely to whichever retry currently reuses the same intent ID.
    #[tokio::test]
    async fn signed_recovery_rejects_replacement_attempt_with_same_intent_id() {
        let backend = build_test_instance().await;
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let quote_id = "quote-signed-replacement".to_string();
        let original = SendIntent::new(
            &backend.storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            25_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("store original attempt");
        let intent_id = original.intent_id;
        let original_attempt_id = original.attempt_id;

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: original_attempt_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store original signed batch");

        original
            .fail(&backend.storage, "retry after signing crash".to_string())
            .await
            .expect("fail original attempt");
        let replacement = SendIntent::new(
            &backend.storage,
            quote_id,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            25_000,
            750,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("store replacement attempt");
        assert_eq!(replacement.intent_id, intent_id);
        assert_ne!(replacement.attempt_id, original_attempt_id);
        assert!(backend
            .storage
            .transition_send_intent(
                &intent_id,
                &replacement.attempt_id,
                &SendIntentState::Pending {
                    created_at: replacement.created_at,
                },
                &SendIntentState::Batched {
                    batch_id,
                    created_at: replacement.created_at,
                },
            )
            .await
            .expect("persist replacement as Batched"));

        backend
            .recover_send_saga()
            .await
            .expect("recover send saga");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get replacement")
            .expect("replacement remains");
        assert_eq!(persisted.attempt_id, replacement.attempt_id);
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
        assert!(backend
            .storage
            .get_send_batch(&batch_id)
            .await
            .expect("get cancelled batch")
            .is_none());
    }

    /// Legacy assignments remain readable, but their missing attempt binding
    /// is not sufficient evidence to attach an intent to a signed transaction.
    #[tokio::test]
    async fn signed_recovery_cancels_legacy_unbound_assignment() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let mut legacy_intent = pending_intent(intent_id, "quote-legacy-signed");
        legacy_intent.attempt_id = Uuid::nil();
        backend
            .storage
            .create_send_intent_if_absent(&legacy_intent)
            .await
            .expect("store legacy intent");
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: Uuid::nil(),
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store legacy signed batch");

        backend
            .recover_send_saga()
            .await
            .expect("recover legacy signed batch");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get legacy intent")
            .expect("legacy intent remains");
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
        assert!(backend
            .storage
            .get_send_batch(&batch_id)
            .await
            .expect("get legacy batch")
            .is_none());
    }

    #[tokio::test]
    async fn signed_recovery_cancels_duplicate_intent_assignments() {
        let backend = build_test_instance().await;
        let first_intent_id = Uuid::new_v4();
        let second_intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;

        for (intent_id, quote_id) in [
            (first_intent_id, "quote-duplicate-first"),
            (second_intent_id, "quote-duplicate-second"),
        ] {
            backend
                .storage
                .create_send_intent_if_absent(&pending_intent(intent_id, quote_id))
                .await
                .expect("store pending intent");
        }

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![
                        BatchOutputAssignment {
                            intent_id: first_intent_id,
                            attempt_id: first_intent_id,
                            vout: 0,
                            fee_contribution_sat: 500,
                        },
                        BatchOutputAssignment {
                            intent_id: first_intent_id,
                            attempt_id: first_intent_id,
                            vout: 1,
                            fee_contribution_sat: 500,
                        },
                    ],
                    fee_sat: 1_000,
                },
            })
            .await
            .expect("store malformed signed batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should reject malformed batch without failing startup");

        for intent_id in [first_intent_id, second_intent_id] {
            let intent = backend
                .storage
                .get_send_intent(&intent_id)
                .await
                .expect("read intent")
                .expect("intent remains active");
            assert!(matches!(intent.state, SendIntentState::Pending { .. }));
        }

        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("read batch")
                .is_none(),
            "malformed signed batch must be durably cancelled and cleaned up"
        );
    }

    /// Recovery must finish a durable cancellation left behind by a crash
    /// after the signed transaction entered the wallet graph. The wallet tx,
    /// batch record, and `Batched` intent must all be compensated.
    #[tokio::test]
    async fn cancelled_recovery_evicts_wallet_tx_and_reverts_intent() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");
        let txid = tx.compute_txid().to_string();
        let apply_at = crate::util::unix_now();

        {
            let mut wallet_with_db = backend.wallet_with_db.lock().await;
            wallet_with_db
                .wallet
                .apply_unconfirmed_txs([(tx, apply_at)]);
            wallet_with_db.persist().expect("persist signed tx");
        }
        assert_wallet_knows_tx(&backend, &txid).await;

        let mut intent = pending_intent(intent_id, "quote-cancelled-recovery");
        intent.state = SendIntentState::Batched {
            batch_id,
            created_at: apply_at,
        };
        backend
            .storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store batched intent");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Cancelled {
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                    // Exercise recovery from a stale durable marker. Cleanup
                    // must advance it beyond the wallet's current last_seen.
                    evict_at: apply_at.saturating_sub(1),
                },
            })
            .await
            .expect("store cancelled batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should finish cancellation");

        let intent = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("read intent")
            .expect("intent remains active");
        assert!(matches!(intent.state, SendIntentState::Pending { .. }));
        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("read batch")
                .is_none(),
            "completed cancellation must delete its durable marker"
        );
        assert_wallet_does_not_know_tx(&backend, &txid).await;
    }

    #[tokio::test]
    async fn cancelled_recovery_failure_does_not_abort_orphan_reconciliation() {
        let backend = build_test_instance().await;
        let cancelled_batch_id = Uuid::new_v4();
        let orphan_batch_id = Uuid::new_v4();
        let orphan_intent_id = Uuid::new_v4();

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id: cancelled_batch_id,
                state: SendBatchState::Cancelled {
                    tx_bytes: vec![0xff],
                    assignments: Vec::new(),
                    fee_sat: 500,
                    evict_at: crate::util::unix_now(),
                },
            })
            .await
            .expect("store malformed cancelled batch");

        let mut orphan = pending_intent(orphan_intent_id, "quote-cancelled-failure-orphan");
        orphan.state = SendIntentState::Batched {
            batch_id: orphan_batch_id,
            created_at: crate::util::unix_now(),
        };
        backend
            .storage
            .create_send_intent_if_absent(&orphan)
            .await
            .expect("store orphaned intent");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("one malformed cancellation should not abort recovery");

        let persisted = backend
            .storage
            .get_send_intent(&orphan_intent_id)
            .await
            .expect("read orphaned intent")
            .expect("orphaned intent remains active");
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
        assert!(
            backend
                .storage
                .get_send_batch(&cancelled_batch_id)
                .await
                .expect("read malformed cancelled batch")
                .is_some(),
            "failed cancellation should remain durable for a later retry"
        );
    }

    #[tokio::test]
    async fn cancelled_batch_does_not_compensate_replacement_attempt() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let original_attempt_id = Uuid::new_v4();
        let replacement_attempt_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let mut replacement = pending_intent(intent_id, "quote-cancelled-replacement");
        replacement.attempt_id = replacement_attempt_id;
        replacement.state = SendIntentState::Batched {
            batch_id,
            created_at: 1_700_000_000,
        };
        backend
            .storage
            .create_send_intent_if_absent(&replacement)
            .await
            .expect("store replacement intent");
        let cancelled_state = SendBatchState::Cancelled {
            tx_bytes,
            assignments: vec![BatchOutputAssignment {
                intent_id,
                attempt_id: original_attempt_id,
                vout: 0,
                fee_contribution_sat: 500,
            }],
            fee_sat: 500,
            evict_at: crate::util::unix_now().saturating_add(1),
        };
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: cancelled_state.clone(),
            })
            .await
            .expect("store cancelled batch");

        backend
            .finish_cancelled_send_batch(batch_id, &cancelled_state)
            .await
            .expect("finish cancellation");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get replacement")
            .expect("replacement remains");
        assert_eq!(persisted.attempt_id, replacement_attempt_id);
        assert!(matches!(
            persisted.state,
            SendIntentState::Batched {
                batch_id: stored_batch_id,
                ..
            } if stored_batch_id == batch_id
        ));
    }

    #[tokio::test]
    async fn cancelled_batch_rejects_late_pending_claim() {
        let backend = build_test_instance().await;
        let batch_id = Uuid::new_v4();
        let pending = SendIntent::new(
            &backend.storage,
            "quote-cancel-wins".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            25_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("store pending intent");
        let intent_id = pending.intent_id;
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let assignments = vec![BatchOutputAssignment {
            intent_id,
            attempt_id: pending.attempt_id,
            vout: 0,
            fee_contribution_sat: 500,
        }];
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        assert!(backend
            .cancel_signed_send_batch(
                batch_id,
                &tx_bytes,
                &assignments,
                500,
                crate::util::unix_now().saturating_add(1),
            )
            .await
            .expect("cancel signed batch"));

        let error = pending
            .assign_to_batch(&backend.storage, batch_id)
            .await
            .expect_err("a deleted cancelled batch cannot accept a late claim");
        assert!(matches!(
            error,
            Error::SendBatchStateConflict {
                batch_id: id,
                expected: "Signed",
            } if id == batch_id
        ));
        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("read intent")
            .expect("intent remains active");
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
    }

    #[tokio::test]
    async fn cancelled_batch_compensates_claim_that_wins_first() {
        let backend = build_test_instance().await;
        let batch_id = Uuid::new_v4();
        let pending = SendIntent::new(
            &backend.storage,
            "quote-claim-wins".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            25_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("store pending intent");
        let intent_id = pending.intent_id;
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let assignments = vec![BatchOutputAssignment {
            intent_id,
            attempt_id: pending.attempt_id,
            vout: 0,
            fee_contribution_sat: 500,
        }];
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        pending
            .assign_to_batch(&backend.storage, batch_id)
            .await
            .expect("claim intent while batch is signed");
        assert!(backend
            .cancel_signed_send_batch(
                batch_id,
                &tx_bytes,
                &assignments,
                500,
                crate::util::unix_now().saturating_add(1),
            )
            .await
            .expect("cancel signed batch"));

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("read intent")
            .expect("intent remains active");
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
        assert!(backend
            .storage
            .get_send_batch(&batch_id)
            .await
            .expect("read batch")
            .is_none());
    }

    /// If recovery repairs one Pending member before finding a later member
    /// that cannot belong to the Signed batch, durable cancellation must undo
    /// that partial repair instead of stranding it in `Batched`.
    #[tokio::test]
    async fn signed_recovery_cancels_partially_repaired_members() {
        let backend = build_test_instance().await;
        let repaired_intent_id = Uuid::new_v4();
        let advanced_intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let replacement_batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;

        backend
            .storage
            .create_send_intent_if_absent(&pending_intent(
                repaired_intent_id,
                "quote-partial-repair",
            ))
            .await
            .expect("store pending intent");
        backend
            .storage
            .create_send_intent_if_absent(&awaiting_intent(
                advanced_intent_id,
                replacement_batch_id,
                "quote-advanced-member",
            ))
            .await
            .expect("store advanced intent");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![
                        BatchOutputAssignment {
                            intent_id: repaired_intent_id,
                            attempt_id: repaired_intent_id,
                            vout: 0,
                            fee_contribution_sat: 250,
                        },
                        BatchOutputAssignment {
                            intent_id: advanced_intent_id,
                            attempt_id: advanced_intent_id,
                            vout: 1,
                            fee_contribution_sat: 250,
                        },
                    ],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should cancel unrecoverable batch");

        let repaired = backend
            .storage
            .get_send_intent(&repaired_intent_id)
            .await
            .expect("read repaired intent")
            .expect("repaired intent remains active");
        assert!(matches!(repaired.state, SendIntentState::Pending { .. }));

        let advanced = backend
            .storage
            .get_send_intent(&advanced_intent_id)
            .await
            .expect("read advanced intent")
            .expect("advanced intent remains active");
        assert!(matches!(
            advanced.state,
            SendIntentState::AwaitingConfirmation {
                batch_id: stored_batch_id,
                ..
            } if stored_batch_id == replacement_batch_id
        ));
        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("read batch")
                .is_none(),
            "unrecoverable signed batch must be cancelled"
        );
    }

    /// A persisted Broadcast batch may represent a crash after durable state
    /// was written but before the tx was accepted or before the BDK wallet
    /// graph was persisted. Recovery must apply the tx locally so its inputs
    /// are not selected by later batches while rebroadcast/reconciliation
    /// catches up.
    #[tokio::test]
    async fn test_recover_send_saga_broadcast_batch_applies_tx_to_wallet_graph() {
        let backend = build_test_instance().await;
        let batch_id = Uuid::new_v4();
        let intent_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");
        let txid = tx.compute_txid().to_string();

        let mut awaiting = awaiting_intent(intent_id, batch_id, "quote-broadcast-wallet-graph");
        awaiting.state = SendIntentState::AwaitingConfirmation {
            batch_id,
            txid: txid.clone(),
            outpoint: format!("{txid}:0"),
            fee_contribution_sat: 500,
            created_at: 1_700_000_000,
        };
        backend
            .storage
            .create_send_intent_if_absent(&awaiting)
            .await
            .expect("store awaiting intent");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: txid.clone(),
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store broadcast batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        assert_still_awaiting(&backend, intent_id).await;
        assert_wallet_knows_tx(&backend, &txid).await;
    }

    /// If a crash leaves an intent in `Batched` while its batch is already
    /// `Broadcast`, recovery must repair that exact member before reserving and
    /// rebroadcasting the transaction.
    #[tokio::test]
    async fn test_recover_send_saga_repairs_exact_batched_broadcast_member() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");
        let computed_txid = tx.compute_txid().to_string();

        let mut batched = pending_intent(intent_id, "quote-batched-repair");
        batched.state = SendIntentState::Batched {
            batch_id,
            created_at: 1_700_000_000,
        };
        backend
            .storage
            .create_send_intent_if_absent(&batched)
            .await
            .expect("store batched intent");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: computed_txid.clone(),
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store broadcast batch");

        tokio::time::timeout(Duration::from_secs(5), backend.recover_send_saga())
            .await
            .expect("recovery timed out")
            .expect("recovery should not error");

        let intent = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent still present");

        match intent.state {
            SendIntentState::AwaitingConfirmation { txid, outpoint, .. } => {
                assert_eq!(txid, computed_txid);
                assert_eq!(outpoint, format!("{computed_txid}:0"));
            }
            other => panic!("expected AwaitingConfirmation, got {:?}", other),
        }
    }

    /// Broadcast orphan repair must not attach a replacement attempt to a
    /// transaction signed for an older generation of the same intent.
    #[tokio::test]
    async fn broadcast_recovery_rejects_replacement_attempt() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let original_attempt_id = Uuid::new_v4();
        let replacement_attempt_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");

        let mut replacement = pending_intent(intent_id, "quote-broadcast-replacement");
        replacement.attempt_id = replacement_attempt_id;
        replacement.state = SendIntentState::Batched {
            batch_id,
            created_at: 1_700_000_000,
        };
        backend
            .storage
            .create_send_intent_if_absent(&replacement)
            .await
            .expect("store replacement attempt");
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: tx.compute_txid().to_string(),
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: original_attempt_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store original broadcast batch");

        backend
            .recover_send_saga()
            .await
            .expect("recover broadcast batch");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get replacement")
            .expect("replacement remains");
        assert_eq!(persisted.attempt_id, replacement_attempt_id);
        assert!(matches!(persisted.state, SendIntentState::Batched { .. }));
        assert_wallet_knows_tx(&backend, &tx.compute_txid().to_string()).await;
        backend
            .cleanup_completed_batches()
            .await
            .expect("cleanup must retain mismatch evidence");
        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("get fenced batch")
                .is_some(),
            "replacement mismatch evidence must be retained"
        );
    }

    #[tokio::test]
    async fn broadcast_recovery_fences_failed_member_and_retains_evidence() {
        let backend = build_test_instance().await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;
        let tx: Transaction = consensus::deserialize(&tx_bytes).expect("valid tx");
        let txid = tx.compute_txid().to_string();

        let mut failed = pending_intent(intent_id, "quote-broadcast-failed");
        failed.state = SendIntentState::Failed {
            reason: "terminal".to_string(),
            created_at: 1_700_000_000,
            failed_at: 1_700_000_001,
        };
        backend
            .storage
            .create_send_intent_if_absent(&failed)
            .await
            .expect("store failed intent");
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: txid.clone(),
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store Broadcast batch");

        backend
            .recover_send_saga()
            .await
            .expect("recover fenced Broadcast batch");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get failed intent")
            .expect("failed intent remains");
        assert!(matches!(persisted.state, SendIntentState::Failed { .. }));
        assert_wallet_knows_tx(&backend, &txid).await;
        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("get fenced batch")
                .is_some(),
            "failed-member evidence must be retained"
        );
    }

    /// A Signed batch member still stored as Pending is repaired by
    /// claiming it for the batch. If another instance advances the intent
    /// after recovery read it, the claim must abort and cancel the old batch
    /// instead of rewinding the record or rebroadcasting its transaction.
    #[tokio::test]
    async fn signed_recovery_pending_repair_rejects_stale_snapshot() {
        let kv = GatedKvStore::default();
        let backend = build_test_instance_with_kv(Arc::new(kv.clone())).await;
        let intent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let tx_bytes = wallet_relevant_send_tx_bytes(&backend).await;

        backend
            .storage
            .create_send_intent_if_absent(&pending_intent(intent_id, "quote-stale-repair"))
            .await
            .expect("store pending intent");
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes,
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: intent_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store signed batch");

        let gate = kv.gate_read(
            ReadPath::Direct,
            PausePoint::AfterRead,
            crate::storage::BDK_NAMESPACE,
            crate::storage::SEND_INTENT_NAMESPACE,
            2,
        );

        let recover_backend = backend.clone();
        let recovery = tokio::spawn(async move { recover_backend.recover_send_saga().await });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("recovery reached the gated read");

        let record = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        let pending = match payment_intent::from_record(&record) {
            SendIntentAny::Pending(intent) => intent,
            _ => panic!("intent should be pending"),
        };
        let replacement_batch = Uuid::new_v4();
        store_test_signed_batch(&backend.storage, replacement_batch, &[intent_id]).await;
        pending
            .assign_to_batch(&backend.storage, replacement_batch)
            .await
            .expect("other instance claims intent")
            .mark_broadcast(
                &backend.storage,
                TEST_TXID.to_string(),
                format!("{TEST_TXID}:0"),
                500,
            )
            .await
            .expect("other instance broadcasts intent");

        gate.release();
        tokio::time::timeout(Duration::from_secs(5), recovery)
            .await
            .expect("recovery timed out")
            .expect("join recovery task")
            .expect("recovery should not error");

        let persisted = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(
                persisted.state,
                SendIntentState::AwaitingConfirmation {
                    batch_id: b,
                    ..
                } if b == replacement_batch
            ),
            "intent must remain AwaitingConfirmation under the replacement batch, got {:?}",
            persisted.state
        );

        assert!(
            backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("get batch")
                .is_none(),
            "stale signed batch must be cancelled"
        );
    }
}
