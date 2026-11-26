//! Payjoin v2 negotiation backed by durable PDK event logs.

use std::future::Future;
use std::str::FromStr;
use std::time::Duration;

use bdk_wallet::bitcoin::{Address, Amount};
use futures::{stream, StreamExt};
use payjoin::persist::{InMemoryPersister, SessionPersister};
use payjoin::send::v2::{self, SendSession, SenderBuilder, SessionEvent, SessionOutcome};
use serde::{Deserialize, Serialize};

use self::persist::{Persister, SessionRecord};
use crate::fee::fee_rate_from_sat_per_vb;
use crate::send::batch_transaction::record::{
    BatchOutputAssignment, SendBatchRecord, SendBatchState,
};
use crate::send::batch_transaction::SendBatch;
use crate::send::payment_intent::record::SendIntentRecord;
use crate::send::payment_intent::{self, SendIntentAny};
use crate::{CdkBdk, Error};

mod persist;
mod receive;
#[cfg(all(test, feature = "esplora"))]
mod tests;

/// Operator-controlled OHTTP transport configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayjoinConfig {
    /// OHTTP relay URL, independent of the receiver's directory.
    pub relay_url: String,
    /// Directory used for incoming Payjoins; requires the Bitcoin RPC chain source.
    #[serde(default)]
    pub directory_url: Option<String>,
}

fn pj_error(error: impl std::fmt::Display) -> Error {
    Error::Payjoin(error.to_string())
}

// Separate limits keep idle incoming quotes from occupying sender capacity.
const MAX_CONCURRENT_SESSIONS: usize = 8;

async fn run_payjoin_worker<F, Fut>(
    cancel: tokio_util::sync::CancellationToken,
    interval: Duration,
    mut process: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), Error>>,
{
    let mut tick = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {},
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            result = process() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "Payjoin processing will be retried");
                }
            }
        }
    }
}

impl CdkBdk {
    pub(crate) async fn run_payjoin_processor(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), Error> {
        let interval = self.batch_config.poll_interval.min(Duration::from_secs(5));
        tokio::join!(
            run_payjoin_worker(cancel.clone(), interval, || self.process_payjoin_sends()),
            run_payjoin_worker(cancel, interval, || self.process_payjoin_receivers()),
        );
        Ok(())
    }

    #[cfg(all(test, feature = "esplora"))]
    async fn process_payjoin_sessions(&self) -> Result<(), Error> {
        let (sends, receives) = tokio::join!(
            self.process_payjoin_sends(),
            self.process_payjoin_receivers(),
        );
        sends?;
        receives
    }

    async fn process_payjoin_sends(&self) -> Result<(), Error> {
        self.reconcile_payjoin_sends().await?;
        // A failed/cancelled handoff may have committed the batch but not yet
        // advanced its intent to AwaitingConfirmation. Resume it without requiring
        // a process restart; these intents are no longer in the pending queue.
        if self
            .storage
            .get_all_send_intents()
            .await?
            .iter()
            .any(|intent| {
                intent.metadata.payjoin.is_some()
                    && matches!(
                        intent.state,
                        payment_intent::record::SendIntentState::Batched { .. }
                    )
            })
        {
            let _processing = self.send_processing.lock().await;
            self.recover_send_saga().await?;
        }
        let intents = self.storage.get_pending_send_intents().await?;
        let mut results = stream::iter(
            intents
                .into_iter()
                .filter(|intent| intent.metadata.payjoin.is_some()),
        )
        .map(|intent| async move {
            if let Err(error) = self.process_payjoin_send(&intent).await {
                tracing::warn!(intent_id = %intent.intent_id, %error, "Payjoin processing failed");
                self.fail_unstarted_payjoin(&intent, &error).await?;
            }
            Ok::<_, Error>(())
        })
        .buffer_unordered(MAX_CONCURRENT_SESSIONS);
        // Drain the pass even if one session fails, without cancelling its peers.
        let mut result = Ok(());
        while let Some(next) = results.next().await {
            result = result.and(next);
        }
        result?;
        Ok(())
    }

