use std::collections::HashSet;

use bdk_wallet::bitcoin::Transaction;
use uuid::Uuid;

use crate::error::Error;
use crate::send::batch_transaction::allocate_batch_fee;
use crate::send::batch_transaction::record::SendBatchState;
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

fn batch_intent_ids(batch_state: &SendBatchState) -> &[Uuid] {
    match batch_state {
        SendBatchState::Built { intent_ids, .. }
        | SendBatchState::Signed { intent_ids, .. }
        | SendBatchState::Broadcast { intent_ids, .. } => intent_ids,
    }
}

fn intent_batch_id(state: &SendIntentState) -> Option<Uuid> {
    match state {
        SendIntentState::Pending { .. } => None,
        SendIntentState::Batched { batch_id, .. }
        | SendIntentState::AwaitingConfirmation { batch_id, .. } => Some(*batch_id),
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
                SendIntentState::Pending { .. } => {
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
            let batch_records =
                load_batch_intents(&self.storage, batch_intent_ids(&batch_record.state)).await?;
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
                    intent_ids,
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

                    let expected_intent_count = intent_ids.len();
                    let mut batched_intents = Vec::new();
                    let mut max_fees = Vec::new();
                    let mut valid_intent_ids = Vec::new();

                    for id in intent_ids {
                        let record = self.storage.get_send_intent(&id).await?;
                        match self
                            .classify_batch_intent_relation(batch_record.batch_id, record.as_ref())
                        {
                            BatchIntentRelation::Valid => {
                                if let Some(record) = record {
                                    max_fees.push(record.max_fee_amount_sat);
                                    valid_intent_ids.push(id);

                                    if let SendIntentAny::Batched(intent) =
                                        payment_intent::from_record(&record)
                                    {
                                        batched_intents.push(intent);
                                    }
                                }
                            }
                            BatchIntentRelation::MissingIntent => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member is missing"
                                );
                                batched_intents.clear();
                                valid_intent_ids.clear();
                                max_fees.clear();
                                break;
                            }
                            BatchIntentRelation::IntentReferencesDifferentBatch => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member references a different batch"
                                );
                                batched_intents.clear();
                                valid_intent_ids.clear();
                                max_fees.clear();
                                break;
                            }
                            BatchIntentRelation::IntentAlreadyAdvanced => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member is already advanced"
                                );
                                batched_intents.clear();
                                valid_intent_ids.clear();
                                max_fees.clear();
                                break;
                            }
                        }
                    }

                    if batched_intents.len() != expected_intent_count
                        || valid_intent_ids.len() != expected_intent_count
                    {
                        tracing::error!(
                            "Signed batch {} recovery aborted because not all members are recoverable",
                            batch_record.batch_id
                        );
                        continue;
                    }

                    let fee_allocations =
                        allocate_batch_fee(fee_sat, &max_fees, &valid_intent_ids)?;
                    let broadcast_details =
                        self.collect_intent_broadcast_details(&tx, &batched_intents)?;
                    let txid = tx.compute_txid().to_string();

                    let signed_batch = crate::send::batch_transaction::SendBatch::<
                        crate::send::batch_transaction::state::Signed,
                    >::reconstruct(
                        batch_record.batch_id, batched_intents
                    );

                    let broadcast_result = match signed_batch
                        .mark_broadcast(&self.storage, txid.clone(), tx_bytes.clone(), fee_sat)
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

                    let mut all_intents_transitioned = true;
                    for (idx, intent) in broadcast_result.intents.into_iter().enumerate() {
                        let intent_id = intent.intent_id;
                        let (intent_txid, outpoint) = &broadcast_details[idx];

                        if let Err(err) = intent
                            .mark_broadcast(
                                &self.storage,
                                intent_txid.clone(),
                                outpoint.clone(),
                                fee_allocations[idx],
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
                        txid
                    );

                    if let Err(err) = self.broadcast_transaction_internal(tx).await {
                        tracing::error!(
                            "Failed to broadcast signed batch {} during recovery: {}",
                            batch_record.batch_id,
                            err
                        );
                    }
                }
                SendBatchState::Broadcast { txid, tx_bytes, .. } => {
                    if let Ok(tx) =
                        bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(&tx_bytes)
                    {
                        // Broadcast state is persisted before the network send,
                        // so recovery intentionally retries rebroadcast here if a
                        // crash happened after persistence but before backend
                        // acceptance was observed.
                        tracing::info!("Re-broadcasting batch {} during recovery", txid);
                        let _ = self.broadcast_transaction_internal(tx).await;
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
                SendIntentAny::Batched(intent) => {
                    let intent_id = intent.intent_id;
                    let batch = batches.iter().find(|b| b.batch_id == intent.state.batch_id);
                    if let Some(batch) = batch {
                        let batch_intent_ids = batch_intent_ids(&batch.state);
                        if !batch_intent_ids.contains(&intent_id) {
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

                        if let SendBatchState::Broadcast {
                            txid,
                            tx_bytes,
                            fee_sat,
                            ..
                        } = &batch.state
                        {
                            if let Ok(tx) =
                                bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(tx_bytes)
                            {
                                let mut intent_ids = Vec::new();
                                let mut max_fees = Vec::new();

                                if let SendBatchState::Broadcast {
                                    intent_ids: ids, ..
                                } = &batch.state
                                {
                                    for id in ids {
                                        if let Some(record) =
                                            self.storage.get_send_intent(id).await?
                                        {
                                            if intent_batch_id(&record.state)
                                                != Some(batch.batch_id)
                                            {
                                                tracing::warn!(
                                                    batch_id = %batch.batch_id,
                                                    intent_id = %id,
                                                    "Skipping batch member with mismatched batch reference during broadcast repair"
                                                );
                                                continue;
                                            }

                                            intent_ids.push(*id);
                                            max_fees.push(record.max_fee_amount_sat);
                                        }
                                    }
                                }

                                // Re-run allocation
                                let fee_allocations =
                                    allocate_batch_fee(*fee_sat, &max_fees, &intent_ids)?;

                                // Find our intent's fee
                                let our_fee = intent_ids
                                    .iter()
                                    .position(|&id| id == intent_id)
                                    .and_then(|idx| fee_allocations.get(idx).copied())
                                    .unwrap_or(0);

                                let details = self.collect_intent_broadcast_details(
                                    &tx,
                                    std::slice::from_ref(&intent),
                                )?;
                                let (_, outpoint) = &details[0];

                                if let Err(e) = intent
                                    .mark_broadcast(
                                        &self.storage,
                                        txid.clone(),
                                        outpoint.clone(),
                                        our_fee,
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to repair broadcast intent {} during recovery: {}",
                                        intent_id,
                                        e
                                    );
                                }
                            }
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

                    match batch {
                        None => {
                            tracing::warn!(
                                batch_id = %batch_id,
                                intent_id = %intent.intent_id,
                                "AwaitingConfirmation intent references missing batch"
                            );
                        }
                        Some(batch)
                            if !batch_intent_ids(&batch.state).contains(&intent.intent_id) =>
                        {
                            tracing::warn!(
                                batch_id = %batch.batch_id,
                                intent_id = %intent.intent_id,
                                "AwaitingConfirmation intent references batch that does not include it"
                            );
                        }
                        Some(_) => {}
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
