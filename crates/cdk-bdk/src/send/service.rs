use std::str::FromStr;

use bdk_wallet::bitcoin::{Address, OutPoint, Transaction};
use cdk_common::payment::{Event, MakePaymentResponse, PaymentIdentifier};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState, QuoteId};
use tokio::time::interval;
use uuid::Uuid;

use crate::chain::{BroadcastErrorKind, BroadcastFailure, BroadcastOutcome};
use crate::error::Error;
use crate::fee::fee_rate_from_sat_per_vb;
use crate::send::batch_transaction::record::{
    BatchOutputAssignment, SendBatchRecord, SendBatchState,
};
use crate::send::batch_transaction::{allocate_batch_fee, state as batch_state, SendBatch};
use crate::send::payment_intent::{self, state as intent_state, SendIntent, SendIntentAny};
use crate::types::PaymentTier;
use crate::util::parse_checked_address;
use crate::CdkBdk;

impl CdkBdk {
    async fn fail_claimed_send_intents(
        &self,
        intents: &[SendIntent<intent_state::BatchClaimed>],
        reason: &str,
    ) {
        for intent in intents {
            let failed_at = crate::util::unix_now();
            if let Err(err) = self
                .storage
                .update_send_intent(
                    &intent.intent_id,
                    &crate::send::payment_intent::record::SendIntentState::Failed {
                        reason: reason.to_string(),
                        created_at: intent.created_at,
                        failed_at,
                    },
                )
                .await
            {
                tracing::error!(
                    "Failed to mark claimed send intent {} failed after terminal batch failure: {}",
                    intent.intent_id,
                    err
                );
            }

            if let Ok(quote_id) = QuoteId::from_str(&intent.quote_id) {
                if let Err(err) = self.payment_sender.send(Event::PaymentFailed {
                    quote_id,
                    reason: reason.to_string(),
                }) {
                    tracing::error!(
                        "Could not send payment failed event for intent {}: {}",
                        intent.intent_id,
                        err
                    );
                }
            }
        }
    }

    pub(crate) async fn finalize_send_intent_and_emit(
        &self,
        intent: SendIntent<intent_state::AwaitingConfirmation>,
    ) -> Result<(), Error> {
        let intent_id = intent.intent_id;
        let quote_id = intent.quote_id.clone();
        let amount = intent.amount;
        let fee = intent.state.fee_contribution_sat;
        let outpoint = intent.state.outpoint.clone();

        intent.finalize(&self.storage).await.map_err(|e| {
            tracing::error!("Failed to finalize send intent {}: {}", intent_id, e);
            e
        })?;

        if let Ok(quote_id) = QuoteId::from_str(&quote_id) {
            let details = MakePaymentResponse {
                payment_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                payment_proof: Some(outpoint),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(amount + fee, CurrencyUnit::Sat),
            };

            if let Err(err) = self
                .payment_sender
                .send(Event::PaymentSuccessful { quote_id, details })
            {
                tracing::error!(
                    "Could not send payment successful event for intent {}: {}",
                    intent_id,
                    err
                );
            }
        }

        Ok(())
    }

    /// Finalize an orphan `AwaitingConfirmation` intent if its persisted
    /// txid has reached the required confirmation depth; otherwise warn and
    /// leave it for the confirmation sync loop.
    pub(crate) async fn try_finalize_orphan_awaiting_intent(
        &self,
        intent: SendIntent<intent_state::AwaitingConfirmation>,
        batch_id: Uuid,
        orphan_reason: &'static str,
    ) {
        let intent_id = intent.intent_id;
        let txid = intent.state.txid.clone();

        let has_confs = {
            let wallet_with_db = self.wallet_with_db.lock().await;
            self.txid_has_required_confirmations(
                &wallet_with_db.wallet,
                &txid,
                "send_intent_recovery",
                &intent_id.to_string(),
            )
        };

        if has_confs {
            tracing::warn!(
                batch_id = %batch_id,
                intent_id = %intent_id,
                txid = %txid,
                orphan_reason,
                "Orphan AwaitingConfirmation intent has reached required \
                 confirmations during recovery; finalizing"
            );
            if let Err(err) = self.finalize_send_intent_and_emit(intent).await {
                tracing::error!(
                    batch_id = %batch_id,
                    intent_id = %intent_id,
                    error = %err,
                    "Failed to finalize orphan AwaitingConfirmation intent during recovery"
                );
            }
        } else {
            tracing::warn!(
                batch_id = %batch_id,
                intent_id = %intent_id,
                txid = %txid,
                orphan_reason,
                "Orphan AwaitingConfirmation intent not yet confirmed; \
                 the confirmation sync loop will finalize it once the tx \
                 reaches the required depth"
            );
        }
    }

    pub(crate) fn fee_reserve_for_estimate(&self, estimated_sat: u64) -> u64 {
        let percent_padded =
            (estimated_sat as f64 * (1.0 + self.fee_reserve.percent_fee_reserve as f64)) as u64;
        let min_reserve = self.fee_reserve.min_fee_reserve.into();
        std::cmp::max(percent_padded, min_reserve)
    }

