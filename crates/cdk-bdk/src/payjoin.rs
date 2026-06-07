//! Payjoin support for the BDK on-chain backend.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use bdk_wallet::bitcoin::{consensus, FeeRate, OutPoint, Script, Sequence, Transaction, TxIn};
use cdk_common::nuts::nut31::PayjoinV2;
use cdk_common::payjoin::{
    format_bip21_amount_from_sats, payjoin_v2_from_bip77_endpoint, payjoin_v2_to_bip77_endpoint,
};
use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::Error;
use crate::send::batch_transaction::record::{
    BatchOutputAssignment, SendBatchRecord, SendBatchState,
};
use crate::types::{PayjoinConfig, PaymentMetadata, PaymentTier};
use crate::util::parse_checked_address;
use crate::CdkBdk;

const PAYJOIN_RECEIVE_SESSION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const PAYJOIN_RECEIVER_MAX_EFFECTIVE_FEE_RATE: FeeRate = FeeRate::ZERO;
/// Minimum fee rate enforced on a sender's original PSBT during the
/// broadcast-suitability check. On backends without `testmempoolaccept` (Esplora)
/// this floor is the primary anti-probing protection; on Bitcoin Core it is an
/// additional constraint on top of the full mempool-acceptance check.
const PAYJOIN_RECEIVER_MIN_ORIGINAL_FEE_RATE: FeeRate = FeeRate::from_sat_per_vb_u32(1);

#[derive(Debug, Clone)]
struct RecordingSessionPersister<E> {
    events: Arc<StdMutex<Vec<E>>>,
    closed: Arc<AtomicBool>,
}

impl<E> RecordingSessionPersister<E>
where
    E: Clone,
{
    fn new(events: Vec<E>, closed: bool) -> Self {
        Self {
            events: Arc::new(StdMutex::new(events)),
            closed: Arc::new(AtomicBool::new(closed)),
        }
    }

    fn events(&self) -> Result<Vec<E>, Error> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn replace(&self, events: Vec<E>, closed: bool) -> Result<(), Error> {
        *self
            .events
            .lock()
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))? =
            events;
        self.closed.store(closed, Ordering::SeqCst);
        Ok(())
    }
}

impl<E> ::payjoin::persist::SessionPersister for RecordingSessionPersister<E>
where
    E: Clone + Send + Sync + 'static,
{
    type InternalStorageError = Error;
    type SessionEvent = E;

    fn save_event(&self, event: Self::SessionEvent) -> Result<(), Self::InternalStorageError> {
        self.events
            .lock()
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))?
            .push(event);

        Ok(())
    }

    fn load(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Self::SessionEvent>>, Self::InternalStorageError> {
        let events = self
            .events
            .lock()
            .map(|events| events.clone())
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))?;

        Ok(Box::new(events.into_iter()))
    }

    fn close(&self) -> Result<(), Self::InternalStorageError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Pre-exposure state for a Payjoin send: everything built and signed before
