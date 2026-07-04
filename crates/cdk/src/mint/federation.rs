//! Federation mode scaffolding for CDK mints.
//!
//! This module is intentionally small at first. Later federation milestones will
//! attach consensus, peer networking, and threshold signing behavior to member
//! mode. Until then, the default standalone mode preserves current mint
//! behavior.

use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::federation::{
    FederationBlindSignatureShareSet, FederationConfig, FederationError, FederationId,
    FederationJournalEntry, FederationMeltOperation, FederationMeltQuoteOperation,
    FederationMeltQuotePaymentObservation, FederationMintOperation, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationMintRequest, FederationOperation,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationPaymentObservationContext, FederationSwapOperation, MemberConfigPrivate, MemberId,
};
use cdk_common::mint::{MeltQuote, MintQuote, Operation, OperationKind};
use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
use cdk_common::{
    Amount, BatchMintRequest, BlindSignature, BlindedMessage, CurrencyUnit, MeltQuoteRequest,
    MeltRequest, MintQuoteRequest, MintQuoteResponse, MintQuoteState, MintRequest, MintResponse,
    PaymentMethod, Proofs, ProofsMethods, PublicKey, QuoteId, RestoreRequest,
    SpendingConditionVerification, State, SwapRequest, SwapResponse,
};
use uuid::Uuid;

use super::verification::{Verification, MAX_PROOF_CONTENT_LEN};
use super::Mint;
use super::MintInput;

/// Defines whether a mint runs as a standalone Cashu mint or as a federation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FederationMode {
    /// Current single-operator mint behavior.
    #[default]
    Standalone,
    /// Placeholder for consensus-backed federation member behavior.
    Member,
}

impl FederationMode {
    /// Returns true when this mode is the federation member placeholder.
    #[must_use]
    pub fn is_member(self) -> bool {
        matches!(self, Self::Member)
    }
}

/// Validated federation config held by a mint member at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationRuntimeConfig {
    public_config: FederationConfig,
    private_config: MemberConfigPrivate,
}

impl FederationRuntimeConfig {
    /// Create a runtime federation config from public and local private config.
    pub fn new(
        public_config: FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Result<Self, Error> {
        public_config.validate()?;
        private_config.validate_against_public(&public_config)?;

        Ok(Self {
            public_config,
            private_config,
        })
    }

    /// Public federation config shared by all members.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    /// Local member private federation config.
    #[must_use]
    pub fn private_config(&self) -> &MemberConfigPrivate {
        &self.private_config
    }

    /// Local member ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.private_config.member_id
    }

