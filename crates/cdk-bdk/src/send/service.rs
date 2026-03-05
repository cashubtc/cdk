use std::str::FromStr;

use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RawTx, RpcApi};
use bdk_esplora::esplora_client::Builder;
// use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::{Address, OutPoint, Transaction};
use cdk_common::payment::{Event, MakePaymentResponse, PaymentIdentifier};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState, QuoteId};
use tokio::time::interval;
use uuid::Uuid;

use crate::error::Error;
use crate::send::batch_transaction::{allocate_batch_fee, state as batch_state, SendBatch};
use crate::send::payment_intent::{self, state as intent_state, SendIntent, SendIntentAny};
use crate::types::PaymentTier;
use crate::{CdkBdk, ChainSource};

impl CdkBdk {
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

    pub(crate) fn fee_reserve_for_estimate(&self, estimated_sat: u64) -> u64 {
        let percent_padded =
            (estimated_sat as f64 * (1.0 + self.fee_reserve.percent_fee_reserve as f64)) as u64;
        let min_reserve = self.fee_reserve.min_fee_reserve.into();
        std::cmp::max(percent_padded, min_reserve)
    }

    pub(crate) fn collect_intent_broadcast_details(
        &self,
        tx: &Transaction,
        intents: &[SendIntent<intent_state::Batched>],
    ) -> Result<Vec<(String, String)>, Error> {
        let mut claimed_vouts = std::collections::HashSet::new();
        let mut details = Vec::with_capacity(intents.len());

        for intent in intents {
            let address = Address::from_str(&intent.address)
                .map_err(|e| Error::Wallet(e.to_string()))?
                .require_network(self.network)
                .map_err(|e| Error::Wallet(e.to_string()))?;
            let vout = tx
                .output
                .iter()
                .enumerate()
                .find_map(|(vout_idx, output)| {
                    if claimed_vouts.contains(&vout_idx) {
                        return None;
                    }
                    Address::from_script(output.script_pubkey.as_script(), self.network)
                        .ok()
                        .filter(|candidate| *candidate == address)
                        .filter(|_| output.value.to_sat() == intent.amount)
                        .map(|_| vout_idx)
                })
                .ok_or(Error::VoutNotFound)?;
            claimed_vouts.insert(vout);

            let txid = tx.compute_txid().to_string();
            let outpoint = OutPoint::new(tx.compute_txid(), vout as u32).to_string();
            details.push((txid, outpoint));
        }

        Ok(details)
    }

    pub(crate) async fn broadcast_transaction_internal(
        &self,
        tx: Transaction,
    ) -> Result<(), Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                let rpc_client: Client = Client::new(
                    &format!("http://{}:{}", rpc_config.host, rpc_config.port),
                    Auth::UserPass(rpc_config.user.clone(), rpc_config.password.clone()),
                )?;

                tracing::info!(
                    "Broadcasting transaction: {} via bitcoin rpc",
                    tx.compute_txid()
                );

