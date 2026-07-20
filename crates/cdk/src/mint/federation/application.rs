async fn finish_federation_transaction<T>(
    transaction: database::mint::DynMintTransaction,
    entry: &FederationJournalEntry,
    phase: &'static str,
    outcome: Result<T, Error>,
) -> Result<T, Error> {
    match outcome {
        Ok(value) => {
            transaction.commit().await.map_err(|err| {
                tracing::error!(
                    operation_id = %entry.operation_id,
                    operation_kind = %entry.envelope.operation.kind(),
                    transaction_phase = phase,
                    "Could not commit federation operation transaction: {err}"
                );
                Error::from(err)
            })?;
            Ok(value)
        }
        Err(error) => {
            transaction.rollback().await.map_err(|rollback_error| {
                tracing::error!(
                    operation_id = %entry.operation_id,
                    operation_kind = %entry.envelope.operation.kind(),
                    transaction_phase = phase,
                    apply_error = %error,
                    "Could not roll back federation operation transaction: {rollback_error}"
                );
                Error::from(rollback_error)
            })?;
            Err(error)
        }
    }
}

async fn finish_federation_apply_transaction<T>(
    mut transaction: database::mint::DynMintTransaction,
    entry: &FederationJournalEntry,
    outcome: Result<T, Error>,
) -> Result<T, Error> {
    let outcome = match outcome {
        Ok(value) => transaction
            .set_federation_operation_application_state(
                entry.operation_id,
                &FederationApplicationState::Applied {},
            )
            .await
            .map(|()| value)
            .map_err(Error::from),
        Err(error) => Err(error),
    };

    finish_federation_transaction(transaction, entry, "apply", outcome).await
}

impl Mint {
    fn claim_active_federation_melt_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Option<FederationActiveMeltPayment> {
        let mut active = match self.federation_active_melt_payments.lock() {
            Ok(active) => active,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !active.insert(quote_id.clone()) {
            return None;
        }

        Some(FederationActiveMeltPayment {
            active: std::sync::Arc::clone(&self.federation_active_melt_payments),
            quote_id: quote_id.clone(),
        })
    }

    /// Compute the current payment-attempt lease actor for an accepted melt.
    ///
    /// The accepted envelope author holds the first lease epoch. Authority
    /// then rotates deterministically through the roster every
    /// [`FEDERATION_MELT_PAYMENT_ACTOR_LEASE_ITEMS`] accepted journal items,
    /// so an offline or Byzantine author cannot veto the payment forever.
    /// The lease is derived only from consensus-ordered state (the melt
    /// operation's journal index and the roster), so every member with the
    /// same journal frontier computes the same single actor, and takeover is
    /// safe because the caller-supplied idempotency key is durable before any
    /// external I/O and contract-honoring backends deduplicate by it.
    async fn federation_melt_payment_lease_actor(
        &self,
        config: &FederationRuntimeConfig,
        author: MemberId,
        operation_index: u64,
    ) -> Result<MemberId, Error> {
        let members: Vec<MemberId> = config.public_config().members.keys().copied().collect();
        let author_position = members
            .iter()
            .position(|member| *member == author)
            .ok_or(Error::Internal)?;
        let journal_frontier = self
            .localstore
            .get_federation_journal_tip()
            .await?
            .map_or(0, |tip| tip.index.saturating_add(1));
        let lease_epoch = journal_frontier.saturating_sub(operation_index.saturating_add(1))
            / FEDERATION_MELT_PAYMENT_ACTOR_LEASE_ITEMS;
        let epoch_steps = usize::try_from(lease_epoch % members.len() as u64).unwrap_or_default();
        let actor_position = (author_position + epoch_steps) % members.len();
        Ok(members[actor_position])
    }

    /// Probe an incoming payment backend without mutating federated quote state.
    pub async fn probe_federation_mint_quote_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<FederationMintQuotePaymentObservation>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.payment_method.is_onchain() && config.public_config().frost_wallet.is_some() {
            if quote.unit != CurrencyUnit::Sat {
                return Err(Error::UnsupportedUnit);
            }
            let Some(payment) = self
                .federation_chain_observation_state()
                .await?
                .quote_payment_at_or_before(&quote.id.to_string(), u64::MAX)?
            else {
                return Ok(None);
            };
            if payment.amount_sat <= quote.amount_paid().value() {
                return Ok(None);
            }
            return Ok(Some(FederationMintQuotePaymentObservation {
                quote: quote.id.to_string(),
                observer: config.member_id(),
                method: quote.payment_method.clone(),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(payment.amount_sat),
                amount_issued: quote.amount_issued().into(),
                payment_id: Some(PaymentIdentifier::PaymentId(payment.payment_id)),
                payment_proof: None,
            }));
        }
        if quote.state() == MintQuoteState::Issued {
            return Ok(None);
        }

        let processor = self
            .payment_processors
            .get(&PaymentProcessorKey::new(
                quote.unit.clone(),
                quote.payment_method.clone(),
            ))
            .ok_or(Error::UnsupportedUnit)?;
        let payments = processor
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;
        let mut observed_quote = quote.clone();
        let mut payment_id = None;
        let mut payment_label: Option<String> = None;

        for payment in payments {
            let candidate_payment_id =
                federation_payment_identifier_from_label(&payment.payment_id);
            let durable_payment_label = federation_payment_identifier_label(&candidate_payment_id);
            if payment.payment_amount.value() == 0
                || observed_quote.payment_ids().contains(&&payment.payment_id)
                || observed_quote
                    .payment_ids()
                    .contains(&&durable_payment_label)
            {
                continue;
            }
            let amount = payment.payment_amount.convert_to(&observed_quote.unit)?;
            // Settlement identity is canonical: every member selects the
            // smallest durable payment label, independent of backend return
            // order, so exact-match evidence groups agree across members.
            if payment_label
                .as_deref()
                .is_none_or(|current| durable_payment_label.as_str() < current)
            {
                payment_label = Some(durable_payment_label);
                payment_id = Some(candidate_payment_id);
            }
            observed_quote.add_payment(amount, payment.payment_id, None)?;
        }

        if observed_quote.state() != MintQuoteState::Paid
            || observed_quote.amount_paid().value() == 0
        {
            return Ok(None);
        }
        if payment_id.is_none() {
            payment_id = observed_quote
                .payments
                .iter()
                .map(|payment| federation_payment_identifier_from_label(&payment.payment_id))
                .min_by_key(federation_payment_identifier_label);
        }

        Ok(Some(FederationMintQuotePaymentObservation {
            quote: observed_quote.id.to_string(),
            observer: config.member_id(),
            method: observed_quote.payment_method.clone(),
            state: MintQuoteState::Paid,
            amount_paid: observed_quote.amount_paid().into(),
            amount_issued: observed_quote.amount_issued().into(),
            payment_id,
            payment_proof: None,
        }))
    }

