use super::*;

impl CdkBdk {
    /// Persist the exposure boundary before reserving the proposal in BDK.
    pub(super) async fn persist_fresh_cut_through_exposure(
        &self,
        record: &mut crate::storage::PayjoinReceiveSessionRecord,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
        closed: bool,
        cut_through: &CutThroughProposal,
    ) -> Result<(), Error> {
        self.apply_payjoin_receive_session_progress(record, persister, closed)?;
        let proposal_txid = cut_through.proposal_tx.compute_txid().to_string();
        record.proposal_tx_bytes = Some(consensus::serialize(&cut_through.proposal_tx));
        record.cut_through = Some(crate::storage::PayjoinCutThroughProgress::Active {
            reservation_id: cut_through.reservation_id,
            send_intent_id: cut_through.send_intent_id,
            proposal_txid: proposal_txid.clone(),
        });

        let intent = self
            .storage
            .get_send_intent(&cut_through.send_intent_id)
            .await?
            .ok_or(Error::SendIntentNotFound(cut_through.send_intent_id))?;
        let created_at = match intent.state {
            crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
                reservation_id,
                created_at,
                ..
            } if reservation_id == cut_through.reservation_id => created_at,
            _ => return Err(Error::Payjoin("stale cut-through reservation".to_string())),
        };
        let exposed = crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
            reservation_id: cut_through.reservation_id,
            receive_quote_id: record.quote_id.clone(),
            original_receive_amount_sat: intent_amount_from_cut_through(&intent),
            original_tx_bytes: consensus::serialize(&cut_through.original_tx),
            proposal_txid,
            receive_outpoint: cut_through.receive_outpoint.clone(),
            melt_outpoint: cut_through.melt_outpoint.clone(),
            fee_contribution_sat: cut_through.fee_contribution_sat,
            conflict_observed_height: None,
            created_at,
        };
        self.storage
            .expose_cut_through(
                record,
                cut_through.send_intent_id,
                cut_through.reservation_id,
                &exposed,
            )
            .await?;

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(cut_through.proposal_tx.clone(), crate::util::unix_now())]);
        wallet_with_db
            .persist()
            .map(|_| ())
            .map_err(Error::Database)
    }

    pub(super) async fn try_build_cut_through_receive_proposal(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsOutputs>,
        quote_id: &str,
        persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
    ) -> Result<Option<PayjoinReceiveProposal>, Error> {
        let events = persister.events()?;
        let Some(original_receive_amount_sat) =
            payjoin_original_receiver_output_amount_from_events(&events)
        else {
            return Ok(None);
        };
        if payjoin_receiver_output_count_from_events(&events).unwrap_or(1) > 2
            || original_receive_amount_sat < self.min_receive_amount_sat
        {
            return Ok(None);
        }

        let (intent, reservation_id) = match self
            .reserved_cut_through_candidate(quote_id, original_receive_amount_sat)
            .await?
        {
            Some(candidate) => candidate,
            None => {
                let Some(candidate) = self
                    .reserve_cut_through_candidate(quote_id, original_receive_amount_sat)
                    .await?
                else {
                    return Ok(None);
                };
                candidate
            }
        };

        match self
            .build_reserved_cut_through_proposal(
                receiver,
                &events,
                &intent,
                reservation_id,
                original_receive_amount_sat,
            )
            .await
        {
            Ok(Some((proposal, cut_through, staged))) => {
                persister.replace(staged.events()?, staged.closed())?;
                Ok(Some(PayjoinReceiveProposal {
                    proposal,
                    cut_through: Some(CutThroughReceiveProposal::Fresh(Box::new(cut_through))),
                    planning_guard: None,
                }))
            }
            Ok(None) => {
                self.storage
                    .release_cut_through_reserved_intent(&intent.intent_id, reservation_id)
                    .await?;
                Ok(None)
            }
            Err(err) => {
                tracing::warn!(
                    reservation_id = %reservation_id,
                    send_intent_id = %intent.intent_id,
                    "Cut-through construction failed before exposure; using ordinary Payjoin: {}",
                    err
                );
                self.storage
                    .release_cut_through_reserved_intent(&intent.intent_id, reservation_id)
                    .await?;
                Ok(None)
            }
        }
    }

    pub(super) async fn reserved_cut_through_candidate(
        &self,
        quote_id: &str,
        original_receive_amount_sat: u64,
    ) -> Result<Option<(crate::send::payment_intent::record::SendIntentRecord, Uuid)>, Error> {
        for intent in self.storage.get_all_send_intents().await? {
            if let crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
                reservation_id,
                ref receive_quote_id,
                original_receive_amount_sat: reserved_amount,
                ..
            } = intent.state
            {
                if receive_quote_id == quote_id
                    && reserved_amount == original_receive_amount_sat
                    && intent.amount_sat <= original_receive_amount_sat
                {
                    return Ok(Some((intent, reservation_id)));
                }
                if receive_quote_id == quote_id {
                    self.storage
                        .release_cut_through_reserved_intent(&intent.intent_id, reservation_id)
                        .await?;
                }
            }
        }
        Ok(None)
    }

    pub(super) async fn exposed_cut_through_for_proposal(
        &self,
        quote_id: &str,
        proposal_psbt: &bdk_wallet::bitcoin::Psbt,
    ) -> Result<bool, Error> {
        let Some(session) = self.storage.get_payjoin_receive_session(quote_id).await? else {
            return Ok(false);
        };
        let proposal_txid = proposal_psbt.unsigned_tx.compute_txid().to_string();
        Ok(matches!(
            session.cut_through,
            Some(crate::storage::PayjoinCutThroughProgress::Active {
                proposal_txid: active,
                ..
            }) if active == proposal_txid
        ))
    }

    pub(super) async fn reserve_cut_through_candidate(
        &self,
        quote_id: &str,
        original_receive_amount_sat: u64,
    ) -> Result<Option<(crate::send::payment_intent::record::SendIntentRecord, Uuid)>, Error> {
        let mut candidates = self.storage.get_pending_send_intents().await?;
        candidates.retain(|intent| intent.amount_sat <= original_receive_amount_sat);
        candidates.sort_by_key(|intent| match intent.state {
            crate::send::payment_intent::record::SendIntentState::Pending { created_at } => {
                created_at
            }
            _ => u64::MAX,
        });
        for candidate in candidates {
            let reservation_id = Uuid::new_v4();
            if let Some(reserved) = self
                .storage
                .reserve_pending_send_intent_for_cut_through(
                    &candidate.intent_id,
                    reservation_id,
                    quote_id,
                    original_receive_amount_sat,
                )
                .await?
            {
                return Ok(Some((reserved, reservation_id)));
            }
        }
        Ok(None)
    }

    #[allow(clippy::type_complexity)]
    async fn build_reserved_cut_through_proposal(
        &self,
        receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsOutputs>,
        events: &[::payjoin::receive::v2::SessionEvent],
        intent: &crate::send::payment_intent::record::SendIntentRecord,
        reservation_id: Uuid,
        original_receive_amount_sat: u64,
    ) -> Result<
        Option<(
            ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>,
            CutThroughProposal,
            RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
        )>,
        Error,
    > {
        let original_tx = payjoin_original_tx_from_events(events)?;
        let melt_address = parse_checked_address(&intent.address, self.network, Error::Payjoin)?;
        let melt_script = melt_address.script_pubkey();
        let surplus_sat = original_receive_amount_sat.saturating_sub(intent.amount_sat);
        let (drain_script, drain_is_surplus, drain_value_sat) = {
            let mut wallet_with_db = self.wallet_with_db.lock().await;
            let address = wallet_with_db
                .wallet
                .reveal_next_address(KeychainKind::External);
            let script = address.address.script_pubkey();
            let dust = TxOut::minimal_non_dust(script.clone()).value.to_sat();
            wallet_with_db.persist().map_err(Error::Database)?;
            (script, surplus_sat >= dust, surplus_sat.max(dust))
        };
        let staged = RecordingSessionPersister::new(events.to_vec(), false);
        let receiver = receiver
            .replace_receiver_outputs(
                vec![
                    TxOut {
                        value: bdk_wallet::bitcoin::Amount::from_sat(intent.amount_sat),
                        script_pubkey: melt_script.clone(),
                    },
                    TxOut {
                        value: bdk_wallet::bitcoin::Amount::from_sat(drain_value_sat),
                        script_pubkey: drain_script.clone(),
                    },
                ],
                drain_script.as_script(),
            )
            .map_err(|err| Error::Payjoin(err.to_string()))?
            .commit_outputs()
            .save(&staged)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = self.contribute_payjoin_inputs(receiver, &staged).await?;
        let proposal = self.finalize_payjoin_proposal(receiver, &staged).await?;
        let proposal_tx =
            proposal.psbt().clone().extract_tx().map_err(|err| {
                Error::Payjoin(format!("Could not extract cut-through tx: {}", err))
            })?;
        let wallet_with_db = self.wallet_with_db.lock().await;
        let (sent, received) = wallet_with_db.wallet.sent_and_received(&proposal_tx);
        let fee_contribution_sat = sent.to_sat().saturating_sub(received.to_sat());
        drop(wallet_with_db);
        if fee_contribution_sat > intent.max_fee_amount_sat {
            return Ok(None);
        }
        let melt_outpoint =
            find_payment_outpoint(&proposal_tx, melt_script.as_script(), intent.amount_sat)
                .ok_or_else(|| {
                    Error::Payjoin("Cut-through proposal missing melt output".to_string())
                })?;
        let receive_outpoint = if drain_is_surplus {
            find_payment_outpoint(&proposal_tx, drain_script.as_script(), 1)
                .unwrap_or(melt_outpoint)
        } else {
            melt_outpoint
        };
        Ok(Some((
            proposal,
            CutThroughProposal {
                reservation_id,
                send_intent_id: intent.intent_id,
                proposal_tx,
                original_tx,
                receive_outpoint: receive_outpoint.to_string(),
                melt_outpoint: melt_outpoint.to_string(),
                fee_contribution_sat,
            },
            staged,
        )))
    }

    /// Release reservations which could not have been exposed before a crash.
    pub(crate) async fn release_stale_cut_through_reservations(&self) -> Result<(), Error> {
        for intent in self.storage.get_all_send_intents().await? {
            if let crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
                reservation_id,
                ..
            } = intent.state
            {
                self.storage
                    .release_cut_through_reserved_intent(&intent.intent_id, reservation_id)
                    .await?;
            }
        }
        Ok(())
    }

    /// Restore every durable receive proposal reservation before batching starts.
    pub(crate) async fn restore_payjoin_receive_reservations(&self) -> Result<(), Error> {
        let sessions = self.storage.get_all_payjoin_receive_sessions().await?;
        let mut transactions = Vec::new();
        let mut abandoned = Vec::new();
        for session in sessions {
            if let Some(crate::storage::PayjoinCutThroughProgress::Abandoned { proposal_txid }) =
                session.cut_through
            {
                if let Ok(txid) = bdk_wallet::bitcoin::Txid::from_str(&proposal_txid) {
                    abandoned.push(txid);
                }
                continue;
            }
            let Some(bytes) = session.proposal_tx_bytes else {
                continue;
            };
            let tx = consensus::deserialize::<Transaction>(&bytes).map_err(|err| {
                Error::Payjoin(format!(
                    "Could not restore persisted receive proposal: {}",
                    err
                ))
            })?;
            transactions.push((tx, crate::util::unix_now()));
        }
        if !transactions.is_empty() {
            let mut wallet = self.wallet_with_db.lock().await;
            wallet.wallet.apply_unconfirmed_txs(transactions);
            wallet.persist().map_err(Error::Database)?;
        }
        for txid in abandoned {
            self.evict_unstaged_send_tx(txid).await?;
        }
        Ok(())
    }

    pub(super) async fn advance_cut_through_settlements_once(&self) -> Result<(), Error> {
        let tip_height = {
            let wallet = self.wallet_with_db.lock().await;
            wallet.wallet.latest_checkpoint().height()
        };
        for intent in self.storage.get_all_send_intents().await? {
            let crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
                reservation_id,
                ref receive_quote_id,
                original_receive_amount_sat,
                ref original_tx_bytes,
                ref proposal_txid,
                ref receive_outpoint,
                ref melt_outpoint,
                fee_contribution_sat,
                conflict_observed_height,
                created_at,
            } = intent.state
            else {
                continue;
            };
            let original_tx =
                consensus::deserialize::<Transaction>(original_tx_bytes).map_err(|err| {
                    Error::Payjoin(format!("Could not deserialize Payjoin original: {}", err))
                })?;
            let original_txid = original_tx.compute_txid().to_string();
            let (proposal_depth, original_depth) = {
                let wallet = self.wallet_with_db.lock().await;
                (
                    self.tx_confirmation_depth(&wallet.wallet, proposal_txid),
                    self.tx_confirmation_depth(&wallet.wallet, &original_txid),
                )
            };
            match known_transaction_decision(proposal_depth, original_depth, self.num_confs) {
                KnownTransactionDecision::Finalize => {
                    self.finalize_confirmed_cut_through(
                        &intent,
                        reservation_id,
                        receive_quote_id,
                        original_receive_amount_sat,
                        receive_outpoint,
                        melt_outpoint,
                        fee_contribution_sat,
                    )
                    .await?;
                    continue;
                }
                KnownTransactionDecision::Abandon => {
                    self.abandon_exposed_cut_through(
                        intent.intent_id,
                        reservation_id,
                        receive_quote_id,
                        proposal_txid,
                    )
                    .await?;
                    continue;
                }
                KnownTransactionDecision::Wait => continue,
                KnownTransactionDecision::CheckConflict => {}
            }

            let outpoints = original_tx
                .input
                .iter()
                .map(|input| input.previous_output)
                .collect::<Vec<_>>();
            let spent = self.chain_source.any_confirmed_spend(&outpoints).await?;
            let (observed, conflict_mature) = advance_conflict_observation(
                spent,
                conflict_observed_height,
                tip_height,
                self.num_confs,
            );
            if conflict_mature {
                self.abandon_exposed_cut_through(
                    intent.intent_id,
                    reservation_id,
                    receive_quote_id,
                    proposal_txid,
                )
                .await?;
            } else if observed != conflict_observed_height {
                self.storage
                    .update_send_intent(
                        &intent.intent_id,
                        &crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
                            reservation_id,
                            receive_quote_id: receive_quote_id.clone(),
                            original_receive_amount_sat,
                            original_tx_bytes: original_tx_bytes.clone(),
                            proposal_txid: proposal_txid.clone(),
                            receive_outpoint: receive_outpoint.clone(),
                            melt_outpoint: melt_outpoint.clone(),
                            fee_contribution_sat,
                            conflict_observed_height: observed,
                            created_at,
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    fn tx_confirmation_depth(
        &self,
        wallet: &bdk_wallet::PersistedWallet<bdk_wallet::rusqlite::Connection>,
        txid: &str,
    ) -> Option<u32> {
        let txid = bdk_wallet::bitcoin::Txid::from_str(txid).ok()?;
        let tx = wallet.get_tx(txid)?;
        match tx.chain_position {
            bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } => Some(
                wallet
                    .latest_checkpoint()
                    .height()
                    .saturating_sub(anchor.block_id.height)
                    .saturating_add(1),
            ),
            bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => Some(0),
        }
    }

    async fn abandon_exposed_cut_through(
        &self,
        intent_id: Uuid,
        reservation_id: Uuid,
        receive_quote_id: &str,
        proposal_txid: &str,
    ) -> Result<(), Error> {
        let mut session = self
            .storage
            .get_payjoin_receive_session(receive_quote_id)
            .await?
            .ok_or_else(|| Error::Payjoin("cut-through receive session missing".to_string()))?;
        session.cut_through = Some(crate::storage::PayjoinCutThroughProgress::Abandoned {
            proposal_txid: proposal_txid.to_string(),
        });
        self.storage
            .abandon_cut_through(intent_id, reservation_id, &session)
            .await?;
        if let Ok(txid) = bdk_wallet::bitcoin::Txid::from_str(proposal_txid) {
            self.evict_unstaged_send_tx(txid).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_confirmed_cut_through(
        &self,
        intent: &crate::send::payment_intent::record::SendIntentRecord,
        reservation_id: Uuid,
        receive_quote_id: &str,
        original_receive_amount_sat: u64,
        receive_outpoint: &str,
        melt_outpoint: &str,
        fee_contribution_sat: u64,
    ) -> Result<(), Error> {
        let finalized_at = crate::util::unix_now();
        let receive_record = crate::storage::FinalizedReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: receive_quote_id.to_string(),
            address: String::new(),
            txid: OutPoint::from_str(receive_outpoint)
                .map(|outpoint| outpoint.txid.to_string())
                .unwrap_or_else(|_| receive_outpoint.to_string()),
            outpoint: receive_outpoint.to_string(),
            payment_id: Some(receive_outpoint.to_string()),
            amount_sat: original_receive_amount_sat,
            finalized_at,
        };
        let send_record = crate::storage::FinalizedSendIntentRecord {
            intent_id: intent.intent_id,
            quote_id: intent.quote_id.clone(),
            total_spent_sat: intent.amount_sat.saturating_add(fee_contribution_sat),
            outpoint: melt_outpoint.to_string(),
            finalized_at,
        };
        let mut session = self
            .storage
            .get_payjoin_receive_session(receive_quote_id)
            .await?
            .ok_or_else(|| Error::Payjoin("cut-through receive session missing".to_string()))?;
        let proposal_txid = match &intent.state {
            crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
                proposal_txid,
                ..
            } => proposal_txid.clone(),
            _ => {
                return Err(Error::Payjoin(
                    "cut-through intent is not exposed".to_string(),
                ))
            }
        };
        session.cut_through =
            Some(crate::storage::PayjoinCutThroughProgress::Confirmed { proposal_txid });
        self.storage
            .finalize_cut_through_pair(&receive_record, &send_record, reservation_id, &session)
            .await?;

        if let Ok(quote_id) = cdk_common::QuoteId::from_str(receive_quote_id) {
            let _ = self
                .payment_sender
                .send(Event::PaymentReceived(WaitPaymentResponse {
                    payment_identifier: PaymentIdentifier::QuoteId(quote_id),
                    payment_amount: Amount::new(original_receive_amount_sat, CurrencyUnit::Sat),
                    payment_id: receive_outpoint.to_string(),
                }));
        }
        if let Ok(quote_id) = cdk_common::QuoteId::from_str(&intent.quote_id) {
            let _ = self.payment_sender.send(Event::PaymentSuccessful {
                quote_id: quote_id.clone(),
                details: MakePaymentResponse {
                    payment_lookup_id: PaymentIdentifier::QuoteId(quote_id),
                    payment_proof: Some(melt_outpoint.to_string()),
                    status: MeltQuoteState::Paid,
                    total_spent: Amount::new(
                        intent.amount_sat.saturating_add(fee_contribution_sat),
                        CurrencyUnit::Sat,
                    ),
                },
            });
        }
        Ok(())
    }
}

fn intent_amount_from_cut_through(
    intent: &crate::send::payment_intent::record::SendIntentRecord,
) -> u64 {
    match intent.state {
        crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
            original_receive_amount_sat,
            ..
        } => original_receive_amount_sat,
        _ => 0,
    }
}