    /// Derive the `intent_id -> vout` mapping for a freshly built batch
    /// transaction.
    ///
    /// Walks the transaction outputs once, with the full intent list, claiming
    /// each output to at most one intent. The resulting assignments are
    /// persisted in the batch's Signed state and reused verbatim through
    /// Broadcast and recovery, which prevents vout aliasing when two intents
    /// in the same batch target identical address+amount pairs.
    ///
    /// `fee_allocations` must be positionally aligned with `intents` (i.e.
    /// `fee_allocations[i]` is the fee for `intents[i]`). This is the natural
    /// output of [`allocate_batch_fee`].
    pub(crate) fn derive_claimed_vout_assignments(
        &self,
        tx: &Transaction,
        intents: &[SendIntent<intent_state::BatchClaimed>],
        fee_allocations: &[u64],
    ) -> Result<Vec<BatchOutputAssignment>, Error> {
        let intent_outputs: Vec<_> = intents
            .iter()
            .map(|intent| IntentOutput {
                intent_id: intent.intent_id,
                address: intent.address.as_str(),
                amount: intent.amount,
            })
            .collect();
        derive_vout_assignments_inner(self.network, tx, &intent_outputs, fee_allocations)
    }

    pub(crate) async fn broadcast_transaction_internal(
        &self,
        tx: Transaction,
    ) -> Result<BroadcastOutcome, BroadcastFailure> {
        self.chain_source.broadcast(tx).await
    }

    pub(crate) fn log_broadcast_failure(
        &self,
        context: &str,
        batch_id: Uuid,
        txid: &str,
        failure: &BroadcastFailure,
    ) {
        match failure.kind {
            BroadcastErrorKind::Rejected => {
                tracing::error!(
                    %batch_id,
                    %txid,
                    error = %failure.message,
                    "{context}: backend rejected signed transaction; keeping batch for operator review/retry"
                );
            }
            BroadcastErrorKind::Transient => {
                tracing::warn!(
                    %batch_id,
                    %txid,
                    error = %failure.message,
                    "{context}: transient broadcast failure; will retry"
                );
            }
            BroadcastErrorKind::Unknown => {
                tracing::warn!(
                    %batch_id,
                    %txid,
                    error = %failure.message,
                    "{context}: ambiguous broadcast failure; will retry conservatively"
                );
            }
        }
    }