                rpc_client.send_raw_transaction(tx.raw_hex())?;
            }
            ChainSource::Esplora { url, .. } => {
                let client = Builder::new(url)
                    .build_async()
                    .map_err(|e| Error::Esplora(e.to_string()))?;

                tracing::info!(
                    "Broadcasting transaction: {} via esplora",
                    tx.compute_txid()
                );

                client
                    .broadcast(&tx)
                    .await
                    .map_err(|e| Error::Esplora(e.to_string()))?;
            }
        }

        Ok(())
    }

    pub(crate) async fn run_batch_processor(&self) -> Result<(), Error> {
        let poll_interval = self.batch_config.poll_interval;
        let mut tick = interval(poll_interval);

        tracing::info!("Starting send saga batch processor");

        loop {
            tokio::select! {
                _ = self.events_cancel_token.cancelled() => {
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
        let mut ready_standard = Vec::new();
        let mut ready_economy = Vec::new();

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
                    if let Ok(quote_id) = QuoteId::from_str(&intent.quote_id) {
                        if let Err(err) = self.payment_sender.send(Event::PaymentFailed {
                            quote_id,
                            reason: format!(
                                "Intent expired after {}s (max: {}s)",
                                age_secs,
                                max_age.as_secs()
                            ),
                        }) {
                            tracing::error!(
                                "Could not send payment failed event for intent {}: {}",
                                intent.intent_id,
                                err
                            );
                        }
                    }
                    // Delete expired intent (best-effort)
                    if let Err(e) = self.storage.delete_send_intent(&intent.intent_id).await {
                        tracing::error!(
                            "Failed to delete expired intent {}: {}",
                            intent.intent_id,
                            e
                        );
                    }
                    continue;
                }
            }

            match intent.tier {
                PaymentTier::Immediate => immediate.push(intent),
                PaymentTier::Standard => {
                    if age_secs >= self.batch_config.standard_deadline.as_secs() {
                        ready_standard.push(intent);
                    }
                }
                PaymentTier::Economy => {
                    if age_secs >= self.batch_config.economy_deadline.as_secs() {
                        ready_economy.push(intent);
                    }
                }
            }
        }

        // If there are immediate intents, allow lower-tier ready intents
        // to piggyback
        let has_immediate = !immediate.is_empty();

        let mut batch_intents: Vec<_> = immediate;

        if has_immediate {
            // Piggyback all ready lower-tier intents
            batch_intents.extend(ready_standard);
            batch_intents.extend(ready_economy);
        } else {
            // Check if we have enough ready intents to form a batch
            let combined: Vec<_> = ready_standard.into_iter().chain(ready_economy).collect();
            if combined.len() >= self.batch_config.min_batch_threshold {
                batch_intents = combined;
            }
        }

        if batch_intents.is_empty() {
            return Ok(());
        }

        // Respect max_batch_size
        batch_intents.truncate(self.batch_config.max_batch_size);

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

        // 1. Build the PSBT
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let mut tx_builder = wallet_with_db.wallet.build_tx();

        for intent in &intents {
            let address = Address::from_str(&intent.address)
                .map_err(|e| Error::Wallet(e.to_string()))?
                .require_network(self.network)
                .map_err(|e| Error::Wallet(e.to_string()))?;
            tx_builder.add_recipient(
                address.clone(),
                bdk_wallet::bitcoin::Amount::from_sat(intent.amount),
            );
        }

        let mut psbt = match tx_builder.finish() {
            Ok(psbt) => psbt,
            Err(e) => {
                tracing::error!("Failed to build batch PSBT: {}", e);
                return Err(Error::Wallet(e.to_string()));
            }
        };

        // Validate batch fee
        let fee = psbt.fee().map_err(|e| Error::Wallet(e.to_string()))?;
        let actual_fee = fee.to_sat();
        let max_fees: Vec<u64> = intents.iter().map(|i| i.max_fee_amount).collect();
        let intent_ids: Vec<Uuid> = intents.iter().map(|i| i.intent_id).collect();

        let fee_allocations = match allocate_batch_fee(actual_fee, &max_fees, &intent_ids) {
            Ok(alloc) => alloc,
            Err(e) => {
                tracing::warn!("Fee allocation failed, cancelling batch: {}", e);
                wallet_with_db.wallet.cancel_tx(&psbt.unsigned_tx);
                return Err(e);
            }
        };

        // Serialize PSBT
        let psbt_bytes = psbt.serialize();

        // Persist wallet state after build
        wallet_with_db.persist()?;

        // 2. Sign
        if !wallet_with_db
            .wallet
            .sign(&mut psbt, Default::default())
            .map_err(|e| Error::Wallet(e.to_string()))?
        {
            wallet_with_db.wallet.cancel_tx(&psbt.unsigned_tx);
            return Err(Error::CouldNotSign);
        }

        wallet_with_db.persist()?;

        // Extract final transaction
        let tx = psbt
            .extract_tx()
            .map_err(|e| Error::Wallet(e.to_string()))?;
        let tx_bytes = bdk_wallet::bitcoin::consensus::serialize(&tx);
        let txid = tx.compute_txid();

        // Drop wallet lock before broadcasting
        drop(wallet_with_db);

        // 3. Transition intents to Batched
        let mut batched_intents = Vec::new();
        for intent in intents {
            let batched = intent.assign_to_batch(&self.storage, batch_id).await?;
            batched_intents.push(batched);
        }

        let broadcast_details = self.collect_intent_broadcast_details(&tx, &batched_intents)?;

        // 4. Create batch as Built, then sign → mark_broadcast using typestates.
        //    Each transition persists atomically.
        let built_batch =
            SendBatch::new(&self.storage, batch_id, psbt_bytes, batched_intents.clone()).await?;

        let signed_batch = match built_batch
            .sign(&self.storage, tx_bytes.clone(), actual_fee)
            .await
        {
            Ok(batch) => batch,
            Err(e) => {
                tracing::error!(
                    "Failed to persist Signed state for batch {}: {}",
                    batch_id,
                    e
                );
                // Reconstruct Built batch for compensation (persistence failed
                // so the batch is still in Built state in storage).
                let built = SendBatch::<batch_state::Built>::reconstruct(batch_id, batched_intents);
                if let Err(comp_err) = built.compensate(&self.storage).await {
                    tracing::error!("Compensation after sign failure also failed: {}", comp_err);
                }
                return Err(e);
            }
        };

        // 5. Persist Broadcast state BEFORE actually broadcasting (crash safety)
        let broadcast_result = match signed_batch
            .mark_broadcast(&self.storage, txid.to_string(), tx_bytes, actual_fee)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    "Failed to persist Broadcast state for batch {}: {}",
                    batch_id,
                    e
                );
                // Reconstruct Signed batch for compensation.
                let signed =
                    SendBatch::<batch_state::Signed>::reconstruct(batch_id, batched_intents);
                if let Err(comp_err) = signed.compensate(&self.storage).await {
                    tracing::error!(
                        "Compensation after broadcast-persist failure also failed: {}",
                        comp_err
                    );
                }
                return Err(e);
            }
        };

        // 6. Transition intents to AwaitingConfirmation before network send.
        for (idx, intent) in broadcast_result.intents.into_iter().enumerate() {
            let (intent_txid, outpoint_str) = &broadcast_details[idx];
            intent
                .mark_broadcast(
                    &self.storage,
                    intent_txid.clone(),
                    outpoint_str.clone(),
                    fee_allocations[idx],
                )
                .await?;
        }

        // 7. Broadcast
        if let Err(e) = self.broadcast_transaction_internal(tx.clone()).await {
            tracing::error!("Broadcast failed for batch {}: {}", batch_id, e);
            // Post-Broadcast-persist failure: the batch record and intents are
            // already marked for reconciliation. Recovery will attempt rebroadcast.
            return Err(e);
        }

        tracing::info!(
            "Batch {} broadcast as txid {} with {} intents",
            batch_id,
            txid,
            broadcast_details.len()
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
            let intent_ids = match &batch.state {
                crate::send::batch_transaction::record::SendBatchState::Broadcast {
                    intent_ids,
                    ..
                } => intent_ids,
                _ => continue, // Only clean up broadcast batches
            };

            let has_active = intent_ids
                .iter()
                .any(|id| all_active_intents.iter().any(|i| i.intent_id == *id));

            if !has_active {
                tracing::info!("Cleaning up completed batch {}", batch.batch_id);
                self.storage.delete_send_batch(&batch.batch_id).await?;
            }
        }
        Ok(())
    }
}
