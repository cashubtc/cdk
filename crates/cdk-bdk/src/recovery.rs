use std::collections::{HashMap, HashSet};

use bdk_wallet::bitcoin::{OutPoint, Transaction};
use uuid::Uuid;

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

                    let expected_intent_count = assignments.len();
                    let mut batched_intents = Vec::new();
                    let mut abort_recovery = false;

                    for assignment in &assignments {
                        let id = assignment.intent_id;
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
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member is missing"
                                );
                                abort_recovery = true;
                                break;
                            }
                            BatchIntentRelation::IntentReferencesDifferentBatch => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member references a different batch"
                                );
                                abort_recovery = true;
                                break;
                            }
                            BatchIntentRelation::IntentAlreadyAdvanced => {
                                tracing::error!(
                                    batch_id = %batch_record.batch_id,
                                    intent_id = %id,
                                    "Signed batch recovery aborted because a member is already advanced"
                                );
                                abort_recovery = true;
                                break;
                            }
                        }
                    }

                    if abort_recovery || batched_intents.len() != expected_intent_count {
                        tracing::error!(
                            "Signed batch {} recovery aborted because not all members are recoverable",
                            batch_record.batch_id
                        );
                        continue;
                    }

                    let txid = tx.compute_txid();
                    let txid_str = txid.to_string();

                    let signed_batch = crate::send::batch_transaction::SendBatch::<
                        crate::send::batch_transaction::state::Signed,
                    >::reconstruct(
                        batch_record.batch_id, batched_intents
                    );

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
                            assignments,
                            ..
                        } = &batch.state
                        {
                            // Use the persisted assignment to attribute the
                            // correct vout and fee. Re-deriving from tx outputs
                            // here would be ambiguous when two intents in the
                            // same batch share (address, amount).
                            let Some(assignment) =
                                assignments.iter().find(|a| a.intent_id == intent_id)
                            else {
                                tracing::error!(
                                    batch_id = %batch.batch_id,
                                    intent_id = %intent_id,
                                    "Broadcast batch has no output assignment for orphan intent during recovery; skipping"
                                );
                                continue;
                            };

                            // Compute the txid from the persisted bytes so we
                            // do not trust the record's txid field blindly.
                            let computed_txid = match bdk_wallet::bitcoin::consensus::deserialize::<
                                Transaction,
                            >(tx_bytes)
                            {
                                Ok(tx) => tx.compute_txid(),
                                Err(err) => {
                                    tracing::error!(
                                        batch_id = %batch.batch_id,
                                        intent_id = %intent_id,
                                        error = %err,
                                        "Failed to deserialize broadcast batch tx during orphan repair"
                                    );
                                    continue;
                                }
                            };
                            let outpoint =
                                OutPoint::new(computed_txid, assignment.vout).to_string();

                            if let Err(e) = intent
                                .mark_broadcast(
                                    &self.storage,
                                    txid.clone(),
                                    outpoint,
                                    assignment.fee_contribution_sat,
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

    use bdk_wallet::bitcoin::Network;
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::common::FeeReserve;
    use cdk_common::{Amount, CurrencyUnit};

    use super::*;
    use crate::send::batch_transaction::record::{
        BatchOutputAssignment, SendBatchRecord, SendBatchState,
    };
    use crate::types::{PaymentMetadata, PaymentTier};
    use crate::{ChainSource, EsploraConfig};

    const TEST_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    /// Build a `CdkBdk` test instance with a bogus Esplora URL. The sync
    /// loop is never started, so the unreachable URL is harmless; the
    /// BDK wallet is empty, which means `txid_has_required_confirmations`
    /// always returns `false` for any txid we ask about.
    async fn build_test_instance() -> CdkBdk {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Leak the tempdir so it outlives the test — CdkBdk opens a
        // sqlite file under it and we don't want the path disappearing
        // while recovery is running.
        let path = tmp.keep();
        let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");

        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");

        let chain_source = ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        });

        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.02,
        };

        CdkBdk::new(
            mnemonic,
            Network::Regtest,
            chain_source,
            path.to_string_lossy().into_owned(),
            fee_reserve,
            Arc::new(kv),
            None,
            1,
            0,
            60,
            Some(5),
            None,
        )
        .expect("build CdkBdk test instance")
    }

    fn awaiting_intent(intent_id: Uuid, batch_id: Uuid, quote_id: &str) -> SendIntentRecord {
        SendIntentRecord {
            intent_id,
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
}