/// the original PSBT is shared with the receiver. The `Sender` itself is not
/// returned — it is saved into `persister`'s event log, from which the
/// background poller replays it; the persisted events plus the signed original
/// are all the poller needs to drive (and resume) the session.
struct PreparedPayjoinSend {
    /// The signed original transaction, broadcastable as the Payjoin fallback.
    original_tx: Transaction,
    original_fee_sat: u64,
    persister: RecordingSessionPersister<::payjoin::send::v2::SessionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PayjoinSendValidation {
    /// The single receiver-script output used for the melt payment proof.
    payment_outpoint: OutPoint,
    /// The mint wallet's net spend above the quoted receiver amount.
    fee_contribution_sat: u64,
}

impl CdkBdk {
    pub(crate) fn requested_payjoin(metadata: Option<&str>) -> Option<PayjoinV2> {
        let value = metadata
            .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok())?;
        value
            .get("payjoin")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
    }
    pub(crate) fn accepted_payjoin_extra(payjoin: &PayjoinV2) -> serde_json::Value {
        serde_json::json!({
            "payjoin": payjoin,
        })
    }
    pub(crate) async fn create_payjoin_receive_extra(
        &self,
        quote_id: &cdk_common::QuoteId,
        address: &bdk_wallet::bitcoin::Address,
        amount_sat: u64,
    ) -> Result<Option<serde_json::Value>, Error> {
        let Some(config) = self.payjoin_config() else {
            return Ok(None);
        };

        let ohttp_keys = fetch_ohttp_keys(config).await?;
        let persister = RecordingSessionPersister::new(Vec::new(), false);
        let mut receiver_builder = ::payjoin::receive::v2::ReceiverBuilder::new(
            address.clone(),
            config.directory_url.clone(),
            ohttp_keys.clone(),
        )
        .map_err(|err| Error::Payjoin(err.to_string()))?
        .with_expiration(Duration::from_secs(config.expiry_secs));
        if amount_sat > 0 {
            receiver_builder =
                receiver_builder.with_amount(bdk_wallet::bitcoin::Amount::from_sat(amount_sat));
        }
        let receiver = receiver_builder
            .build()
            .save(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let pj_uri = receiver.pj_uri().to_string();
        let endpoint = extract_bip21_payjoin_endpoint(&pj_uri)?;
        let payjoin = payjoin_v2_from_bip77_endpoint(&endpoint)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        self.storage
            .put_payjoin_receive_session(&crate::storage::PayjoinReceiveSessionRecord {
                quote_id: quote_id.to_string(),
                fallback_address: address.to_string(),
                amount_sat,
                expires_at: payjoin.expires_at,
                events: persister.events()?,
                closed: persister.closed(),
            })
            .await?;

        tracing::debug!(
            quote_id = %quote_id,
            fallback_address = %address,
            amount_sat,
            endpoint = %payjoin.endpoint,
            expires_at = payjoin.expires_at,
            "Created Payjoin receive session"
        );

        Ok(Some(serde_json::json!({ "payjoin": payjoin })))
    }

    pub(crate) async fn run_payjoin_receive_poller(
        &self,
        cancel_token: CancellationToken,
    ) -> Result<(), Error> {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        tracing::info!("Starting Payjoin receive poller");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = tick.tick() => {
                    let now = crate::util::unix_now();
                    let sessions = self.storage.get_all_payjoin_receive_sessions().await?;
                    let active_count = sessions
                        .iter()
                        .filter(|record| !record.closed && record.expires_at >= now)
                        .count();
                    tracing::debug!(
                        session_count = sessions.len(),
                        active_count,
                        "Polling Payjoin receive sessions"
                    );
                    for record in sessions {
                        if should_prune_payjoin_receive_session(&record, now) {
                            tracing::debug!(
                                quote_id = %record.quote_id,
                                expires_at = record.expires_at,
                                now,
                                "Pruning closed Payjoin receive session"
                            );
                            if let Err(err) = self.storage.delete_payjoin_receive_session(&record.quote_id).await {
                                tracing::warn!(
                                    quote_id = %record.quote_id,
                                    "Payjoin receive session pruning failed: {}",
                                    err
                                );
                            }
                            continue;
                        }
                        if record.closed {
                            tracing::trace!(
                                quote_id = %record.quote_id,
                                "Skipping closed Payjoin receive session"
                            );
                            continue;
                        }
                        if payjoin_receive_session_expired(&record, now) {
                            tracing::debug!(
                                quote_id = %record.quote_id,
                                expires_at = record.expires_at,
                                now,
                                "Closing expired Payjoin receive session"
                            );
                            if let Err(err) = self.close_payjoin_receive_session(record).await {
                                tracing::warn!(
                                    "Payjoin receive session close-on-expiry failed: {}",
                                    err
                                );
                            }
                            continue;
                        }
                        tracing::debug!(
                            quote_id = %record.quote_id,
                            fallback_address = %record.fallback_address,
                            event_count = record.events.len(),
                            "Processing Payjoin receive session"
                        );
                        if let Err(err) = self.process_payjoin_receive_session(record).await {
                            tracing::warn!("Payjoin receive session processing failed: {}", err);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn close_payjoin_receive_session(
        &self,
        mut record: crate::storage::PayjoinReceiveSessionRecord,
    ) -> Result<(), Error> {
        record.closed = true;
        self.storage.put_payjoin_receive_session(&record).await
    }

    async fn process_payjoin_receive_session(
        &self,
        mut record: crate::storage::PayjoinReceiveSessionRecord,
    ) -> Result<(), Error> {
        use ::payjoin::persist::OptionalTransitionOutcome;

        let Some(config) = self.payjoin_config() else {
            return Ok(());
        };
        let fallback_address =
            parse_checked_address(&record.fallback_address, self.network, Error::Payjoin)?;
        let fallback_script = fallback_address.script_pubkey();
        let persister = RecordingSessionPersister::new(record.events.clone(), record.closed);
        let (session, _history) = ::payjoin::receive::v2::replay_event_log(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        tracing::debug!(
            quote_id = %record.quote_id,
            state = payjoin_receive_session_state_name(&session),
            event_count = record.events.len(),
            "Replayed Payjoin receive session"
        );
        let mut closed = record.closed;

        let result: Result<(), Error> = async {
            let payjoin_proposal = match session {
                ::payjoin::receive::v2::ReceiveSession::Initialized(receiver) => {
                    tracing::debug!(
                        quote_id = %record.quote_id,
                        "Polling Payjoin directory for original PSBT"
                    );
                    let (request, context) = receiver
                        .create_poll_request(&config.ohttp_relay_url)
                        .map_err(|err| Error::Payjoin(err.to_string()))?;
                    let response = payjoin_http_request(request).await?;
                    match receiver
                        .process_response(&response, context)
                        .save(&persister)
                        .map_err(|err| Error::Payjoin(err.to_string()))?
                    {
                        OptionalTransitionOutcome::Progress(unchecked) => {
                            tracing::debug!(
                                quote_id = %record.quote_id,
                                "Received Payjoin original PSBT"
                            );
                            Some(
                                self.accept_payjoin_receive_proposal(
                                    unchecked,
                                    &fallback_script,
                                    &record.quote_id,
                                    &persister,
                                )
                                .await?,
                            )
                        }
                        OptionalTransitionOutcome::Stasis(_) => {
                            tracing::debug!(
                                quote_id = %record.quote_id,
                                "No Payjoin original PSBT available yet"
                            );
                            None
                        }
                    }
                }
                ::payjoin::receive::v2::ReceiveSession::UncheckedOriginalPayload(unchecked) => {
                    Some(
                        self.accept_payjoin_receive_proposal(
                            unchecked,
                            &fallback_script,
                            &record.quote_id,
                            &persister,
                        )
                        .await?,
                    )
                }
                ::payjoin::receive::v2::ReceiveSession::MaybeInputsOwned(receiver) => {
                    let receiver = self
                        .check_payjoin_inputs_not_owned(receiver, &persister)
                        .await?;
                    Some(
                        self.accept_payjoin_checked_inputs(
                            receiver,
                            &fallback_script,
                            &record.quote_id,
                            &persister,
                        )
                        .await?,
                    )
                }
                ::payjoin::receive::v2::ReceiveSession::MaybeInputsSeen(receiver) => Some(
                    self.accept_payjoin_checked_inputs(
                        receiver,
                        &fallback_script,
                        &record.quote_id,
                        &persister,
                    )
                    .await?,
                ),
                ::payjoin::receive::v2::ReceiveSession::OutputsUnknown(receiver) => {
                    let receiver = self.identify_payjoin_receiver_outputs(
                        receiver,
                        &fallback_script,
                        &record.quote_id,
                        &persister,
                    )?;
                    Some(
                        self.accept_payjoin_wants_outputs(receiver, &persister)
                            .await?,
                    )
                }
                ::payjoin::receive::v2::ReceiveSession::WantsOutputs(receiver) => Some(
                    self.accept_payjoin_wants_outputs(receiver, &persister)
                        .await?,
                ),
                ::payjoin::receive::v2::ReceiveSession::WantsInputs(receiver) => {
                    let receiver = self.contribute_payjoin_inputs(receiver, &persister).await?;
                    Some(self.finalize_payjoin_proposal(receiver, &persister).await?)
                }
                ::payjoin::receive::v2::ReceiveSession::WantsFeeRange(receiver) => {
                    let receiver = apply_zero_receiver_fee_range(receiver, &persister)?;
                    Some(self.finalize_payjoin_proposal(receiver, &persister).await?)
                }
                ::payjoin::receive::v2::ReceiveSession::ProvisionalProposal(receiver) => {
                    Some(self.finalize_payjoin_proposal(receiver, &persister).await?)
                }
                ::payjoin::receive::v2::ReceiveSession::PayjoinProposal(proposal) => Some(proposal),
                ::payjoin::receive::v2::ReceiveSession::HasReplyableError(receiver) => {
                    let (request, context) = receiver
                        .create_error_request(&config.ohttp_relay_url)
                        .map_err(|err| Error::Payjoin(err.to_string()))?;
                    let response = payjoin_http_request(request).await?;
                    receiver
                        .process_error_response(&response, context)
                        .save(&persister)
                        .map_err(|err| Error::Payjoin(err.to_string()))?;
                    closed = true;
                    None
                }
                ::payjoin::receive::v2::ReceiveSession::Closed(_) => {
                    closed = true;
                    None
                }
                _ => None,
            };

            if let Some(proposal) = payjoin_proposal {
                update_payjoin_receive_credit_cap(&mut record);
                if let Err(err) = ensure_payjoin_receiver_credit(
                    proposal.psbt(),
                    &fallback_script,
                    record.amount_sat,
                ) {
                    closed = true;
                    return Err(err);
                }
                tracing::debug!(
                    quote_id = %record.quote_id,
                    "Posting Payjoin proposal response"
                );
                self.persist_payjoin_receive_session_progress(&mut record, &persister, closed)
                    .await?;

                let (request, context) = proposal
                    .create_post_request(&config.ohttp_relay_url)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
                let response = payjoin_http_request(request).await?;
                proposal
                    .process_response(&response, context)
                    .save(&persister)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
            }

            Ok(())
        }
        .await;

        self.persist_payjoin_receive_session_progress(&mut record, &persister, closed)
            .await?;
        result
    }
    async fn persist_payjoin_receive_session_progress(
        &self,
        record: &mut crate::storage::PayjoinReceiveSessionRecord,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
        closed: bool,
    ) -> Result<(), Error> {
        record.events = persister.events()?;
        update_payjoin_receive_credit_cap(record);
        record.closed = closed || persister.closed();
        self.storage.put_payjoin_receive_session(record).await
    }
    async fn accept_payjoin_receive_proposal(
        &self,
        unchecked: ::payjoin::receive::v2::Receiver<
            ::payjoin::receive::v2::UncheckedOriginalPayload,
        >,
        fallback_script: &bdk_wallet::bitcoin::Script,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>, Error>
    {
        // The mint is a non-interactive receiver (auto-published URI per quote),
        // so validate the original is broadcastable before advancing — this is the
        // probing/poisoning defense (inputs are only recorded as seen afterwards).
        let chain_source = &self.chain_source;
        let can_broadcast =
            move |tx: &Transaction| -> Result<bool, ::payjoin::ImplementationError> {
                match chain_source
                    .accepts_broadcast(tx)
                    .map_err(::payjoin::ImplementationError::new)?
                {
                    // Bitcoin Core: trust the testmempoolaccept verdict.
                    Some(allowed) => Ok(allowed),
                    // Esplora (no dry-run): rely on the enforced minimum fee rate.
                    None => Ok(true),
                }
            };
        let receiver = unchecked
            .check_broadcast_suitability(
                Some(PAYJOIN_RECEIVER_MIN_ORIGINAL_FEE_RATE),
                can_broadcast,
            )
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let receiver = self
            .check_payjoin_inputs_not_owned(receiver, persister)
            .await?;

        self.accept_payjoin_checked_inputs(receiver, fallback_script, quote_id, persister)
            .await
    }
    async fn accept_payjoin_checked_inputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsSeen>,
        fallback_script: &bdk_wallet::bitcoin::Script,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>, Error>
    {
        let receiver = self
            .check_payjoin_inputs_not_seen(receiver, quote_id, persister)
            .await?;
        let receiver =
            self.identify_payjoin_receiver_outputs(receiver, fallback_script, quote_id, persister)?;

        self.accept_payjoin_wants_outputs(receiver, persister).await
    }
    async fn accept_payjoin_wants_outputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsOutputs>,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>, Error>
    {
        let receiver = receiver
            .commit_outputs()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = self.contribute_payjoin_inputs(receiver, persister).await?;

        self.finalize_payjoin_proposal(receiver, persister).await
    }
    async fn check_payjoin_inputs_not_owned(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsOwned>,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsSeen>, Error>
    {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let mut is_owned = |script: &bdk_wallet::bitcoin::Script| {
            Ok(wallet_with_db.wallet.is_mine(script.to_owned()))
        };
        let receiver = receiver
            .check_inputs_not_owned(&mut is_owned)
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        drop(wallet_with_db);

        Ok(receiver)
    }
    async fn check_payjoin_inputs_not_seen(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsSeen>,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::OutputsUnknown>, Error>
    {
        let original_input_outpoints =
            payjoin_original_input_outpoints_from_events(&persister.events()?)?;
        let mut seen_outpoints = HashSet::new();
        for outpoint in &original_input_outpoints {
            if self
                .storage
                .is_payjoin_input_seen(&outpoint.to_string())
                .await?
            {
                seen_outpoints.insert(*outpoint);
            }
        }
        tracing::debug!(
            quote_id,
            input_count = original_input_outpoints.len(),
            seen_input_count = seen_outpoints.len(),
            "Checked Payjoin original input replay index"
        );

        let mut is_known =
            |outpoint: &bdk_wallet::bitcoin::OutPoint| Ok(seen_outpoints.contains(outpoint));
        let staged_persister =
            RecordingSessionPersister::new(persister.events()?, persister.closed());
        let receiver = match receiver
            .check_no_inputs_seen_before(&mut is_known)
            .save(&staged_persister)
        {
            Ok(receiver) => receiver,
            Err(err) => {
                persister.replace(staged_persister.events()?, staged_persister.closed())?;
                return Err(Error::Payjoin(err.to_string()));
            }
        };

        let checked_outpoints = original_input_outpoints
            .into_iter()
            .map(|outpoint| outpoint.to_string())
            .collect::<Vec<_>>();
        self.storage
            .mark_payjoin_inputs_seen(&checked_outpoints)
            .await?;
        persister.replace(staged_persister.events()?, staged_persister.closed())?;

        Ok(receiver)
    }
    fn identify_payjoin_receiver_outputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::OutputsUnknown>,
        fallback_script: &bdk_wallet::bitcoin::Script,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsOutputs>, Error> {
        let mut is_receiver_output =
            |script: &bdk_wallet::bitcoin::Script| Ok(script == fallback_script);
        let receiver = receiver
            .identify_receiver_outputs(&mut is_receiver_output)
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let events = persister.events()?;
        if let Some(receiver_output_count) = payjoin_receiver_output_count_from_events(&events) {
            tracing::debug!(
                quote_id,
                receiver_output_count,
                "Identified Payjoin original PSBT receiver outputs"
            );
        }

        Ok(receiver)
    }
    async fn contribute_payjoin_inputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsInputs>,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::ProvisionalProposal>, Error>
    {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let candidate_inputs = wallet_with_db
            .wallet
            .list_unspent()
            .filter_map(|utxo| {
                let psbt_input = wallet_with_db
                    .wallet
                    .get_psbt_input(utxo.clone(), None, false)
                    .ok()?;
                ::payjoin::receive::InputPair::new(
                    TxIn {
                        previous_output: utxo.outpoint,
                        script_sig: Default::default(),
                        sequence: Sequence::MAX,
                        witness: Default::default(),
                    },
                    psbt_input,
                    None,
                )
                .ok()
            })
            .collect::<Vec<_>>();
        let selected = receiver
            .try_preserving_privacy(candidate_inputs.clone())
            .or_else(|_| {
                candidate_inputs.into_iter().next().ok_or_else(|| {
                    Error::Payjoin("no Payjoin contribution input available".to_string())
                })
            })?;
        let receiver = receiver
            .contribute_inputs([selected])
            .map_err(|err| Error::Payjoin(err.to_string()))?
            .commit_inputs()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = apply_zero_receiver_fee_range(receiver, persister)?;
        drop(wallet_with_db);

        Ok(receiver)
    }
    async fn finalize_payjoin_proposal(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::ProvisionalProposal>,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>, Error>
    {
        let wallet_with_db = self.wallet_with_db.lock().await;
        let receiver = receiver
            .finalize_proposal(|psbt| {
                let mut psbt = psbt.clone();
                wallet_with_db
                    .wallet
                    .sign(&mut psbt, Default::default())
                    .map_err(|err| -> ::payjoin::ImplementationError {
                        ::payjoin::ImplementationError::new(std::io::Error::other(err.to_string()))
                    })?;
                Ok(psbt)
            })
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        drop(wallet_with_db);

        Ok(receiver)
    }
    /// Start an optional Payjoin send for an onchain melt.
    ///
    /// This only *prepares* the send: it builds and signs the original PSBT and
    /// the Payjoin sender, reserves the original's inputs locally, persists the
    /// send session, and returns `Pending` immediately. The actual negotiation
    /// (post the original, poll for the proposal, broadcast the Payjoin tx or
    /// the original fallback) is driven asynchronously by
    /// [`Self::run_payjoin_send_poller`], so a slow or unresponsive receiver
    /// never blocks the melt and there is no artificial negotiation timeout.
    ///
    /// Any failure here happens *before* anything is shared with the receiver,
    /// so it is wrapped in [`Error::PayjoinSendNotStarted`] to tell the caller a
    /// direct onchain fallback is safe (no signed transaction has been exposed).
    pub(crate) async fn start_payjoin_send(
        &self,
        quote_id: &cdk_common::QuoteId,
        address: &str,
        amount_sat: u64,
        max_fee_sat: u64,
        tier: PaymentTier,
        payjoin: &PayjoinV2,
    ) -> Result<MakePaymentResponse, Error> {
        let prepared = self
            .prepare_payjoin_send(address, amount_sat, max_fee_sat, tier, payjoin)
            .await
            .map_err(|err| Error::PayjoinSendNotStarted(Box::new(err)))?;
        let PreparedPayjoinSend {
            original_tx,
            original_fee_sat,
            persister,
        } = prepared;

        // Reserve the original's inputs immediately so a concurrent melt/batch
        // cannot select the same coins while the (potentially long-lived)
        // negotiation runs. The poller evicts/replaces this tx if a Payjoin
        // proposal with different inputs arrives.
        let original_txid = original_tx.compute_txid();
        let reservation_result = {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            wallet_with_db
                .wallet
                .apply_unconfirmed_txs([(original_tx.clone(), crate::util::unix_now())]);
            wallet_with_db.persist().map_err(Error::Database)
        };
        if let Err(err) = reservation_result {
            if let Err(evict_err) = self.evict_unstaged_payjoin_tx(original_txid).await {
                return Err(Error::PayjoinSendNotStarted(Box::new(Error::Payjoin(
                    format!(
                        "Could not persist reservation of original Payjoin tx {}: {}; \
                         additionally could not persist eviction of the in-memory reservation: {}",
                        original_txid, err, evict_err
                    ),
                ))));
            }
            return Err(Error::PayjoinSendNotStarted(Box::new(err)));
        }

        // Persist the full send session so the background poller can drive and
        // resume it across restarts.
        let record = crate::storage::PayjoinSendSessionRecord {
            quote_id: quote_id.to_string(),
            fallback_address: address.to_string(),
            amount_sat,
            max_fee_sat,
            tier,
            original_tx_bytes: consensus::serialize(&original_tx),
            original_fee_sat,
            events: persister.events()?,
            closed: false,
        };
        if let Err(err) = self.storage.put_payjoin_send_session(&record).await {
            // Could not persist the session. Nothing was shared with the
            // receiver, so undo the local reservation and report a recoverable
            // error: the caller falls back to a direct onchain send.
            if let Err(evict_err) = self.evict_unstaged_payjoin_tx(original_txid).await {
                return Err(Error::PayjoinSendNotStarted(Box::new(Error::Payjoin(
                    format!(
                        "Could not persist Payjoin send session: {}; additionally could not \
                         persist eviction of reserved original tx {}: {}",
                        err, original_txid, evict_err
                    ),
                ))));
            }
            return Err(Error::PayjoinSendNotStarted(Box::new(err)));
        }

        tracing::debug!(
            quote_id = %quote_id,
            original_txid = %original_txid,
            "Started Payjoin send session; negotiation runs in the background poller"
        );

        // Return Pending immediately. Until the poller stages a send intent,
        // `check_outgoing_payment` reports `Unknown` ("keep polling"), matching
        // the queued direct-send convention; `total_spent` is the 0 sentinel.
        Ok(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: Amount::new(0, CurrencyUnit::Sat),
        })
    }

    /// Build and sign the original PSBT and the Payjoin sender, saving the
    /// sender into a fresh persister's event log. Nothing is shared with the
    /// receiver here.
    async fn prepare_payjoin_send(
        &self,
        address: &str,
        amount_sat: u64,
        max_fee_sat: u64,
        tier: PaymentTier,
        payjoin: &PayjoinV2,
    ) -> Result<PreparedPayjoinSend, Error> {
        use ::payjoin::UriExt;

        if self.payjoin_config().is_none() {
            return Err(Error::Payjoin(
                "operator did not configure Payjoin directory and OHTTP relay".to_string(),
            ));
        }

        let fallback_address = parse_checked_address(address, self.network, Error::Wallet)?;
        let sat_per_vb = self
            .estimate_fee_rate_sat_per_vb(tier)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tier = ?tier,
                    error = %e,
                    "Payjoin fee-rate estimation failed, using configured fallback"
                );
                self.batch_config.fee_estimation.fallback_sat_per_vb
            });
        let fee_rate = bdk_wallet::bitcoin::FeeRate::from_sat_per_vb_u32(sat_per_vb.ceil() as u32);

        let (original_psbt, original_fee_sat, original_tx) = {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            let mut tx_builder = wallet_with_db.wallet.build_tx();
            tx_builder.add_recipient(
                fallback_address.clone(),
                bdk_wallet::bitcoin::Amount::from_sat(amount_sat),
            );
            tx_builder.fee_rate(fee_rate);
            let mut original_psbt = tx_builder
                .finish()
                .map_err(|err| Error::Payjoin(format!("Could not build original PSBT: {}", err)))?;
            let original_fee_sat = original_psbt
                .fee()
                .map_err(|err| {
                    Error::Payjoin(format!("Could not calculate original PSBT fee: {}", err))
                })?
                .to_sat();
            if original_fee_sat > max_fee_sat {
                return Err(Error::Payjoin(format!(
                    "original Payjoin PSBT fee {} exceeds max fee {}",
                    original_fee_sat, max_fee_sat
                )));
            }
            if !wallet_with_db
                .wallet
                .sign(&mut original_psbt, Default::default())
                .map_err(|err| Error::Payjoin(format!("Could not sign original PSBT: {}", err)))?
            {
                return Err(Error::CouldNotSign);
            }
            wallet_with_db
                .persist()
                .map_err(|err| Error::Payjoin(format!("Could not persist wallet: {}", err)))?;
            // Capture the broadcastable fallback transaction before the PSBT is
            // consumed by the sender builder. This is the same signed original
            // we share with the receiver, so broadcasting it later can never
            // conflict with a Payjoin proposal derived from it.
            let original_tx = original_psbt.clone().extract_tx().map_err(|err| {
                Error::Payjoin(format!("Could not extract original Payjoin tx: {}", err))
            })?;
            (original_psbt, original_fee_sat, original_tx)
        };

        let pj_uri = build_payjoin_uri(address, amount_sat, payjoin)?;
        let pj_uri = ::payjoin::Uri::try_from(pj_uri.as_str())
            .map_err(|err| Error::Payjoin(format!("Invalid Payjoin URI: {}", err)))?
            .assume_checked()
            .check_pj_supported()
            .map_err(|_| {
                Error::Payjoin("Payjoin URI did not contain supported pj params".to_string())
            })?;
        let persister = RecordingSessionPersister::new(Vec::new(), false);
        // Save the sender into the event log; the poller replays it from there.
        let _sender = ::payjoin::send::v2::SenderBuilder::new(original_psbt, pj_uri)
            .build_recommended(fee_rate)
            .map_err(|err| Error::Payjoin(err.to_string()))?
            .save(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        Ok(PreparedPayjoinSend {
            original_tx,
            original_fee_sat,
            persister,
        })
    }

    /// Background poller that drives every open Payjoin send session to
    /// completion, mirroring [`Self::run_payjoin_receive_poller`]. It posts the
    /// original PSBT, polls for the proposal, and broadcasts either the Payjoin
    /// transaction or the signed original fallback. Because it lists persisted
    /// sessions each tick, it transparently resumes in-flight sends after a
    /// restart.
    pub(crate) async fn run_payjoin_send_poller(
        &self,
        cancel_token: CancellationToken,
    ) -> Result<(), Error> {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        tracing::info!("Starting Payjoin send poller");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = tick.tick() => {
                    let sessions = self.storage.get_all_payjoin_send_sessions().await?;
                    let active_count = sessions.iter().filter(|record| !record.closed).count();
                    tracing::debug!(
                        session_count = sessions.len(),
                        active_count,
                        "Polling Payjoin send sessions"
                    );
                    for record in sessions {
                        if record.closed {
                            continue;
                        }
                        if let Err(err) = self.process_payjoin_send_session(record).await {
                            tracing::warn!("Payjoin send session processing failed: {}", err);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn process_payjoin_send_session(
        &self,
        mut record: crate::storage::PayjoinSendSessionRecord,
    ) -> Result<(), Error> {
        use ::payjoin::persist::OptionalTransitionOutcome;
        use ::payjoin::send::v2::{SendSession, SessionOutcome};

        let Some(config) = self.payjoin_config() else {
            return Ok(());
        };

        // Idempotency: if this quote already has a staged or finalized send, the
        // session is done. Close it so we stop polling.
        if self
            .storage
            .get_send_intent_by_quote_id(&record.quote_id)
            .await?
            .is_some()
            || self
                .storage
                .get_finalized_intent_by_quote_id(&record.quote_id)
                .await?
                .is_some()
        {
            if !record.closed {
                record.closed = true;
                self.storage.put_payjoin_send_session(&record).await?;
            }
            return Ok(());
        }

        let persister = RecordingSessionPersister::new(record.events.clone(), record.closed);
        let session = match ::payjoin::send::v2::replay_event_log(&persister) {
            Ok((session, _history)) => session,
            Err(err) => {
                // The session can no longer be replayed (most commonly because
                // the Payjoin parameters have expired). Broadcast the signed
                // original fallback so the melt still settles.
                tracing::debug!(
                    quote_id = %record.quote_id,
                    error = %err,
                    "Payjoin send session not replayable (expired?); broadcasting original fallback"
                );
                return self.broadcast_payjoin_send_fallback(&mut record).await;
            }
        };

        match session {
            SendSession::WithReplyKey(sender) => {
                tracing::debug!(
                    quote_id = %record.quote_id,
                    "Posting original PSBT to Payjoin directory"
                );
                let (request, context) = sender
                    .create_v2_post_request(&config.ohttp_relay_url)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
                let response = payjoin_http_request(request).await?;
                sender
                    .process_response(&response, context)
                    .save(&persister)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
                self.persist_payjoin_send_progress(&mut record, &persister)
                    .await?;
            }
            SendSession::PollingForProposal(sender) => {
                let (request, context) = sender
                    .create_poll_request(&config.ohttp_relay_url)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
                let response = payjoin_http_request(request).await?;
                match sender
                    .process_response(&response, context)
                    .save(&persister)
                    .map_err(|err| Error::Payjoin(err.to_string()))?
                {
                    OptionalTransitionOutcome::Progress(proposal_psbt) => {
                        tracing::debug!(
                            quote_id = %record.quote_id,
                            "Received Payjoin proposal PSBT"
                        );
                        self.persist_payjoin_send_progress(&mut record, &persister)
                            .await?;
                        self.finalize_and_stage_payjoin_send(&mut record, proposal_psbt)
                            .await?;
                    }
                    OptionalTransitionOutcome::Stasis(_) => {
                        tracing::debug!(
                            quote_id = %record.quote_id,
                            "No Payjoin proposal available yet"
                        );
                        self.persist_payjoin_send_progress(&mut record, &persister)
                            .await?;
                    }
                }
            }
            SendSession::Closed(outcome) => match outcome {
                SessionOutcome::Success(proposal_psbt) => {
                    // Crash/resume: the proposal was received before staging.
                    self.finalize_and_stage_payjoin_send(&mut record, proposal_psbt)
                        .await?;
                }
                SessionOutcome::Failure | SessionOutcome::Cancel => {
                    tracing::debug!(
                        quote_id = %record.quote_id,
                        "Payjoin send session closed without success; broadcasting original fallback"
                    );
                    self.broadcast_payjoin_send_fallback(&mut record).await?;
                }
            },
        }

        Ok(())
    }

    /// Sign the Payjoin proposal, evict the locally-reserved original, then
    /// stage and broadcast the Payjoin transaction. If the proposal would make
    /// the mint spend more than the quote amount plus max fee, broadcast the
    /// original fallback instead (it is already within budget).
    async fn finalize_and_stage_payjoin_send(
        &self,
        record: &mut crate::storage::PayjoinSendSessionRecord,
        proposal_psbt: bdk_wallet::bitcoin::Psbt,
    ) -> Result<(), Error> {
        let fallback_address =
            parse_checked_address(&record.fallback_address, self.network, Error::Payjoin)?;

        let mut final_psbt = proposal_psbt;
        let (tx, validation) = {
            let wallet_with_db = self.wallet_with_db.lock().await;
            if !wallet_with_db
                .wallet
                .sign(&mut final_psbt, Default::default())
                .map_err(|err| Error::Payjoin(format!("Could not sign Payjoin PSBT: {}", err)))?
            {
                return Err(Error::CouldNotSign);
            }
            let tx = final_psbt
                .extract_tx()
                .map_err(|err| Error::Payjoin(format!("Could not extract Payjoin tx: {}", err)))?;
            let (sent, received) = wallet_with_db.wallet.sent_and_received(&tx);
            let validation = validate_payjoin_send_transaction(
                &tx,
                fallback_address.script_pubkey().as_script(),
                record.amount_sat,
                record.max_fee_sat,
                sent.to_sat(),
                received.to_sat(),
            );
            (tx, validation)
        };
        let validation = match validation {
            Ok(validation) => validation,
            Err(err) => {
                tracing::warn!(
                    quote_id = %record.quote_id,
                    error = %err,
                    "Payjoin proposal exceeds local spend limits or altered the payment output; \
                     broadcasting original fallback instead"
                );
                return self.broadcast_payjoin_send_fallback(record).await;
            }
        };

        // The Payjoin tx spends the original's inputs plus the receiver's, so
        // evict the locally-reserved original before applying the Payjoin tx to
        // avoid a conflicting double-application in the wallet graph.
        if let Ok(original_tx) = consensus::deserialize::<Transaction>(&record.original_tx_bytes) {
            let original_txid = original_tx.compute_txid();
            if original_txid != tx.compute_txid() {
                self.evict_unstaged_payjoin_tx(original_txid).await?;
            }
        }

        self.stage_and_broadcast_payjoin_send(
            &record.quote_id,
            &record.fallback_address,
            record.amount_sat,
            record.max_fee_sat,
            record.tier,
            tx,
            validation,
        )
        .await?;

        record.closed = true;
        self.storage.put_payjoin_send_session(record).await?;
        Ok(())
    }

    /// Stage and broadcast the signed original transaction as the Payjoin
    /// fallback, then close the session.
    async fn broadcast_payjoin_send_fallback(
        &self,
        record: &mut crate::storage::PayjoinSendSessionRecord,
    ) -> Result<(), Error> {
        let original_tx = consensus::deserialize::<Transaction>(&record.original_tx_bytes)
            .map_err(|err| {
                Error::Payjoin(format!(
                    "Could not deserialize original Payjoin tx: {}",
                    err
                ))
            })?;
        let fallback_address =
            parse_checked_address(&record.fallback_address, self.network, Error::Payjoin)?;
        let validation = PayjoinSendValidation {
            payment_outpoint: require_payjoin_send_payment_output(
                &original_tx,
                fallback_address.script_pubkey().as_script(),
                record.amount_sat,
            )?,
            fee_contribution_sat: record.original_fee_sat,
        };

        self.stage_and_broadcast_payjoin_send(
            &record.quote_id,
            &record.fallback_address,
            record.amount_sat,
            record.max_fee_sat,
            record.tier,
            original_tx,
            validation,
        )
        .await?;

        record.closed = true;
        self.storage.put_payjoin_send_session(record).await?;
        Ok(())
    }

    /// Durably stage and broadcast a chosen Payjoin send transaction (either the
    /// Payjoin proposal or the original fallback), creating a send intent keyed
    /// by `quote_id` so `check_outgoing_payment` can track it.
    #[allow(clippy::too_many_arguments)]
    async fn stage_and_broadcast_payjoin_send(
        &self,
        quote_id: &str,
        address: &str,
        amount_sat: u64,
        max_fee_sat: u64,
        tier: PaymentTier,
        tx: Transaction,
        validation: PayjoinSendValidation,
    ) -> Result<(), Error> {
        let txid = tx.compute_txid();
        let outpoint = validation.payment_outpoint;
        let fee_contribution_sat = validation.fee_contribution_sat;
        {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            wallet_with_db
                .wallet
                .apply_unconfirmed_txs([(tx.clone(), crate::util::unix_now())]);
            wallet_with_db.persist().map_err(Error::Database)?;
        }

        let pending = crate::send::payment_intent::SendIntent::new(
            &self.storage,
            quote_id.to_string(),
            address.to_string(),
            amount_sat,
            max_fee_sat,
            tier,
            PaymentMetadata::default(),
        )
        .await?;
        let batch_id = Uuid::new_v4();
        let pending_for_failure = pending.clone();
        let batched = match pending.assign_to_batch(&self.storage, batch_id).await {
            Ok(batched) => batched,
            Err(err) => {
                if let Err(evict_err) = self.evict_unstaged_payjoin_tx(txid).await {
                    return Err(Error::Payjoin(format!(
                        "Payjoin staging failed before batch assignment: {}; additionally could \
                         not persist eviction of unstaged tx {}: {}",
                        err, txid, evict_err
                    )));
                }
                if let Err(fail_err) = pending_for_failure
                    .fail(
                        &self.storage,
                        format!("Payjoin staging failed before batch assignment: {}", err),
                    )
                    .await
                {
                    tracing::warn!(
                        quote_id,
                        error = %fail_err,
                        "Could not mark Payjoin send intent failed after assignment failure"
                    );
                }
                return Err(err);
            }
        };
        let tx_bytes = consensus::serialize(&tx);
        let assignment = BatchOutputAssignment {
            intent_id: batched.intent_id,
            vout: outpoint.vout,
            fee_contribution_sat,
        };
        if let Err(err) = self
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: vec![assignment.clone()],
                    fee_sat: fee_contribution_sat,
                },
            })
            .await
        {
            let reason = format!("Payjoin staging failed before broadcast: {}", err);
            if let Err(evict_err) = self.evict_unstaged_payjoin_tx(txid).await {
                return Err(Error::Payjoin(format!(
                    "{}; additionally could not persist eviction of unstaged tx {}: {}",
                    reason, txid, evict_err
                )));
            }
            match batched.revert_to_pending(&self.storage).await {
                Ok(pending) => {
                    if let Err(fail_err) = pending.fail(&self.storage, reason.clone()).await {
                        tracing::warn!(
                            quote_id,
                            error = %fail_err,
                            "Could not mark Payjoin send intent failed after staging failure"
                        );
                    }
                }
                Err(revert_err) => {
                    tracing::warn!(
                        quote_id,
                        error = %revert_err,
                        "Could not revert Payjoin send intent after staging failure"
                    );
                }
            }
            return Err(err);
        }
        if let Err(err) = self
            .storage
            .update_send_batch(
                &batch_id,
                &SendBatchState::Broadcast {
                    txid: txid.to_string(),
                    tx_bytes,
                    assignments: vec![assignment],
                    fee_sat: fee_contribution_sat,
                },
            )
            .await
        {
            tracing::warn!(
                quote_id,
                batch_id = %batch_id,
                error = %err,
                "Payjoin signed batch is durable but could not be marked broadcast"
            );
            return Ok(());
        }
        if let Err(err) = batched
            .mark_broadcast(
                &self.storage,
                txid.to_string(),
                outpoint.to_string(),
                fee_contribution_sat,
            )
            .await
        {
            tracing::warn!(
                quote_id,
                batch_id = %batch_id,
                txid = %txid,
                error = %err,
                "Payjoin batch is durable but send intent could not be marked awaiting confirmation"
            );
            return Ok(());
        }

        match self.broadcast_transaction_internal(tx.clone()).await {
            Ok(crate::chain::BroadcastOutcome::Accepted)
            | Ok(crate::chain::BroadcastOutcome::AlreadyKnown) => {}
            Err(failure) => {
                tracing::warn!(
                    quote_id,
                    txid = %txid,
                    error = %failure.message,
                    "Payjoin transaction is durably staged but broadcast failed"
                );
            }
        }

        Ok(())
    }

    /// Persist the current Payjoin send session event log without changing the
    /// poller's terminal `closed` flag (which is set only after the resulting
    /// transaction is staged).
    async fn persist_payjoin_send_progress(
        &self,
        record: &mut crate::storage::PayjoinSendSessionRecord,
        persister: &RecordingSessionPersister<::payjoin::send::v2::SessionEvent>,
    ) -> Result<(), Error> {
        record.events = persister.events()?;
        self.storage.put_payjoin_send_session(record).await
    }

    async fn evict_unstaged_payjoin_tx(
        &self,
        txid: bdk_wallet::bitcoin::Txid,
    ) -> Result<(), Error> {
        let evict_time = crate::util::unix_now().saturating_add(1);
        let mut wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db
            .wallet
            .apply_evicted_txs([(txid, evict_time)]);
        wallet_with_db.persist().map_err(Error::Database)?;
        Ok(())
    }

    pub(crate) fn payjoin_config(&self) -> Option<&PayjoinConfig> {
        self.payjoin_config.as_ref()
    }
}

async fn fetch_ohttp_keys(config: &PayjoinConfig) -> Result<::payjoin::OhttpKeys, Error> {
    #[cfg(feature = "payjoin-local-https")]
    {
        if let Some(cert_der) = config.local_tls_cert_der.clone() {
            return ::payjoin::io::fetch_ohttp_keys_with_cert(
                &config.ohttp_relay_url,
                &config.directory_url,
                &cert_der,
            )
            .await
            .map_err(|err| Error::Payjoin(err.to_string()));
        }
    }

    ::payjoin::io::fetch_ohttp_keys(&config.ohttp_relay_url, &config.directory_url)
        .await
        .map_err(|err| Error::Payjoin(err.to_string()))
}

fn extract_bip21_payjoin_endpoint(uri: &str) -> Result<String, Error> {
    let query = uri.split_once('?').map(|(_, query)| query).ok_or_else(|| {
        Error::Payjoin("Payjoin URI did not include query parameters".to_string())
    })?;

    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key == "pj" {
            return Ok(value.into_owned());
        }
    }

    Err(Error::Payjoin(
        "Payjoin URI did not include a pj endpoint".to_string(),
    ))
}

fn payjoin_receive_session_state_name(
    session: &::payjoin::receive::v2::ReceiveSession,
) -> &'static str {
    match session {
        ::payjoin::receive::v2::ReceiveSession::Initialized(_) => "initialized",
        ::payjoin::receive::v2::ReceiveSession::UncheckedOriginalPayload(_) => {
            "unchecked_original_payload"
        }
        ::payjoin::receive::v2::ReceiveSession::MaybeInputsOwned(_) => "maybe_inputs_owned",
        ::payjoin::receive::v2::ReceiveSession::MaybeInputsSeen(_) => "maybe_inputs_seen",
        ::payjoin::receive::v2::ReceiveSession::OutputsUnknown(_) => "outputs_unknown",
        ::payjoin::receive::v2::ReceiveSession::WantsOutputs(_) => "wants_outputs",
        ::payjoin::receive::v2::ReceiveSession::WantsInputs(_) => "wants_inputs",
        ::payjoin::receive::v2::ReceiveSession::WantsFeeRange(_) => "wants_fee_range",
        ::payjoin::receive::v2::ReceiveSession::ProvisionalProposal(_) => "provisional_proposal",
        ::payjoin::receive::v2::ReceiveSession::PayjoinProposal(_) => "payjoin_proposal",
        ::payjoin::receive::v2::ReceiveSession::HasReplyableError(_) => "has_replyable_error",
        ::payjoin::receive::v2::ReceiveSession::Closed(_) => "closed",
        _ => "unknown",
    }
}

fn payjoin_receive_session_expired(
    record: &crate::storage::PayjoinReceiveSessionRecord,
    now: u64,
) -> bool {
    record.expires_at < now
}

fn should_prune_payjoin_receive_session(
    record: &crate::storage::PayjoinReceiveSessionRecord,
    now: u64,
) -> bool {
    record.closed
        && record
            .expires_at
            .saturating_add(PAYJOIN_RECEIVE_SESSION_RETENTION_SECS)
            < now
}

fn build_payjoin_uri(address: &str, amount_sat: u64, payjoin: &PayjoinV2) -> Result<String, Error> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("amount", &format_bip21_amount_from_sats(amount_sat));
    serializer.append_pair("pj", &build_payjoin_endpoint(payjoin)?);
    Ok(format!("bitcoin:{}?{}", address, serializer.finish()))
}

fn build_payjoin_endpoint(payjoin: &PayjoinV2) -> Result<String, Error> {
    // The payjoin sender expects a BIP21/BIP77 `pj` URI. Cashu uses Unix
    // timestamp; BIP77 URI fragments use encoded `EX1`, so rebuild it only at
    // this library boundary.
    payjoin_v2_to_bip77_endpoint(payjoin).map_err(|err| Error::Payjoin(err.to_string()))
}

fn update_payjoin_receive_credit_cap(record: &mut crate::storage::PayjoinReceiveSessionRecord) {
    if let Some(amount_sat) = payjoin_original_receiver_output_amount_from_events(&record.events) {
        if record.amount_sat != amount_sat {
            tracing::debug!(
                quote_id = %record.quote_id,
                fallback_address = %record.fallback_address,
                previous_amount_sat = record.amount_sat,
                credit_cap_amount_sat = amount_sat,
                "Updated Payjoin receive credit cap from original PSBT receiver outputs"
            );
        }
        record.amount_sat = amount_sat;
    }
}

fn zero_receiver_fee_range() -> (Option<FeeRate>, Option<FeeRate>) {
    (None, Some(PAYJOIN_RECEIVER_MAX_EFFECTIVE_FEE_RATE))
}

fn apply_zero_receiver_fee_range(
    receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsFeeRange>,
    persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::ProvisionalProposal>, Error> {
    let (min_fee_rate, max_effective_fee_rate) = zero_receiver_fee_range();
    receiver
        .apply_fee_range(min_fee_rate, max_effective_fee_rate)
        .save(persister)
        .map_err(|err| Error::Payjoin(err.to_string()))
}

fn ensure_payjoin_receiver_credit(
    psbt: &bdk_wallet::bitcoin::Psbt,
    fallback_script: &Script,
    minimum_amount_sat: u64,
) -> Result<(), Error> {
    let credited_amount_sat = payjoin_receiver_output_amount(psbt, fallback_script)?;
    if credited_amount_sat < minimum_amount_sat {
        return Err(Error::Payjoin(format!(
            "Payjoin proposal receiver output amount {} is below original amount {}",
            credited_amount_sat, minimum_amount_sat
        )));
    }

    Ok(())
}

fn payjoin_receiver_output_amount(
    psbt: &bdk_wallet::bitcoin::Psbt,
    fallback_script: &Script,
) -> Result<u64, Error> {
    psbt.unsigned_tx
        .output
        .iter()
        .filter(|output| output.script_pubkey.as_script() == fallback_script)
        .try_fold(0_u64, |amount_sat, output| {
            amount_sat
                .checked_add(output.value.to_sat())
                .ok_or_else(|| {
                    Error::Payjoin("Payjoin receiver output amount overflow".to_string())
                })
        })
}

fn payjoin_receiver_output_count_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Option<usize> {
    events.iter().rev().find_map(|event| match event {
        ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vouts) => Some(vouts.len()),
        _ => None,
    })
}

fn payjoin_original_input_outpoints_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Result<Vec<OutPoint>, Error> {
    let original = events.iter().rev().find_map(|event| match event {
        ::payjoin::receive::v2::SessionEvent::RetrievedOriginalPayload { original, .. } => {
            Some(original)
        }
        _ => None,
    });
    let Some(original) = original else {
        return Err(Error::Payjoin(
            "Payjoin original payload event missing".to_string(),
        ));
    };

    let mut outpoints = Vec::new();
    let mut collect_outpoint = |outpoint: &OutPoint| {
        outpoints.push(*outpoint);
        Ok(false)
    };
    original
        .check_no_inputs_seen_before(&mut collect_outpoint)
        .map_err(|err| Error::Payjoin(err.to_string()))?;

    Ok(outpoints)
}

fn payjoin_original_receiver_output_amount_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Option<u64> {
    let mut receiver_vouts = None;
    let mut committed_outputs = None;

    for event in events {
        match event {
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vouts) => {
                receiver_vouts = Some(vouts.as_slice());
            }
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(outputs) => {
                committed_outputs = Some(outputs.as_slice());
            }
            _ => {}
        }
    }

    let receiver_vouts = receiver_vouts?;
    let committed_outputs = committed_outputs?;

    receiver_vouts.iter().try_fold(0_u64, |amount_sat, vout| {
        let output = committed_outputs.get(*vout)?;
        amount_sat.checked_add(output.value.to_sat())
    })
}

async fn payjoin_http_request(request: ::payjoin::Request) -> Result<Vec<u8>, Error> {
    let response = reqwest::Client::new()
        .post(request.url)
        .header(reqwest::header::CONTENT_TYPE, request.content_type)
        .body(request.body)
        .send()
        .await
        .map_err(|err| Error::Payjoin(err.to_string()))?;
    if !response.status().is_success() {
        return Err(Error::Payjoin(format!(
            "Payjoin HTTP request failed with status {}",
            response.status()
        )));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::Payjoin(err.to_string()))
}

fn find_payment_outpoint(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
) -> Option<OutPoint> {
    // The payment proof records one outpoint, so require one receiver-script
    // output to cover the full quote. A proposal that only pays the quote via
    // multiple smaller outputs is valid-looking value-wise but not representable
    // by the current proof model.
    tx.output
        .iter()
        .enumerate()
        .find(|(_, output)| {
            output.script_pubkey.as_script() == payment_script
                && output.value.to_sat() >= amount_sat
        })
        .map(|(vout, _)| OutPoint::new(tx.compute_txid(), vout as u32))
}

fn require_payjoin_send_payment_output(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
) -> Result<OutPoint, Error> {
    find_payment_outpoint(tx, payment_script, amount_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction missing payment output for {} sats",
            amount_sat
        ))
    })
}

/// Validate a signed Payjoin send by local wallet accounting.
///
/// A receiver may contribute inputs and increase the receiver output, so the
/// proposal's total transaction fee is not the mint's fee contribution. The
/// relevant budget is the mint wallet's net spend (`sent - received`), which
/// must stay within `amount_sat + max_fee_sat`. The recorded fee contribution is
/// therefore `mint_net_spend_sat - amount_sat`.
fn validate_payjoin_send_transaction(
    tx: &Transaction,
    payment_script: &Script,
    amount_sat: u64,
    max_fee_sat: u64,
    sent_sat: u64,
    received_sat: u64,
) -> Result<PayjoinSendValidation, Error> {
    let payment_outpoint = require_payjoin_send_payment_output(tx, payment_script, amount_sat)?;
    let mint_net_spend_sat = sent_sat.checked_sub(received_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction wallet receive amount {} exceeds sent amount {}",
            received_sat, sent_sat
        ))
    })?;
    let max_net_spend_sat = amount_sat.checked_add(max_fee_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin spend cap overflow for amount {} and max fee {}",
            amount_sat, max_fee_sat
        ))
    })?;
    if mint_net_spend_sat > max_net_spend_sat {
        return Err(Error::Payjoin(format!(
            "Payjoin transaction spends {} sats from mint wallet, exceeding cap {}",
            mint_net_spend_sat, max_net_spend_sat
        )));
    }
    let fee_contribution_sat = mint_net_spend_sat.checked_sub(amount_sat).ok_or_else(|| {
        Error::Payjoin(format!(
            "Payjoin transaction mint net spend {} is below payment amount {}",
            mint_net_spend_sat, amount_sat
        ))
    })?;

    Ok(PayjoinSendValidation {
        payment_outpoint,
        fee_contribution_sat,
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::{transaction, Amount as BitcoinAmount, Psbt, ScriptBuf, TxOut, Txid};

    use super::*;

    fn test_psbt_with_outputs(outputs: Vec<TxOut>) -> Psbt {
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(
                    Txid::from_str(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    )
                    .expect("valid txid"),
                    0,
                ),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            }],
            output: outputs,
        };
        Psbt::from_unsigned_tx(tx).expect("valid test psbt")
    }

    #[test]
    fn amountless_payjoin_receive_session_cap_comes_from_original_receiver_outputs() {
        let events = vec![
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![1]),
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
                TxOut {
                    value: BitcoinAmount::from_sat(8_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(3_000),
                    script_pubkey: ScriptBuf::new(),
                },
            ]),
        ];
        let mut record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 0,
            expires_at: 1_700_000_000,
            events,
            closed: false,
        };

        update_payjoin_receive_credit_cap(&mut record);

        assert_eq!(record.amount_sat, 3_000);
    }

    #[test]
    fn payjoin_receive_fee_range_keeps_sender_min_and_zeroes_receiver_fee_cap() {
        let (min_fee_rate, max_effective_fee_rate) = zero_receiver_fee_range();

        assert_eq!(min_fee_rate, None);
        assert_eq!(max_effective_fee_rate, Some(FeeRate::ZERO));
    }

    #[test]
    fn payjoin_receiver_credit_sums_final_receiver_outputs() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(2_000),
                script_pubkey: fallback_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(9_000),
                script_pubkey: other_script,
            },
            TxOut {
                value: BitcoinAmount::from_sat(3_000),
                script_pubkey: fallback_script.clone(),
            },
        ]);

        assert_eq!(
            payjoin_receiver_output_amount(&psbt, &fallback_script).expect("sum outputs"),
            5_000
        );
    }

    #[test]
    fn payjoin_receiver_credit_accepts_unreduced_receiver_output() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(5_000),
            script_pubkey: fallback_script.clone(),
        }]);

        ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
            .expect("sender-funded payjoin keeps receiver output whole");
    }

    #[test]
    fn payjoin_receiver_credit_rejects_reduced_receiver_output() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(4_999),
            script_pubkey: fallback_script.clone(),
        }]);

        let err = ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
            .expect_err("receiver output below original amount must be rejected");

        assert!(err.to_string().contains("below original amount"));
    }

    #[test]
    fn payjoin_send_payment_output_accepts_exact_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(9_000),
                script_pubkey: other_script,
            },
            TxOut {
                value: BitcoinAmount::from_sat(10_000),
                script_pubkey: payment_script.clone(),
            },
        ]);

        let outpoint =
            require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
                .expect("payment output is present");

        assert_eq!(outpoint.vout, 1);
    }

    #[test]
    fn payjoin_send_payment_output_accepts_larger_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(12_000),
            script_pubkey: payment_script.clone(),
        }]);

        let outpoint =
            require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
                .expect("larger payment output is present");

        assert_eq!(outpoint.vout, 0);
    }

    #[test]
    fn payjoin_send_payment_output_rejects_smaller_single_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(9_999),
                script_pubkey: payment_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(10_000),
                script_pubkey: other_script,
            },
        ]);

        let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
            .expect_err("altered payment output must be rejected");

        assert!(err.to_string().contains("missing payment output"));
    }

    #[test]
    fn payjoin_send_payment_output_rejects_split_only_outputs() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(6_000),
                script_pubkey: payment_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(4_000),
                script_pubkey: payment_script.clone(),
            },
        ]);

        let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
            .expect_err("split-only receiver outputs are unsupported");

        assert!(err.to_string().contains("missing payment output"));
    }

    #[test]
    fn payjoin_send_validation_accepts_net_spend_within_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let validation = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            12_000,
            1_000,
        )
        .expect("net spend at cap is accepted");

        assert_eq!(validation.fee_contribution_sat, 1_000);
    }

    #[test]
    fn payjoin_send_validation_accepts_larger_receiver_output_with_local_fee_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(12_000),
            script_pubkey: payment_script.clone(),
        }]);

        let validation = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            20_000,
            9_500,
        )
        .expect("receiver-funded larger output is accepted when mint spend is capped");

        assert_eq!(validation.fee_contribution_sat, 500);
    }

    #[test]
    fn payjoin_send_validation_rejects_net_spend_above_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let err = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            12_001,
            1_000,
        )
        .expect_err("net spend above amount plus max fee is rejected");

        assert!(err.to_string().contains("exceeding cap"));
    }

    #[test]
    fn payjoin_send_validation_rejects_net_spend_below_payment_amount() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let err = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            9_999,
            0,
        )
        .expect_err("mint net spend below quote cannot produce fee contribution");

        assert!(err.to_string().contains("below payment amount"));
    }

    #[test]
    fn payjoin_original_receiver_output_amount_sums_all_receiver_outputs() {
        let events = vec![
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0, 2]),
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
                TxOut {
                    value: BitcoinAmount::from_sat(21_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(99_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(34_000),
                    script_pubkey: ScriptBuf::new(),
                },
            ]),
        ];

        assert_eq!(
            payjoin_original_receiver_output_amount_from_events(&events),
            Some(55_000)
        );
    }

    #[test]
    fn payjoin_receive_amount_missing_events_returns_none() {
        let events = vec![::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0])];

        assert_eq!(
            payjoin_original_receiver_output_amount_from_events(&events),
            None
        );
    }

    #[test]
    fn payjoin_original_input_outpoints_come_from_retrieved_payload_event() {
        let first_outpoint = OutPoint::new(
            Txid::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("valid txid"),
            0,
        );
        let second_outpoint = OutPoint::new(
            Txid::from_str("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .expect("valid txid"),
            1,
        );
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: first_outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Default::default(),
                },
                TxIn {
                    previous_output: second_outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Default::default(),
                },
            ],
            output: vec![TxOut {
                value: BitcoinAmount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let psbt = Psbt::from_unsigned_tx(tx).expect("valid unsigned psbt");
        let event = serde_json::json!({
            "RetrievedOriginalPayload": {
                "original": {
                    "psbt": psbt,
                    "params": {
                        "v": 2,
                        "output_substitution": "Enabled",
                        "additional_fee_contribution": null,
                        "min_fee_rate": 250
                    }
                },
                "reply_key": null
            }
        });
        let event = serde_json::from_value(event).expect("deserialize Payjoin session event");

        assert_eq!(
            payjoin_original_input_outpoints_from_events(&[event])
                .expect("extract original input outpoints"),
            vec![first_outpoint, second_outpoint]
        );
    }

    #[test]
    fn payjoin_receive_session_expiry_is_strictly_in_the_past() {
        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 1_000,
            expires_at: 100,
            events: Vec::new(),
            closed: false,
        };

        assert!(!payjoin_receive_session_expired(&record, 100));
        assert!(payjoin_receive_session_expired(&record, 101));
    }

    #[test]
    fn payjoin_receive_session_prunes_closed_records_after_retention() {
        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 1_000,
            expires_at: 100,
            events: Vec::new(),
            closed: true,
        };
        let retention_edge = 100 + PAYJOIN_RECEIVE_SESSION_RETENTION_SECS;

        assert!(!should_prune_payjoin_receive_session(
            &record,
            retention_edge
        ));
        assert!(should_prune_payjoin_receive_session(
            &record,
            retention_edge + 1
        ));

        let mut open_record = record;
        open_record.closed = false;
        assert!(!should_prune_payjoin_receive_session(
            &open_record,
            retention_edge + 1
        ));
    }

    #[test]
    fn builds_payjoin_endpoint_from_normalized_fields() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
            "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
            1_720_547_781,
        )
        .expect("valid Payjoin keys");

        assert_eq!(
            build_payjoin_endpoint(&payjoin).expect("endpoint builds"),
            "https://payjoin.example/pj#EX1C4UC6ES-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
    }
}