    /// Probe or initiate an outgoing federated payment without finalizing its saga.
    pub async fn probe_federation_melt_quote_payment(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<FederationMeltQuotePaymentObservation>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.state != cdk_common::MeltQuoteState::Pending {
            return Ok(None);
        }
        let Some(saga) = self.localstore.get_melt_saga_by_quote_id(quote_id).await? else {
            return Ok(None);
        };
        let operation_id = saga
            .federation_operation_id
            .as_deref()
            .ok_or(Error::Internal)?
            .parse::<FederationOperationId>()?;
        if quote.payment_method.is_onchain() && config.public_config().frost_wallet.is_some() {
            if quote.unit != CurrencyUnit::Sat {
                return Err(Error::UnsupportedUnit);
            }
            let Some(receipt) = self
                .federation_transaction_state()
                .await?
                .completed_payment_for_quote(&quote.id.to_string())?
            else {
                return Ok(None);
            };
            validate_federation_onchain_melt_receipt(&quote, operation_id, &receipt)?;
            return Ok(Some(FederationMeltQuotePaymentObservation {
                quote: quote.id.to_string(),
                observer: config.member_id(),
                method: quote.payment_method.clone(),
                state: cdk_common::MeltQuoteState::Paid,
                amount: quote.amount().into(),
                fee_reserve: quote.fee_reserve().into(),
                payment_id: Some(PaymentIdentifier::QuoteId(quote.id.clone())),
                payment_proof: Some(receipt.outpoint.to_string()),
            }));
        }
        let entry = self
            .localstore
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
            .ok_or(Error::Internal)?;
        let author = entry
            .envelope
            .verify_authorization(config.public_config())?;
        let lease_actor = self
            .federation_melt_payment_lease_actor(&config, author, entry.index)
            .await?;
        let is_payment_actor = lease_actor == config.member_id();
        let _active_payment = if is_payment_actor {
            let Some(active_payment) = self.claim_active_federation_melt_payment(quote_id) else {
                return Ok(None);
            };
            Some(active_payment)
        } else {
            None
        };

        let payment = match saga.state {
            SagaStateEnum::Melt(MeltSagaState::SetupComplete) if is_payment_actor => {
                let Some(payment) = self
                    .attempt_federation_external_melt_payment(quote.clone(), saga)
                    .await?
                else {
                    return Ok(None);
                };
                payment
            }
            SagaStateEnum::Melt(
                MeltSagaState::SetupComplete
                | MeltSagaState::PaymentAttempted
                | MeltSagaState::Finalizing,
            ) => self.check_melt_payment_status(&quote).await?,
            SagaStateEnum::Swap(_) => return Err(Error::Internal),
        };

        match payment.status {
            cdk_common::MeltQuoteState::Paid => {}
            cdk_common::MeltQuoteState::Failed | cdk_common::MeltQuoteState::Unpaid
                if is_payment_actor => {}
            cdk_common::MeltQuoteState::Failed
            | cdk_common::MeltQuoteState::Unpaid
            | cdk_common::MeltQuoteState::Pending
            | cdk_common::MeltQuoteState::Unknown => return Ok(None),
        }

        Ok(Some(FederationMeltQuotePaymentObservation {
            quote: quote.id.to_string(),
            observer: config.member_id(),
            method: quote.payment_method.clone(),
            state: payment.status,
            amount: quote.amount().into(),
            fee_reserve: quote.fee_reserve().into(),
            payment_id: Some(payment.payment_lookup_id),
            payment_proof: payment.payment_proof,
        }))
    }

    async fn persisted_signature_shares_for_outputs(
        &self,
        operation_id: FederationOperationId,
        outputs: &[BlindedMessage],
    ) -> Result<Vec<BlindSignature>, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let member_id = config.member_id();
        let mut signatures = Vec::with_capacity(outputs.len());

        for (index, output) in outputs.iter().enumerate() {
            let output_index = u32::try_from(index)
                .map_err(|_| Error::Custom("federation output index overflow".to_string()))?;
            let share = self
                .localstore
                .get_federation_signature_shares_for_output(operation_id, output_index)
                .await?
                .into_iter()
                .find(|share| {
                    share.member_id == member_id
                        && share.output_index == output_index
                        && share.blinded_secret == output.blinded_secret
                        && share.keyset_id == output.keyset_id
                        && share.amount == output.amount
                })
                .ok_or_else(|| {
                    Error::Custom(format!(
                        "missing persisted federation signature share for operation {operation_id} output {output_index}"
                    ))
                })?;
            signatures.push(super::federation_signature_share_to_blind_signature(share));
        }