    /// Federation roster ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.public_config.federation_id
    }

    /// Wrap a federation operation in this mint member's federation envelope.
    #[must_use]
    pub fn operation_envelope(
        &self,
        operation: FederationOperation,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(self.federation_id(), operation)
    }

    /// Build the consensus operation for creating a mint quote.
    #[must_use]
    pub fn mint_quote_operation<Q>(
        &self,
        quote: Q,
        request: MintQuoteRequest,
        response: MintQuoteResponse<String>,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: quote.to_string(),
                request,
                response,
            },
        ))
    }

    /// Build the consensus operation for observing a mint quote payment state.
    #[must_use]
    pub fn mint_quote_payment_observation<Q>(
        &self,
        quote: Q,
        method: PaymentMethod,
        state: MintQuoteState,
        amount_paid: Amount,
        amount_issued: Amount,
        payment_proof: Option<String>,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: self.member_id(),
                method,
                state,
                amount_paid,
                amount_issued,
                payment_proof,
            },
        ))
    }

    /// Build the consensus operation for creating a melt quote.
    pub fn melt_quote_operation<Q>(
        &self,
        quote: Q,
        request: MeltQuoteRequest,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MeltQuote(
            FederationMeltQuoteOperation {
                quote: quote.to_string(),
                request,
            },
        ))
    }

    /// Build the consensus operation for observing an outgoing melt payment state.
    #[must_use]
    pub fn melt_quote_payment_observation<Q>(
        &self,
        quote: Q,
        method: PaymentMethod,
        state: cdk_common::MeltQuoteState,
        amount: Amount,
        fee_reserve: Amount,
        payment_proof: Option<String>,
        failure_reason: Option<String>,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MeltQuotePayment(
            FederationMeltQuotePaymentObservation {
                quote: quote.to_string(),
                observer: self.member_id(),
                method,
                state,
                amount,
                fee_reserve,
                payment_proof,
                failure_reason,
            },
        ))
    }

    /// Validate outputs against the active v3/BLS aggregate keysets.
    pub fn validate_output_keysets(&self, outputs: &[BlindedMessage]) -> Result<(), Error> {
        Ok(self.public_config.validate_blinded_outputs(outputs)?)
    }

    /// Validate outputs from a swap request.
    pub fn validate_swap_outputs(&self, request: &SwapRequest) -> Result<(), Error> {
        self.validate_output_keysets(request.outputs())
    }

    /// Validate outputs from a restore request.
    pub fn validate_restore_outputs(&self, request: &RestoreRequest) -> Result<(), Error> {
        Ok(self
            .public_config
            .validate_blinded_change_outputs(&request.outputs)?)
    }

    /// Validate change outputs from a melt request.
    pub fn validate_melt_change_outputs(
        &self,
        request: &MeltRequest<QuoteId>,
    ) -> Result<(), Error> {
        match request.outputs() {
            Some(outputs) => Ok(self
                .public_config
                .validate_blinded_change_outputs(outputs)?),
            None => Ok(()),
        }
    }

    /// Verify spent proofs against the federation aggregate BLS keysets.
    pub fn verify_proofs(&self, proofs: &Proofs) -> Result<(), Error> {
        for proof in proofs {
            if proof.keyset_id.get_version() != cdk_common::nuts::KeySetVersion::Version02 {
                return Err(FederationError::InvalidFederatedKeysetVersion {
                    keyset_id: proof.keyset_id,
                }
                .into());
            }

            let keyset = self
                .public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == proof.keyset_id)
                .ok_or(FederationError::UnknownFederatedKeyset {
                    keyset_id: proof.keyset_id,
                })?;
            let aggregate_key = keyset.aggregate_keys.get(&proof.amount).ok_or(
                FederationError::UnknownFederatedKeysetAmount {
                    keyset_id: keyset.id,
                    amount: proof.amount,
                },
            )?;

            if proof.dleq.is_some() {
                return Err(cdk_common::dhke::Error::TokenNotVerified.into());
            }

            cdk_common::dhke::verify_bls_message(
                PublicKey::from(*aggregate_key),
                proof.c,
                proof.secret.as_bytes(),
            )?;
        }

        Ok(())
    }

    /// Build the consensus operation for signing mint outputs.
    pub fn mint_operation(&self, input: MintInput) -> Result<FederationOperationEnvelope, Error> {
        self.validate_output_keysets(input.outputs())?;
        Ok(
            self.operation_envelope(FederationOperation::Mint(FederationMintOperation {
                request: mint_request_from_input(input),
            })),
        )
    }

    /// Sign an accepted mint operation into local threshold BLS signature shares.
    pub fn mint_signature_shares(
        &self,
        operation_id: FederationOperationId,
        input: &MintInput,
    ) -> Result<FederationBlindSignatureShareSet, Error> {
        Ok(self.private_config.sign_blinded_messages(
            &self.public_config,
            operation_id,
            input.outputs(),
        )?)
    }

    /// Convert an accepted mint operation into this member's wallet-facing share response.
    pub fn mint_share_response(
        &self,
        operation_id: FederationOperationId,
        input: &MintInput,
    ) -> Result<MintResponse, Error> {
        let share_set = self.mint_signature_shares(operation_id, input)?;
        Ok(MintResponse {
            signatures: share_set
                .shares
                .into_iter()
                .map(|share| BlindSignature {
                    amount: share.amount,
                    keyset_id: share.keyset_id,
                    c: PublicKey::from(share.share.signature()),
                    dleq: None,
                })
                .collect(),
        })
    }

    /// Build the consensus operation for spending proofs against a melt quote.
    pub fn melt_operation(
        &self,
        request: MeltRequest<QuoteId>,
    ) -> Result<FederationOperationEnvelope, Error> {
        self.validate_melt_change_outputs(&request)?;
        let mut consensus_request = MeltRequest::new(
            request.quote().to_string(),
            request.inputs().clone(),
            request.outputs().clone(),
        )
        .prefer_async(request.is_prefer_async());
        if let Some(fee_index) = request.selected_fee_index() {
            consensus_request = consensus_request.fee_index(fee_index);
        }

        Ok(
            self.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
                request: consensus_request,
            })),
        )
    }

    /// Build the consensus operation for signing swap outputs.
    pub fn swap_operation(
        &self,
        request: SwapRequest,
    ) -> Result<FederationOperationEnvelope, Error> {
        self.validate_swap_outputs(&request)?;
        Ok(
            self.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
                request,
            })),
        )
    }

    /// Sign an accepted swap operation into local threshold BLS signature shares.
    pub fn swap_signature_shares(
        &self,
        operation_id: FederationOperationId,
        request: &SwapRequest,
    ) -> Result<FederationBlindSignatureShareSet, Error> {
        Ok(self.private_config.sign_blinded_messages(
            &self.public_config,
            operation_id,
            request.outputs(),
        )?)
    }

    /// Convert an accepted swap operation into this member's wallet-facing share response.
    pub fn swap_share_response(
        &self,
        operation_id: FederationOperationId,
        request: &SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let share_set = self.swap_signature_shares(operation_id, request)?;
        Ok(SwapResponse {
            signatures: share_set
                .shares
                .into_iter()
                .map(|share| BlindSignature {
                    amount: share.amount,
                    keyset_id: share.keyset_id,
                    c: PublicKey::from(share.share.signature()),
                    dleq: None,
                })
                .collect(),
        })
    }
}

impl Mint {
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
            if entry.status == FederationOperationStatus::Applied {
                continue;
            }