    fn sign_payjoin_psbt(
        &self,
        wallet: &bdk_wallet::Wallet,
        psbt: &mut bdk_wallet::bitcoin::Psbt,
    ) -> Result<bool, bdk_wallet::signer::SignerError> {
        wallet.sign_with_signers(
            psbt,
            &[&self.signers[0], &self.signers[1]],
            bdk_wallet::SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
    }

    pub(crate) async fn reconcile_payjoin_sends(&self) -> Result<(), Error> {
        for (id, mut record) in self.storage.payjoin_records().await? {
            if record.address.is_some() || record.completed {
                continue;
            }
            let intent_id = uuid::Uuid::parse_str(&id).map_err(pj_error)?;
            if self
                .storage
                .get_finalized_intent(&intent_id)
                .await?
                .is_some()
            {
                record.completed = true;
                Persister::<SessionEvent>::new(self.storage.clone(), id)
                    .write(&record)
                    .await?;
                continue;
            }
            let Some(mut intent) = self.storage.get_send_intent(&intent_id).await? else {
                continue;
            };
            let persister = Persister::<SessionEvent>::new(self.storage.clone(), id.clone());
            let fallback = match v2::replay_event_log_async(&persister).await {
                Ok((_, history)) => Some(history.fallback_tx()),
                Err(error) if error.is_expired() => error.expiry_fallback_tx().cloned(),
                Err(error) => return Err(pj_error(error)),
            };
            let Some(fallback) = fallback else {
                continue;
            };
            let txid = fallback.compute_txid();
            let _processing = self.send_processing.lock().await;
            let confirmed = {
                let wallet = self.wallet_with_db.lock().await;
                self.txid_has_required_confirmations(
                    &wallet.wallet,
                    &txid.to_string(),
                    "payjoin_fallback",
                    &id,
                )
            };
            if !confirmed {
                continue;
            }
            let (fee, vout) = self.payjoin_sender_assignment(&intent, &fallback).await?;
            let assignment = BatchOutputAssignment {
                intent_id,
                vout,
                fee_contribution_sat: fee,
            };
            self.storage
                .store_send_batch(&SendBatchRecord {
                    batch_id: intent_id,
                    state: SendBatchState::Broadcast {
                        txid: txid.to_string(),
                        tx_bytes: bdk_wallet::bitcoin::consensus::serialize(&fallback),
                        assignments: vec![assignment],
                        fee_sat: fee,
                    },
                })
                .await?;
            use crate::send::payment_intent::record::SendIntentState;
            let created_at = match intent.state {
                SendIntentState::Pending { created_at }
                | SendIntentState::Batched { created_at, .. }
                | SendIntentState::AwaitingConfirmation { created_at, .. }
                | SendIntentState::Failed { created_at, .. } => created_at,
            };
            intent.state = SendIntentState::AwaitingConfirmation {
                batch_id: intent_id,
                txid: txid.to_string(),
                outpoint: bdk_wallet::bitcoin::OutPoint::new(txid, vout).to_string(),
                fee_contribution_sat: fee,
                created_at,
            };
            self.storage
                .update_send_intent(&intent_id, &intent.state)
                .await?;
            if let crate::send::payment_intent::SendIntentAny::AwaitingConfirmation(intent) =
                crate::send::payment_intent::from_record(&intent)
            {
                self.finalize_send_intent_and_emit(intent).await?;
            }
            record.completed = true;
            persister.write(&record).await?;
        }
        Ok(())
    }

    pub(crate) async fn fail_unstarted_payjoin(
        &self,
        intent: &SendIntentRecord,
        error: &Error,
    ) -> Result<(), Error> {
        if self
            .storage
            .kv_store
            .kv_read(
                crate::storage::BDK_NAMESPACE,
                persist::NAMESPACE,
                &intent.intent_id.to_string(),
            )
            .await?
            .is_none()
        {
            if let crate::send::payment_intent::SendIntentAny::Pending(intent) =
                crate::send::payment_intent::from_record(intent)
            {
                self.fail_send_intents(&[intent], &error.to_string()).await;
            }
        }
        Ok(())
    }

    pub(crate) fn validate_payjoin_metadata(&self, metadata: Option<&str>) -> Result<(), Error> {
        let Some(metadata) = metadata else {
            return Ok(());
        };
        let value: serde_json::Value = serde_json::from_str(metadata)?;
        if let Some(value) = value.get("payjoin") {
            let params: cdk_common::nuts::nut31::PayjoinV2 = serde_json::from_value(value.clone())?;
            if self.payjoin_config.is_none() {
                return Err(Error::Payjoin(
                    "Payjoin relay is not configured".to_string(),
                ));
            }
            if cdk_common::payjoin::payjoin_v2_is_expired_at(&params, crate::util::unix_now()) {
                return Err(Error::Payjoin("Destination has expired".to_string()));
            }
            let endpoint =
                cdk_common::payjoin::payjoin_v2_to_bip77_endpoint(&params).map_err(pj_error)?;
            payjoin::uri::PjParam::parse(endpoint).map_err(pj_error)?;
        }
        Ok(())
    }

    /// Enable Payjoin v2 using an operator-selected OHTTP relay.
    pub fn with_payjoin(mut self, config: PayjoinConfig) -> Result<Self, Error> {
        let url = reqwest::Url::parse(&config.relay_url).map_err(pj_error)?;
        if url.scheme() != "https" || url.host_str().is_none() {
            return Err(Error::InvalidConfig(
                "Payjoin relay must use HTTPS".to_string(),
            ));
        }
        if let Some(directory) = &config.directory_url {
            let directory = reqwest::Url::parse(directory).map_err(pj_error)?;
            if directory.scheme() != "https" || directory.host_str().is_none() {
                return Err(Error::InvalidConfig(
                    "Payjoin directory must use HTTPS".to_string(),
                ));
            }
            #[cfg(feature = "bitcoin-rpc")]
            let supported = matches!(self.chain_source, crate::ChainSource::BitcoinRpc(_));
            #[cfg(not(feature = "bitcoin-rpc"))]
            let supported = false;
            if !supported {
                return Err(Error::InvalidConfig(
                    "Payjoin receiving requires Bitcoin RPC for transaction validation".to_string(),
                ));
            }
        }
        self.payjoin_config = Some(config);
        Ok(self)
    }

    fn payjoin_relay_url(&self) -> Result<&str, Error> {
        self.payjoin_config
            .as_ref()
            .map(|config| config.relay_url.as_str())
            .ok_or_else(|| Error::Payjoin("Payjoin relay is not configured".to_string()))
    }

    async fn payjoin_request(&self, request: payjoin::Request) -> Result<Vec<u8>, Error> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(20));
        #[cfg(all(test, feature = "payjoin-regtest"))]
        let client = match &self.payjoin_test_root {
            Some(cert) => {
                client.add_root_certificate(reqwest::Certificate::from_der(cert).map_err(pj_error)?)
            }
            None => client,
        };
        let client = client.build().map_err(pj_error)?;
        let mut response = client
            .post(request.url)
            .header(reqwest::header::CONTENT_TYPE, request.content_type)
            .body(request.body)
            .send()
            .await
            .map_err(pj_error)?
            .error_for_status()
            .map_err(pj_error)?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(pj_error)? {
            if body.len() + chunk.len() > 1_048_576 {
                return Err(Error::Payjoin("Oversized relay response".to_string()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub(crate) async fn process_payjoin_send(
        &self,
        intent: &SendIntentRecord,
    ) -> Result<(), Error> {
        let persister =
            Persister::<SessionEvent>::new(self.storage.clone(), intent.intent_id.to_string());
        // A durable batch owns the transaction once negotiation has completed.
        if self
            .storage
            .get_send_batch(&intent.intent_id)
            .await?
            .is_some()
        {
            let _processing = self.send_processing.lock().await;
            return self.recover_send_saga().await;
        }
        let params = intent
            .metadata
            .payjoin
            .as_ref()
            .ok_or_else(|| Error::Payjoin("Missing destination parameters".to_string()))?;
        let exists = self
            .storage
            .kv_store
            .kv_read(
                crate::storage::BDK_NAMESPACE,
                persist::NAMESPACE,
                &persister.id,
            )
            .await?
            .is_some();
        if !exists {
            self.payjoin_relay_url()?;
            if cdk_common::payjoin::payjoin_v2_is_expired_at(params, crate::util::unix_now()) {
                return Err(Error::Payjoin("Destination has expired".to_string()));
            }
            let endpoint =
                cdk_common::payjoin::payjoin_v2_to_bip77_endpoint(params).map_err(pj_error)?;
            let payjoin::uri::PjParam::V2(pj_param) =
                payjoin::uri::PjParam::parse(endpoint).map_err(pj_error)?
            else {
                return Err(Error::Payjoin("Expected a v2 endpoint".to_string()));
            };
            let address = Address::from_str(&intent.address)
                .map_err(pj_error)?
                .require_network(self.network)
                .map_err(pj_error)?;
            let rate = self
                .estimate_fee_rate_sat_per_vb(intent.tier)
                .await
                .unwrap_or(self.batch_config.fee_estimation.fallback_sat_per_vb);
            let rate = fee_rate_from_sat_per_vb(rate)?;
            let mut wallet = self.wallet_with_db.lock().await;
            let reserved = self.storage.payjoin_reserved_inputs().await?;
            let mut builder = wallet.wallet.build_tx();
            builder.add_recipient(address.clone(), Amount::from_sat(intent.amount_sat));
            builder.fee_rate(rate).unspendable(reserved);
            let mut psbt = builder.finish().map_err(pj_error)?;
            wallet.persist()?;
            let original_fee = psbt.fee().map_err(pj_error)?.to_sat();
            if original_fee > intent.max_fee_amount_sat {
                return Err(Error::EstimatedFeeTooHigh {
                    estimated_fee: original_fee,
                    max_fee: intent.max_fee_amount_sat,
                });
            }
            if !self
                .sign_payjoin_psbt(&wallet.wallet, &mut psbt)
                .map_err(pj_error)?
            {
                return Err(Error::CouldNotSign);
            }
            let transition = SenderBuilder::from_parts(
                psbt.clone(),
                &pj_param,
                &address,
                Some(Amount::from_sat(intent.amount_sat)),
            )
            .always_disable_output_substitution()
            .build_with_additional_fee(
                Amount::from_sat(intent.max_fee_amount_sat - original_fee),
                None,
                rate,
                true,
            )
            .map_err(pj_error)?;
            let memory = InMemoryPersister::<SessionEvent>::default();
            transition.save(&memory).map_err(pj_error)?;
            let events = memory
                .load()
                .map_err(pj_error)?
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?;
            persister
                .insert(&SessionRecord {
                    events,
                    closed: false,
                    inputs: psbt
                        .unsigned_tx
                        .input
                        .iter()
                        .map(|i| i.previous_output)
                        .collect(),
                    expires_at: params.expires_at,
                    completed: false,
                    address: None,
                    input_value_sat: 0,
                })
                .await?;
        }
        let (session, history) = match v2::replay_event_log_async(&persister).await {
            Ok(replayed) => replayed,
            Err(error) if error.is_expired() => {
                let tx = error
                    .expiry_fallback_tx()
                    .cloned()
                    .ok_or_else(|| pj_error(&error))?;
                return self.finish_payjoin_send(intent, tx).await;
            }
            Err(error) => return Err(pj_error(error)),
        };
        let expired = crate::util::unix_now() >= persister.record().await?.expires_at;
        let proposal = match session {
            SendSession::Closed(SessionOutcome::Success(psbt)) => Some(psbt),
            SendSession::Closed(SessionOutcome::Aborted) | SendSession::PendingFallback(_) => {
                return self
                    .finish_payjoin_send(intent, history.fallback_tx())
                    .await;
            }
            SendSession::WithReplyKey(sender) if expired || self.payjoin_config.is_none() => {
                sender
                    .cancel()
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
                return self
                    .finish_payjoin_send(intent, history.fallback_tx())
                    .await;
            }
            SendSession::PollingForProposal(sender) if expired || self.payjoin_config.is_none() => {
                sender
                    .cancel()
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
                return self
                    .finish_payjoin_send(intent, history.fallback_tx())
                    .await;
            }
            SendSession::WithReplyKey(sender) => {
                let (request, context) = sender
                    .create_v2_post_request(self.payjoin_relay_url()?)
                    .map_err(pj_error)?;
                let response = self.payjoin_request(request).await?;
                sender
                    .process_response(&response, context)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
                None
            }
            SendSession::PollingForProposal(sender) => {
                let (request, context) = sender
                    .create_poll_request(self.payjoin_relay_url()?)
                    .map_err(pj_error)?;
                let response = self.payjoin_request(request).await?;
                sender
                    .process_response(&response, context)
                    .save_async(&persister)
                    .await
                    .map_err(pj_error)?;
                // Replay the persisted result on the next tick, including after a crash.
                None
            }
        };
        if let Some(mut psbt) = proposal {
            let wallet = self.wallet_with_db.lock().await;
            if !self
                .sign_payjoin_psbt(&wallet.wallet, &mut psbt)
                .map_err(pj_error)?
            {
                return Err(Error::CouldNotSign);
            }
            let tx = psbt.clone().extract_tx().map_err(pj_error)?;
            drop(wallet);
            self.finish_payjoin_send(intent, tx).await?;
        }
        Ok(())
    }

    async fn payjoin_sender_assignment(
        &self,
        intent: &SendIntentRecord,
        tx: &bdk_wallet::bitcoin::Transaction,
    ) -> Result<(u64, u32), Error> {
        let wallet = self.wallet_with_db.lock().await;
        // Count only our net expenditure; receiver inputs and fees belong to the receiver.
        let script = Address::from_str(&intent.address)
            .map_err(pj_error)?
            .require_network(self.network)
            .map_err(pj_error)?
            .script_pubkey();
        let (sent, _) = wallet.wallet.sent_and_received(tx);
        let received = tx
            .output
            .iter()
            .filter(|o| o.script_pubkey != script && wallet.wallet.is_mine(o.script_pubkey.clone()))
            .try_fold(0u64, |sum, o| sum.checked_add(o.value.to_sat()))
            .ok_or(Error::FeeEstimationOverflow)?;
        let fee = sent
            .to_sat()
            .checked_sub(received)
            .and_then(|n| n.checked_sub(intent.amount_sat))
            .ok_or_else(|| Error::Payjoin("Invalid wallet expenditure".to_string()))?;
        if fee > intent.max_fee_amount_sat {
            return Err(Error::EstimatedFeeTooHigh {
                estimated_fee: fee,
                max_fee: intent.max_fee_amount_sat,
            });
        }

        let vout = tx
            .output
            .iter()
            .position(|o| o.script_pubkey == script && o.value.to_sat() >= intent.amount_sat)
            .ok_or(Error::VoutNotFound)? as u32;
        Ok((fee, vout))
    }

    async fn finish_payjoin_send(
        &self,
        intent: &SendIntentRecord,
        tx: bdk_wallet::bitcoin::Transaction,
    ) -> Result<(), Error> {
        let _processing = self.send_processing.lock().await;
        if self
            .storage
            .get_send_batch(&intent.intent_id)
            .await?
            .is_some()
        {
            return self.recover_send_saga().await;
        }
        let (fee, vout) = self.payjoin_sender_assignment(intent, &tx).await?;
        let SendIntentAny::Pending(pending) = payment_intent::from_record(intent) else {
            return Err(Error::Payjoin("Expected a pending intent".to_string()));
        };
        let signed = SendBatch::from_payjoin(&self.storage, pending, &tx, vout, fee).await?;
        let txid = tx.compute_txid();
        let broadcast = signed
            .mark_broadcast(
                &self.storage,
                txid.to_string(),
                bdk_wallet::bitcoin::consensus::serialize(&tx),
                vec![BatchOutputAssignment {
                    intent_id: intent.intent_id,
                    vout,
                    fee_contribution_sat: fee,
                }],
                fee,
            )
            .await?;
        for intent in broadcast.intents {
            intent
                .mark_broadcast(
                    &self.storage,
                    txid.to_string(),
                    bdk_wallet::bitcoin::OutPoint::new(txid, vout).to_string(),
                    fee,
                )
                .await?;
        }
        {
            let mut wallet = self.wallet_with_db.lock().await;
            wallet
                .wallet
                .apply_unconfirmed_txs([(tx.clone(), crate::util::unix_now())]);
            wallet.persist()?;
        }
        if let Err(failure) = self.broadcast_transaction_internal(tx).await {
            self.log_broadcast_failure(
                "Payjoin broadcast failed",
                intent.intent_id,
                &txid.to_string(),
                &failure,
            );
        }
        Ok(())
    }
}

// Subtract the receiver's contribution once across all outputs to this quote.
// Matching spent outpoints also covers legacy sender transaction-ID changes.
pub(crate) fn net_received_amount(
    tx: &bdk_wallet::bitcoin::Transaction,
    vout: u32,
    address: &str,
    records: &[(String, persist::SessionRecord)],
) -> u64 {
    let output = &tx.output[vout as usize];
    let contribution = records
        .iter()
        .filter(|(_, r)| r.address.as_deref() == Some(address))
        .filter(|(_, r)| {
            tx.input
                .iter()
                .any(|i| r.inputs.contains(&i.previous_output))
        })
        .fold(0u64, |sum, (_, r)| sum.saturating_add(r.input_value_sat));
    let preceding = tx.output[..vout as usize]
        .iter()
        .filter(|o| o.script_pubkey == output.script_pubkey)
        .fold(0u64, |sum, o| sum.saturating_add(o.value.to_sat()));
    output
        .value
        .to_sat()
        .saturating_sub(contribution.saturating_sub(preceding))
}