    pub(crate) async fn run_batch_processor(
        &self,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<(), Error> {
        let poll_interval = self.batch_config.poll_interval;
        let mut tick = interval(poll_interval);

        tracing::info!("Starting send saga batch processor");

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Batch processor cancelled");
                    break;
                }
                _ = tick.tick() => {
                    if let Err(e) = self.process_ready_intents().await {
                        tracing::error!("Batch processor cycle failed: {}", e);
                    }
                }
                _ = self.batch_notify.notified() => {
                    if let Err(e) = self.process_ready_intents().await {
                        tracing::error!("Batch processor (notify) cycle failed: {}", e);
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn process_ready_intents(&self) -> Result<(), Error> {
        let pending = self.storage.get_pending_send_intents().await?;
        if pending.is_empty() {
            return Ok(());
        }

        let now = crate::util::unix_now();

        let mut immediate = Vec::new();
        let mut standard = Vec::new();
        let mut economy = Vec::new();
        let mut has_ready_standard = false;
        let mut has_ready_economy = false;

        for intent in &pending {
            let created_at = match &intent.state {
                crate::send::payment_intent::record::SendIntentState::Pending { created_at } => {
                    *created_at
                }
                _ => continue,
            };
            let age_secs = now.saturating_sub(created_at);

            // Check for expiry before tier sorting
            if let Some(max_age) = self.batch_config.max_intent_age {
                if age_secs > max_age.as_secs() {
                    tracing::warn!(
                        "Expiring stale intent {} (age: {}s, max: {}s)",
                        intent.intent_id,
                        age_secs,
                        max_age.as_secs()
                    );
                    let reason = format!(
                        "Intent expired after {}s (max: {}s)",
                        age_secs,
                        max_age.as_secs()
                    );
                    if let Err(e) = self
                        .storage
                        .update_send_intent(
                            &intent.intent_id,
                            &crate::send::payment_intent::record::SendIntentState::Failed {
                                reason: reason.clone(),
                                created_at,
                                failed_at: now,
                            },
                        )
                        .await
                    {
                        tracing::error!(
                            "Failed to mark expired intent {} failed: {}",
                            intent.intent_id,
                            e
                        );
                    }
                    if let Ok(quote_id) = QuoteId::from_str(&intent.quote_id) {
                        if let Err(err) = self
                            .payment_sender
                            .send(Event::PaymentFailed { quote_id, reason })
                        {
                            tracing::error!(
                                "Could not send payment failed event for intent {}: {}",
                                intent.intent_id,
                                err
                            );
                        }
                    }
                    continue;
                }
            }

            match intent.tier {
                PaymentTier::Immediate => immediate.push(intent),
                PaymentTier::Standard => {
                    if age_secs >= self.batch_config.standard_deadline.as_secs() {
                        has_ready_standard = true;
                    }
                    standard.push(intent);
                }
                PaymentTier::Economy => {
                    if age_secs >= self.batch_config.economy_deadline.as_secs() {
                        has_ready_economy = true;
                    }
                    economy.push(intent);
                }
            }
        }

        let batch_intents = select_batch_intents(
            immediate,
            standard,
            has_ready_standard,
            economy,
            has_ready_economy,
            self.batch_config.max_batch_size,
        );

        if batch_intents.is_empty() {
            return Ok(());
        }

        tracing::info!("Processing batch of {} intents", batch_intents.len());

        let batch_id = Uuid::new_v4();
        let intent_ids = batch_intents
            .iter()
            .map(|intent| intent.intent_id)
            .collect::<Vec<_>>();
        let claimed_records = self
            .storage
            .claim_pending_send_intents_for_batch(&intent_ids, batch_id)
            .await?;
        if claimed_records.is_empty() {
            return Ok(());
        }

        let mut claimed_intents: Vec<SendIntent<intent_state::BatchClaimed>> = Vec::new();
        for record in &claimed_records {
            match payment_intent::from_record(record) {
                SendIntentAny::BatchClaimed(intent) => claimed_intents.push(intent),
                _ => continue,
            }
        }

        self.build_sign_broadcast_batch(batch_id, claimed_intents)
            .await
    }

    pub(crate) async fn build_sign_broadcast_batch(
        &self,
        batch_id: Uuid,
        intents: Vec<SendIntent<intent_state::BatchClaimed>>,
    ) -> Result<(), Error> {
        let mut highest_tier = PaymentTier::Economy;
        let mut recipients = Vec::with_capacity(intents.len());
        for intent in &intents {
            if intent.tier == PaymentTier::Immediate {
                highest_tier = PaymentTier::Immediate;
            } else if intent.tier == PaymentTier::Standard && highest_tier != PaymentTier::Immediate
            {
                highest_tier = PaymentTier::Standard;
            }

            let address = match parse_checked_address(&intent.address, self.network, Error::Wallet)
            {
                Ok(address) => address,
                Err(e) => {
                    let reason = e.to_string();
                    self.fail_claimed_send_intents(&intents, &reason).await;
                    return Err(e);
                }
            };
            recipients.push((address, intent.amount));
        }

        let sat_per_vb = self
            .estimate_fee_rate_sat_per_vb(highest_tier)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tier = ?highest_tier,
                    error = %e,
                    "Batch fee-rate estimation failed, using configured fallback"
                );
                self.batch_config.fee_estimation.fallback_sat_per_vb
            });

        let fee_rate = match fee_rate_from_sat_per_vb(sat_per_vb) {
            Ok(fee_rate) => fee_rate,
            Err(e) => {
                let reason = e.to_string();
                self.fail_claimed_send_intents(&intents, &reason).await;
                return Err(e);
            }
        };

        // 1. Build the PSBT
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let mut tx_builder = wallet_with_db.wallet.build_tx();
        for (address, amount) in recipients {
            tx_builder.add_recipient(address, bdk_wallet::bitcoin::Amount::from_sat(amount));
        }
        tx_builder.fee_rate(fee_rate);

        let mut psbt = match tx_builder.finish() {
            Ok(psbt) => psbt,
            Err(e) => {
                tracing::error!("Failed to build batch PSBT: {}", e);

                let error_text = e.to_string();
                drop(wallet_with_db);
                self.fail_claimed_send_intents(&intents, &error_text).await;

                return Err(Error::Wallet(e.to_string()));
            }
        };

        // Validate batch fee
        let fee = match psbt.fee() {
            Ok(fee) => fee,
            Err(e) => {
                let err = Error::Wallet(e.to_string());
                let reason = err.to_string();
                drop(wallet_with_db);
                self.fail_claimed_send_intents(&intents, &reason).await;
                return Err(err);
            }
        };
        let actual_fee = fee.to_sat();
        let max_fees: Vec<u64> = intents.iter().map(|i| i.max_fee_amount).collect();
        let intent_ids: Vec<Uuid> = intents.iter().map(|i| i.intent_id).collect();

        let fee_allocations = match allocate_batch_fee(actual_fee, &max_fees, &intent_ids) {
            Ok(alloc) => alloc,
            Err(e) => {
                tracing::warn!("Fee allocation failed, cancelling batch: {}", e);
                let reason = e.to_string();
                drop(wallet_with_db);
                self.fail_claimed_send_intents(&intents, &reason).await;
                return Err(e);
            }
        };

        // Persist wallet state after build
        if let Err(e) = wallet_with_db.persist() {
            let err = Error::Database(e);
            let reason = err.to_string();
            drop(wallet_with_db);
            self.fail_claimed_send_intents(&intents, &reason).await;
            return Err(err);
        }

        // 2. Sign
        let signed = match wallet_with_db.wallet.sign(&mut psbt, Default::default()) {
            Ok(signed) => signed,
            Err(e) => {
                let err = Error::Wallet(e.to_string());
                let reason = err.to_string();
                drop(wallet_with_db);
                self.fail_claimed_send_intents(&intents, &reason).await;
                return Err(err);
            }
        };
        if !signed {
            let reason = Error::CouldNotSign.to_string();
            drop(wallet_with_db);
            self.fail_claimed_send_intents(&intents, &reason).await;
            return Err(Error::CouldNotSign);
        }

        if let Err(e) = wallet_with_db.persist() {
            tracing::warn!(
                "Could not persist BDK wallet after signing batch {}; continuing with persisted send batch recovery path: {}",
                batch_id,
                e
            );
        }

        // Extract final transaction
        let tx = psbt
            .extract_tx()
            .map_err(|e| Error::Wallet(e.to_string()))?;
        let tx_bytes = bdk_wallet::bitcoin::consensus::serialize(&tx);
        let txid = tx.compute_txid();

        // Apply the freshly built tx to BDK's tx graph so the next batch
        // cycle's coin selection treats its inputs as spent. Without this,
        // concurrent melts each call `finish()` against the same UTXO view
        // and pick the same input, causing double-spends rejected by bitcoind.
        let apply_time = crate::util::unix_now();
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(tx.clone(), apply_time)]);
        if let Err(e) = wallet_with_db.persist() {
            tracing::warn!(
                batch_id = %batch_id,
                "Could not persist BDK wallet after applying unconfirmed tx: {}",
                e
            );
        }

        // Drop wallet lock before broadcasting
        drop(wallet_with_db);

        // 3. Record per-intent vout + fee mapping once, at the only place we have
        // ground truth: the freshly built transaction plus the fee allocation
        // in memory. Persist this Signed batch before moving any intent out
        // of Pending; this makes every post-sign crash/failure recoverable
        // from the signed transaction bytes instead of reverting into a new
        // batch.
        let assignments = self.derive_claimed_vout_assignments(&tx, &intents, &fee_allocations)?;
        let intent_count = assignments.len();

        if let Err(e) = self
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat: actual_fee,
                },
            })
            .await
        {
            // Persisting Signed failed after the wallet's tx graph was
            // advanced. Revert the apply so the UTXOs aren't stranded in
            // an orphaned unconfirmed tx. Downstream failures all leave a
            // durable Signed/Broadcast record that recovery can replay.
            let evict_time = apply_time.saturating_add(1);
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            wallet_with_db
                .wallet
                .apply_evicted_txs([(txid, evict_time)]);
            if let Err(persist_err) = wallet_with_db.persist() {
                tracing::warn!(
                    batch_id = %batch_id,
                    "Could not persist BDK wallet after evicting unconfirmed tx on store_send_batch failure: {}",
                    persist_err
                );
            }
            drop(wallet_with_db);
            return Err(e);
        }

        // 4. Transition intents to Batched after the signed transaction is durable.
        let mut batched_intents = Vec::new();
        for intent in intents {
            let batched = intent.assign_to_batch(&self.storage).await?;
            batched_intents.push(batched);
        }
        let signed_batch =
            SendBatch::<batch_state::Signed>::reconstruct(batch_id, batched_intents.clone());

        // 5. Persist Broadcast state BEFORE actually broadcasting (crash safety)
        let broadcast_result = match signed_batch
            .mark_broadcast(
                &self.storage,
                txid.to_string(),
                tx_bytes,
                assignments.clone(),
                actual_fee,
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    "Failed to persist Broadcast state for batch {}: {}",
                    batch_id,
                    e
                );
                // The Signed batch is already durable. Recovery will promote
                // it to Broadcast and retry the network send.
                return Err(e);
            }
        };

        // 6. Transition intents to AwaitingConfirmation before network send.
        //    Pair each intent with its assignment via intent_id rather than
        //    positional index, so any future reordering of either list is safe.
        let assignment_by_intent: std::collections::HashMap<Uuid, &BatchOutputAssignment> =
            assignments.iter().map(|a| (a.intent_id, a)).collect();
        let txid_string = txid.to_string();

        for intent in broadcast_result.intents {
            let assignment = assignment_by_intent.get(&intent.intent_id).ok_or_else(|| {
                Error::BatchAssignmentMissing {
                    batch_id,
                    intent_id: intent.intent_id,
                }
            })?;
            let outpoint = OutPoint::new(txid, assignment.vout).to_string();
            intent
                .mark_broadcast(
                    &self.storage,
                    txid_string.clone(),
                    outpoint,
                    assignment.fee_contribution_sat,
                )
                .await?;
        }

        // 7. Broadcast
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
                    "Initial broadcast failed",
                    batch_id,
                    &txid_string,
                    &failure,
                );
                // Post-Broadcast-persist failure: the batch record and intents are
                // already marked for reconciliation. Recovery will attempt rebroadcast.
                return Err(Error::Wallet(format!(
                    "Broadcast failed after signed batch persistence: {}",
                    failure.message
                )));
            }
        }

        tracing::info!(
            "Batch {} broadcast as txid {} with {} intents",
            batch_id,
            txid,
            intent_count
        );

        Ok(())
    }

    pub(crate) async fn check_send_saga_confirmations(&self) -> Result<(), Error> {
        let all_persisted = self.storage.get_all_send_intents().await?;

        // Reconstruct typed intents and filter for AwaitingConfirmation
        let awaiting: Vec<_> = all_persisted
            .iter()
            .filter_map(|pi| match payment_intent::from_record(pi) {
                SendIntentAny::AwaitingConfirmation(intent) => Some(intent),
                _ => None,
            })
            .collect();

        let wallet_with_db = self.wallet_with_db.lock().await;

        let mut to_finalize = Vec::new();

        for intent in awaiting {
            if self.txid_has_required_confirmations(
                &wallet_with_db.wallet,
                &intent.state.txid,
                "send_intent",
                &intent.intent_id.to_string(),
            ) {
                to_finalize.push(intent);
            }
        }

        drop(wallet_with_db);

        for intent in to_finalize {
            self.finalize_send_intent_and_emit(intent).await?;
        }

        self.cleanup_completed_batches().await
    }

    pub(crate) async fn cleanup_completed_batches(&self) -> Result<(), Error> {
        let batches = self.storage.get_all_send_batches().await?;
        let all_active_intents = self.storage.get_all_send_intents().await?;

        for batch in batches {
            let assignments = match &batch.state {
                crate::send::batch_transaction::record::SendBatchState::Broadcast {
                    assignments,
                    ..
                } => assignments,
                _ => continue, // Only clean up broadcast batches
            };

            let has_active = assignments.iter().any(|a| {
                all_active_intents
                    .iter()
                    .any(|i| i.intent_id == a.intent_id)
            });

            if !has_active {
                tracing::info!("Cleaning up completed batch {}", batch.batch_id);
                self.storage.delete_send_batch(&batch.batch_id).await?;
            }
        }
        Ok(())
    }

    /// Re-broadcast any `Broadcast`-state batch whose transaction the BDK
    /// wallet does not currently know about.
    ///
    /// `Broadcast` state is persisted before the network send (see the
    /// hot path in `build_sign_broadcast_batch`), so a transient Esplora
    /// failure at the moment of broadcast can leave a batch durably in
    /// that state with its tx never having reached the network. The
    /// one-shot in `recover_send_saga` only covers process restarts;
    /// this helper closes the steady-state gap by retrying on every
    /// sync-reconciliation tick.
    ///
    /// Staleness signal: `wallet.get_tx(txid).is_none()`. If the wallet
    /// sees the tx (confirmed or unconfirmed in mempool), we leave it
    /// alone. Per-batch failures are logged and swallowed; the next
    /// reconciliation tick retries naturally.
    #[tracing::instrument(skip_all)]
    pub(crate) async fn rebroadcast_stuck_batches(&self) -> Result<(), Error> {
        let batches = self.storage.get_all_send_batches().await?;

        // Collect candidates while holding the wallet lock (needed for
        // `get_tx`), then drop the lock before any network I/O so the
        // sync loop is never blocked on Esplora latency.
        let candidates: Vec<(Uuid, String, Transaction)> = {
            let wallet_with_db = self.wallet_with_db.lock().await;
            batches
                .into_iter()
                .filter_map(|rec| {
                    let crate::send::batch_transaction::record::SendBatchState::Broadcast {
                        txid,
                        tx_bytes,
                        ..
                    } = rec.state
                    else {
                        return None;
                    };

                    let parsed_txid = match bdk_wallet::bitcoin::Txid::from_str(&txid) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(
                                batch_id = %rec.batch_id,
                                txid = %txid,
                                "Skipping rebroadcast: failed to parse persisted txid: {e}"
                            );
                            return None;
                        }
                    };

                    if wallet_with_db.wallet.get_tx(parsed_txid).is_some() {
                        // Wallet knows the tx (confirmed or in mempool);
                        // no rebroadcast needed.
                        return None;
                    }

                    match bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(&tx_bytes) {
                        Ok(tx) => Some((rec.batch_id, txid, tx)),
                        Err(e) => {
                            tracing::warn!(
                                batch_id = %rec.batch_id,
                                txid = %txid,
                                "Skipping rebroadcast: failed to deserialize persisted tx: {e}"
                            );
                            None
                        }
                    }
                })
                .collect()
        };

        for (batch_id, txid, tx) in candidates {
            tracing::info!(%batch_id, %txid, "Rebroadcasting stuck batch");
            match self.broadcast_transaction_internal(tx.clone()).await {
                Ok(BroadcastOutcome::Accepted) => {
                    tracing::info!(%batch_id, %txid, "Rebroadcast accepted");
                    // Apply the now-broadcast tx to BDK's tx graph so the
                    // next batch cycle doesn't reselect its inputs.
                    let apply_time = crate::util::unix_now();
                    let mut wallet_with_db = self.wallet_with_db.lock().await;
                    wallet_with_db
                        .wallet
                        .apply_unconfirmed_txs([(tx, apply_time)]);
                    if let Err(e) = wallet_with_db.persist() {
                        tracing::warn!(
                            %batch_id,
                            "Could not persist BDK wallet after applying rebroadcast tx: {}",
                            e
                        );
                    }
                    drop(wallet_with_db);
                }
                Ok(BroadcastOutcome::AlreadyKnown) => {
                    tracing::info!(%batch_id, %txid, "Rebroadcast tx already known");
                }
                Err(failure) => {
                    self.log_broadcast_failure("Rebroadcast failed", batch_id, &txid, &failure);
                    // Swallow: next reconciliation tick will retry.
                }
            }
        }

        Ok(())
    }
}