            if self.apply_federation_journal_entry(entry).await? {
                applied.push(entry.operation_id);
            }
        }

        Ok(applied)
    }

    async fn apply_federation_journal_entry(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<bool, Error> {
        match &entry.envelope.operation {
            FederationOperation::MintQuote(operation) => {
                self.apply_federation_mint_quote(entry.operation_id, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::MintQuotePayment(observation) => {
                self.apply_federation_mint_quote_payment(entry.operation_id, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Mint(operation) => {
                self.apply_federation_mint(entry.operation_id, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::MeltQuote(operation) => {
                self.apply_federation_melt_quote(entry.operation_id, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::MeltQuotePayment(observation) => {
                self.apply_federation_melt_quote_payment(entry.operation_id, observation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Swap(operation) => {
                self.apply_federation_swap(entry.operation_id, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Melt(operation) => {
                self.apply_federation_melt(entry.operation_id, operation)
                    .await?;
                Ok(true)
            }
            FederationOperation::Restore(_) => Ok(false),
        }
    }

    async fn apply_federation_mint_quote(
        &self,
        operation_id: FederationOperationId,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        let quote = mint_quote_from_federation_operation(operation)?;
        let mut tx = self.localstore.begin_transaction().await?;

        match tx.get_mint_quote(&quote.id).await? {
            Some(existing) => {
                ensure_mint_quote_matches(&existing, &quote)?;
            }
            None => {
                tx.add_mint_quote(quote).await?;
            }
        }

        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_federation_mint_quote_payment(
        &self,
        operation_id: FederationOperationId,
        observation: &FederationMintQuotePaymentObservation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&observation.quote)?;
        let mut tx = self.localstore.begin_transaction().await?;
        let mut quote = tx
            .get_mint_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if quote.payment_method != observation.method {
            return Err(Error::InvalidPaymentMethod);
        }

        let observed_paid = observation.amount_paid.with_unit(quote.unit.clone());
        if observed_paid < quote.amount_paid() {
            return Err(Error::Custom(
                "federation mint quote payment observation would reduce amount_paid".to_string(),
            ));
        }

        let additional_amount = observed_paid
            .checked_sub(&quote.amount_paid())
            .map_err(|_| Error::AmountOverflow)?;
        if additional_amount.value() > 0 {
            let config = self.federation_config().ok_or(Error::Internal)?;
            config
                .public_config()
                .ensure_single_payment_observation_can_finalize(
                    FederationPaymentObservationContext::MintQuote,
                )?;

            let payment_id = observation.payment_proof.clone().unwrap_or_else(|| {
                format!(
                    "federation:{}:{}",
                    operation_id,
                    mint_quote_state_label(observation.state)
                )
            });
            quote.add_payment(additional_amount, payment_id, None)?;
            tx.update_mint_quote(&mut quote).await?;
        }

        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_federation_mint(
        &self,
        operation_id: FederationOperationId,
        operation: &FederationMintOperation,
    ) -> Result<(), Error> {
        let config = self.federation_config().ok_or(Error::Internal)?;
        let input = mint_input_from_federation_request(&operation.request)?;
        input.validate()?;
        config.validate_output_keysets(input.outputs())?;

        let output_verification = self.verify_outputs(input.outputs())?;
        let outputs_amount = output_verification.amount;
        let blinded_secrets = input
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        let signatures =
            federation_share_signatures(config.mint_signature_shares(operation_id, &input)?);

        let mut tx = self.localstore.begin_transaction().await?;
        let quote_ids = input.quote_ids();
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

        let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
        let signatures_present = existing_signatures
            .iter()
            .filter(|sig| sig.is_some())
            .count();
        if signatures_present != 0 && signatures_present != existing_signatures.len() {
            tx.rollback().await?;
            return Err(Error::Custom(
                "federation mint replay found partially persisted signatures".to_string(),
            ));
        }

        let operation_record = if input.is_batch() {
            Operation::new(
                federation_operation_uuid(operation_id),
                OperationKind::BatchMint,
                outputs_amount.clone().into(),
                Amount::ZERO,
                Amount::ZERO,
                None,
                Some(payment_method.clone()),
            )
        } else {
            Operation::new(
                federation_operation_uuid(operation_id),
                OperationKind::Mint,
                outputs_amount.clone().into(),
                Amount::ZERO,
                Amount::ZERO,
                None,
                Some(payment_method.clone()),
            )
        };

        if signatures_present == 0 {
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

        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_federation_swap(
        &self,
        operation_id: FederationOperationId,
        operation: &FederationSwapOperation,
    ) -> Result<(), Error> {
        let config = self.federation_config().ok_or(Error::Internal)?;
        config.validate_swap_outputs(&operation.request)?;
        let input_proofs = operation.request.inputs();
        if input_proofs.is_empty() {
            return Err(Error::TransactionUnbalanced(
                0,
                operation.request.output_amount()?.to_u64(),
                0,
            ));
        }

        let input_verification = self.verify_federation_inputs(input_proofs).await?;
        operation.request.verify_spending_conditions()?;
        let output_verification = self.verify_outputs(operation.request.outputs())?;
        self.verify_transaction_balanced(
            input_verification.clone(),
            output_verification.clone(),
            input_proofs,
        )
        .await?;

        let fee_breakdown = self.get_proofs_fee(input_proofs).await?;
        let signatures = federation_share_signatures(
            config.swap_signature_shares(operation_id, &operation.request)?,
        );
        let input_ys = input_proofs.ys()?;
        let blinded_secrets = operation
            .request
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();
        let operation_record = Operation::new(
            federation_operation_uuid(operation_id),
            OperationKind::Swap,
            output_verification.amount.clone().into(),
            input_verification.amount.clone().into(),
            fee_breakdown.total,
            None,
            None,
        );

        let mut tx = self.localstore.begin_transaction().await?;
        let mut proofs = match tx
            .add_proofs(input_proofs.clone(), None, &operation_record)
            .await
        {
            Ok(proofs) => proofs,
            Err(database::Error::Duplicate | database::Error::AttemptUpdateSpentProof) => {
                tx.get_proofs(&input_ys).await?
            }
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        };

        if proofs.state != State::Spent {
            Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent).await?;
        }

        let existing_signatures = tx.get_blind_signatures(&blinded_secrets).await?;
        let signatures_present = existing_signatures
            .iter()
            .filter(|sig| sig.is_some())
            .count();
        if signatures_present != 0 && signatures_present != existing_signatures.len() {
            tx.rollback().await?;
            return Err(Error::Custom(
                "federation swap replay found partially persisted signatures".to_string(),
            ));
        }

        if signatures_present == 0 {
            tx.add_blinded_messages(None, operation.request.outputs(), &operation_record)
                .await?;
            tx.add_blind_signatures(&blinded_secrets, &signatures, None)
                .await?;
            tx.add_completed_operation(&operation_record, &fee_breakdown.per_keyset)
                .await?;
        }

        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;

        for y in input_ys {
            self.pubsub_manager.proof_state((y, State::Spent));
        }

        Ok(())
    }

    async fn apply_federation_melt_quote(
        &self,
        operation_id: FederationOperationId,
        operation: &FederationMeltQuoteOperation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&operation.quote)?;
        if let Some(existing) = self.localstore.get_melt_quote(&quote_id).await? {
            ensure_melt_quote_matches_request(&existing, &operation.request)?;
        } else {
            self.get_federation_melt_quote(quote_id, operation.request.clone())
                .await?;
        }

        let mut tx = self.localstore.begin_transaction().await?;
        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_federation_melt_quote_payment(
        &self,
        operation_id: FederationOperationId,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&observation.quote)?;
        let mut quote = self
            .localstore
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        ensure_melt_quote_matches_payment_observation(&quote, observation)?;

        if quote.state != observation.state {
            let config = self.federation_config().ok_or(Error::Internal)?;
            config
                .public_config()
                .ensure_single_payment_observation_can_finalize(
                    FederationPaymentObservationContext::MeltQuote,
                )?;

            match observation.state {
                cdk_common::MeltQuoteState::Paid => {
                    self.apply_paid_melt_quote_payment_observation(
                        operation_id,
                        &mut quote,
                        observation,
                    )
                    .await?;
                }
                cdk_common::MeltQuoteState::Unpaid | cdk_common::MeltQuoteState::Failed => {
                    if let Some(saga) = self.localstore.get_melt_saga_by_quote_id(&quote_id).await?
                    {
                        let payment_response =
                            melt_quote_payment_response(operation_id, &quote, observation)?;
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
                        self.update_melt_quote_payment_observation_state(&quote_id, observation)
                            .await?;
                    }
                }
                _ => {
                    self.update_melt_quote_payment_observation_state(&quote_id, observation)
                        .await?;
                }
            }
        } else {
            ensure_melt_quote_payment_proof_matches(&quote, observation)?;
        }

        let mut tx = self.localstore.begin_transaction().await?;
        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_paid_melt_quote_payment_observation(
        &self,
        operation_id: FederationOperationId,
        quote: &mut MeltQuote,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let saga = self
            .localstore
            .get_melt_saga_by_quote_id(&quote.id)
            .await?
            .ok_or_else(|| {
                Error::Custom(
                    "federation paid melt observation found no pending melt saga".to_string(),
                )
            })?;
        let payment_response = melt_quote_payment_response(operation_id, quote, observation)?;
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

    async fn update_melt_quote_payment_observation_state(
        &self,
        quote_id: &QuoteId,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let mut tx = self.localstore.begin_transaction().await?;
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
        tx.commit().await?;
        Ok(())
    }

    async fn apply_federation_melt(
        &self,
        operation_id: FederationOperationId,
        operation: &FederationMeltOperation,
    ) -> Result<(), Error> {
        let request = MeltRequest::<QuoteId>::try_from(operation.request.clone())
            .map_err(|_| Error::UnknownQuote)?;

        self.melt_federation(&request, operation_id).await?;

        let mut tx = self.localstore.begin_transaction().await?;
        tx.update_federation_operation_status(operation_id, FederationOperationStatus::Applied)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Verify federated melt or swap inputs against aggregate BLS keysets.
    pub(crate) async fn verify_federation_inputs(
        &self,
        inputs: &Proofs,
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

        for proof in inputs {
            let secret_len = proof.secret.len();
            if secret_len > MAX_PROOF_CONTENT_LEN {
                tracing::warn!(
                    "Proof secret exceeds max content length: {} > {}",
                    secret_len,
                    MAX_PROOF_CONTENT_LEN
                );
                return Err(Error::ProofContentTooLarge {
                    actual: secret_len,
                    max: MAX_PROOF_CONTENT_LEN,
                });
            }

            if let Some(witness) = &proof.witness {
                let witness_str = serde_json::to_string(witness)?;
                let witness_len = witness_str.len();
                if witness_len > MAX_PROOF_CONTENT_LEN {
                    tracing::warn!(
                        "Proof witness exceeds max content length: {} > {}",
                        witness_len,
                        MAX_PROOF_CONTENT_LEN
                    );
                    return Err(Error::ProofContentTooLarge {
                        actual: witness_len,
                        max: MAX_PROOF_CONTENT_LEN,
                    });
                }
            }
        }

        Self::check_inputs_unique(inputs)?;
        let unit = self.verify_inputs_keyset(inputs).await?;
        if unit == CurrencyUnit::Auth {
            return Err(Error::UnsupportedUnit);
        }

        let amount = inputs.total_amount()?.with_unit(unit);
        self.federation_config()
            .ok_or(Error::Internal)?
            .verify_proofs(inputs)?;

        Ok(Verification { amount })
    }
}

fn federation_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn mint_request_from_input(input: MintInput) -> FederationMintRequest {
    match input {
        MintInput::Single(request) => FederationMintRequest::Single(MintRequest {
            quote: request.quote.to_string(),
            outputs: request.outputs,
            signature: request.signature,
        }),
        MintInput::Batch(request) => FederationMintRequest::Batch(BatchMintRequest {
            quotes: request
                .quotes
                .into_iter()
                .map(|quote| quote.to_string())
                .collect(),
            quote_amounts: request.quote_amounts,
            outputs: request.outputs,
            signatures: request.signatures,
        }),
    }
}

fn mint_input_from_federation_request(request: &FederationMintRequest) -> Result<MintInput, Error> {
    match request {
        FederationMintRequest::Single(request) => Ok(MintInput::Single(MintRequest {
            quote: quote_id_from_federation(&request.quote)?,
            outputs: request.outputs.clone(),
            signature: request.signature.clone(),
        })),
        FederationMintRequest::Batch(request) => Ok(MintInput::Batch(BatchMintRequest {
            quotes: request
                .quotes
                .iter()
                .map(|quote| quote_id_from_federation(quote))
                .collect::<Result<Vec<_>, _>>()?,
            quote_amounts: request.quote_amounts.clone(),
            outputs: request.outputs.clone(),
            signatures: request.signatures.clone(),
        })),
    }
}

fn quote_id_from_federation(quote: &str) -> Result<QuoteId, Error> {
    Ok(QuoteId::from_str(quote)?)
}

fn mint_quote_from_federation_operation(
    operation: &FederationMintQuoteOperation,
) -> Result<MintQuote, Error> {
    let quote_id = quote_id_from_federation(&operation.quote)?;
    if operation.response.quote() != &operation.quote {
        return Err(Error::UnknownQuote);
    }

    let method = operation.request.payment_method();
    let unit = operation.request.unit();
    let amount = operation
        .request
        .amount()
        .map(|amount| amount.with_unit(unit.clone()));
    let (amount_paid, amount_issued, extra_json) =
        mint_quote_response_amounts(&operation.response, &unit)?;

    Ok(MintQuote::new(
        Some(quote_id.clone()),
        operation.response.request().to_string(),
        unit.clone(),
        amount,
        operation.response.expiry().unwrap_or_default(),
        PaymentIdentifier::QuoteId(quote_id),
        operation.request.pubkey(),
        amount_paid,
        amount_issued,
        method,
        cdk_common::util::unix_time(),
        Vec::new(),
        Vec::new(),
        extra_json,
    ))
}

fn mint_quote_response_amounts(
    response: &MintQuoteResponse<String>,
    unit: &CurrencyUnit,
) -> Result<
    (
        Amount<CurrencyUnit>,
        Amount<CurrencyUnit>,
        Option<serde_json::Value>,
    ),
    Error,
> {
    let zero = Amount::new(0, unit.clone());
    match response {
        MintQuoteResponse::Bolt11(response) => {
            let amount = response
                .amount
                .clone()
                .map(|amount| amount.with_unit(unit.clone()))
                .unwrap_or_else(|| zero.clone());
            let (amount_paid, amount_issued) = match response.state {
                MintQuoteState::Unpaid => (zero.clone(), zero),
                MintQuoteState::Paid => (amount, zero),
                MintQuoteState::Issued => (amount.clone(), amount),
            };
            Ok((amount_paid, amount_issued, None))
        }
        MintQuoteResponse::Bolt12(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
        MintQuoteResponse::Onchain(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
        MintQuoteResponse::Custom { response, .. } => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            Some(response.extra.clone()),
        )),
    }
}

fn ensure_mint_quote_matches(existing: &MintQuote, expected: &MintQuote) -> Result<(), Error> {
    if existing.id != expected.id
        || existing.unit != expected.unit
        || existing.request != expected.request
        || existing.amount != expected.amount
        || existing.expiry != expected.expiry
        || existing.pubkey != expected.pubkey
        || existing.payment_method != expected.payment_method
    {
        return Err(Error::Custom(
            "federation mint quote replay found mismatched local quote".to_string(),
        ));
    }

    Ok(())
}

fn ensure_melt_quote_matches_request(
    existing: &MeltQuote,
    request: &MeltQuoteRequest,
) -> Result<(), Error> {
    if existing.payment_method != request.method()
        || existing.request.to_string() != melt_quote_request_string(request)
    {
        return Err(Error::Custom(
            "federation melt quote replay found mismatched local quote".to_string(),
        ));
    }

    Ok(())
}

fn ensure_melt_quote_matches_payment_observation(
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
) -> Result<(), Error> {
    if quote.payment_method != observation.method {
        return Err(Error::InvalidPaymentMethod);
    }

    if quote.amount() != observation.amount.with_unit(quote.unit.clone())
        || quote.fee_reserve() != observation.fee_reserve.with_unit(quote.unit.clone())
    {
        return Err(Error::Custom(
            "federation melt payment observation does not match local quote amounts".to_string(),
        ));
    }

    Ok(())
}

fn ensure_melt_quote_payment_proof_matches(
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
) -> Result<(), Error> {
    if observation.state == cdk_common::MeltQuoteState::Paid
        && observation.payment_proof.is_some()
        && quote.payment_proof != observation.payment_proof
    {
        return Err(Error::Custom(
            "federation melt payment observation found mismatched payment proof".to_string(),
        ));
    }

    Ok(())
}

fn melt_quote_payment_response(
    operation_id: FederationOperationId,
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
) -> Result<MakePaymentResponse, Error> {
    let payment_lookup_id = quote.request_lookup_id.clone().unwrap_or_else(|| {
        PaymentIdentifier::CustomId(format!("federation-melt-observation-{operation_id}"))
    });
    let amount = observation.amount.with_unit(quote.unit.clone());
    let fee_reserve = observation.fee_reserve.with_unit(quote.unit.clone());
    let total_spent = amount
        .checked_add(&fee_reserve)
        .map_err(|_| Error::AmountOverflow)?;

    Ok(MakePaymentResponse {
        payment_lookup_id,
        payment_proof: observation.payment_proof.clone(),
        status: observation.state,
        total_spent,
    })
}

fn melt_quote_request_string(request: &MeltQuoteRequest) -> String {
    match request {
        MeltQuoteRequest::Bolt11(request) => request.request.to_string(),
        MeltQuoteRequest::Bolt12(request) => request.request.clone(),
        MeltQuoteRequest::Onchain(request) => request.request.clone(),
        MeltQuoteRequest::Custom(request) => request.request.clone(),
    }
}

fn federation_share_signatures(share_set: FederationBlindSignatureShareSet) -> Vec<BlindSignature> {
    share_set
        .shares
        .into_iter()
        .map(|share| BlindSignature {
            amount: share.amount,
            keyset_id: share.keyset_id,
            c: PublicKey::from(share.share.signature()),
            dleq: None,
        })
        .collect()
}

fn validate_federation_mint_quotes(
    quotes: &[database::mint::Acquired<MintQuote>],
) -> Result<(PaymentMethod, CurrencyUnit), Error> {
    let first = quotes.first().ok_or(Error::UnknownQuote)?;
    let payment_method = first.payment_method.clone();
    let unit = first.unit.clone();

    for quote in quotes {
        if quote.payment_method != payment_method {
            return Err(Error::InvalidPaymentMethod);
        }
        if quote.unit != unit {
            return Err(Error::UnitMismatch);
        }
    }

    Ok((payment_method, unit))
}

fn expected_federation_mint_amounts(
    input: &MintInput,
    quotes: &[database::mint::Acquired<MintQuote>],
    outputs_amount: Amount<CurrencyUnit>,
) -> Result<Vec<Amount<CurrencyUnit>>, Error> {
    match input {
        MintInput::Single(_) => Ok(vec![outputs_amount]),
        MintInput::Batch(request) => quotes
            .iter()
            .enumerate()
            .map(|(index, quote)| {
                if quote.state() == MintQuoteState::Unpaid {
                    return Err(Error::UnpaidQuote);
                }
                if quote.state() == MintQuoteState::Issued {
                    return Ok(quote.amount_issued());
                }

                match &request.quote_amounts {
                    Some(amounts) => amounts
                        .get(index)
                        .cloned()
                        .map(|amount| amount.with_unit(quote.unit.clone()))
                        .ok_or(Error::TransactionUnbalanced(0, 0, 0)),
                    None => Ok(quote.amount_mintable()),
                }
            })
            .collect(),
    }
}

fn mint_quote_state_label(state: MintQuoteState) -> &'static str {
    match state {
        MintQuoteState::Unpaid => "unpaid",
        MintQuoteState::Paid => "paid",
        MintQuoteState::Issued => "issued",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationError, FederationMintRequest,
        FederationOperation, InMemoryFederationConsensus, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut00::KnownMethod;
    use cdk_common::nuts::nut01::{verify_blind_signature_share, BlsG1PublicKey, BlsSecretKey};
    use cdk_common::quote_id::QuoteId;
    use cdk_common::secret::Secret;
    use cdk_common::{
        Amount, BatchMintRequest, BlindedMessage, CurrencyUnit, Id, MeltRequest,
        MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse,
        MintQuoteState, MintRequest, PaymentMethod, Proof, PublicKey, RestoreRequest, SecretKey,
        SwapRequest,
    };

    use super::{FederationMode, FederationRuntimeConfig};

    fn runtime_config() -> FederationRuntimeConfig {
        let member = TrustedDealerMember::new(
            cdk_common::federation::MemberId::new(1).expect("member id"),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com").expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        );
        let keyset = TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(
                Amount::from(1),
                vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
            )]),
        );
        let bundle = generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member],
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        let private_config = bundle
            .private_configs
            .get(&cdk_common::federation::MemberId::new(1).expect("member id"))
            .expect("private config")
            .clone();

        FederationRuntimeConfig::new(bundle.public_config.clone(), private_config)
            .expect("runtime config")
    }

    fn output_for_runtime(
        runtime_config: &FederationRuntimeConfig,
        label: &[u8],
    ) -> BlindedMessage {
        let keyset = &runtime_config.public_config().keysets[0];
        BlindedMessage::new(
            Amount::from(1),
            keyset.id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
        )
    }

    fn proof_for_runtime(runtime_config: &FederationRuntimeConfig, label: &str) -> Proof {
        let keyset = &runtime_config.public_config().keysets[0];
        let secret = Secret::new(format!("federated-proof-{label}"));
        let aggregate_secret = BlsSecretKey::from_reduced_bytes(&[2; 32]);
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(&aggregate_secret);

        Proof::new(
            Amount::from(1),
            keyset.id,
            secret,
            PublicKey::from(signature),
        )
    }

    fn non_v3_output_for_runtime(runtime_config: &FederationRuntimeConfig) -> BlindedMessage {
        let keyset = &runtime_config.public_config().keysets[0];
        let non_v3_keyset_id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        BlindedMessage::new(
            Amount::from(1),
            non_v3_keyset_id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(b"non-v3 mint output")),
        )
    }

    fn bolt11_quote_request(amount: u64) -> MintQuoteRequest {
        MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(amount),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
    }

    fn bolt11_quote_response(quote: &QuoteId, amount: u64) -> MintQuoteResponse<String> {
        MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote.to_string(),
            request: format!("invoice-{quote}"),
            amount: Some(Amount::from(amount)),
            unit: Some(CurrencyUnit::Sat),
            state: MintQuoteState::Unpaid,
            expiry: None,
            pubkey: None,
        })
    }

    #[test]
    fn federation_mode_defaults_to_standalone() {
        assert_eq!(FederationMode::default(), FederationMode::Standalone);
        assert!(!FederationMode::default().is_member());
        assert!(FederationMode::Member.is_member());
    }

    #[test]
    fn runtime_config_validates_public_and_private_configs() {
        let runtime_config = runtime_config();

        assert_eq!(
            runtime_config.federation_id(),
            runtime_config.public_config().federation_id
        );
        assert_eq!(runtime_config.member_id().get(), 1);
    }

    #[test]
    fn runtime_config_builds_mint_quote_consensus_operations() {
        let runtime_config = runtime_config();
        let quote = QuoteId::new();
        let envelope = runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(10),
            bolt11_quote_response(&quote, 10),
        );

        assert_eq!(envelope.federation_id, runtime_config.federation_id());
        let quote_string = quote.to_string();
        assert_eq!(envelope.operation.quote(), Some(quote_string.as_str()));
        assert!(matches!(
            envelope.operation,
            FederationOperation::MintQuote(_)
        ));

        let mut consensus = InMemoryFederationConsensus::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
        );
        let first_response = consensus
            .submit_envelope(envelope.clone())
            .expect("submit quote");
        let duplicate_response = consensus
            .submit_envelope(envelope)
            .expect("duplicate quote");
        let conflict = runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(11),
            bolt11_quote_response(&quote, 11),
        );

        assert_eq!(first_response.operation_id, duplicate_response.operation_id);
        assert!(matches!(
            consensus.submit_envelope(conflict),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn runtime_config_builds_mint_payment_observation_operations() {
        let runtime_config = runtime_config();
        let quote = QuoteId::new();
        let paid = runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(10),
            Amount::ZERO,
            Some("settled".to_string()),
        );

        assert_eq!(paid.federation_id, runtime_config.federation_id());
        assert!(matches!(
            paid.operation,
            FederationOperation::MintQuotePayment(_)
        ));

        let mut consensus = InMemoryFederationConsensus::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
        );
        consensus.submit_envelope(paid).expect("submit paid");
        let conflicting_paid = runtime_config.mint_quote_payment_observation(
            quote,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(10),
            Amount::ZERO,
            Some("different-settlement".to_string()),
        );

        assert!(matches!(
            consensus.submit_envelope(conflicting_paid),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn runtime_config_builds_single_and_batch_mint_operations() {
        let runtime_config = runtime_config();
        let quote = QuoteId::new();
        let single = runtime_config
            .mint_operation(super::MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output_for_runtime(&runtime_config, b"single mint")],
                signature: None,
            }))
            .expect("single mint operation");

        match single.operation {
            FederationOperation::Mint(operation) => match operation.request {
                FederationMintRequest::Single(request) => {
                    assert_eq!(request.quote, quote.to_string());
                }
                FederationMintRequest::Batch(_) => panic!("expected single mint request"),
            },
            _ => panic!("expected mint operation"),
        }

        let quote_one = QuoteId::new();
        let quote_two = QuoteId::new();
        let quote_three = QuoteId::new();
        let batch = runtime_config
            .mint_operation(super::MintInput::Batch(BatchMintRequest {
                quotes: vec![quote_one.clone(), quote_two.clone()],
                quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
                outputs: vec![output_for_runtime(&runtime_config, b"batch mint")],
                signatures: None,
            }))
            .expect("batch mint operation");

        assert_eq!(batch.operation.conflict_keys().len(), 2);
        let mut consensus = InMemoryFederationConsensus::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
        );
        consensus.submit_envelope(batch).expect("submit batch mint");

        let overlapping_batch = runtime_config
            .mint_operation(super::MintInput::Batch(BatchMintRequest {
                quotes: vec![quote_two, quote_three],
                quote_amounts: Some(vec![Amount::from(1), Amount::from(1)]),
                outputs: vec![output_for_runtime(
                    &runtime_config,
                    b"overlapping batch mint",
                )],
                signatures: None,
            }))
            .expect("overlapping batch mint operation");

        assert!(matches!(
            consensus.submit_envelope(overlapping_batch),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn runtime_config_rejects_sliding_window_mint_output_attack() {
        let runtime_config = runtime_config();
        let quote = QuoteId::new();
        let output_a = output_for_runtime(&runtime_config, b"sliding-window-output-a");
        let output_b = output_for_runtime(&runtime_config, b"sliding-window-output-b");
        let output_c = output_for_runtime(&runtime_config, b"sliding-window-output-c");
        let output_d = output_for_runtime(&runtime_config, b"sliding-window-output-d");

        let envelope_for_outputs = |outputs| {
            runtime_config
                .mint_operation(super::MintInput::Single(MintRequest {
                    quote: quote.clone(),
                    outputs,
                    signature: None,
                }))
                .expect("mint operation")
        };

        let first =
            envelope_for_outputs(vec![output_a.clone(), output_b.clone(), output_c.clone()]);
        let overlapping_one =
            envelope_for_outputs(vec![output_b.clone(), output_c.clone(), output_d.clone()]);
        let overlapping_two =
            envelope_for_outputs(vec![output_c.clone(), output_d.clone(), output_a.clone()]);
        let overlapping_three = envelope_for_outputs(vec![output_d, output_a, output_b]);

        let first_operation_id = first.operation_id().expect("first operation id");
        let attempted_operation_ids = [
            overlapping_one
                .operation_id()
                .expect("overlap one operation id"),
            overlapping_two
                .operation_id()
                .expect("overlap two operation id"),
            overlapping_three
                .operation_id()
                .expect("overlap three operation id"),
        ];
        for attempted_operation_id in attempted_operation_ids {
            assert_ne!(first_operation_id, attempted_operation_id);
        }

        let mut consensus = InMemoryFederationConsensus::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
        );
        let first_response = consensus
            .submit_envelope(first.clone())
            .expect("submit first sliding-window mint");
        let duplicate_response = consensus
            .submit_envelope(first)
            .expect("duplicate sliding-window mint");

        assert_eq!(first_response.operation_id, first_operation_id);
        assert_eq!(duplicate_response.operation_id, first_operation_id);

        for envelope in [overlapping_one, overlapping_two, overlapping_three] {
            assert!(matches!(
                consensus.submit_envelope(envelope),
                Err(FederationError::ConflictingOperation { .. })
            ));
        }

        let entries = consensus.journal().entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operation_id, first_operation_id);
        assert_eq!(entries[0].envelope.operation.conflict_keys().len(), 1);
    }

    #[test]
    fn runtime_config_rejects_non_v3_mint_outputs_before_consensus() {
        let runtime_config = runtime_config();
        let input = super::MintInput::Single(MintRequest {
            quote: QuoteId::new(),
            outputs: vec![non_v3_output_for_runtime(&runtime_config)],
            signature: None,
        });

        assert!(matches!(
            runtime_config.mint_operation(input),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[test]
    fn runtime_config_rejects_non_v3_swap_restore_and_change_outputs() {
        let runtime_config = runtime_config();
        let output = non_v3_output_for_runtime(&runtime_config);
        let swap = SwapRequest::new(Vec::new(), vec![output.clone()]);
        let restore = RestoreRequest {
            outputs: vec![output.clone()],
        };
        let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output]));

        assert!(matches!(
            runtime_config.validate_swap_outputs(&swap),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
        assert!(matches!(
            runtime_config.validate_restore_outputs(&restore),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
        assert!(matches!(
            runtime_config.validate_melt_change_outputs(&melt),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
        assert!(matches!(
            runtime_config.melt_operation(melt),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[test]
    fn runtime_config_verifies_aggregate_bls_proofs() {
        let runtime_config = runtime_config();
        let proof = proof_for_runtime(&runtime_config, "valid");

        runtime_config
            .verify_proofs(&vec![proof])
            .expect("aggregate BLS proof should verify");
    }

    #[test]
    fn runtime_config_rejects_invalid_aggregate_bls_proofs() {
        let runtime_config = runtime_config();
        let mut proof = proof_for_runtime(&runtime_config, "invalid");
        proof.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(b"not a valid signature"));

        assert!(runtime_config.verify_proofs(&vec![proof]).is_err());
    }

    #[test]
    fn runtime_config_rejects_non_v3_aggregate_proofs() {
        let runtime_config = runtime_config();
        let mut proof = proof_for_runtime(&runtime_config, "non-v3");
        let keyset = &runtime_config.public_config().keysets[0];
        proof.keyset_id = Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );

        assert!(matches!(
            runtime_config.verify_proofs(&vec![proof]),
            Err(cdk_common::Error::Federation(
                FederationError::InvalidFederatedKeysetVersion { .. }
            ))
        ));
    }

    #[test]
    fn runtime_config_builds_swap_operations() {
        let runtime_config = runtime_config();
        let swap = SwapRequest::new(
            Vec::new(),
            vec![output_for_runtime(&runtime_config, b"swap operation")],
        );
        let envelope = runtime_config.swap_operation(swap).expect("swap operation");

        assert_eq!(envelope.federation_id, runtime_config.federation_id());
        assert!(matches!(envelope.operation, FederationOperation::Swap(_)));
        assert_eq!(envelope.operation.conflict_keys().len(), 0);
    }

    #[test]
    fn runtime_config_builds_melt_operations() {
        let runtime_config = runtime_config();
        let quote = QuoteId::new();
        let mut change_output = output_for_runtime(&runtime_config, b"melt operation change");
        change_output.amount = Amount::ZERO;
        let melt = MeltRequest::new(quote.clone(), Vec::new(), Some(vec![change_output]))
            .prefer_async(true)
            .fee_index(42);
        let envelope = runtime_config.melt_operation(melt).expect("melt operation");

        assert_eq!(envelope.federation_id, runtime_config.federation_id());
        assert_eq!(envelope.operation.conflict_keys().len(), 1);
        match envelope.operation {
            FederationOperation::Melt(operation) => {
                assert_eq!(operation.request.quote(), &quote.to_string());
                assert!(operation.request.is_prefer_async());
                assert_eq!(operation.request.selected_fee_index(), Some(42));
            }
            _ => panic!("expected melt operation"),
        }
    }

    #[test]
    fn runtime_config_accepts_zero_amount_melt_change_placeholders() {
        let runtime_config = runtime_config();
        let mut output = output_for_runtime(&runtime_config, b"zero amount change placeholder");
        output.amount = Amount::ZERO;
        let melt = MeltRequest::new(QuoteId::new(), Vec::new(), Some(vec![output.clone()]));
        let restore = RestoreRequest {
            outputs: vec![output.clone()],
        };

        assert!(runtime_config.validate_melt_change_outputs(&melt).is_ok());
        assert!(runtime_config.validate_restore_outputs(&restore).is_ok());
        assert!(matches!(
            runtime_config.validate_output_keysets(&[output]),
            Err(cdk_common::Error::Federation(
                FederationError::UnknownFederatedKeysetAmount { .. }
            ))
        ));
    }

    #[test]
    fn runtime_config_signs_accepted_mint_operations() {
        let runtime_config = runtime_config();
        let keyset = &runtime_config.public_config().keysets[0];
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk axum federation mint share");
        let input = super::MintInput::Single(MintRequest {
            quote: QuoteId::new(),
            outputs: vec![BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(blinded_message),
            )],
            signature: None,
        });
        let operation_id = runtime_config
            .mint_operation(input.clone())
            .expect("mint operation")
            .operation_id()
            .expect("operation id");

        let share_set = runtime_config
            .mint_signature_shares(operation_id, &input)
            .expect("signature shares");
        let share = share_set.shares.first().expect("signature share");
        let public_share = keyset
            .public_shares
            .get(&runtime_config.member_id())
            .expect("member shares")
            .get(&Amount::from(1))
            .expect("amount share");

        assert_eq!(share_set.operation_id, operation_id);
        assert_eq!(share_set.member_id, runtime_config.member_id());
        assert_eq!(share.operation_id, operation_id);
        assert!(
            verify_blind_signature_share(&share.share, &blinded_message, public_share)
                .expect("share verification")
        );

        let response = runtime_config
            .mint_share_response(operation_id, &input)
            .expect("mint share response");
        let signature = response.signatures.first().expect("signature share");

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, keyset.id);
        assert!(signature.dleq.is_none());
        assert_eq!(
            signature.c.as_bls_g1().expect("response share point"),
            share.share.signature()
        );
    }

    #[test]
    fn runtime_config_signs_accepted_swap_operations() {
        let runtime_config = runtime_config();
        let keyset = &runtime_config.public_config().keysets[0];
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"cdk federation swap share");
        let swap = SwapRequest::new(
            Vec::new(),
            vec![BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(blinded_message),
            )],
        );
        let operation_id = runtime_config
            .swap_operation(swap.clone())
            .expect("swap operation")
            .operation_id()
            .expect("operation id");

        let share_set = runtime_config
            .swap_signature_shares(operation_id, &swap)
            .expect("signature shares");
        let share = share_set.shares.first().expect("signature share");
        let public_share = keyset
            .public_shares
            .get(&runtime_config.member_id())
            .expect("member shares")
            .get(&Amount::from(1))
            .expect("amount share");

        assert_eq!(share_set.operation_id, operation_id);
        assert_eq!(share_set.member_id, runtime_config.member_id());
        assert_eq!(share.operation_id, operation_id);
        assert!(
            verify_blind_signature_share(&share.share, &blinded_message, public_share)
                .expect("share verification")
        );

        let response = runtime_config
            .swap_share_response(operation_id, &swap)
            .expect("swap share response");
        let signature = response.signatures.first().expect("signature share");

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, keyset.id);
        assert!(signature.dleq.is_none());
        assert_eq!(
            signature.c.as_bls_g1().expect("response share point"),
            share.share.signature()
        );
    }
}
