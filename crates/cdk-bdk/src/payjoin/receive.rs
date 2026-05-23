use super::*;

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

        let ohttp_keys = self.cached_ohttp_keys(config).await?;
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

        let endpoint = receiver.pj_uri().extras.endpoint();
        let payjoin = payjoin_v2_from_bip77_endpoint(&endpoint)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: quote_id.to_string(),
            fallback_address: address.to_string(),
            amount_sat,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: payjoin.expires_at,
            events: persister.events()?,
            closed: persister.closed(),
        };
        self.storage.put_payjoin_receive_session(&record).await?;

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
                    futures::stream::iter(sessions)
                        .for_each_concurrent(PAYJOIN_POLL_CONCURRENCY, |record| async move {
                            if let Err(err) = self.handle_payjoin_receive_session_once(record, now).await {
                                tracing::warn!("Payjoin receive session processing failed: {}", err);
                            }
                        })
                        .await;
                    if let Err(err) = self.advance_cut_through_settlements_once().await {
                        tracing::warn!("Cut-through settlement processing failed: {}", err);
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn recover_payjoin_receive_sessions_once(&self) -> Result<(), Error> {
        let now = crate::util::unix_now();
        let sessions = self.storage.get_all_payjoin_receive_sessions().await?;

        for record in sessions {
            if let Err(err) = self.handle_payjoin_receive_session_once(record, now).await {
                tracing::warn!("Payjoin receive session recovery failed: {}", err);
            }
        }

        Ok(())
    }

    pub(super) async fn handle_payjoin_receive_session_once(
        &self,
        mut record: crate::storage::PayjoinReceiveSessionRecord,
        now: u64,
    ) -> Result<(), Error> {
        if record.closed {
            if record.should_prune(now, PAYJOIN_RECEIVE_SESSION_RETENTION_SECS) {
                if matches!(
                    record.cut_through,
                    Some(crate::storage::PayjoinCutThroughProgress::Active { .. })
                ) {
                    tracing::debug!(
                        quote_id = %record.quote_id,
                        "Retaining active cut-through receive session"
                    );
                    return Ok(());
                }
                if !self.payjoin_receive_credit_cap_resolved(&record).await? {
                    tracing::debug!(
                        quote_id = %record.quote_id,
                        "Retaining aged Payjoin receive session: signed proposal unresolved"
                    );
                    return Ok(());
                }
                tracing::debug!(
                    quote_id = %record.quote_id,
                    expires_at = record.expires_at,
                    now,
                    "Pruning closed Payjoin receive session"
                );
                self.storage
                    .delete_payjoin_receive_session(&record.quote_id)
                    .await?;
            } else {
                tracing::trace!(
                    quote_id = %record.quote_id,
                    "Skipping closed Payjoin receive session"
                );
            }
            return Ok(());
        }

        if record.is_expired(now) {
            tracing::debug!(
                quote_id = %record.quote_id,
                expires_at = record.expires_at,
                now,
                "Closing expired Payjoin receive session"
            );
            record.closed = true;
            return self.storage.put_payjoin_receive_session(&record).await;
        }

        if self.payjoin_config().is_none() {
            tracing::trace!(
                quote_id = %record.quote_id,
                "Payjoin receive config unavailable; leaving open session for fallback address detection"
            );
            return Ok(());
        }

        tracing::debug!(
            quote_id = %record.quote_id,
            fallback_address = %record.fallback_address,
            event_count = record.events.len(),
            "Processing Payjoin receive session"
        );
        self.process_payjoin_receive_session(record).await
    }

    pub(super) async fn payjoin_receive_credit_cap_resolved(
        &self,
        record: &crate::storage::PayjoinReceiveSessionRecord,
    ) -> Result<bool, Error> {
        payjoin_receive_credit_cap_resolved(&self.storage, self.network, record).await
    }

    pub(crate) async fn process_payjoin_receive_session(
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
        let mut suppress_final_progress_persist = false;

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
                    #[cfg(test)]
                    self.planning_test_hooks
                        .pause(crate::PlanningPausePoint::PayjoinReceivePoll)
                        .await;
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
                        self.accept_payjoin_wants_outputs(receiver, &record.quote_id, &persister)
                            .await?,
                    )
                }
                ::payjoin::receive::v2::ReceiveSession::WantsOutputs(receiver) => Some(
                    self.accept_payjoin_wants_outputs(receiver, &record.quote_id, &persister)
                        .await?,
                ),
                ::payjoin::receive::v2::ReceiveSession::WantsInputs(receiver) => {
                    let planning_guard = self.tx_planning_lock.clone().lock_owned().await;
                    let receiver = self.contribute_payjoin_inputs(receiver, &persister).await?;
                    Some(PayjoinReceiveProposal {
                        proposal: self.finalize_payjoin_proposal(receiver, &persister).await?,
                        cut_through: None,
                        planning_guard: Some(planning_guard),
                    })
                }
                ::payjoin::receive::v2::ReceiveSession::WantsFeeRange(receiver) => {
                    let planning_guard = self.tx_planning_lock.clone().lock_owned().await;
                    let receiver = apply_zero_receiver_fee_range(receiver, &persister)?;
                    Some(PayjoinReceiveProposal {
                        proposal: self.finalize_payjoin_proposal(receiver, &persister).await?,
                        cut_through: None,
                        planning_guard: Some(planning_guard),
                    })
                }
                ::payjoin::receive::v2::ReceiveSession::ProvisionalProposal(receiver) => {
                    let planning_guard = self.tx_planning_lock.clone().lock_owned().await;
                    Some(PayjoinReceiveProposal {
                        proposal: self.finalize_payjoin_proposal(receiver, &persister).await?,
                        cut_through: None,
                        planning_guard: Some(planning_guard),
                    })
                }
                ::payjoin::receive::v2::ReceiveSession::PayjoinProposal(proposal) => {
                    let proposal_txid = proposal.psbt().unsigned_tx.compute_txid().to_string();
                    match &record.cut_through {
                        Some(crate::storage::PayjoinCutThroughProgress::Confirmed {
                            proposal_txid: terminal,
                        })
                        | Some(crate::storage::PayjoinCutThroughProgress::Abandoned {
                            proposal_txid: terminal,
                        }) if terminal == &proposal_txid => {
                            closed = true;
                            None
                        }
                        _ => {
                            let cut_through = self
                                .exposed_cut_through_for_proposal(&record.quote_id, proposal.psbt())
                                .await?
                                .then_some(CutThroughReceiveProposal::Exposed);
                            Some(PayjoinReceiveProposal {
                                proposal,
                                cut_through,
                                planning_guard: None,
                            })
                        }
                    }
                }
                ::payjoin::receive::v2::ReceiveSession::HasReplyableError(receiver) => {
                    if let Some(error_reply) =
                        latest_payjoin_receive_replyable_error(&persister.events()?)
                    {
                        tracing::warn!(
                            quote_id = %record.quote_id,
                            error_reply = %error_reply,
                            "Sending Payjoin receiver rejection to sender"
                        );
                    }
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

            if let Some(payjoin_proposal) = payjoin_proposal {
                let PayjoinReceiveProposal {
                    proposal,
                    cut_through,
                    planning_guard,
                } = payjoin_proposal;
                update_payjoin_receive_credit_cap(&mut record);
                if cut_through.is_none() {
                    if let Err(err) = ensure_payjoin_receiver_credit(
                        proposal.psbt(),
                        &fallback_script,
                        record.amount_sat,
                    ) {
                        closed = true;
                        return Err(err);
                    }
                    update_payjoin_receive_proposal_receiver_outpoints(
                        &mut record,
                        proposal.psbt(),
                        &fallback_script,
                    );
                }
                tracing::debug!(
                    quote_id = %record.quote_id,
                    "Posting Payjoin proposal response"
                );
                let proposal_tx = proposal.psbt().clone().extract_tx().map_err(|err| {
                    Error::Payjoin(format!("Could not extract proposal: {}", err))
                })?;
                record.proposal_tx_bytes = Some(consensus::serialize(&proposal_tx));
                match cut_through.as_ref() {
                    Some(CutThroughReceiveProposal::Fresh(cut_through)) => {
                        if let Err(err) = self
                            .persist_fresh_cut_through_exposure(
                                &mut record,
                                &persister,
                                closed,
                                cut_through,
                            )
                            .await
                        {
                            suppress_final_progress_persist = true;
                            return Err(err);
                        }
                    }
                    Some(CutThroughReceiveProposal::Exposed) => {
                        tracing::debug!(
                            quote_id = %record.quote_id,
                            "Reposting exposed cut-through Payjoin proposal"
                        );
                        self.persist_payjoin_receive_session_progress(
                            &mut record,
                            &persister,
                            closed,
                        )
                        .await?;
                        let mut wallet = self.wallet_with_db.lock().await;
                        wallet.wallet.apply_unconfirmed_txs([(
                            proposal_tx.clone(),
                            crate::util::unix_now(),
                        )]);
                        wallet.persist().map_err(Error::Database)?;
                    }
                    None => {
                        self.persist_payjoin_receive_session_progress(
                            &mut record,
                            &persister,
                            closed,
                        )
                        .await?;
                        let mut wallet = self.wallet_with_db.lock().await;
                        wallet.wallet.apply_unconfirmed_txs([(
                            proposal_tx.clone(),
                            crate::util::unix_now(),
                        )]);
                        wallet.persist().map_err(Error::Database)?;
                    }
                }

                // Fresh proposals are now durable and applied to BDK. Replayed
                // proposals arrive without a guard. In either case, construct
                // and send the directory request outside transaction planning.
                self.release_planning_before_payjoin_post(planning_guard)
                    .await;

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

        if result.is_err() {
            if let Some(error_reply) = latest_payjoin_receive_replyable_error(&persister.events()?)
            {
                tracing::warn!(
                    quote_id = %record.quote_id,
                    error_reply = %error_reply,
                    "Payjoin receiver rejected original PSBT"
                );
            }
        }
        // Skip the rewrite on no-progress polls: the record (with its
        // PSBT-sized event log) only needs persisting when something changed.
        let progressed = persister.is_dirty() || closed != record.closed;
        if !suppress_final_progress_persist && progressed {
            self.persist_payjoin_receive_session_progress(&mut record, &persister, closed)
                .await?;
        }
        result
    }

    pub(crate) async fn release_planning_before_payjoin_post(
        &self,
        planning_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) {
        drop(planning_guard);
        #[cfg(test)]
        self.planning_test_hooks
            .pause(crate::PlanningPausePoint::PayjoinReceivePost)
            .await;
    }

    pub(super) async fn accept_payjoin_receive_proposal(
        &self,
        unchecked: ::payjoin::receive::v2::Receiver<
            ::payjoin::receive::v2::UncheckedOriginalPayload,
        >,
        fallback_script: &bdk_wallet::bitcoin::Script,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<PayjoinReceiveProposal, Error> {
        // The mint is a non-interactive receiver (auto-published URI per quote),
        // so validate the original is broadcastable before advancing — this is the
        // probing/poisoning defense (inputs are only recorded as seen afterwards).
        // The check runs before entering the (synchronous) payjoin closure so the
        // RPC round trip does not block the async runtime.
        let original_tx = payjoin_original_tx_from_events(&persister.events()?)?;
        // Bitcoin Core: trust the testmempoolaccept verdict. Esplora has no
        // dry-run (`None`) and relies on the enforced minimum fee rate.
        let broadcastable = self
            .chain_source
            .accepts_broadcast(&original_tx)
            .await?
            .unwrap_or(true);
        let receiver = unchecked
            .check_broadcast_suitability(Some(PAYJOIN_RECEIVER_MIN_ORIGINAL_FEE_RATE), |_tx| {
                Ok(broadcastable)
            })
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let receiver = self
            .check_payjoin_inputs_not_owned(receiver, persister)
            .await?;

        self.accept_payjoin_checked_inputs(receiver, fallback_script, quote_id, persister)
            .await
    }
    pub(super) async fn accept_payjoin_checked_inputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsSeen>,
        fallback_script: &bdk_wallet::bitcoin::Script,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<PayjoinReceiveProposal, Error> {
        let receiver = self
            .check_payjoin_inputs_not_seen(receiver, quote_id, persister)
            .await?;
        let receiver =
            self.identify_payjoin_receiver_outputs(receiver, fallback_script, quote_id, persister)?;

        self.accept_payjoin_wants_outputs(receiver, quote_id, persister)
            .await
    }
    pub(super) async fn accept_payjoin_wants_outputs(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsOutputs>,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<PayjoinReceiveProposal, Error> {
        let planning_guard = self.tx_planning_lock.clone().lock_owned().await;
        if let Some(mut cut_through) = self
            .try_build_cut_through_receive_proposal(receiver.clone(), quote_id, persister)
            .await?
        {
            cut_through.planning_guard = Some(planning_guard);
            return Ok(cut_through);
        }

        let receiver = receiver
            .commit_outputs()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = self.contribute_payjoin_inputs(receiver, persister).await?;

        Ok(PayjoinReceiveProposal {
            proposal: self.finalize_payjoin_proposal(receiver, persister).await?,
            cut_through: None,
            planning_guard: Some(planning_guard),
        })
    }

    pub(super) async fn check_payjoin_inputs_not_owned(
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
    pub(super) async fn check_payjoin_inputs_not_seen(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::MaybeInputsSeen>,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::OutputsUnknown>, Error>
    {
        let original_input_outpoints =
            payjoin_original_input_outpoints_from_events(&persister.events()?)?;
        let seen_outpoints = futures::future::try_join_all(original_input_outpoints.iter().map(
            |outpoint| async move {
                let seen = self
                    .storage
                    .is_payjoin_input_seen(&outpoint.to_string())
                    .await?;
                Ok::<_, Error>(seen.then_some(*outpoint))
            },
        ))
        .await?
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
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
    pub(super) fn identify_payjoin_receiver_outputs(
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
    pub(super) async fn contribute_payjoin_inputs(
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
        // Keep only the first candidate for the error-path fallback instead of
        // cloning the whole O(UTXO set) list (each entry may carry whole-tx
        // `non_witness_utxo` data).
        let fallback_input = candidate_inputs.first().cloned();
        let selected = receiver
            .try_preserving_privacy(candidate_inputs)
            .or_else(|_| {
                fallback_input.ok_or_else(|| {
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
    pub(super) async fn finalize_payjoin_proposal(
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
}
