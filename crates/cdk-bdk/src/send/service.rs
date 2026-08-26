use std::str::FromStr;

use bdk_wallet::bitcoin::{Address, OutPoint, Transaction, Txid};
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
use crate::send::payment_intent::record::SendIntentState;
use crate::send::payment_intent::{self, state as intent_state, SendIntent, SendIntentAny};
use crate::types::PaymentTier;
use crate::CdkBdk;

impl CdkBdk {
    async fn fail_send_intents(&self, intents: &[SendIntent<intent_state::Pending>], reason: &str) {
        for intent in intents {
            if let Err(err) = intent.clone().fail(&self.storage, reason.to_string()).await {
                tracing::error!(
                    "Failed to mark send intent {} failed after terminal batch failure: {}",
                    intent.intent_id,
                    err
                );
                continue;
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

        let finalized = intent.finalize(&self.storage).await.map_err(|e| {
            tracing::error!("Failed to finalize send intent {}: {}", intent_id, e);
            e
        })?;

        if finalized {
            let Ok(quote_id) = QuoteId::from_str(&quote_id) else {
                return Ok(());
            };
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
    pub(crate) fn derive_pending_vout_assignments(
        &self,
        tx: &Transaction,
        intents: &[SendIntent<intent_state::Pending>],
        fee_allocations: &[u64],
    ) -> Result<Vec<BatchOutputAssignment>, Error> {
        let intent_outputs: Vec<_> = intents
            .iter()
            .map(|intent| IntentOutput {
                intent_id: intent.intent_id,
                attempt_id: intent.attempt_id,
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
        let normalized = self.storage.normalize_legacy_pending_attempt_ids().await?;
        if normalized > 0 {
            tracing::info!(
                normalized,
                "Assigned attempt IDs to legacy pending send intents"
            );
        }

        // Cancellation is durable precisely so transient wallet/storage
        // failures can be retried. Drain those markers on every processor
        // cycle rather than requiring a process restart.
        self.resume_cancelled_send_batches().await?;

        // A transient Signed -> Broadcast storage failure leaves a fully signed
        // transaction durable but unavailable to the normal pending selector.
        // Reuse the idempotent recovery path on a later processor cycle instead
        // of requiring a process restart.
        if self
            .storage
            .get_all_send_batches()
            .await?
            .iter()
            .any(|batch| {
                matches!(
                    &batch.state,
                    SendBatchState::Signed { tx_bytes, .. }
                        if bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(tx_bytes)
                            .is_ok()
                )
            })
        {
            self.recover_send_saga().await?;
        }

        let broadcast_owned_intents: std::collections::HashSet<_> = self
            .storage
            .get_all_send_batches()
            .await?
            .into_iter()
            .filter_map(|batch| match batch.state {
                SendBatchState::Broadcast { assignments, .. } => Some(assignments),
                _ => None,
            })
            .flatten()
            .map(|assignment| assignment.intent_id)
            .collect();
        let pending: Vec<_> = self
            .storage
            .get_pending_send_intents()
            .await?
            .into_iter()
            .filter(|intent| {
                let eligible = !broadcast_owned_intents.contains(&intent.intent_id);
                if !eligible {
                    tracing::warn!(
                        intent_id = %intent.intent_id,
                        attempt_id = %intent.attempt_id,
                        "Keeping Pending replacement fenced by durable Broadcast evidence"
                    );
                }
                eligible
            })
            .collect();
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
                    let failed_state =
                        crate::send::payment_intent::record::SendIntentState::Failed {
                            reason: reason.clone(),
                            created_at,
                            failed_at: now,
                        };
                    match self
                        .storage
                        .transition_send_intent(
                            &intent.intent_id,
                            &intent.attempt_id,
                            &intent.state,
                            &failed_state,
                        )
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => {
                            tracing::info!(
                                "Skipping expiry of intent {}: durable state changed concurrently",
                                intent.intent_id
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to mark expired intent {} failed: {}",
                                intent.intent_id,
                                e
                            );
                            continue;
                        }
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

        // Reconstruct typed SendIntent<Pending> from persisted state
        let mut pending_intents: Vec<SendIntent<intent_state::Pending>> = Vec::new();
        for pi in &batch_intents {
            match payment_intent::from_record(pi) {
                SendIntentAny::Pending(intent) => pending_intents.push(intent),
                _ => continue,
            }
        }

        self.build_sign_broadcast_batch(pending_intents).await
    }

    pub(crate) async fn build_sign_broadcast_batch(
        &self,
        intents: Vec<SendIntent<intent_state::Pending>>,
    ) -> Result<(), Error> {
        let batch_id = Uuid::new_v4();

        let mut highest_tier = PaymentTier::Economy;
        let mut recipients = Vec::with_capacity(intents.len());
        for intent in &intents {
            if intent.tier == PaymentTier::Immediate {
                highest_tier = PaymentTier::Immediate;
            } else if intent.tier == PaymentTier::Standard && highest_tier != PaymentTier::Immediate
            {
                highest_tier = PaymentTier::Standard;
            }

            let address = match Address::from_str(&intent.address)
                .map_err(|e| Error::Wallet(e.to_string()))
                .and_then(|address| {
                    address
                        .require_network(self.network)
                        .map_err(|e| Error::Wallet(e.to_string()))
                }) {
                Ok(address) => address,
                Err(e) => {
                    let reason = e.to_string();
                    self.fail_send_intents(&intents, &reason).await;
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
                self.fail_send_intents(&intents, &reason).await;
                return Err(e);
            }
        };

        // Load durable reservations before taking the wallet mutex. The actual
        // reservations and coin selection remain in one critical section, while
        // storage I/O cannot stall every other wallet user.
        let durable_broadcasts = self.load_broadcast_reservations().await?;

        // 1. Build the PSBT
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        Self::reserve_broadcast_transactions_locked(
            &mut wallet_with_db.wallet,
            &durable_broadcasts,
            0,
        )?;
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
                self.fail_send_intents(&intents, &error_text).await;

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
                self.fail_send_intents(&intents, &reason).await;
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
                self.fail_send_intents(&intents, &reason).await;
                return Err(e);
            }
        };

        // Persist wallet state after build
        if let Err(e) = wallet_with_db.persist() {
            let err = Error::Database(e);
            let reason = err.to_string();
            drop(wallet_with_db);
            self.fail_send_intents(&intents, &reason).await;
            return Err(err);
        }

        // 2. Sign
        let signed = match wallet_with_db.wallet.sign(&mut psbt, Default::default()) {
            Ok(signed) => signed,
            Err(e) => {
                let err = Error::Wallet(e.to_string());
                let reason = err.to_string();
                drop(wallet_with_db);
                self.fail_send_intents(&intents, &reason).await;
                return Err(err);
            }
        };
        if !signed {
            let reason = Error::CouldNotSign.to_string();
            drop(wallet_with_db);
            self.fail_send_intents(&intents, &reason).await;
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

        // 3. Record per-intent vout + fee mapping once, at the only place we have
        // ground truth: the freshly built transaction plus the fee allocation
        // in memory. Persist Signed before applying the transaction to BDK, so
        // every durable wallet-graph mutation has a recovery record. Keep the
        // wallet lock through both operations so another local batch cannot
        // select the same inputs in between.
        let assignments = self.derive_pending_vout_assignments(&tx, &intents, &fee_allocations)?;
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
            drop(wallet_with_db);
            return Err(e);
        }

        // Apply the freshly built tx to BDK's tx graph so the next batch
        // cycle's coin selection treats its inputs as spent. Without this,
        // concurrent melts each call `finish()` against the same UTXO view
        // and pick the same input, causing double-spends rejected by bitcoind.
        let apply_time = reserve_unconfirmed_tx(&mut wallet_with_db.wallet, &tx)?;
        if let Err(e) = wallet_with_db.persist() {
            tracing::warn!(
                batch_id = %batch_id,
                "Could not persist BDK wallet after applying unconfirmed tx: {}",
                e
            );
        }

        // Drop wallet lock before broadcasting or compensating.
        drop(wallet_with_db);

        // 4. Transition intents to Batched after the signed transaction is durable.
        let mut batched_intents: Vec<SendIntent<intent_state::Batched>> = Vec::new();
        for intent in intents {
            let batched = match intent.assign_to_batch(&self.storage, batch_id).await {
                Ok(batched) => batched,
                Err(e) => {
                    tracing::warn!(
                        batch_id = %batch_id,
                        error = %e,
                        "Batch member could not be claimed; compensating signed batch"
                    );
                    let cancelled = match self
                        .cancel_signed_send_batch(
                            batch_id,
                            &tx_bytes,
                            &assignments,
                            actual_fee,
                            apply_time.saturating_add(1),
                        )
                        .await
                    {
                        Ok(cancelled) => cancelled,
                        Err(cancel_err) => {
                            tracing::error!(
                                batch_id = %batch_id,
                                error = %cancel_err,
                                "Failed to durably compensate signed batch"
                            );
                            return Err(cancel_err);
                        }
                    };
                    if !cancelled {
                        tracing::info!(
                            batch_id = %batch_id,
                            "Skipping compensation because the signed batch changed concurrently"
                        );
                        return Err(e);
                    }
                    return Err(e);
                }
            };
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
            let (stored_txid, tx_bytes, assignments) = match &batch.state {
                crate::send::batch_transaction::record::SendBatchState::Broadcast {
                    txid,
                    tx_bytes,
                    assignments,
                    ..
                } => (txid, tx_bytes, assignments),
                _ => continue, // Only clean up broadcast batches
            };

            let has_active = assignments.iter().any(|a| {
                all_active_intents
                    .iter()
                    .any(|i| i.intent_id == a.intent_id && i.attempt_id == a.attempt_id)
            });

            if has_active {
                continue;
            }

            // Missing active state can also mean corruption or replacement.
            // Retain the Broadcast evidence unless every assigned output has
            // an exact finalized tombstone.
            let Some((txid, _)) = decode_broadcast_tx(&batch.batch_id, stored_txid, tx_bytes)
            else {
                continue;
            };
            let mut all_finalized = !assignments.is_empty();
            for assignment in assignments {
                let expected_outpoint = OutPoint::new(txid, assignment.vout).to_string();
                let finalized = self
                    .storage
                    .get_finalized_intent(&assignment.intent_id)
                    .await?;
                if finalized
                    .as_ref()
                    .is_none_or(|record| record.outpoint != expected_outpoint)
                {
                    all_finalized = false;
                    break;
                }
            }

            if all_finalized {
                tracing::info!("Cleaning up completed batch {}", batch.batch_id);
                self.storage
                    .delete_send_batch_if_state(&batch.batch_id, &batch.state)
                    .await?;
            } else {
                tracing::warn!(
                    batch_id = %batch.batch_id,
                    "Retaining Broadcast batch because exact completion is not proven"
                );
            }
        }
        Ok(())
    }

    /// Re-broadcast any unconfirmed `Broadcast`-state batch.
    ///
    /// `Broadcast` state is persisted before the network send (see the
    /// hot path in `build_sign_broadcast_batch`), so a transient Esplora
    /// failure at the moment of broadcast can leave a batch durably in
    /// that state with its tx never having reached the network. The
    /// one-shot in `recover_send_saga` only covers process restarts;
    /// this helper closes the steady-state gap by retrying on every
    /// sync-reconciliation tick.
    ///
    /// The stored txid and persisted transaction bytes must identify the same
    /// transaction. A locally tracked unconfirmed transaction is still retried
    /// because BDK wallet presence does not prove backend acceptance. Confirmed
    /// transactions are skipped. Per-batch failures are logged and swallowed;
    /// the next reconciliation tick retries naturally.
    #[tracing::instrument(skip_all)]
    pub(crate) async fn rebroadcast_stuck_batches(&self) -> Result<(), Error> {
        let batches = self.storage.get_all_send_batches().await?;
        let mut reservations = Vec::new();
        let mut eligible = Vec::new();
        for record in batches {
            if let Some(reservation) = decode_broadcast_reservation(&record.batch_id, &record.state)
            {
                reservations.push(reservation);
            }
            if let Some((txid, tx)) = self
                .prepare_broadcast_batch(record.batch_id, &record.state)
                .await?
            {
                eligible.push((record.batch_id, txid, tx));
            }
        }

        // Reserve every candidate in BDK before releasing the wallet lock.
        // A durable Broadcast record means its inputs must not be selected by
        // another batch even when the backend has not accepted the transaction
        // yet. The reservation timestamp must strictly beat any eviction marker
        // or BDK will continue to treat the transaction as non-canonical.
        let candidates: Vec<(Uuid, String, Transaction)> = {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            let mut candidates = Vec::new();

            for (batch_id, txid, tx) in &reservations {
                if wallet_with_db
                    .wallet
                    .get_tx(*txid)
                    .is_some_and(|wallet_tx| {
                        matches!(
                            wallet_tx.chain_position,
                            bdk_wallet::chain::ChainPosition::Confirmed { .. }
                        )
                    })
                {
                    // Confirmed transactions no longer need rebroadcasting.
                    continue;
                }

                if let Err(err) = reserve_unconfirmed_tx(&mut wallet_with_db.wallet, tx) {
                    tracing::error!(
                        %batch_id,
                        %txid,
                        error = %err,
                        "Cannot reserve rebroadcast transaction"
                    );
                }
            }

            for (batch_id, txid, tx) in eligible {
                if wallet_with_db.wallet.get_tx(txid).is_some_and(|wallet_tx| {
                    matches!(
                        wallet_tx.chain_position,
                        bdk_wallet::chain::ChainPosition::Confirmed { .. }
                    )
                }) {
                    continue;
                }
                candidates.push((batch_id, txid.to_string(), tx));
            }

            if !reservations.is_empty() {
                if let Err(err) = wallet_with_db.persist() {
                    tracing::warn!(
                        "Could not persist BDK wallet after reserving rebroadcast transactions: {}",
                        err
                    );
                }
            }

            candidates
        };

        for (batch_id, txid, tx) in candidates {
            tracing::info!(%batch_id, %txid, "Rebroadcasting stuck batch");
            match self.broadcast_transaction_internal(tx.clone()).await {
                Ok(outcome) => match outcome {
                    BroadcastOutcome::Accepted => {
                        tracing::info!(%batch_id, %txid, "Rebroadcast accepted");
                    }
                    BroadcastOutcome::AlreadyKnown => {
                        tracing::info!(%batch_id, %txid, "Rebroadcast tx already known");
                    }
                },
                Err(failure) => {
                    self.log_broadcast_failure("Rebroadcast failed", batch_id, &txid, &failure);
                    // Swallow: next reconciliation tick will retry.
                }
            }
        }

        Ok(())
    }

    /// Load every decodable durable Broadcast transaction for reservation.
    ///
    /// Strict intent eligibility deliberately is not checked here: even corrupt
    /// or superseded durable evidence must fence its transaction inputs. Strict
    /// checks are only authority for network rebroadcast.
    pub(crate) async fn load_broadcast_reservations(
        &self,
    ) -> Result<Vec<(Uuid, Txid, Transaction)>, Error> {
        let batches = self.storage.get_all_send_batches().await?;
        Ok(batches
            .into_iter()
            .filter_map(|record| decode_broadcast_reservation(&record.batch_id, &record.state))
            .collect())
    }

    /// Refresh preloaded Broadcast reservations while the caller holds the
    /// wallet mutex.
    ///
    /// Sync request construction uses a `minimum_last_seen` strictly newer
    /// than the request's eviction timestamp. Reservation and sync snapshot
    /// creation must remain in the same wallet critical section.
    pub(crate) fn reserve_broadcast_transactions_locked(
        wallet: &mut bdk_wallet::Wallet,
        reservations: &[(Uuid, Txid, Transaction)],
        minimum_last_seen: u64,
    ) -> Result<(), Error> {
        for (_, txid, tx) in reservations {
            if wallet.get_tx(*txid).is_some_and(|wallet_tx| {
                matches!(
                    wallet_tx.chain_position,
                    bdk_wallet::chain::ChainPosition::Confirmed { .. }
                )
            }) {
                continue;
            }
            reserve_unconfirmed_tx_at_least(wallet, tx, minimum_last_seen)?;
        }
        Ok(())
    }

    /// Validate and repair a durable Broadcast batch before it can reserve
    /// wallet inputs or reach a chain backend.
    ///
    /// Eligibility is all-or-nothing. Exact Batched members are advanced using
    /// their compare-and-set transition, then the batch and all members are
    /// re-read. Any corruption, replacement, failure, or mismatch leaves the
    /// durable evidence untouched and fences the transaction from the network.
    pub(crate) async fn prepare_broadcast_batch(
        &self,
        batch_id: Uuid,
        state: &SendBatchState,
    ) -> Result<Option<(Txid, Transaction)>, Error> {
        let SendBatchState::Broadcast {
            txid: stored_txid,
            tx_bytes,
            assignments,
            fee_sat,
        } = state
        else {
            return Ok(None);
        };
        let Some((txid, tx)) = decode_broadcast_tx(&batch_id, stored_txid, tx_bytes) else {
            return Ok(None);
        };

        let unique_intent_ids: std::collections::HashSet<_> =
            assignments.iter().map(|item| item.intent_id).collect();
        let unique_vouts: std::collections::HashSet<_> =
            assignments.iter().map(|item| item.vout).collect();
        let allocated_fee = assignments.iter().try_fold(0_u64, |total, assignment| {
            total.checked_add(assignment.fee_contribution_sat)
        });
        if assignments.is_empty()
            || unique_intent_ids.len() != assignments.len()
            || unique_vouts.len() != assignments.len()
            || allocated_fee != Some(*fee_sat)
            || assignments.iter().any(|item| {
                item.attempt_id.is_nil()
                    || usize::try_from(item.vout).map_or(true, |vout| vout >= tx.output.len())
            })
        {
            tracing::error!(
                %batch_id,
                "Fencing Broadcast batch with empty, duplicate, legacy, or out-of-range assignments"
            );
            return Ok(None);
        }

        let expected_txid = txid.to_string();
        let mut repairs = Vec::new();
        for assignment in assignments {
            let Some(record) = self.storage.get_send_intent(&assignment.intent_id).await? else {
                tracing::error!(
                    %batch_id,
                    intent_id = %assignment.intent_id,
                    "Fencing Broadcast batch with a missing member"
                );
                return Ok(None);
            };
            if record.attempt_id != assignment.attempt_id {
                tracing::error!(
                    %batch_id,
                    intent_id = %assignment.intent_id,
                    assignment_attempt_id = %assignment.attempt_id,
                    current_attempt_id = %record.attempt_id,
                    "Fencing Broadcast batch owned by a replacement send attempt"
                );
                return Ok(None);
            }
            let Some(output) = usize::try_from(assignment.vout)
                .ok()
                .and_then(|vout| tx.output.get(vout))
            else {
                return Ok(None);
            };
            let address = Address::from_str(&record.address)
                .ok()
                .and_then(|address| address.require_network(self.network).ok());
            if assignment.fee_contribution_sat > record.max_fee_amount_sat
                || output.value.to_sat() != record.amount_sat
                || address
                    .as_ref()
                    .is_none_or(|address| output.script_pubkey != address.script_pubkey())
            {
                tracing::error!(
                    %batch_id,
                    intent_id = %assignment.intent_id,
                    "Fencing Broadcast batch whose output or fee does not match its intent"
                );
                return Ok(None);
            }
            match payment_intent::from_record(&record) {
                SendIntentAny::Batched(intent) if intent.state.batch_id == batch_id => {
                    repairs.push((intent, assignment));
                }
                SendIntentAny::AwaitingConfirmation(intent)
                    if intent.state.batch_id == batch_id
                        && intent.state.txid == expected_txid
                        && intent.state.outpoint
                            == OutPoint::new(txid, assignment.vout).to_string()
                        && intent.state.fee_contribution_sat == assignment.fee_contribution_sat => {
                }
                _ => {
                    tracing::error!(
                        %batch_id,
                        intent_id = %assignment.intent_id,
                        "Fencing Broadcast batch with a mismatched member"
                    );
                    return Ok(None);
                }
            }
        }

        // Validate the entire batch before mutating any member.
        for (intent, assignment) in repairs {
            let intent_id = intent.intent_id;
            let result = intent
                .mark_broadcast(
                    &self.storage,
                    expected_txid.clone(),
                    OutPoint::new(txid, assignment.vout).to_string(),
                    assignment.fee_contribution_sat,
                )
                .await;
            if let Err(err) = result {
                // A concurrent worker may have completed the exact same repair.
                // The fresh validation below decides eligibility.
                tracing::info!(
                    %batch_id,
                    %intent_id,
                    error = %err,
                    "Broadcast member repair compare-and-set did not win; revalidating"
                );
            }
        }

        // Verify the batch itself did not change while members were repaired.
        let Some(current_batch) = self.storage.get_send_batch(&batch_id).await? else {
            return Ok(None);
        };
        if current_batch.state != *state {
            return Ok(None);
        }

        for assignment in assignments {
            let Some(record) = self.storage.get_send_intent(&assignment.intent_id).await? else {
                return Ok(None);
            };
            if record.attempt_id != assignment.attempt_id {
                return Ok(None);
            }
            let SendIntentState::AwaitingConfirmation {
                batch_id: intent_batch_id,
                txid: intent_txid,
                outpoint,
                fee_contribution_sat,
                ..
            } = record.state
            else {
                return Ok(None);
            };
            if intent_batch_id != batch_id
                || intent_txid != expected_txid
                || outpoint != OutPoint::new(txid, assignment.vout).to_string()
                || fee_contribution_sat != assignment.fee_contribution_sat
            {
                return Ok(None);
            }
        }

        Ok(Some((txid, tx)))
    }
}

/// Reserve an unconfirmed transaction in BDK's canonical graph.
///
/// The last-seen timestamp must strictly exceed an existing eviction marker;
/// otherwise BDK keeps the transaction non-canonical and coin selection may
/// reuse its inputs.
pub(crate) fn reserve_unconfirmed_tx(
    wallet: &mut bdk_wallet::Wallet,
    tx: &Transaction,
) -> Result<u64, Error> {
    reserve_unconfirmed_tx_at_least(wallet, tx, 0)
}

/// Reserve an unconfirmed transaction with a caller-provided lower bound for
/// its last-seen timestamp.
pub(crate) fn reserve_unconfirmed_tx_at_least(
    wallet: &mut bdk_wallet::Wallet,
    tx: &Transaction,
    minimum_last_seen: u64,
) -> Result<u64, Error> {
    let txid = tx.compute_txid();
    let now = crate::util::unix_now();
    let last_seen = match wallet.tx_graph().get_last_evicted(txid) {
        Some(last_evicted) => {
            minimum_last_seen
                .max(now)
                .max(last_evicted.checked_add(1).ok_or_else(|| {
                    Error::Wallet(format!(
                        "Cannot reserve transaction {txid} after maximum eviction timestamp"
                    ))
                })?)
        }
        None => minimum_last_seen.max(now),
    };
    wallet.apply_unconfirmed_txs([(tx.clone(), last_seen)]);
    Ok(last_seen)
}

/// Decode a persisted `Broadcast` transaction and derive its identity from
/// the transaction bytes.
///
/// Both durable representations must agree. A mismatch is retained for
/// diagnosis and fenced from the network.
fn decode_broadcast_tx(
    batch_id: &Uuid,
    stored_txid: &str,
    tx_bytes: &[u8],
) -> Option<(Txid, Transaction)> {
    let tx = match bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(tx_bytes) {
        Ok(tx) => tx,
        Err(e) => {
            tracing::warn!(
                batch_id = %batch_id,
                txid = %stored_txid,
                "Skipping rebroadcast: failed to deserialize persisted tx: {e}"
            );
            return None;
        }
    };
    let computed_txid = tx.compute_txid();
    if stored_txid != computed_txid.to_string() {
        tracing::error!(
            batch_id = %batch_id,
            stored_txid = %stored_txid,
            computed_txid = %computed_txid,
            "Skipping rebroadcast: persisted txid does not match transaction bytes"
        );
        return None;
    }
    Some((computed_txid, tx))
}

/// Decode a Broadcast transaction for input reservation without granting it
/// authority for network I/O.
pub(crate) fn decode_broadcast_reservation(
    batch_id: &Uuid,
    state: &SendBatchState,
) -> Option<(Uuid, Txid, Transaction)> {
    let SendBatchState::Broadcast { tx_bytes, .. } = state else {
        return None;
    };
    match bdk_wallet::bitcoin::consensus::deserialize::<Transaction>(tx_bytes) {
        Ok(tx) => Some((*batch_id, tx.compute_txid(), tx)),
        Err(error) => {
            tracing::warn!(
                %batch_id,
                %error,
                "Cannot reserve undecodable durable Broadcast transaction"
            );
            None
        }
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
    attempt_id: Uuid,
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
        let address = Address::from_str(intent.address)
            .map_err(|e| Error::Wallet(e.to_string()))?
            .require_network(network)
            .map_err(|e| Error::Wallet(e.to_string()))?;
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
            attempt_id: intent.attempt_id,
            vout: vout as u32,
            fee_contribution_sat: fee_allocations[idx],
        });
    }

    Ok(assignments)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::transaction::Version;
    use bdk_wallet::bitcoin::{
        consensus, Amount as BtcAmount, Network, ScriptBuf, Transaction, TxOut,
    };
    use uuid::Uuid;

    use super::*;
    use crate::send::payment_intent::record::{SendIntentRecord, SendIntentState};
    use crate::send::payment_intent::state::Batched as IntentBatched;
    use crate::send::payment_intent::SendIntent;
    use crate::testutil::{store_test_signed_batch, GatedKvStore, PausePoint, ReadPath};
    use crate::types::{BatchConfig, PaymentMetadata, PaymentTier};

    const ADDR_A: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
    const ADDR_B: &str = "bcrt1q6rhpng9evdsfnn833a4f4vej0asu6dk5srld6x";

    #[tokio::test]
    async fn concurrent_finalization_emits_one_success_event() {
        let backend =
            crate::testutil::build_test_backend(Arc::new(GatedKvStore::default()), None).await;
        let quote_id = QuoteId::new();
        let pending = SendIntent::new(
            &backend.storage,
            quote_id.to_string(),
            ADDR_A.to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("create intent");
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&backend.storage, batch_id, &[pending.intent_id]).await;
        let awaiting = pending
            .assign_to_batch(&backend.storage, batch_id)
            .await
            .expect("assign intent")
            .mark_broadcast(
                &backend.storage,
                "txid-concurrent-finalize".to_string(),
                "txid-concurrent-finalize:0".to_string(),
                250,
            )
            .await
            .expect("mark intent broadcast");
        let mut events = backend.payment_sender.subscribe();

        let (first, second) = tokio::join!(
            backend.finalize_send_intent_and_emit(awaiting.clone()),
            backend.finalize_send_intent_and_emit(awaiting),
        );
        first.expect("first finalizer");
        second.expect("second finalizer");

        let event = events.try_recv().expect("one payment event");
        assert!(
            matches!(
                event,
                Event::PaymentSuccessful {
                    quote_id: event_quote_id,
                    ..
                } if event_quote_id == quote_id
            ),
            "expected payment successful event"
        );
        assert!(
            matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "only the winning finalizer may emit an event"
        );
    }

    fn make_batched_intent(
        intent_id: Uuid,
        address: &str,
        amount: u64,
    ) -> SendIntent<IntentBatched> {
        SendIntent {
            intent_id,
            attempt_id: Uuid::new_v4(),
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
            attempt_id: intent.attempt_id,
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

    /// An intent whose expiry is detected from a stale `Pending` snapshot
    /// must not be failed when another worker has already batched and
    /// broadcast it: failing it would compensate (refund) a melt whose
    /// payment still goes out.
    #[tokio::test]
    async fn expiry_cannot_fail_broadcast_intent() {
        let kv = GatedKvStore::default();
        let backend = crate::testutil::build_test_backend(
            Arc::new(kv.clone()),
            Some(BatchConfig {
                max_intent_age: Some(Duration::ZERO),
                ..Default::default()
            }),
        )
        .await;

        let quote_id = QuoteId::new().to_string();
        let intent_id = Uuid::new_v4();
        backend
            .storage
            .create_send_intent_if_absent(&SendIntentRecord {
                intent_id,
                attempt_id: Uuid::new_v4(),
                quote_id: quote_id.clone(),
                address: ADDR_A.to_string(),
                amount_sat: 10_000,
                max_fee_amount_sat: 500,
                tier: PaymentTier::Immediate,
                metadata: PaymentMetadata::default(),
                state: SendIntentState::Pending {
                    created_at: 1_700_000_000,
                },
            })
            .await
            .expect("store expired pending intent");

        let mut events = backend.payment_sender.subscribe();

        let gate = kv.gate_read(
            ReadPath::Direct,
            PausePoint::AfterRead,
            crate::storage::BDK_NAMESPACE,
            crate::storage::SEND_INTENT_NAMESPACE,
            1,
        );

        let processor_backend = backend.clone();
        let processor =
            tokio::spawn(async move { processor_backend.process_ready_intents().await });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("batch processor reached the gated read");

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
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&backend.storage, batch_id, &[intent_id]).await;
        pending
            .assign_to_batch(&backend.storage, batch_id)
            .await
            .expect("assign")
            .mark_broadcast(
                &backend.storage,
                "txid-broadcast".to_string(),
                "txid-broadcast:0".to_string(),
                250,
            )
            .await
            .expect("broadcast");

        gate.release();
        tokio::time::timeout(Duration::from_secs(5), processor)
            .await
            .expect("batch processor timed out")
            .expect("join batch processor")
            .expect("process_ready_intents should not error");

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
                } if b == batch_id
            ),
            "durable state must remain AwaitingConfirmation, got {:?}",
            persisted.state
        );

        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn batch_processor_resumes_durable_cancellation() {
        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");
        let backend =
            crate::testutil::build_test_backend(Arc::new(kv), Some(BatchConfig::default())).await;
        let batch_id = Uuid::new_v4();
        let pending = SendIntent::new(
            &backend.storage,
            "quote-resume-cancelled".to_string(),
            ADDR_A.to_string(),
            10_000,
            500,
            PaymentTier::Standard,
            PaymentMetadata::default(),
        )
        .await
        .expect("store pending intent");
        let intent_id = pending.intent_id;
        let attempt_id = pending.attempt_id;
        store_test_signed_batch(&backend.storage, batch_id, &[intent_id]).await;
        pending
            .assign_to_batch(&backend.storage, batch_id)
            .await
            .expect("assign intent to cancelled batch");

        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: vec![TxOut {
                value: BtcAmount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Cancelled {
                    tx_bytes: consensus::serialize(&tx),
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                    evict_at: crate::util::unix_now().saturating_add(1),
                },
            })
            .await
            .expect("store durable cancellation");

        backend
            .process_ready_intents()
            .await
            .expect("processor should resume cancellation");

        let intent = backend
            .storage
            .get_send_intent(&intent_id)
            .await
            .expect("read intent")
            .expect("intent remains active");
        assert!(matches!(intent.state, SendIntentState::Pending { .. }));
        assert!(backend
            .storage
            .get_send_batch(&batch_id)
            .await
            .expect("read batch")
            .is_none());
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
        assert_eq!(assignments[0].attempt_id, intent_a.attempt_id);
        assert_eq!(assignments[0].vout, 0);
        assert_eq!(assignments[0].fee_contribution_sat, 50);
        assert_eq!(assignments[1].intent_id, intent_b.intent_id);
        assert_eq!(assignments[1].attempt_id, intent_b.attempt_id);
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

        use bdk_wallet::bitcoin::{consensus, Address, OutPoint};
        use bdk_wallet::keys::bip39::Mnemonic;
        use bdk_wallet::KeychainKind;
        use cdk_common::common::FeeReserve;
        use cdk_common::{Amount, CurrencyUnit};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;
        use uuid::Uuid;

        use super::super::decode_broadcast_tx;
        use super::{BtcAmount, LockTime, Network, ScriptBuf, TxOut, Version};
        use crate::send::batch_transaction::record::{
            BatchOutputAssignment, SendBatchRecord, SendBatchState,
        };
        use crate::send::payment_intent::record::{SendIntentRecord, SendIntentState};
        use crate::types::{PaymentMetadata, PaymentTier};
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
            )
            .expect("build CdkBdk test instance")
        }

        /// A minimal valid transaction so `consensus::deserialize` can
        /// round-trip it during rebroadcast.
        fn valid_tx() -> super::Transaction {
            super::Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: Vec::new(),
                output: vec![TxOut {
                    value: BtcAmount::from_sat(10_000),
                    script_pubkey: ScriptBuf::new(),
                }],
            }
        }