fn select_batch_intents<T>(
    immediate: Vec<T>,
    standard: Vec<T>,
    has_ready_standard: bool,
    economy: Vec<T>,
    has_ready_economy: bool,
    max_batch_size: usize,
) -> Vec<T> {
    if immediate.is_empty() && !has_ready_standard && !has_ready_economy {
        return Vec::new();
    }

    let mut batch_intents = immediate;
    batch_intents.extend(standard);
    batch_intents.extend(economy);
    batch_intents.truncate(max_batch_size);
    batch_intents
}

/// Pure helper that does the vout-derivation work for
/// [`CdkBdk::derive_vout_assignments`].
///
/// Kept separate so it can be unit-tested without constructing a full
/// `CdkBdk` instance.
struct IntentOutput<'a> {
    intent_id: Uuid,
    address: &'a str,
    amount: u64,
}

fn derive_vout_assignments_inner(
    network: bdk_wallet::bitcoin::Network,
    tx: &Transaction,
    intents: &[IntentOutput<'_>],
    fee_allocations: &[u64],
) -> Result<Vec<BatchOutputAssignment>, Error> {
    if intents.len() != fee_allocations.len() {
        return Err(Error::Wallet(format!(
            "intent count ({}) does not match fee allocation count ({})",
            intents.len(),
            fee_allocations.len()
        )));
    }

    let mut claimed_vouts = std::collections::HashSet::new();
    let mut assignments = Vec::with_capacity(intents.len());

    for (idx, intent) in intents.iter().enumerate() {
        let address = parse_checked_address(intent.address, network, Error::Wallet)?;
        let vout = tx
            .output
            .iter()
            .enumerate()
            .find_map(|(vout_idx, output)| {
                if claimed_vouts.contains(&vout_idx) {
                    return None;
                }
                Address::from_script(output.script_pubkey.as_script(), network)
                    .ok()
                    .filter(|candidate| *candidate == address)
                    .filter(|_| output.value.to_sat() == intent.amount)
                    .map(|_| vout_idx)
            })
            .ok_or(Error::VoutNotFound)?;
        claimed_vouts.insert(vout);

        assignments.push(BatchOutputAssignment {
            intent_id: intent.intent_id,
            vout: vout as u32,
            fee_contribution_sat: fee_allocations[idx],
        });
    }

    Ok(assignments)
}

#[cfg(test)]
mod tests {
    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::transaction::Version;
    use bdk_wallet::bitcoin::{Amount as BtcAmount, Network, ScriptBuf, TxOut};
    use uuid::Uuid;

    use super::*;
    use crate::send::payment_intent::state::Batched as IntentBatched;
    use crate::send::payment_intent::SendIntent;
    use crate::types::{PaymentMetadata, PaymentTier};

    const ADDR_A: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
    const ADDR_B: &str = "bcrt1q6rhpng9evdsfnn833a4f4vej0asu6dk5srld6x";

    fn make_batched_intent(
        intent_id: Uuid,
        address: &str,
        amount: u64,
    ) -> SendIntent<IntentBatched> {
        SendIntent {
            intent_id,
            quote_id: format!("q-{}", intent_id),
            address: address.to_string(),
            amount,
            max_fee_amount: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            created_at: 1_700_000_000,
            state: IntentBatched {
                batch_id: Uuid::new_v4(),
            },
        }
    }

    fn intent_output(intent: &SendIntent<IntentBatched>) -> IntentOutput<'_> {
        IntentOutput {
            intent_id: intent.intent_id,
            address: intent.address.as_str(),
            amount: intent.amount,
        }
    }

    fn tx_with_outputs(outputs: Vec<TxOut>) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: outputs,
        }
    }

    fn script_for(address: &str) -> ScriptBuf {
        Address::from_str(address)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap()
            .script_pubkey()
    }

    #[test]
    fn select_batch_intents_piggybacks_waiting_lower_tiers_on_immediate() {
        let selected = select_batch_intents(
            vec!["immediate"],
            vec!["standard-a", "standard-b"],
            false,
            vec!["economy-a", "economy-b"],
            false,
            5,
        );

        assert_eq!(
            selected,
            vec![
                "immediate",
                "standard-a",
                "standard-b",
                "economy-a",
                "economy-b"
            ]
        );
    }

    #[test]
    fn select_batch_intents_prioritizes_immediate_when_truncated() {
        let selected = select_batch_intents(
            vec!["immediate-a", "immediate-b"],
            vec!["standard"],
            true,
            vec!["economy-a", "economy-b"],
            true,
            3,
        );

        assert_eq!(selected, vec!["immediate-a", "immediate-b", "standard"]);
    }

    #[test]
    fn select_batch_intents_deadline_trigger_includes_all_pending_tiers() {
        let selected = select_batch_intents(
            Vec::<&str>::new(),
            vec!["standard-waiting-a", "standard-ready", "standard-waiting-b"],
            true,
            vec!["economy-waiting"],
            false,
            10,
        );

        assert_eq!(
            selected,
            vec![
                "standard-waiting-a",
                "standard-ready",
                "standard-waiting-b",
                "economy-waiting"
            ]
        );
    }

    #[test]
    fn select_batch_intents_economy_deadline_trigger_includes_all_pending_tiers() {
        let selected = select_batch_intents(
            Vec::<&str>::new(),
            vec!["standard-waiting"],
            false,
            vec!["economy-ready", "economy-waiting"],
            true,
            10,
        );

        assert_eq!(
            selected,
            vec!["standard-waiting", "economy-ready", "economy-waiting"]
        );
    }

    #[test]
    fn select_batch_intents_waits_for_deadline_without_immediate() {
        let selected = select_batch_intents(
            Vec::<&str>::new(),
            vec!["waiting-standard"],
            false,
            vec!["waiting-economy"],
            false,
            10,
        );

        assert!(selected.is_empty());
    }

    /// Two intents pay the same address for the same amount within one batch.
    /// The derivation must produce distinct vouts — one for each output —
    /// rather than aliasing both intents onto the same vout.
    #[test]
    fn derive_vout_assignments_disambiguates_same_address_same_amount() {
        let intent_a = make_batched_intent(Uuid::new_v4(), ADDR_A, 10_000);
        let intent_b = make_batched_intent(Uuid::new_v4(), ADDR_A, 10_000);

        let script = script_for(ADDR_A);
        let tx = tx_with_outputs(vec![
            TxOut {
                value: BtcAmount::from_sat(10_000),
                script_pubkey: script.clone(),
            },
            TxOut {
                value: BtcAmount::from_sat(10_000),
                script_pubkey: script,
            },
        ]);

        let assignments = derive_vout_assignments_inner(
            Network::Regtest,
            &tx,
            &[intent_output(&intent_a), intent_output(&intent_b)],
            &[50, 50],
        )
        .expect("derive");

        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0].intent_id, intent_a.intent_id);
        assert_eq!(assignments[0].vout, 0);
        assert_eq!(assignments[0].fee_contribution_sat, 50);
        assert_eq!(assignments[1].intent_id, intent_b.intent_id);
        assert_eq!(assignments[1].vout, 1);
        assert_eq!(assignments[1].fee_contribution_sat, 50);

        // The two intents must never alias onto the same vout — this is the
        // core invariant that eliminates recovery-time ambiguity.
        assert_ne!(assignments[0].vout, assignments[1].vout);
    }

    /// Intents target distinct addresses; assignment should find each by
    /// address regardless of output order.
    #[test]
    fn derive_vout_assignments_handles_distinct_addresses() {
        let intent_a = make_batched_intent(Uuid::new_v4(), ADDR_A, 10_000);
        let intent_b = make_batched_intent(Uuid::new_v4(), ADDR_B, 20_000);

        // Outputs intentionally in B, A order so we also exercise the fact
        // that positional order doesn't drive assignment.
        let tx = tx_with_outputs(vec![
            TxOut {
                value: BtcAmount::from_sat(20_000),
                script_pubkey: script_for(ADDR_B),
            },
            TxOut {
                value: BtcAmount::from_sat(10_000),
                script_pubkey: script_for(ADDR_A),
            },
        ]);

        let assignments = derive_vout_assignments_inner(
            Network::Regtest,
            &tx,
            &[intent_output(&intent_a), intent_output(&intent_b)],
            &[10, 20],
        )
        .expect("derive");

        assert_eq!(assignments[0].intent_id, intent_a.intent_id);
        assert_eq!(assignments[0].vout, 1);
        assert_eq!(assignments[1].intent_id, intent_b.intent_id);
        assert_eq!(assignments[1].vout, 0);
    }

    /// If no output matches an intent's (address, amount), derivation must
    /// fail rather than silently misattribute.
    #[test]
    fn derive_vout_assignments_errors_when_output_missing() {
        let intent = make_batched_intent(Uuid::new_v4(), ADDR_A, 99_999);

        let tx = tx_with_outputs(vec![TxOut {
            value: BtcAmount::from_sat(10_000),
            script_pubkey: script_for(ADDR_A),
        }]);

        let result =
            derive_vout_assignments_inner(Network::Regtest, &tx, &[intent_output(&intent)], &[10]);
        assert!(matches!(result, Err(Error::VoutNotFound)));
    }

    /// Misaligned intents and fee_allocations must be caught.
    #[test]
    fn derive_vout_assignments_errors_on_length_mismatch() {
        let intent = make_batched_intent(Uuid::new_v4(), ADDR_A, 10_000);
        let tx = tx_with_outputs(vec![TxOut {
            value: BtcAmount::from_sat(10_000),
            script_pubkey: script_for(ADDR_A),
        }]);
        let result = derive_vout_assignments_inner(
            Network::Regtest,
            &tx,
            &[intent_output(&intent)],
            &[10, 20],
        );
        assert!(matches!(result, Err(Error::Wallet(_))));
    }

    // ── rebroadcast_stuck_batches ────────────────────────────────────

    mod rebroadcast {
        use std::str::FromStr;
        use std::sync::Arc;
        use std::time::Duration;

        use bdk_wallet::bitcoin::consensus;
        use bdk_wallet::keys::bip39::Mnemonic;
        use cdk_common::common::FeeReserve;
        use cdk_common::{Amount, CurrencyUnit};
        use uuid::Uuid;

        use super::{BtcAmount, LockTime, Network, ScriptBuf, TxOut, Version};
        use crate::send::batch_transaction::record::{
            BatchOutputAssignment, SendBatchRecord, SendBatchState,
        };
        use crate::{CdkBdk, ChainSource, EsploraConfig};

        const TEST_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";

        /// Build a `CdkBdk` test instance with a bogus Esplora URL and an
        /// empty BDK wallet. Because the wallet is empty, `get_tx` returns
        /// `None` for any txid, which is exactly the staleness signal the
        /// rebroadcast path is keyed on. The bogus URL means any call to
        /// `broadcast_transaction_internal` fails quickly without touching
        /// the network; `rebroadcast_stuck_batches` swallows that failure
        /// and still returns `Ok(())`.
        async fn build_test_instance() -> CdkBdk {
            let tmp = tempfile::tempdir().expect("tempdir");
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
                546,
                60,
                Some(5),
                None,
                None,
            )
            .expect("build CdkBdk test instance")
        }

        /// Serialize a minimal valid transaction so `consensus::deserialize`
        /// can round-trip it during rebroadcast.
        fn valid_tx_bytes() -> Vec<u8> {
            let tx = super::Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: Vec::new(),
                output: vec![TxOut {
                    value: BtcAmount::from_sat(10_000),
                    script_pubkey: ScriptBuf::new(),
                }],
            };
            consensus::serialize(&tx)
        }

        /// No persisted batches → nothing to do; must return Ok.
        #[tokio::test]
        async fn rebroadcast_noop_when_storage_empty() {
            let backend = build_test_instance().await;
            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should not error");
        }

        /// A `Broadcast`-state batch whose tx the wallet does not know
        /// about is a stuck batch. The method must attempt to rebroadcast,
        /// fail (unreachable URL), log, and still return Ok. The batch
        /// record must remain in `Broadcast` state for the next retry.
        #[tokio::test]
        async fn rebroadcast_stuck_batch_survives_transport_failure() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let intent_id = Uuid::new_v4();

            let batch = SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: TEST_TXID.to_string(),
                    tx_bytes: valid_tx_bytes(),
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

            tokio::time::timeout(Duration::from_secs(10), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should swallow transport errors");

            // Batch must still be in Broadcast state for the next retry.
            let after = backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("fetch batch")
                .expect("batch still present");
            assert!(
                matches!(after.state, SendBatchState::Broadcast { .. }),
                "batch must remain in Broadcast state after failed rebroadcast; got {:?}",
                after.state
            );
        }

        /// `Built`-state batches are not yet broadcast candidates. The
        /// rebroadcast helper must ignore them entirely. We rely on the
        /// method completing quickly without error; the garbage tx_bytes
        /// would trigger a deserialize warning if the filter were wrong.
        #[tokio::test]
        async fn rebroadcast_ignores_built_batch() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();

            let batch = SendBatchRecord {
                batch_id,
                state: SendBatchState::Built {
                    psbt_bytes: vec![0xff],
                    intent_ids: vec![Uuid::new_v4()],
                },
            };
            backend
                .storage
                .store_send_batch(&batch)
                .await
                .expect("store batch");

            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should not error");

            // Built batches must be left untouched.
            let after = backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("fetch batch")
                .expect("batch still present");
            assert!(matches!(after.state, SendBatchState::Built { .. }));
        }

        /// `Signed`-state batches are handled by recovery, not by the
        /// steady-state rebroadcast loop. The helper must ignore them.
        #[tokio::test]
        async fn rebroadcast_ignores_signed_batch() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();

            let batch = SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: vec![0xff],
                    assignments: vec![BatchOutputAssignment {
                        intent_id: Uuid::new_v4(),
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

            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should not error");

            let after = backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("fetch batch")
                .expect("batch still present");
            assert!(matches!(after.state, SendBatchState::Signed { .. }));
        }

        /// A persisted txid that fails to parse must not abort the loop
        /// or propagate an error. Other batches on the same tick would
        /// still be processed; here we just verify no error is returned.
        #[tokio::test]
        async fn rebroadcast_skips_unparsable_txid() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();

            let batch = SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: "not-a-valid-txid".to_string(),
                    tx_bytes: valid_tx_bytes(),
                    assignments: vec![BatchOutputAssignment {
                        intent_id: Uuid::new_v4(),
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

            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should skip malformed txid gracefully");
        }
    }
}