        Ok(signatures)
    }

    /// Return this member's persisted wallet-facing mint share response.
    pub async fn persisted_mint_share_response(
        &self,
        operation_id: FederationOperationId,
        input: &MintInput,
    ) -> Result<MintResponse, Error> {
        Ok(MintResponse {
            signatures: self
                .persisted_signature_shares_for_outputs(operation_id, input.outputs())
                .await?,
        })
    }

    /// Return this member's persisted wallet-facing swap share response.
    pub async fn persisted_swap_share_response(
        &self,
        operation_id: FederationOperationId,
        request: &SwapRequest,
    ) -> Result<SwapResponse, Error> {
        Ok(SwapResponse {
            signatures: self
                .persisted_signature_shares_for_outputs(operation_id, request.outputs())
                .await?,
        })
    }

    pub(crate) fn reject_standalone_federation_path(&self, operation: &str) -> Result<(), Error> {
        if self.federation_config().is_some() {
            return Err(Error::Custom(format!(
                "federation member requires consensus acceptance before {operation}"
            )));
        }

        Ok(())
    }

    /// Return the federation runtime config after replaying applied config transitions.
    pub async fn effective_federation_config(
        &self,
    ) -> Result<Option<FederationRuntimeConfig>, Error> {
        let Some(config) = self.federation_config() else {
            return Ok(None);
        };

        let mut cache = self.federation_effective_config_cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            return Ok(Some(cached.clone()));
        }

        #[cfg(test)]
        self.federation_effective_config_rebuild_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut effective = config.clone();
        let mut rotations = self
            .localstore
            .get_federation_materialized_state_keys()
            .await?
            .into_iter()
            .filter_map(|record| match record.key {
                FederationMaterializedStateKey::KeysetRotation { keyset } => {
                    Some((record.operation_index, record.operation_id, keyset))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        rotations
            .sort_by_key(|(operation_index, operation_id, _)| (*operation_index, *operation_id));
        for (_, _, keyset) in rotations {
            effective =
                effective.apply_keyset_rotation(&FederationKeysetRotationOperation { keyset })?;
        }

        *cache = Some(effective.clone());

        Ok(Some(effective))
    }

    pub(crate) fn verify_federation_outputs_with_config(
        &self,
        config: &FederationRuntimeConfig,
        outputs: &[BlindedMessage],
    ) -> Result<Verification, Error> {
        if outputs.is_empty() {
            tracing::debug!("verify_federation_outputs called with empty outputs");
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        let outputs_count = outputs.len();
        if outputs_count > self.max_outputs {
            tracing::warn!(
                "Federation request exceeds max outputs limit: {} > {}",
                outputs_count,
                self.max_outputs
            );
            return Err(Error::MaxOutputsExceeded {
                actual: outputs_count,
                max: self.max_outputs,
            });
        }

        Self::check_outputs_unique(outputs)?;
        let mut keyset_units = HashSet::new();
        for output in outputs {
            let keyset = config.public_config().validate_blinded_output(output)?;
            let keyset_info = super::keysets::federated_keyset_to_mint_keyset_info(keyset);
            if keyset_info.is_expired() {
                tracing::debug!(
                    "Transaction attempted with expired federated keyset in outputs: {}.",
                    keyset.id
                );
                return Err(Error::ExpiredKeyset);
            }
            keyset_units.insert(keyset.unit.clone());
        }

        if keyset_units.len() != 1 {
            tracing::debug!(
                "Federation transaction attempted with multiple units in outputs: {:?}.",
                keyset_units
            );
            return Err(Error::MultipleUnits);
        }

        let unit = keyset_units.into_iter().next().ok_or(Error::Internal)?;
        let amount = Amount::try_sum(outputs.iter().map(|output| output.amount))?.with_unit(unit);

        Ok(Verification { amount })
    }

    pub(crate) fn verify_federation_change_outputs_with_config(
        &self,
        config: &FederationRuntimeConfig,
        outputs: &[BlindedMessage],
    ) -> Result<Verification, Error> {
        if outputs.is_empty() {
            tracing::debug!("verify_federation_change_outputs called with empty outputs");
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        let outputs_count = outputs.len();
        if outputs_count > self.max_outputs {
            tracing::warn!(
                "Federation request exceeds max outputs limit: {} > {}",
                outputs_count,
                self.max_outputs
            );
            return Err(Error::MaxOutputsExceeded {
                actual: outputs_count,
                max: self.max_outputs,
            });
        }

        Self::check_outputs_unique(outputs)?;
        let mut keyset_units = HashSet::new();
        for output in outputs {
            let keyset = config
                .public_config()
                .validate_blinded_change_output(output)?;
            let keyset_info = super::keysets::federated_keyset_to_mint_keyset_info(keyset);
            if keyset_info.is_expired() {
                tracing::debug!(
                    "Transaction attempted with expired federated keyset in change outputs: {}.",
                    keyset.id
                );
                return Err(Error::ExpiredKeyset);
            }
            keyset_units.insert(keyset.unit.clone());
        }

        if keyset_units.len() != 1 {
            tracing::debug!(
                "Federation transaction attempted with multiple units in change outputs: {:?}.",
                keyset_units
            );
            return Err(Error::MultipleUnits);
        }

        let unit = keyset_units.into_iter().next().ok_or(Error::Internal)?;
        let amount = Amount::try_sum(outputs.iter().map(|output| output.amount))?.with_unit(unit);

        Ok(Verification { amount })
    }

    pub(crate) fn get_federation_proofs_fee_with_config(
        &self,
        config: &FederationRuntimeConfig,
        proofs: &Proofs,
    ) -> Result<crate::fees::ProofsFeeBreakdown, Error> {
        let mut proofs_per_keyset = HashMap::new();
        let mut fee_per_keyset = HashMap::new();

        for proof in proofs {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                fee_per_keyset.entry(proof.keyset_id)
            {
                let keyset = config
                    .public_config()
                    .keysets
                    .iter()
                    .find(|keyset| keyset.id == proof.keyset_id)
                    .ok_or(Error::UnknownKeySet)?;
                entry.insert(keyset.input_fee_ppk);
            }

            proofs_per_keyset
                .entry(proof.keyset_id)
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        Ok(calculate_fee(&proofs_per_keyset, &fee_per_keyset)?)
    }

    /// Apply accepted federation journal entries to this member's local mint database.
    ///
    /// This is intentionally idempotent so startup replay, duplicate operation
    /// submission, and private REST catch-up can all use the same path.
    pub async fn apply_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, Error> {
        let mut applied = Vec::new();

        for entry in entries {
            if self.apply_accepted_federation_operation(entry).await? {
                applied.push(entry.operation_id);
            }
        }

        Ok(applied)
    }

    /// Apply one accepted federation operation to this member's local mint database.
    ///
    /// This is the deterministic apply boundary used by journal replay, live
    /// finalization, and catch-up. Admission checks may reject malformed
    /// requests earlier, but accepted-order apply remains the authoritative
    /// stateful validity decision. Already applied entries are skipped for
    /// idempotent replay; locally rejected or failed entries are never
    /// materialized again through this path.
    pub async fn apply_accepted_federation_operation(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<bool, Error> {
        if let Some(persisted_entry) = self
            .localstore
            .get_federation_journal_entry_by_operation_id(entry.operation_id)
            .await?
        {
            match persisted_entry.status() {
                FederationOperationStatus::Applied => return Ok(false),
                FederationOperationStatus::Rejected => {
                    return Err(Error::Custom(format!(
                        "federation operation {} has local status {} and is not accepted for apply",
                        persisted_entry.operation_id,
                        federation_operation_status_label(persisted_entry.status())
                    )));
                }
                FederationOperationStatus::Failed => {
                    ensure_same_federation_journal_entry_for_retry(&persisted_entry, entry)?;
                }
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying => {}
            }
        }

        match entry.status() {
            FederationOperationStatus::Accepted | FederationOperationStatus::Applying => {}
            FederationOperationStatus::Applied => return Ok(false),
            FederationOperationStatus::Rejected | FederationOperationStatus::Failed => {
                return Err(Error::Custom(format!(
                    "federation operation {} has local status {} and is not accepted for apply",
                    entry.operation_id,
                    federation_operation_status_label(entry.status())
                )));
            }
        }

        match &entry.envelope.operation {
            FederationOperation::AddressAllocation(operation) => {
                self.apply_federation_address_allocation(entry, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::ChainObservation(observation) => {
                self.apply_federation_chain_observation(entry, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::MintQuote(operation) => {
                self.apply_federation_mint_quote(entry, operation).await?;
                Ok(true)
            }
            FederationOperation::MintQuotePayment(observation) => {
                self.apply_federation_mint_quote_payment(entry, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Mint(operation) => {
                self.apply_federation_mint(entry, operation).await?;
                Ok(true)
            }
            FederationOperation::MeltQuote(operation) => {
                self.apply_federation_melt_quote(entry, operation).await?;
                Ok(true)
            }
            FederationOperation::MeltQuotePayment(observation) => {
                self.apply_federation_melt_quote_payment(entry, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Swap(operation) => {
                self.apply_federation_swap(entry, operation).await?;
                Ok(true)
            }
            FederationOperation::Melt(operation) => {
                self.apply_federation_melt(entry, operation).await?;
                Ok(true)
            }
            FederationOperation::KeysetRotation(operation) => {
                self.apply_federation_keyset_rotation(entry, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::TransactionProposal(proposal) => {
                self.apply_federation_transaction_proposal(entry, proposal)
                    .await?;
                Ok(true)
            }
            FederationOperation::TransactionSigned(signed) => {
                self.apply_federation_signed_transaction(entry, signed)
                    .await?;
                Ok(true)
            }
            FederationOperation::TransactionBroadcastIntent(intent) => {
                self.apply_federation_transaction_broadcast_intent(entry, intent)
                    .await?;
                Ok(true)
            }
            FederationOperation::TransactionBroadcastObservation(observation) => {
                self.apply_federation_transaction_broadcast_observation(entry, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Bark(operation) => {
                self.apply_federation_bark(entry, operation).await?;
                Ok(true)
            }
        }
    }

    async fn apply_federation_bark(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationBarkOperation,
    ) -> Result<(), Error> {
        operation.validate()?;
        self.validate_federation_bark_operation_submission(operation)
            .await?;
        if let FederationBarkOperation::ReceiveIntent(intent) = operation {
            let runtime_config = self.federation_config().ok_or(Error::Internal)?;
            intent.validate_secret_acknowledgements(runtime_config.public_config())?;
        }
        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = transaction
            .apply_federation_bark_operation(entry, operation)
            .await
            .map_err(Error::from);
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    async fn require_prior_applied_melt_quote(
        &self,
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let entries = self.localstore.get_federation_journal_entries().await?;
        let quote = quote.to_string();
        let has_prior_quote = Self::has_prior_applied_federation_operation(
            &entries,
            max_operation_index,
            |operation| {
                matches!(
                    operation,
                    FederationOperation::MeltQuote(operation) if operation.quote == quote
                )
            },
        );
        if !has_prior_quote {
            return Err(Error::UnknownQuote);
        }
        Ok(())
    }

    async fn require_prior_applied_mint_quote(
        &self,
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let entries = self.localstore.get_federation_journal_entries().await?;
        Self::require_prior_applied_mint_quote_in_entries(&entries, quote, max_operation_index)
    }

    async fn require_prior_applied_mint_quotes(
        &self,
        quotes: &[QuoteId],
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let entries = self.localstore.get_federation_journal_entries().await?;
        for quote in quotes {
            Self::require_prior_applied_mint_quote_in_entries(
                &entries,
                quote,
                max_operation_index,
            )?;
        }
        Ok(())
    }

    fn require_prior_applied_mint_quote_in_entries(
        entries: &[FederationJournalEntry],
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let quote = quote.to_string();
        let has_prior_quote = Self::has_prior_applied_federation_operation(
            entries,
            max_operation_index,
            |operation| {
                matches!(
                    operation,
                    FederationOperation::MintQuote(operation) if operation.quote == quote
                )
            },
        );
        if !has_prior_quote {
            return Err(Error::UnknownQuote);
        }
        Ok(())
    }

    fn has_prior_applied_federation_operation(
        entries: &[FederationJournalEntry],
        max_operation_index: u64,
        mut matches_operation: impl FnMut(&FederationOperation) -> bool,
    ) -> bool {
        entries.iter().any(|entry| {
            entry.index < max_operation_index
                && entry.status() == FederationOperationStatus::Applied
                && matches_operation(&entry.envelope.operation)
        })
    }

    fn federation_payment_observation_can_finalize_at_or_before(
        &self,
        candidate: &FederationPaymentObservation,
        observations: &[FederationPaymentObservationRecord],
        max_operation_index: u64,
    ) -> Result<bool, Error> {
        let config = self.federation_config().ok_or(Error::Internal)?;
        let policy = *config
            .public_config()
            .payment_verification_policy(candidate.context());
        let mut eligible = observations
            .iter()
            .filter(|record| record.operation_index <= max_operation_index)
            .cloned()
            .collect::<Vec<_>>();
        let is_onchain = match candidate {
            FederationPaymentObservation::MintQuote(observation) => observation.method.is_onchain(),
            FederationPaymentObservation::MeltQuote(observation) => observation.method.is_onchain(),
        };
        if is_onchain {
            eligible.sort_by_key(|record| record.operation_index);
            let mut latest_by_observer = HashMap::new();
            for record in eligible {
                latest_by_observer.insert(record.observation.observer(), record);
            }
            eligible = latest_by_observer.into_values().collect();
            eligible.sort_by_key(|record| record.operation_index);
        }
        let required = policy.required_matching_observations();
        let quorum = candidate.quorum(eligible.iter().map(|record| &record.observation), required);

        match policy.evaluate_quorum(&quorum) {
            Ok(()) => {
                let report = FederationPaymentObservationReport::from_records(
                    candidate.context(),
                    candidate.quote(),
                    required,
                    &eligible,
                );
                Ok(!report.has_competing_quorums())
            }
            Err(FederationError::PaymentObservationQuorumNotSatisfied { .. }) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    fn federation_mint_payment_quorum_covers_amount_at_or_before(
        &self,
        quote: &MintQuote,
        required_amount: &Amount<CurrencyUnit>,
        observations: &[FederationPaymentObservationRecord],
        max_operation_index: u64,
    ) -> Result<bool, Error> {
        for record in observations
            .iter()
            .filter(|record| record.operation_index <= max_operation_index)
        {
            let FederationPaymentObservation::MintQuote(observation) = &record.observation else {
                continue;
            };
            if observation.quote != quote.id.to_string()
                || observation.method != quote.payment_method
                || observation.state != MintQuoteState::Paid
                || observation.amount_paid < required_amount.clone().into()
            {
                continue;
            }
            if self.federation_payment_observation_can_finalize_at_or_before(
                &record.observation,
                observations,
                max_operation_index,
            )? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn apply_federation_mint_quote(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        self.validate_federation_onchain_mint_quote_allocation(operation)
            .await?;
        let proposed_quote = mint_quote_from_federation_operation(operation)?;
        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quote = match tx.get_mint_quote(&proposed_quote.id).await? {
                Some(existing) => {
                    ensure_mint_quote_matches(&existing, &proposed_quote)?;
                    existing
                }
                None => {
                    tx.add_mint_quote(proposed_quote.clone()).await?;
                    database::mint::Acquired::from(proposed_quote)
                }
            };

            let observations = tx
                .get_federation_payment_observations(
                    FederationPaymentObservationContext::MintQuote,
                    &operation.quote,
                )
                .await?;
            let paid_notification = self
                .apply_mint_payment_observations_to_quote(&mut tx, &mut quote, &observations)
                .await?
                .map(|amount_paid| ((*quote).clone(), amount_paid));

            Ok(((*quote).clone(), paid_notification))
        }
        .await;
        let (published_quote, paid_notification) =
            finish_federation_apply_transaction(tx, entry, outcome).await?;

        self.publish_federation_mint_quote_created(&published_quote)?;
        if let Some((quote, amount_paid)) = paid_notification {
            self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }

        Ok(())
    }

    async fn validate_federation_onchain_mint_quote_allocation(
        &self,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(());
        };
        if runtime.public_config().frost_wallet.is_none()
            || !matches!(operation.request, MintQuoteRequest::Onchain(_))
        {
            return Ok(());
        }
        let (MintQuoteRequest::Onchain(request), MintQuoteResponse::Onchain(response)) =
            (&operation.request, &operation.response)
        else {
            return Err(Error::InvalidPaymentMethod);
        };
        let quote_id = quote_id_from_federation(&operation.quote)?;
        if operation.request_lookup_id != PaymentIdentifier::QuoteId(quote_id)
            || response.unit != request.unit
            || response.pubkey != request.pubkey
            || response.expiry.is_some()
            || response.amount_paid != Amount::ZERO
            || response.amount_issued != Amount::ZERO
        {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote response does not match its request"
                    .to_string(),
            }
            .into());
        }

        let application = runtime.cdk_onchain_application()?;
        let allocation = self
            .federation_address_allocation(
                FederationAddressAllocationScope {
                    application_id: application.application_id,
                    application_key_epoch: application.application_key_epoch,
                },
                &operation.quote,
            )
            .await?
            .ok_or_else(|| FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote has no accepted address allocation"
                    .to_string(),
            })?;
        if allocation.keychain != FederationAddressKeychain::External {
            return Err(FederationError::AddressAllocationInvalid {
                reason: "federated on-chain mint quote requires an external address allocation"
                    .to_string(),
            }
            .into());
        }
        let wallet = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        let script = wallet.derive_cdk_onchain_script(
            runtime.public_config(),
            application.application_id,
            application.application_key_epoch,
            allocation.keychain,
            allocation.derivation_index,
        )?;
        let expected_address =
            cdk_common::bitcoin::Address::from_script(script.as_script(), application.network)
                .map_err(|_| FederationError::OperationStateInvalid {
                    reason: "federated on-chain mint quote address is not representable"
                        .to_string(),
                })?;
        if response.request != expected_address.to_string() {
            return Err(FederationError::OperationStateInvalid {
                reason: "federated on-chain mint quote address does not match the derived address"
                    .to_string(),
            }
            .into());
        }
        Ok(())
    }

    fn publish_federation_mint_quote_created(&self, quote: &MintQuote) -> Result<(), Error> {
        match quote.payment_method {
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11) => {
                let response: MintQuoteBolt11Response<QuoteId> = quote.clone().into();
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt11Response(response));
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt12) => {
                let response: MintQuoteBolt12Response<QuoteId> = quote.clone().try_into()?;
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt12Response(response));
            }
            cdk_common::PaymentMethod::Custom(ref method) => {
                if let Ok(response) = MintQuoteCustomResponse::try_from(quote.clone()) {
                    self.pubsub_manager
                        .publish(NotificationPayload::CustomMintQuoteResponse(
                            method.clone(),
                            response,
                        ));
                }
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) => {
                let response: MintQuoteOnchainResponse<QuoteId> = quote.clone().try_into()?;
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteOnchainResponse(response));
            }
        }

        Ok(())
    }

    async fn apply_federation_keyset_rotation(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<(), Error> {
        self.effective_federation_config()
            .await?
            .ok_or(Error::Internal)?
            .apply_keyset_rotation(operation)?;

        let transaction = self.localstore.begin_transaction().await?;
        finish_federation_apply_transaction(transaction, entry, Ok(())).await?;
        *self.federation_effective_config_cache.lock().await = None;
        Ok(())
    }

    async fn apply_federation_mint_quote_payment(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMintQuotePaymentObservation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&observation.quote)?;
        self.require_prior_applied_mint_quote(&quote_id, entry.index)
            .await?;
        let record = payment_observation_record_from_entry(entry)?;
        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quote = tx.get_mint_quote(&quote_id).await?;
            if quote.is_none()
                && observation.method.is_onchain()
                && self
                    .federation_config()
                    .is_some_and(|runtime| runtime.public_config().frost_wallet.is_some())
            {
                return Err(Error::UnknownQuote);
            }
            if let Some(quote) = quote.as_deref() {
                self.validate_federation_onchain_payment_observation(
                    &mut tx,
                    quote,
                    observation,
                    record.operation_index,
                    false,
                )
                .await?;
            }
            tx.add_federation_payment_observation(&record).await?;
            let observations = tx
                .get_federation_payment_observations(
                    FederationPaymentObservationContext::MintQuote,
                    &observation.quote,
                )
                .await?;
            let mut paid_notification = None;
            if let Some(mut quote) = quote.take() {
                if let Some(amount_paid) = self
                    .apply_mint_payment_observations_to_quote(&mut tx, &mut quote, &observations)
                    .await?
                {
                    paid_notification = Some(((*quote).clone(), amount_paid));
                }
            }
            Ok(paid_notification)
        }
        .await;
        let paid_notification = finish_federation_apply_transaction(tx, entry, outcome).await?;

        if let Some((quote, amount_paid)) = paid_notification {
            self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }

        Ok(())
    }

    async fn apply_mint_payment_observations_to_quote(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &mut database::mint::Acquired<MintQuote>,
        observations: &[FederationPaymentObservationRecord],
    ) -> Result<Option<Amount<CurrencyUnit>>, Error> {
        let mut paid_notification = None;

        for record in observations {
            let FederationPaymentObservation::MintQuote(observation) = &record.observation else {
                continue;
            };
            if quote.payment_method != observation.method {
                return Err(Error::InvalidPaymentMethod);
            }

            let can_finalize = self.federation_payment_observation_can_finalize_at_or_before(
                &record.observation,
                observations,
                record.operation_index,
            )?;
            let observed_paid = observation.amount_paid.with_unit(quote.unit.clone());
            if can_finalize && observed_paid > quote.amount_paid() {
                self.validate_federation_onchain_payment_observation(
                    tx,
                    quote,
                    observation,
                    record.operation_index,
                    true,
                )
                .await?;
                let additional_amount = observed_paid
                    .checked_sub(&quote.amount_paid())
                    .map_err(|_| Error::AmountOverflow)?;
                let payment_id = observation
                    .payment_id
                    .as_ref()
                    .map(federation_payment_identifier_label)
                    .or_else(|| observation.payment_proof.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "federation:{}:{}",
                            record.operation_id,
                            mint_quote_state_label(observation.state)
                        )
                    });
                // Payment observations currently carry no wall-clock evidence.
                // Preserve deterministic replay without storing journal order
                // in the Unix timestamp field.
                quote.add_payment(additional_amount, payment_id, Some(0))?;
                tx.update_mint_quote(quote).await?;
                paid_notification = Some(quote.amount_paid());
            }
        }

        Ok(paid_notification)
    }

    async fn validate_federation_onchain_payment_observation(
        &self,
        tx: &mut database::mint::DynMintTransaction,
        quote: &MintQuote,
        observation: &FederationMintQuotePaymentObservation,
        max_operation_index: u64,
        mark_liability: bool,
    ) -> Result<(), Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(());
        };
        if runtime.public_config().frost_wallet.is_none() || !quote.payment_method.is_onchain() {
            return Ok(());
        }
        if quote.unit != CurrencyUnit::Sat
            || observation.method != quote.payment_method
            || observation.state != MintQuoteState::Paid
            || observation.payment_proof.is_some()
        {
            return Err(Error::InvalidPaymentRequest);
        }
        let bytes = tx
            .kv_read(
                CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                CHAIN_OBSERVATION_STATE_KEY,
            )
            .await?
            .ok_or_else(|| FederationError::ChainObservationStateInvalid {
                reason: "on-chain payment observation has no wallet UTXO projection".to_string(),
            })?;
        let mut state =
            cdk_common::federation::FederationChainObservationState::from_bytes(&bytes)?;
        let payment = state
            .quote_payment_at_or_before(&observation.quote, max_operation_index)?
            .ok_or(Error::UnpaidQuote)?;
        if u64::from(observation.amount_paid) != payment.amount_sat
            || observation.payment_id != Some(PaymentIdentifier::PaymentId(payment.payment_id))
        {
            return Err(FederationError::ChainObservationStateInvalid {
                reason: "on-chain payment observation does not match accepted wallet outputs"
                    .to_string(),
            }
            .into());
        }
        if mark_liability {
            state.mark_quote_liability_issued_at_or_before(
                &observation.quote,
                max_operation_index,
                payment.amount_sat,
                payment.payment_id,
            )?;
            tx.kv_write(
                CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                CHAIN_OBSERVATION_STATE_KEY,
                &state.to_bytes()?,
            )
            .await?;
        }
        Ok(())
    }

    async fn apply_federation_mint(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMintOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let input = mint_input_from_federation_request(&operation.request)?;
        input.validate()?;
        let output_verification =
            self.verify_federation_outputs_with_config(&config, input.outputs())?;
        let outputs_amount = output_verification.amount;
        let blinded_secrets = input
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        let quote_ids = input.quote_ids();
        self.require_prior_applied_mint_quotes(&quote_ids, entry.index)
            .await?;

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quotes = Vec::with_capacity(quote_ids.len());
            for quote_id in &quote_ids {
                quotes.push(
                    tx.get_mint_quote(quote_id)
                        .await?
                        .ok_or(Error::UnknownQuote)?,
                );
            }

            let (payment_method, unit) = validate_federation_mint_quotes(&quotes)?;
            if outputs_amount.unit() != &unit {
                return Err(Error::UnsupportedUnit);
            }
            let expected_amounts =
                expected_federation_mint_amounts(&input, &quotes, outputs_amount.clone())?;
            let total_expected = expected_amounts.iter().try_fold(0_u64, |total, amount| {
                total
                    .checked_add(amount.value())
                    .ok_or(Error::AmountOverflow)
            })?;
            if outputs_amount.value() != total_expected {
                return Err(Error::TransactionUnbalanced(
                    total_expected,
                    outputs_amount.value(),
                    0,
                ));
            }

            validate_federation_mint_quote_signatures(&input, &quotes)?;

            let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
            let signatures_present = existing_signatures
                .iter()
                .filter(|sig| sig.is_some())
                .count();
            if signatures_present != 0 && signatures_present != existing_signatures.len() {
                return Err(Error::Custom(
                    "federation mint replay found partially persisted signatures".to_string(),
                ));
            }
            validate_federation_mint_quote_states_for_signing(
                &quotes,
                &expected_amounts,
                signatures_present,
                existing_signatures.len(),
            )?;

            let max_payment_observation_index = entry.index.saturating_sub(1);
            for (quote, amount_issued) in quotes.iter().zip(&expected_amounts) {
                let required_amount = quote
                    .amount_issued()
                    .checked_add(amount_issued)
                    .map_err(|_| Error::AmountOverflow)?;
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationContext::MintQuote,
                        &quote.id.to_string(),
                    )
                    .await?;
                if !self.federation_mint_payment_quorum_covers_amount_at_or_before(
                    quote,
                    &required_amount,
                    &observations,
                    max_payment_observation_index,
                )? {
                    return Err(Error::UnpaidQuote);
                }
            }

            // Consensus orders completion but does not attest its wall-clock time.
            let completed_at = Some(0);
            let operation_record = if input.is_batch() {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::BatchMint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    completed_at,
                    Some(payment_method.clone()),
                )
            } else {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::Mint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    completed_at,
                    Some(payment_method.clone()),
                )
            };

            let share_set = config.mint_signature_shares(operation_id, &input)?;
            let signature_share_records =
                FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, entry)?;
            tx.add_federation_signature_shares(&signature_share_records)
                .await?;
            let mut issued_notifications = Vec::new();

            if signatures_present == 0 {
                let signatures = federation_share_signatures(share_set);
                let quote_id = match input.is_batch() {
                    true => None,
                    false => quote_ids.first().cloned(),
                };
                tx.add_blinded_messages(quote_id.as_ref(), input.outputs(), &operation_record)
                    .await?;
                tx.add_blind_signatures(&blinded_secrets, &signatures, quote_id)
                    .await?;
                let fee_by_keyset = HashMap::new();
                tx.add_completed_operation(&operation_record, &fee_by_keyset)
                    .await?;
            }

            for (quote, amount_issued) in quotes.iter_mut().zip(expected_amounts) {
                match quote.state() {
                    MintQuoteState::Paid => {
                        quote.add_issuance(amount_issued)?;
                        tx.update_mint_quote(quote).await?;
                        issued_notifications.push((quote.clone(), quote.amount_issued()));
                    }
                    MintQuoteState::Issued => {
                        if quote.amount_issued() < amount_issued {
                            return Err(Error::IssuedQuote);
                        }
                    }
                    MintQuoteState::Unpaid => {
                        return Err(Error::UnpaidQuote);
                    }
                }
            }

            #[cfg(test)]
            if crate::test_helpers::mint::should_fail_for("FEDERATION_MINT_APPLY_BEFORE_STATUS") {
                return Err(Error::Internal);
            }

            Ok(issued_notifications)
        }
        .await;
        let issued_notifications = finish_federation_apply_transaction(tx, entry, outcome).await?;

        for (quote, amount_issued) in issued_notifications {
            self.pubsub_manager.mint_quote_issue(&quote, amount_issued);
        }

        Ok(())
    }

    async fn apply_federation_swap(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationSwapOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let input_proofs = operation.request.inputs();
        if input_proofs.is_empty() {
            return Err(Error::TransactionUnbalanced(
                0,
                operation.request.output_amount()?.to_u64(),
                0,
            ));
        }

        let input_verification = self.verify_federation_inputs_with_config_and_context(
            &config,
            input_proofs,
            FederationProofVerificationContext::SwapApply,
        )?;
        operation.request.verify_spending_conditions()?;
        let output_verification =
            self.verify_federation_outputs_with_config(&config, operation.request.outputs())?;
        self.verify_federation_transaction_balanced_with_config(
            &config,
            input_verification.clone(),
            output_verification.clone(),
            input_proofs,
        )?;

        let fee_breakdown = self.get_federation_proofs_fee_with_config(&config, input_proofs)?;
        let input_ys = input_proofs.ys()?;
        let blinded_secrets = operation
            .request
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        // Consensus orders completion but does not attest its wall-clock time.
        let operation_record = Operation::new(
            federation_operation_uuid(operation_id),
            OperationKind::Swap,
            output_verification.amount.clone().into(),
            input_verification.amount.clone().into(),
            fee_breakdown.total,
            Some(0),
            None,
        );
        let completed_swap = self
            .localstore
            .get_completed_operation(operation_record.id())
            .await?
            .is_some_and(|operation| operation.kind() == OperationKind::Swap);

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut proofs = match tx
                .add_proofs(input_proofs.clone(), None, &operation_record)
                .await
            {
                Ok(proofs) => proofs,
                Err(database::Error::Duplicate | database::Error::AttemptUpdateSpentProof) => {
                    tx.get_proofs(&input_ys).await?
                }
                Err(err) => return Err(err.into()),
            };

            let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
            let signatures_present = existing_signatures
                .iter()
                .filter(|sig| sig.is_some())
                .count();

            match proofs.state {
                State::Unspent if signatures_present == 0 => {}
                State::Unspent if signatures_present == existing_signatures.len() => {
                    return Err(Error::BlindedMessageAlreadySigned)
                }
                State::Unspent => {
                    return Err(Error::Custom(
                        "federation swap found partially persisted signatures for fresh inputs"
                            .to_string(),
                    ))
                }
                State::Spent
                    if signatures_present == existing_signatures.len()
                        && signatures_present > 0
                        && completed_swap =>
                {
                    let share_set =
                        config.swap_signature_shares(operation_id, &operation.request)?;
                    let signature_share_records =
                        FederationSignatureShareRecord::from_accepted_journal_entry(
                            &share_set, entry,
                        )?;
                    tx.add_federation_signature_shares(&signature_share_records)
                        .await?;
                    return Ok(false);
                }
                State::Spent => {
                    return Err(Error::TokenAlreadySpent);
                }
                State::Pending | State::Reserved | State::PendingSpent => {
                    return Err(Error::TokenPending);
                }
            }

            Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent).await?;

            if signatures_present == 0 {
                let share_set = config.swap_signature_shares(operation_id, &operation.request)?;
                let signature_share_records =
                    FederationSignatureShareRecord::from_accepted_journal_entry(&share_set, entry)?;
                let signatures = federation_share_signatures(share_set);
                tx.add_federation_signature_shares(&signature_share_records)
                    .await?;
                tx.add_blinded_messages(None, operation.request.outputs(), &operation_record)
                    .await?;
                tx.add_blind_signatures(&blinded_secrets, &signatures, None)
                    .await?;
                tx.add_completed_operation(&operation_record, &fee_breakdown.per_keyset)
                    .await?;
            }

            #[cfg(test)]
            if crate::test_helpers::mint::should_fail_for("FEDERATION_SWAP_APPLY_BEFORE_STATUS") {
                return Err(Error::Internal);
            }

            Ok(true)
        }
        .await;
        let notify_spent = finish_federation_apply_transaction(tx, entry, outcome).await?;

        if notify_spent {
            for y in input_ys {
                self.pubsub_manager.proof_state((y, State::Spent));
            }
        }

        Ok(())
    }

    async fn apply_federation_melt_quote(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMeltQuoteOperation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&operation.quote)?;
        let quote = operation.into_melt_quote(quote_id)?;
        ensure_melt_quote_matches_request(&quote, &operation.request)?;

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            if let Some(existing) = tx.get_melt_quote(&quote.id).await? {
                ensure_melt_quote_matches_request(&existing, &operation.request)?;
                if !federation_melt_quote_immutable_terms_match(&existing, &quote) {
                    return Err(Error::InvalidPaymentRequest);
                }
            } else {
                tx.add_melt_quote(quote).await?;
            }
            Ok(())
        }
        .await;
        finish_federation_apply_transaction(tx, entry, outcome).await
    }

    async fn apply_federation_melt_quote_payment(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let quote_id = quote_id_from_federation(&observation.quote)?;
        let record = payment_observation_record_from_entry(entry)?;
        let mut quote = self
            .localstore
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        ensure_melt_quote_matches_payment_observation(&quote, observation)?;
        let onchain_receipt = self
            .validate_federation_onchain_melt_payment_observation(&quote, observation)
            .await?;

        let can_finalize = {
            let mut tx = self.localstore.begin_transaction().await?;
            let outcome = async {
                tx.add_federation_payment_observation(&record).await?;
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationContext::MeltQuote,
                        &observation.quote,
                    )
                    .await?;
                self.federation_payment_observation_can_finalize_at_or_before(
                    &record.observation,
                    &observations,
                    record.operation_index,
                )
            }
            .await;
            finish_federation_transaction(tx, entry, "payment_observation", outcome).await?
        };
        if quote.state == cdk_common::MeltQuoteState::Paid
            && observation.state != cdk_common::MeltQuoteState::Paid
        {
            // A terminal paid settlement wins over later stale pending/unpaid
            // observations from other members. The evidence is still persisted
            // above so operators can audit observer lag or disagreement.
        } else if quote.state == cdk_common::MeltQuoteState::Unpaid
            && observation.state == cdk_common::MeltQuoteState::Pending
            && self
                .localstore
                .get_melt_saga_by_quote_id(&quote_id)
                .await?
                .is_none()
        {
            // Failed melt recovery rolls the quote back to unpaid and clears
            // saga state. Late pending observations from other members must not
            // resurrect a melt that no longer has a pending saga to finalize.
        } else if quote.state != observation.state {
            if can_finalize {
                match observation.state {
                    cdk_common::MeltQuoteState::Paid => {
                        self.apply_paid_melt_quote_payment_observation(
                            operation_id,
                            &mut quote,
                            observation,
                            onchain_receipt.as_ref(),
                        )
                        .await?;
                    }
                    cdk_common::MeltQuoteState::Unpaid | cdk_common::MeltQuoteState::Failed => {
                        if let Some(saga) =
                            self.localstore.get_melt_saga_by_quote_id(&quote_id).await?
                        {
                            let payment_response = melt_quote_payment_response(
                                operation_id,
                                &quote,
                                observation,
                                onchain_receipt.as_ref(),
                            )?;
                            super::saga_recovery::process_melt_saga_outcome(
                                &saga,
                                &mut quote,
                                &payment_response,
                                &self.localstore,
                                &self.pubsub_manager,
                                self,
                            )
                            .await?;
                        } else {
                            self.update_melt_quote_payment_observation_state(
                                entry,
                                &quote_id,
                                observation,
                            )
                            .await?;
                        }
                    }
                    cdk_common::MeltQuoteState::Pending | cdk_common::MeltQuoteState::Unknown => {
                        self.update_melt_quote_payment_observation_state(
                            entry,
                            &quote_id,
                            observation,
                        )
                        .await?;
                    }
                }
            }
        } else if can_finalize {
            ensure_melt_quote_payment_proof_matches(&quote, observation)?;
            if observation.state == cdk_common::MeltQuoteState::Paid {
                if let Some(saga) = self.localstore.get_melt_saga_by_quote_id(&quote_id).await? {
                    let melt_operation_id = saga
                        .federation_operation_id
                        .as_deref()
                        .ok_or(Error::Internal)?
                        .parse::<FederationOperationId>()?;
                    self.resume_paid_federation_melt_finalization(
                        &mut quote,
                        &saga,
                        melt_operation_id,
                    )
                    .await?;
                }
            }
        }

        let transaction = self.localstore.begin_transaction().await?;
        finish_federation_apply_transaction(transaction, entry, Ok(())).await
    }

    async fn apply_paid_melt_quote_payment_observation(
        &self,
        operation_id: FederationOperationId,
        quote: &mut MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
        onchain_receipt: Option<&FederationOnchainMeltPaymentReceipt>,
    ) -> Result<(), Error> {
        let saga = self
            .localstore
            .get_melt_saga_by_quote_id(&quote.id)
            .await?
            .ok_or_else(|| FederationError::OperationStateInvalid {
                reason: "federation paid melt observation found no pending melt saga".to_string(),
            })?;
        let payment_response =
            melt_quote_payment_response(operation_id, quote, observation, onchain_receipt)?;
        super::saga_recovery::process_melt_saga_outcome(
            &saga,
            quote,
            &payment_response,
            &self.localstore,
            &self.pubsub_manager,
            self,
        )
        .await
    }

    async fn validate_federation_onchain_melt_payment_observation(
        &self,
        quote: &MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<Option<FederationOnchainMeltPaymentReceipt>, Error> {
        let Some(runtime) = self.federation_config() else {
            return Ok(None);
        };
        if runtime.public_config().frost_wallet.is_none() || !quote.payment_method.is_onchain() {
            return Ok(None);
        }
        if quote.unit != CurrencyUnit::Sat
            || observation.state != cdk_common::MeltQuoteState::Paid
            || observation.payment_id != Some(PaymentIdentifier::QuoteId(quote.id.clone()))
        {
            return Err(FederationError::OperationStateInvalid {
                reason: "federation on-chain melt observation does not match the quote identity"
                    .to_string(),
            }
            .into());
        }
        let receipt = self
            .federation_transaction_state()
            .await?
            .completed_payment_for_quote(&quote.id.to_string())?
            .ok_or_else(|| FederationError::OperationStateInvalid {
                reason:
                    "federation on-chain melt observation has no completed payment in ordered state"
                        .to_string(),
            })?;
        let melt_operation_id = match self.localstore.get_melt_saga_by_quote_id(&quote.id).await? {
            Some(saga) => saga
                .federation_operation_id
                .as_deref()
                .ok_or(Error::Internal)?
                .parse::<FederationOperationId>()?,
            None if quote.state == cdk_common::MeltQuoteState::Paid => receipt.melt_operation_id,
            None => {
                return Err(FederationError::OperationStateInvalid {
                    reason: "federation on-chain melt observation has no accepted melt operation"
                        .to_string(),
                }
                .into());
            }
        };
        validate_federation_onchain_melt_receipt(quote, melt_operation_id, &receipt)?;
        let expected_proof = receipt.outpoint.to_string();
        if observation.payment_proof.as_deref() != Some(expected_proof.as_str()) {
            return Err(FederationError::OperationStateInvalid {
                reason:
                    "federation on-chain melt observation proof does not match the payment outpoint"
                        .to_string(),
            }
            .into());
        }
        Ok(Some(receipt))
    }

    async fn update_melt_quote_payment_observation_state(
        &self,
        entry: &FederationJournalEntry,
        quote_id: &QuoteId,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        if observation.state == cdk_common::MeltQuoteState::Unknown {
            tracing::warn!(
                "Federated melt quote {} observed unknown backend payment status; leaving durable quote state unchanged",
                quote_id
            );
            return Ok(());
        }

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            let mut quote = tx
                .get_melt_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            ensure_melt_quote_matches_payment_observation(&quote, observation)?;
            if quote.state != observation.state {
                tx.update_melt_quote_state(
                    &mut quote,
                    observation.state,
                    observation.payment_proof.clone(),
                )
                .await?;
            } else {
                ensure_melt_quote_payment_proof_matches(&quote, observation)?;
            }
            Ok(())
        }
        .await;
        finish_federation_transaction(tx, entry, "melt_quote_state", outcome).await
    }

    async fn apply_federation_melt(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationMeltOperation,
    ) -> Result<(), Error> {
        let operation_id = entry.operation_id;
        let request = MeltRequest::<QuoteId>::try_from(operation.request.clone())
            .map_err(|_| Error::UnknownQuote)?;
        self.require_prior_applied_melt_quote(request.quote(), entry.index)
            .await?;
        let melt_remains_pending = match self
            .melt_federation(&request, operation_id)
            .await
        {
            Ok(response) => match response.state() {
                cdk_common::MeltQuoteState::Pending => true,
                cdk_common::MeltQuoteState::Paid => false,
                cdk_common::MeltQuoteState::Unpaid
                | cdk_common::MeltQuoteState::Failed
                | cdk_common::MeltQuoteState::Unknown => return Err(Error::Internal),
            },
            Err(Error::PaymentFailed) => true,
            Err(err) => return Err(err),
        };

        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = async {
            let quote = transaction
                .get_melt_quote(request.quote())
                .await?
                .ok_or(Error::UnknownQuote)?;
            if melt_remains_pending {
                let runtime = self.federation_config().ok_or(Error::Internal)?;
                persist_pending_onchain_intent(
                    &mut transaction,
                    runtime,
                    &quote,
                    operation_id,
                    federation_accepted_order(entry),
                )
                .await
            } else if quote.state == cdk_common::MeltQuoteState::Paid {
                Ok(())
            } else {
                Err(Error::Internal)
            }
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    pub(super) async fn verify_federation_inputs_with_context(
        &self,
        inputs: &Proofs,
        context: FederationProofVerificationContext,
    ) -> Result<Verification, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        self.verify_federation_inputs_with_config_and_context(&config, inputs, context)
    }

    #[cfg(test)]
    pub(crate) fn verify_federation_inputs_with_config(
        &self,
        config: &FederationRuntimeConfig,
        inputs: &Proofs,
    ) -> Result<Verification, Error> {
        self.verify_federation_inputs_with_config_inner(config, inputs, None)
    }

    fn verify_federation_inputs_with_config_and_context(
        &self,
        config: &FederationRuntimeConfig,
        inputs: &Proofs,
        context: FederationProofVerificationContext,
    ) -> Result<Verification, Error> {
        self.verify_federation_inputs_with_config_inner(config, inputs, Some(context))
    }

    fn verify_federation_inputs_with_config_inner(
        &self,
        config: &FederationRuntimeConfig,
        inputs: &Proofs,
        context: Option<FederationProofVerificationContext>,
    ) -> Result<Verification, Error> {
        let inputs_count = inputs.len();
        if inputs_count > self.max_inputs {
            tracing::warn!(
                "Federation request exceeds max inputs limit: {} > {}",
                inputs_count,
                self.max_inputs
            );
            return Err(Error::MaxInputsExceeded {
                actual: inputs_count,
                max: self.max_inputs,
            });
        }

        FederationRuntimeConfig::validate_proof_content_bounds(inputs)?;

        Self::check_inputs_unique(inputs)?;
        match context {
            Some(context) => config.verify_proofs_with_performance_context(inputs, context)?,
            None => config.verify_proofs(inputs)?,
        }
        let mut keyset_units = HashSet::new();
        for proof in inputs {
            let keyset = config
                .public_config()
                .keysets
                .iter()
                .find(|keyset| keyset.id == proof.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            let keyset_info = super::keysets::federated_keyset_to_mint_keyset_info(keyset);
            if keyset_info.is_expired() {
                tracing::debug!(
                    "Transaction attempted with expired federated keyset in inputs: {}.",
                    proof.keyset_id
                );
                return Err(Error::ExpiredKeyset);
            }
            keyset_units.insert(keyset.unit.clone());
        }

        if keyset_units.len() != 1 {
            tracing::debug!(
                "Federation transaction attempted with multiple units in inputs: {:?}.",
                keyset_units
            );
            return Err(Error::MultipleUnits);
        }

        let unit = keyset_units.into_iter().next().ok_or(Error::Internal)?;
        if unit == CurrencyUnit::Auth {
            return Err(Error::UnsupportedUnit);
        }

        let amount = inputs.total_amount()?.with_unit(unit);

        Ok(Verification { amount })
    }

    fn verify_federation_transaction_balanced_with_config(
        &self,
        config: &FederationRuntimeConfig,
        input_verification: Verification,
        output_verification: Verification,
        inputs: &Proofs,
    ) -> Result<(), Error> {
        let fee_breakdown = self.get_federation_proofs_fee_with_config(config, inputs)?;

        if output_verification.amount.unit() != input_verification.amount.unit() {
            tracing::debug!(
                "Federation output unit {:?} does not match input unit {:?}",
                output_verification.amount.unit(),
                input_verification.amount.unit()
            );
            return Err(Error::UnitMismatch);
        }

        let fee_typed = fee_breakdown
            .total
            .with_unit(input_verification.amount.unit().clone());
        let expected_output = input_verification.amount.checked_sub(&fee_typed)?;

        if output_verification.amount != expected_output {
            return Err(Error::TransactionUnbalanced(
                input_verification.amount.value(),
                output_verification.amount.value(),
                fee_breakdown.total.to_u64(),
            ));
        }

        Ok(())
    }
}