        /// Serialize [`valid_tx`] for storage in a `Broadcast` record.
        fn valid_tx_bytes() -> Vec<u8> {
            consensus::serialize(&valid_tx())
        }

        async fn wallet_relevant_tx(backend: &CdkBdk) -> super::Transaction {
            let script_pubkey = {
                let mut wallet_with_db = backend.wallet_with_db.lock().await;
                wallet_with_db
                    .wallet
                    .reveal_next_address(KeychainKind::External)
                    .address
                    .script_pubkey()
            };

            super::Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: Vec::new(),
                output: vec![TxOut {
                    value: BtcAmount::from_sat(10_000),
                    script_pubkey,
                }],
            }
        }

        async fn store_eligible_broadcast(
            backend: &CdkBdk,
            batch_id: Uuid,
            tx: &super::Transaction,
        ) {
            let intent_id = Uuid::new_v4();
            let txid = tx.compute_txid();
            let address = Address::from_script(&tx.output[0].script_pubkey, backend.network)
                .expect("test output script has an address");
            backend
                .storage
                .create_send_intent_if_absent(&SendIntentRecord {
                    intent_id,
                    attempt_id: intent_id,
                    quote_id: format!("quote-{intent_id}"),
                    address: address.to_string(),
                    amount_sat: 10_000,
                    max_fee_amount_sat: 500,
                    tier: PaymentTier::Immediate,
                    metadata: PaymentMetadata::default(),
                    state: SendIntentState::AwaitingConfirmation {
                        batch_id,
                        txid: txid.to_string(),
                        outpoint: OutPoint::new(txid, 0).to_string(),
                        fee_contribution_sat: 500,
                        created_at: 1_700_000_000,
                    },
                })
                .await
                .expect("store eligible intent");
            backend
                .storage
                .store_send_batch(&SendBatchRecord {
                    batch_id,
                    state: SendBatchState::Broadcast {
                        txid: txid.to_string(),
                        tx_bytes: consensus::serialize(tx),
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
                .expect("store eligible Broadcast batch");
        }

        async fn serve_esplora_response(
            status: &'static str,
            body: &'static str,
        ) -> (String, tokio::task::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test Esplora server");
            let address = listener.local_addr().expect("test server address");
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept broadcast request");
                let mut request = [0u8; 4096];
                let _bytes_read = stream
                    .read(&mut request)
                    .await
                    .expect("read broadcast request");
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write broadcast response");
            });

            (format!("http://{address}"), server)
        }

        async fn serve_blocked_esplora_response() -> (
            String,
            oneshot::Receiver<()>,
            oneshot::Sender<()>,
            tokio::task::JoinHandle<()>,
        ) {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test Esplora server");
            let address = listener.local_addr().expect("test server address");
            let (request_seen_tx, request_seen_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept broadcast request");
                let mut request = [0u8; 4096];
                let _bytes_read = stream
                    .read(&mut request)
                    .await
                    .expect("read broadcast request");
                let _ = request_seen_tx.send(());
                release_rx.await.expect("release blocked response");
                let body = "temporarily unavailable";
                let response = format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write broadcast response");
            });

            (
                format!("http://{address}"),
                request_seen_rx,
                release_tx,
                server,
            )
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
                    txid: valid_tx().compute_txid().to_string(),
                    tx_bytes: valid_tx_bytes(),
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: Uuid::nil(),
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

        /// Applying a signed transaction locally before its first network
        /// broadcast must not suppress retries after a transient failure.
        #[tokio::test]
        async fn rebroadcast_retries_locally_tracked_unconfirmed_tx() {
            let mut backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let tx = wallet_relevant_tx(&backend).await;
            let txid = tx.compute_txid();

            {
                let mut wallet_with_db = backend.wallet_with_db.lock().await;
                wallet_with_db
                    .wallet
                    .apply_unconfirmed_txs([(tx.clone(), crate::util::unix_now())]);
                wallet_with_db.persist().expect("persist local transaction");
                assert!(wallet_with_db.wallet.get_tx(txid).is_some());
            }

            store_eligible_broadcast(&backend, batch_id, &tx).await;

            let (url, server) =
                serve_esplora_response("503 Service Unavailable", "temporarily unavailable").await;
            backend.chain_source = ChainSource::Esplora(EsploraConfig {
                url,
                parallel_requests: 1,
            });

            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should swallow transport errors");
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("locally tracked transaction was not sent to Esplora")
                .expect("join test Esplora server");
        }

        /// A durable Broadcast transaction must be present in BDK before the
        /// network request begins, so another batch cannot select its inputs
        /// while rebroadcast is waiting on the backend.
        #[tokio::test]
        async fn rebroadcast_reserves_transaction_before_network_io() {
            let mut backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let tx = wallet_relevant_tx(&backend).await;
            let txid = tx.compute_txid();
            store_eligible_broadcast(&backend, batch_id, &tx).await;

            let (url, request_seen, release, server) = serve_blocked_esplora_response().await;
            backend.chain_source = ChainSource::Esplora(EsploraConfig {
                url,
                parallel_requests: 1,
            });
            let rebroadcast_backend = backend.clone();
            let rebroadcast =
                tokio::spawn(async move { rebroadcast_backend.rebroadcast_stuck_batches().await });

            tokio::time::timeout(Duration::from_secs(5), request_seen)
                .await
                .expect("rebroadcast request was not sent")
                .expect("request signal dropped");
            {
                let wallet_with_db =
                    tokio::time::timeout(Duration::from_secs(1), backend.wallet_with_db.lock())
                        .await
                        .expect("network request must not hold the wallet lock");
                assert!(
                    wallet_with_db.wallet.get_tx(txid).is_some(),
                    "transaction must be reserved before network I/O"
                );
            }

            release.send(()).expect("release blocked response");
            tokio::time::timeout(Duration::from_secs(5), rebroadcast)
                .await
                .expect("rebroadcast timed out")
                .expect("join rebroadcast task")
                .expect("rebroadcast should swallow transport errors");
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("server timed out")
                .expect("join test Esplora server");
        }

        /// Reapplication must use a last-seen timestamp strictly newer than an
        /// existing eviction marker or BDK will keep the transaction excluded
        /// from its canonical graph.
        #[tokio::test]
        async fn rebroadcast_reservation_beats_last_evicted_timestamp() {
            let mut backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let tx = wallet_relevant_tx(&backend).await;
            let txid = tx.compute_txid();
            let evicted_at = crate::util::unix_now().saturating_add(60);
            {
                let mut wallet_with_db = backend.wallet_with_db.lock().await;
                wallet_with_db
                    .wallet
                    .apply_unconfirmed_txs([(tx.clone(), evicted_at.saturating_sub(1))]);
                wallet_with_db
                    .wallet
                    .apply_evicted_txs([(txid, evicted_at)]);
                wallet_with_db
                    .persist()
                    .expect("persist evicted transaction");
                assert!(wallet_with_db.wallet.get_tx(txid).is_none());
                assert_eq!(
                    wallet_with_db.wallet.tx_graph().get_last_evicted(txid),
                    Some(evicted_at)
                );
            }
            store_eligible_broadcast(&backend, batch_id, &tx).await;

            let (url, server) =
                serve_esplora_response("503 Service Unavailable", "temporarily unavailable").await;
            backend.chain_source = ChainSource::Esplora(EsploraConfig {
                url,
                parallel_requests: 1,
            });
            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("rebroadcast should swallow transport errors");
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("server timed out")
                .expect("join test Esplora server");

            let wallet_with_db = backend.wallet_with_db.lock().await;
            let wallet_tx = wallet_with_db
                .wallet
                .get_tx(txid)
                .expect("reservation must restore canonical transaction");
            let bdk_wallet::chain::ChainPosition::Unconfirmed {
                last_seen: Some(last_seen),
                ..
            } = wallet_tx.chain_position
            else {
                panic!("reserved transaction must be unconfirmed with a last-seen timestamp");
            };
            assert!(last_seen > evicted_at);
        }

        /// A reservation made atomically with sync request construction must
        /// be newer than the eviction timestamp that the in-flight request can
        /// later apply.
        #[tokio::test]
        async fn sync_reservation_survives_its_request_eviction_timestamp() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let tx = wallet_relevant_tx(&backend).await;
            let txid = tx.compute_txid();
            store_eligible_broadcast(&backend, batch_id, &tx).await;

            let sync_started_at = crate::util::unix_now().saturating_add(60);
            let reservation_time = sync_started_at
                .checked_add(1)
                .expect("test timestamp has room");
            let reservations = backend
                .load_broadcast_reservations()
                .await
                .expect("load Broadcast reservations");
            let mut wallet_with_db = backend.wallet_with_db.lock().await;
            CdkBdk::reserve_broadcast_transactions_locked(
                &mut wallet_with_db.wallet,
                &reservations,
                reservation_time,
            )
            .expect("reserve eligible Broadcast batch");
            wallet_with_db
                .wallet
                .apply_evicted_txs([(txid, sync_started_at)]);

            let wallet_tx = wallet_with_db
                .wallet
                .get_tx(txid)
                .expect("sync eviction must not defeat its atomic reservation");
            let bdk_wallet::chain::ChainPosition::Unconfirmed {
                last_seen: Some(last_seen),
                ..
            } = wallet_tx.chain_position
            else {
                panic!("reserved transaction must remain canonical and unconfirmed");
            };
            assert!(last_seen > sync_started_at);
        }

        /// An already-known response proves the backend has the transaction,
        /// so a wallet that was missing it must record and persist it locally.
        #[tokio::test]
        async fn rebroadcast_applies_already_known_tx_to_wallet() {
            let mut backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();
            let tx = wallet_relevant_tx(&backend).await;
            let txid = tx.compute_txid();

            {
                let wallet_with_db = backend.wallet_with_db.lock().await;
                assert!(wallet_with_db.wallet.get_tx(txid).is_none());
            }

            store_eligible_broadcast(&backend, batch_id, &tx).await;

            let (url, server) =
                serve_esplora_response("400 Bad Request", "transaction already known").await;
            backend.chain_source = ChainSource::Esplora(EsploraConfig {
                url,
                parallel_requests: 1,
            });

            tokio::time::timeout(Duration::from_secs(5), backend.rebroadcast_stuck_batches())
                .await
                .expect("rebroadcast timed out")
                .expect("already-known rebroadcast should succeed");
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("transaction was not sent to Esplora")
                .expect("join test Esplora server");

            let wallet_with_db = backend.wallet_with_db.lock().await;
            assert!(
                wallet_with_db.wallet.get_tx(txid).is_some(),
                "already-known transaction must be tracked by the local wallet"
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
                        attempt_id: Uuid::nil(),
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

        /// Conflicting durable transaction identities are fenced.
        #[test]
        fn decode_broadcast_tx_rejects_txid_mismatch() {
            let tx_bytes = valid_tx_bytes();
            assert!(decode_broadcast_tx(&Uuid::new_v4(), TEST_TXID, &tx_bytes).is_none());
        }

        #[test]
        fn decode_broadcast_tx_accepts_matching_identity() {
            let tx = valid_tx();
            let txid = tx.compute_txid();
            let (decoded_txid, decoded) = decode_broadcast_tx(
                &Uuid::new_v4(),
                &txid.to_string(),
                &consensus::serialize(&tx),
            )
            .expect("matching transaction");
            assert_eq!(decoded_txid, txid);
            assert_eq!(decoded, tx);
        }

        /// Undecodable bytes are the only reason a `Broadcast` record is
        /// skipped: without valid bytes there is nothing safe to broadcast.
        #[test]
        fn decode_broadcast_tx_rejects_undecodable_bytes() {
            assert!(decode_broadcast_tx(&Uuid::new_v4(), TEST_TXID, &[0xff]).is_none());
        }

        /// A malformed stored identity is retained but never reaches the
        /// reservation or network path.
        #[tokio::test]
        async fn rebroadcast_malformed_stored_txid_is_fenced() {
            let backend = build_test_instance().await;
            let batch_id = Uuid::new_v4();

            let batch = SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: "not-a-valid-txid".to_string(),
                    tx_bytes: valid_tx_bytes(),
                    assignments: vec![BatchOutputAssignment {
                        intent_id: Uuid::new_v4(),
                        attempt_id: Uuid::nil(),
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
                .expect("rebroadcast should swallow transport errors");

            // Batch must still be in Broadcast state for the next retry.
            let after = backend
                .storage
                .get_send_batch(&batch_id)
                .await
                .expect("fetch batch")
                .expect("batch still present");
            assert!(matches!(after.state, SendBatchState::Broadcast { .. }));
        }
    }
}