fn advance_conflict_observation(
    spent: bool,
    observed_height: Option<u32>,
    tip_height: u32,
    num_confs: u32,
) -> (Option<u32>, bool) {
    if !spent {
        return (None, false);
    }
    let observed_height = observed_height.unwrap_or(tip_height);
    let depth = tip_height.saturating_sub(observed_height).saturating_add(1);
    (Some(observed_height), depth >= num_confs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownTransactionDecision {
    Finalize,
    Abandon,
    Wait,
    CheckConflict,
}

fn known_transaction_decision(
    proposal_depth: Option<u32>,
    original_depth: Option<u32>,
    num_confs: u32,
) -> KnownTransactionDecision {
    if proposal_depth.is_some_and(|depth| depth > 0 && depth >= num_confs) {
        KnownTransactionDecision::Finalize
    } else if proposal_depth.is_some_and(|depth| depth > 0) {
        KnownTransactionDecision::Wait
    } else if original_depth.is_some_and(|depth| depth > 0 && depth >= num_confs) {
        KnownTransactionDecision::Abandon
    } else if original_depth.is_some_and(|depth| depth > 0) {
        KnownTransactionDecision::Wait
    } else {
        KnownTransactionDecision::CheckConflict
    }
}

#[cfg(test)]
mod tests {
    use super::{
        advance_conflict_observation, known_transaction_decision, KnownTransactionDecision,
    };

    #[test]
    fn known_transactions_respect_confirmation_depth_and_proposal_precedence() {
        assert_eq!(
            known_transaction_decision(Some(1), None, 2),
            KnownTransactionDecision::Wait
        );
        assert_eq!(
            known_transaction_decision(Some(2), None, 2),
            KnownTransactionDecision::Finalize
        );
        assert_eq!(
            known_transaction_decision(None, Some(1), 2),
            KnownTransactionDecision::Wait
        );
        assert_eq!(
            known_transaction_decision(None, Some(2), 2),
            KnownTransactionDecision::Abandon
        );
        assert_eq!(
            known_transaction_decision(Some(2), Some(2), 2),
            KnownTransactionDecision::Finalize
        );
        assert_eq!(
            known_transaction_decision(None, None, 2),
            KnownTransactionDecision::CheckConflict
        );
    }

    #[test]
    fn unknown_spend_waits_for_configured_observation_depth() {
        assert_eq!(
            advance_conflict_observation(true, None, 100, 2),
            (Some(100), false)
        );
        assert_eq!(
            advance_conflict_observation(true, Some(100), 101, 2),
            (Some(100), true)
        );
    }

    #[test]
    fn unknown_spend_reorg_clears_observation() {
        assert_eq!(
            advance_conflict_observation(false, Some(100), 101, 2),
            (None, false)
        );
    }
}
