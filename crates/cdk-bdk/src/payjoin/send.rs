use super::*;

impl CdkBdk {
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_payjoin_send(
        &self,
        quote_id: &cdk_common::QuoteId,
        address: &str,
        amount_sat: u64,
        max_fee_sat: u64,
        tier: PaymentTier,
        metadata: PaymentMetadata,
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
            planning_guard,
        } = prepared;

        let intent = SendIntent::<intent_state::PayjoinNegotiating>::new_payjoin(
            &self.storage,
            quote_id.to_string(),
            address.to_string(),
            amount_sat,
            max_fee_sat,
            tier,
            metadata,
            consensus::serialize(&original_tx),
            original_fee_sat,
            persister.events()?,
        )
        .await
        .map_err(|err| Error::PayjoinSendNotStarted(Box::new(err)))?;

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
            if let Err(evict_err) = self.evict_unstaged_send_tx(original_txid).await {
                return Err(Error::PayjoinSendNotStarted(Box::new(Error::Payjoin(
                    format!(
                        "Could not persist reservation of original Payjoin tx {}: {}; \
                         additionally could not persist eviction of the in-memory reservation: {}",
                        original_txid, err, evict_err
                    ),
                ))));
            }
            if let Err(fail_err) = intent
                .fail(
                    &self.storage,
                    format!("Could not persist Payjoin original tx reservation: {}", err),
                )
                .await
            {
                tracing::warn!(
                    quote_id = %quote_id,
                    error = %fail_err,
                    "Could not mark Payjoin send intent failed after reservation failure"
                );
            }
            return Err(Error::PayjoinSendNotStarted(Box::new(err)));
        }

        // The negotiating intent and BDK reservation are both durable. The
        // directory negotiation must not retain the planning lock.
        drop(planning_guard);

        tracing::debug!(
            quote_id = %quote_id,
            original_txid = %original_txid,
            "Started Payjoin send session; negotiation runs in the background poller"
        );

        // Return Pending immediately. `check_outgoing_payment` can see the
        // PayjoinNegotiating intent and drive recovery if the poller is not
        // running yet.
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
    pub(super) async fn prepare_payjoin_send(
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
        let pj_uri = build_payjoin_uri(address, amount_sat, payjoin)?;
        let pj_uri = ::payjoin::Uri::try_from(pj_uri.as_str())
            .map_err(|err| Error::Payjoin(format!("Invalid Payjoin URI: {}", err)))?
            .assume_checked()
            .check_pj_supported()
            .map_err(|_| {
                Error::Payjoin("Payjoin URI did not contain supported pj params".to_string())
            })?;
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
        let fee_rate = crate::fee::fee_rate_from_sat_per_vb(sat_per_vb)?;
        let planning_guard = self.tx_planning_lock.clone().lock_owned().await;

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
            planning_guard,
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
                    let intents = self.payjoin_send_intents().await?;
                    tracing::debug!(
                        intent_count = intents.len(),
                        active_count = intents.len(),
                        "Polling Payjoin send intents"
                    );
                    futures::stream::iter(intents)
                        .for_each_concurrent(PAYJOIN_POLL_CONCURRENCY, |intent| async move {
                            if let Err(err) = self.process_payjoin_send_intent(intent).await {
                                tracing::warn!("Payjoin send intent processing failed: {}", err);
                            }
                        })
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Drive every open receive session, exposed cut-through settlement, and
    /// send intent once — exactly one tick's worth of the background pollers'
    /// work. Test-only: production progress comes from the pollers themselves.
    #[cfg(test)]
    pub(crate) async fn recover_payjoin_sessions_once(&self) -> Result<(), Error> {
        self.recover_payjoin_receive_sessions_once().await?;
        self.advance_cut_through_settlements_once().await?;
        self.recover_payjoin_send_intents_once().await
    }

    /// Startup-only, DB-only recovery. A settlement still `Reserved` here
    /// means the process died between reserving the melt intent and exposing
    /// the proposal, so the reservation can be released safely. While the
    /// service is running, `Reserved` is a live transient state owned by an
    /// active receive session and must not be reaped; periodic driving goes
    /// through [`Self::advance_cut_through_settlements_once`] instead. This
    /// must therefore complete before any session is driven.
    #[cfg(test)]
    pub(super) async fn recover_payjoin_send_intents_once(&self) -> Result<(), Error> {
        for intent in self.payjoin_send_intents().await? {
            if let Err(err) = self.process_payjoin_send_intent(intent).await {
                tracing::warn!("Payjoin send intent recovery failed: {}", err);
            }
        }

        Ok(())
    }

    pub(super) async fn payjoin_send_intents(
        &self,
    ) -> Result<Vec<SendIntent<intent_state::PayjoinNegotiating>>, Error> {
        let records = self.storage.get_all_send_intents().await?;
        Ok(records
            .iter()
            .filter_map(
                |record| match crate::send::payment_intent::from_record(record) {
                    crate::send::payment_intent::SendIntentAny::PayjoinNegotiating(intent) => {
                        Some(intent)
                    }
                    _ => None,
                },
            )
            .collect())
    }

    /// Restore BDK coin reservations for Payjoin sends that were durable when
    /// the process stopped but had not yet reached the shared staging pipeline.
    ///
    /// This runs synchronously during startup before normal batching starts.
    /// Re-applying an original transaction already present in BDK is
    /// idempotent, while a transaction missing because of the intent/wallet
    /// persistence crash window becomes reserved again.
    pub(crate) async fn restore_payjoin_send_reservations(&self) -> Result<(), Error> {
        let intents = self.payjoin_send_intents().await?;
        if intents.is_empty() {
            return Ok(());
        }

        let recovered_at = crate::util::unix_now();
        let mut original_txs = Vec::with_capacity(intents.len());
        for intent in &intents {
            let original_tx = consensus::deserialize::<Transaction>(
                &intent.state.original_tx_bytes,
            )
            .map_err(|err| {
                Error::Payjoin(format!(
                    "Could not deserialize original transaction for Payjoin send {}: {}",
                    intent.quote_id, err
                ))
            })?;
            original_txs.push((original_tx, recovered_at));
        }

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db.wallet.apply_unconfirmed_txs(original_txs);
        wallet_with_db.persist().map_err(Error::Database)?;
        tracing::info!(
            intent_count = intents.len(),
            "Restored Payjoin send reservations during startup"
        );
        Ok(())
    }

    pub(crate) async fn process_payjoin_send_intent(
        &self,
        mut intent: SendIntent<intent_state::PayjoinNegotiating>,
    ) -> Result<(), Error> {
        use ::payjoin::persist::OptionalTransitionOutcome;
        use ::payjoin::send::v2::{SendSession, SessionOutcome};

        // Idempotency: if this quote already has a staged or finalized send,
        // the negotiation has completed on another path.
        let active_done = self
            .storage
            .get_send_intent_by_quote_id(&intent.quote_id)
            .await?
            .is_some_and(|active| {
                !matches!(
                    active.state,
                    crate::send::payment_intent::record::SendIntentState::PayjoinNegotiating { .. }
                )
            });
        if active_done
            || self
                .storage
                .get_finalized_intent_by_quote_id(&intent.quote_id)
                .await?
                .is_some()
        {
            return Ok(());
        }

        let Some(config) = self.payjoin_config() else {
            tracing::debug!(
                quote_id = %intent.quote_id,
                "Payjoin send config unavailable; broadcasting original fallback"
            );
            return self.broadcast_payjoin_send_fallback(intent).await;
        };

        let persister = RecordingSessionPersister::new(intent.state.events.clone(), false);
        let session = match ::payjoin::send::v2::replay_event_log(&persister) {
            Ok((session, _history)) => session,
            Err(err) => {
                // The session can no longer be replayed (most commonly because
                // the Payjoin parameters have expired). Broadcast the signed
                // original fallback so the melt still settles.
                tracing::debug!(
                    quote_id = %intent.quote_id,
                    error = %err,
                    "Payjoin send session not replayable (expired?); broadcasting original fallback"
                );
                return self.broadcast_payjoin_send_fallback(intent).await;
            }
        };

        match session {
            SendSession::WithReplyKey(sender) => {
                tracing::debug!(
                    quote_id = %intent.quote_id,
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
                self.persist_payjoin_send_progress(&mut intent, &persister)
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
                            quote_id = %intent.quote_id,
                            "Received Payjoin proposal PSBT"
                        );
                        self.persist_payjoin_send_progress(&mut intent, &persister)
                            .await?;
                        self.finalize_and_stage_payjoin_send(intent, proposal_psbt)
                            .await?;
                    }
                    OptionalTransitionOutcome::Stasis(_) => {
                        tracing::debug!(
                            quote_id = %intent.quote_id,
                            "No Payjoin proposal available yet"
                        );
                        self.persist_payjoin_send_progress(&mut intent, &persister)
                            .await?;
                    }
                }
            }
            SendSession::Closed(outcome) => match outcome {
                SessionOutcome::Success(proposal_psbt) => {
                    // Crash/resume: the proposal was received before staging.
                    self.finalize_and_stage_payjoin_send(intent, proposal_psbt)
                        .await?;
                }
                SessionOutcome::Failure | SessionOutcome::Cancel => {
                    tracing::debug!(
                        quote_id = %intent.quote_id,
                        "Payjoin send session closed without success; broadcasting original fallback"
                    );
                    self.broadcast_payjoin_send_fallback(intent).await?;
                }
            },
        }

        Ok(())
    }

    /// Sign the Payjoin proposal, evict the locally-reserved original, then
    /// stage and broadcast the Payjoin transaction. If the proposal would make
    /// the mint spend more than the quote amount plus max fee, broadcast the
    /// original fallback instead (it is already within budget).
    pub(super) async fn finalize_and_stage_payjoin_send(
        &self,
        intent: SendIntent<intent_state::PayjoinNegotiating>,
        proposal_psbt: bdk_wallet::bitcoin::Psbt,
    ) -> Result<(), Error> {
        let fallback_address =
            parse_checked_address(&intent.address, self.network, Error::Payjoin)?;

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
                intent.amount,
                intent.max_fee_amount,
                sent.to_sat(),
                received.to_sat(),
            );
            (tx, validation)
        };
        let validation = match validation {
            Ok(validation) => validation,
            Err(err) => {
                tracing::warn!(
                    quote_id = %intent.quote_id,
                    error = %err,
                    "Payjoin proposal exceeds local spend limits or altered the payment output; \
                     broadcasting original fallback instead"
                );
                return self.broadcast_payjoin_send_fallback(intent).await;
            }
        };

        let planning_guard = self.tx_planning_lock.clone().lock_owned().await;

        // The Payjoin tx spends the original's inputs plus the receiver's, so
        // evict the locally-reserved original before applying the Payjoin tx to
        // avoid a conflicting double-application in the wallet graph.
        if let Ok(original_tx) =
            consensus::deserialize::<Transaction>(&intent.state.original_tx_bytes)
        {
            let original_txid = original_tx.compute_txid();
            if original_txid != tx.compute_txid() {
                self.evict_unstaged_send_tx(original_txid).await?;
            }
        }

        self.stage_and_broadcast_payjoin_send(tx, validation, intent, planning_guard)
            .await?;
        Ok(())
    }

    /// Stage and broadcast the signed original transaction as the Payjoin
    /// fallback, then close the session.
    pub(super) async fn broadcast_payjoin_send_fallback(
        &self,
        intent: SendIntent<intent_state::PayjoinNegotiating>,
    ) -> Result<(), Error> {
        let original_tx = consensus::deserialize::<Transaction>(&intent.state.original_tx_bytes)
            .map_err(|err| {
                Error::Payjoin(format!(
                    "Could not deserialize original Payjoin tx: {}",
                    err
                ))
            })?;
        let fallback_address =
            parse_checked_address(&intent.address, self.network, Error::Payjoin)?;
        let validation = PayjoinSendValidation {
            payment_outpoint: require_payjoin_send_payment_output(
                &original_tx,
                fallback_address.script_pubkey().as_script(),
                intent.amount,
            )?,
            fee_contribution_sat: intent.state.original_fee_sat,
        };

        let planning_guard = self.tx_planning_lock.clone().lock_owned().await;
        self.stage_and_broadcast_payjoin_send(original_tx, validation, intent, planning_guard)
            .await?;
        Ok(())
    }

    /// Durably stage and broadcast a chosen Payjoin send transaction (either the
    /// Payjoin proposal or the original fallback), moving the negotiating send
    /// intent into the batch pipeline so `check_outgoing_payment` can track it.
    ///
    /// By the time a transaction reaches staging the fully-signed original has
    /// already been posted to the payjoin directory, so the receiver can
    /// broadcast it regardless of local state. On staging failure the intent
    /// therefore must NOT be marked Failed — that would release the melt's
    /// proofs while the payment may still confirm on-chain. It stays in
    /// PayjoinNegotiating so the poller retries staging.
    pub(super) async fn stage_and_broadcast_payjoin_send(
        &self,
        tx: Transaction,
        validation: PayjoinSendValidation,
        payjoin_intent: SendIntent<intent_state::PayjoinNegotiating>,
        planning_guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), Error> {
        let quote_id = payjoin_intent.quote_id.clone();
        let fee_contribution_sat = validation.fee_contribution_sat;
        {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            wallet_with_db
                .wallet
                .apply_unconfirmed_txs([(tx.clone(), crate::util::unix_now())]);
            wallet_with_db.persist().map_err(Error::Database)?;
        }

        let batch_id = Uuid::new_v4();
        let assignment = BatchOutputAssignment {
            intent_id: payjoin_intent.intent_id,
            vout: validation.payment_outpoint.vout,
            fee_contribution_sat,
        };

        match self
            .stage_and_broadcast_signed_send_batch(
                batch_id,
                &tx,
                vec![assignment],
                fee_contribution_sat,
                vec![StageableSendIntent::Payjoin(payjoin_intent)],
                planning_guard,
            )
            .await
        {
            Ok(StagedBroadcastOutcome::Broadcast) => Ok(()),
            Ok(StagedBroadcastOutcome::PendingRecovery(err)) => {
                // Staging is durable, so the payjoin session can close; recovery
                // finishes the broadcast.
                tracing::warn!(
                    quote_id,
                    batch_id = %batch_id,
                    error = %err,
                    "Payjoin transaction is durably staged; recovery will complete the broadcast"
                );
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    quote_id,
                    batch_id = %batch_id,
                    error = %err,
                    "Payjoin staging failed after exposure, will retry"
                );
                Err(err)
            }
        }
    }
}
