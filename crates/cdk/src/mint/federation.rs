//! Federation mode support for CDK mints.
//!
//! This module applies accepted federation operations to local mint state,
//! validates aggregate BLS keysets, and produces member signature shares while
//! preserving standalone mint behavior outside federation mode.

mod address_allocation;
mod chain_observation;
mod frost_rotation;
mod projection;
mod transaction_policy;
mod transaction_state;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
#[cfg(feature = "prometheus")]
use std::time::Instant;

use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAddressAllocationOperation, FederationAddressAllocationScope,
    FederationAddressKeychain, FederationApplicationState, FederationBlindSignatureShareSet,
    FederationChainObservation, FederationConfig, FederationConfigDigest, FederationError,
    FederationFrostApplicationId, FederationFrostRotationGuard, FederationFrostRotationOperation,
    FederationFrostRotationTransition, FederationId, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltOperation, FederationMeltQuoteMetadata,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOnchainMeltPaymentReceipt, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationQuarantine,
    FederationPaymentObservationQuarantineReason, FederationPaymentObservationQuarantineRecord,
    FederationPaymentObservationRecord, FederationSignatureShareRecord,
    FederationSignedTransaction, FederationSwapOperation, FederationTransactionBroadcastIntent,
    FederationTransactionBroadcastObservation, FederationTransactionProposal,
    FederationTransactionScope, FederationTransactionSettlementPolicy, MemberConfigPrivate,
    MemberId,
};
use cdk_common::mint::{
    MeltQuote, MeltSagaState, MintQuote, Operation, OperationKind, SagaStateEnum,
};
use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
use cdk_common::{
    Amount, BatchMintRequest, BlindSignature, BlindedMessage, CurrencyUnit,
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltRequest, MintQuoteBolt11Response,
    MintQuoteBolt12Response, MintQuoteCustomResponse, MintQuoteOnchainResponse, MintQuoteRequest,
    MintQuoteResponse, MintQuoteState, MintRequest, MintResponse, NotificationPayload,
    PaymentMethod, Proofs, ProofsMethods, PublicKey, QuoteId, RestoreRequest,
    SpendingConditionVerification, State, SwapRequest, SwapResponse,
};
use uuid::Uuid;

use super::melt::ensure_melt_quote_matches_request;
use super::verification::{Verification, MAX_PROOF_CONTENT_LEN};
use super::Mint;
use super::MintInput;
use crate::fees::calculate_fee;
use crate::types::PaymentProcessorKey;

use self::chain_observation::{
    CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
    CHAIN_OBSERVATION_STATE_KEY,
};
use self::transaction_state::persist_pending_onchain_intent;

pub use self::transaction_policy::{
    FederationCpfpPolicy, FederationFeeRateSource, FederationOnchainTransactionPolicy,
};
pub use cdk_common::federation::FederationTransactionPolicyAudit;

struct FederationActiveMeltPayment {
    active: std::sync::Arc<std::sync::Mutex<HashSet<QuoteId>>>,
    quote_id: QuoteId,
}

impl Drop for FederationActiveMeltPayment {
    fn drop(&mut self) {
        let mut active = match self.active.lock() {
            Ok(active) => active,
            Err(poisoned) => poisoned.into_inner(),
        };
        active.remove(&self.quote_id);
    }
}

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
    /// Returns true when this mint runs as a federation member.
    #[must_use]
    pub fn is_member(self) -> bool {
        matches!(self, Self::Member)
    }
}

/// Validated federation config held by a mint member at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationRuntimeConfig {
    public_config: FederationConfig,
    config_digest: FederationConfigDigest,
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
        let config_digest = public_config.config_digest_for_validated_config()?;

        Ok(Self {
            public_config,
            config_digest,
            private_config,
        })
    }

    /// Public federation config shared by all members.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    /// Digest of the public config validated when this runtime was created.
    #[must_use]
    pub fn config_digest(&self) -> FederationConfigDigest {
        self.config_digest
    }

    /// Return the single active CDK on-chain FROST application.
    ///
    /// Quote routing is intentionally unambiguous. Future FROST-backed payment
    /// applications must add an explicit selector instead of depending on list
    /// order.
    pub fn active_cdk_onchain_application(
        &self,
    ) -> Result<&cdk_common::federation::FederationFrostApplicationConfig, Error> {
        let wallet = self.public_config.frost_wallet.as_ref().ok_or_else(|| {
            FederationError::AddressAllocationInvalid {
                reason: "federation has no accepted FROST wallet configuration".to_string(),
            }
        })?;
        let mut active = wallet
            .cdk_onchain_applications
            .iter()
            .filter(|application| {
                application.state == cdk_common::federation::FederationFrostEpochState::Active
                    && wallet
                        .active_epochs
                        .application_key_epochs
                        .get(&application.application_id)
                        == Some(&application.application_key_epoch)
            });
        let application =
            active
                .next()
                .ok_or_else(|| FederationError::AddressAllocationInvalid {
                    reason: "federation has no active CDK on-chain application".to_string(),
                })?;
        if active.next().is_some() {
            return Err(FederationError::AddressAllocationInvalid {
                reason:
                    "federation has multiple active CDK on-chain applications without a selector"
                        .to_string(),
            }
            .into());
        }
        Ok(application)
    }

    /// Resolve the versioned outgoing settlement policy for an accepted wallet scope.
    pub fn onchain_transaction_settlement_policy(
        &self,
        scope: FederationTransactionScope,
    ) -> Result<FederationTransactionSettlementPolicy, Error> {
        let wallet = self
            .public_config
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        let application = wallet
            .cdk_onchain_applications
            .iter()
            .find(|application| {
                application.root_key_epoch == scope.root_key_epoch
                    && application.application_id == scope.application_id
                    && application.application_key_epoch == scope.application_key_epoch
                    && application.network == scope.network
                    && application.fee_policy_version == scope.policy_version
            })
            .ok_or_else(|| {
                Error::Custom(
                    "accepted transaction scope has no FROST application policy".to_string(),
                )
            })?;
        let observation_policy = self
            .public_config
            .payment_verification_policy(FederationPaymentObservationContext::MeltQuote);
        Ok(FederationTransactionSettlementPolicy::new(
            application.confirmation_policy.minimum_confirmations,
            observation_policy.required_matching_observations(),
        )?)
    }

    /// Build and authorize one consensus-controlled descriptor allocation.
    pub fn address_allocation_operation(
        &self,
        quote: impl Into<String>,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<FederationOperationEnvelope, Error> {
        let operation = FederationAddressAllocationOperation::new(
            quote,
            application_id,
            application_key_epoch,
            keychain,
        )?;
        self.validate_federated_address_allocation_submission(&operation)?;
        Ok(FederationOperationEnvelope::new(
            self.public_config.federation_id,
            FederationOperation::AddressAllocation(operation),
        )
        .authorize(&self.private_config)?)
    }

    fn validate_federated_address_allocation_submission(
        &self,
        operation: &FederationAddressAllocationOperation,
    ) -> Result<(), Error> {
        operation.validate()?;
        let wallet = self.public_config.frost_wallet.as_ref().ok_or_else(|| {
            FederationError::AddressAllocationInvalid {
                reason: "federation has no accepted FROST wallet configuration".to_string(),
            }
        })?;
        let application = wallet
            .cdk_onchain_applications
            .iter()
            .find(|application| {
                application.application_id == operation.application_id
                    && application.application_key_epoch == operation.application_key_epoch
            })
            .ok_or_else(|| FederationError::AddressAllocationInvalid {
                reason: "allocation references an unknown CDK application epoch".to_string(),
            })?;
        if application.state != cdk_common::federation::FederationFrostEpochState::Active
            || wallet
                .active_epochs
                .application_key_epochs
                .get(&operation.application_id)
                != Some(&operation.application_key_epoch)
        {
            return Err(FederationError::AddressAllocationInvalid {
                reason: "allocation requires the active CDK application epoch".to_string(),
            }
            .into());
        }
        Ok(())
    }

    /// Build an authenticated operation from one bounded member-local chain observation.
    pub fn chain_observation_operation(
        &self,
        observation: FederationChainObservation,
    ) -> Result<FederationOperationEnvelope, Error> {
        self.validate_federated_chain_observation_submission(&observation)?;
        Ok(self.operation_envelope(FederationOperation::ChainObservation(observation)))
    }

    fn validate_federated_chain_observation_submission(
        &self,
        observation: &FederationChainObservation,
    ) -> Result<(), Error> {
        observation.validate()?;
        self.validate_federation_observer(observation.observer())?;
        if observation.observer() != self.member_id() {
            return Err(FederationError::OperationAuthorMismatch {
                author: self.member_id(),
                claimed_member: observation.observer(),
            }
            .into());
        }
        if let FederationChainObservation::Deposit(observation) = observation {
            let wallet = self.public_config.frost_wallet.as_ref().ok_or_else(|| {
                FederationError::AddressAllocationInvalid {
                    reason: "deposit requires an accepted FROST wallet configuration".to_string(),
                }
            })?;
            let known = wallet.cdk_onchain_applications.iter().any(|application| {
                application.application_id == observation.allocation.application_id
                    && application.application_key_epoch
                        == observation.allocation.application_key_epoch
                    && matches!(
                        application.state,
                        cdk_common::federation::FederationFrostEpochState::Active
                            | cdk_common::federation::FederationFrostEpochState::Retiring
                    )
            });
            if !known {
                return Err(FederationError::AddressAllocationInvalid {
                    reason: "deposit references an unknown or retired application epoch"
                        .to_string(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn validate_federation_observer(&self, observer: MemberId) -> Result<(), Error> {
        if self.public_config.members.contains_key(&observer) {
            return Ok(());
        }

        Err(FederationError::UnknownMember {
            member_id: observer,
        }
        .into())
    }

    pub(crate) fn has_active_non_auth_keyset(&self) -> bool {
        self.public_config
            .keysets
            .iter()
            .any(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
    }

    /// Return a new runtime config with a replaced public config.
    pub fn with_public_config(&self, public_config: FederationConfig) -> Result<Self, Error> {
        Self::new(public_config, self.private_config.clone())
    }

    /// Return a validated runtime config view after applying one keyset rotation.
    pub fn apply_keyset_rotation(
        &self,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<Self, Error> {
        let secret_shares = self
            .private_config
            .keyset_secret_shares
            .get(&operation.keyset.id)
            .ok_or(FederationError::MissingPrivateKeyset {
                keyset_id: operation.keyset.id,
            })?;
        operation
            .keyset
            .validate_member_rotation_proof_of_possession(
                self.public_config.federation_id,
                self.private_config.member_id,
                secret_shares,
            )?;
        self.with_public_config(self.public_config.apply_keyset_rotation(operation)?)
    }

    /// Return a validated runtime view after one accepted FROST lifecycle transition.
    pub fn apply_frost_rotation(
        &self,
        operation: &FederationFrostRotationOperation,
        guard: &FederationFrostRotationGuard,
    ) -> Result<Self, Error> {
        let mut public = self.public_config.clone();
        let wallet = public.frost_wallet.clone().ok_or_else(|| {
            Error::Custom("FROST rotation requires an accepted wallet configuration".to_string())
        })?;
        public.frost_wallet = Some(wallet.apply_rotation(&public, operation, guard)?);
        self.with_public_config(public)
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
            .authorize(&self.private_config)
            .expect("validated federation identity key signs operation envelopes")
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
        let quote = quote.to_string();
        let request_lookup_id = match QuoteId::from_str(&quote) {
            Ok(quote_id) => PaymentIdentifier::QuoteId(quote_id),
            Err(_) => PaymentIdentifier::CustomId(quote.clone()),
        };
        self.mint_quote_operation_with_lookup_id(quote, request_lookup_id, request, response)
    }

    /// Build the consensus operation for creating a mint quote with the
    /// backend's stable payment lookup identifier.
    #[must_use]
    pub fn mint_quote_operation_with_lookup_id<Q>(
        &self,
        quote: Q,
        request_lookup_id: PaymentIdentifier,
        request: MintQuoteRequest,
        response: MintQuoteResponse<String>,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: quote.to_string(),
                request_lookup_id,
                request,
                response,
            },
        ))
    }

    /// Validate a federated mint quote before consensus admission.
    ///
    /// This checks only operation-local transcript material. Payment state and
    /// quote persistence remain in accepted consensus apply.
    pub fn validate_federated_mint_quote_submission(
        &self,
        operation: &FederationMintQuoteOperation,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&operation.quote)?;
        validate_payment_method_name(&operation.request.payment_method())?;
        validate_payment_method_name(&operation.response.method())?;
        if operation.request.payment_method() != operation.response.method() {
            return Err(Error::InvalidPaymentMethod);
        }
        if operation.response.quote() != &operation.quote {
            return Err(Error::UnknownQuote);
        }
        if operation.response.request().is_empty() {
            return Err(Error::Custom(
                "federation mint quote response has empty payment request".to_string(),
            ));
        }
        mint_quote_response_amounts(&operation.response, &operation.request.unit())?;

        Ok(())
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
        self.mint_quote_payment_observation_with_metadata(
            quote,
            method,
            state,
            amount_paid,
            amount_issued,
            payment_proof,
            None,
        )
    }

    /// Build the consensus operation for observing a mint quote payment state
    /// with optional backend payment metadata.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn mint_quote_payment_observation_with_metadata<Q>(
        &self,
        quote: Q,
        method: PaymentMethod,
        state: MintQuoteState,
        amount_paid: Amount,
        amount_issued: Amount,
        payment_proof: Option<String>,
        payment_id: Option<PaymentIdentifier>,
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
                payment_id,
                payment_proof,
            },
        ))
    }

    /// Validate a federated mint quote payment observation before consensus admission.
    ///
    /// This is side-effect free: it verifies structural safety and roster
    /// membership without mutating quote state or recording payment evidence.
    pub fn validate_federated_mint_payment_observation_submission(
        &self,
        observation: &FederationMintQuotePaymentObservation,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&observation.quote)?;
        self.validate_federation_observer(observation.observer)?;
        validate_payment_method_name(&observation.method)?;
        if observation.amount_issued > observation.amount_paid {
            return Err(Error::Custom(
                "federation mint payment observation issues more than was paid".to_string(),
            ));
        }
        FederationOperation::MintQuotePayment(observation.clone())
            .balance_audit()
            .map_err(Error::from)?;

        Ok(())
    }

    /// Build the consensus operation for creating a melt quote.
    pub fn melt_quote_operation<Q>(
        &self,
        quote: Q,
        request: MeltQuoteRequest,
        response: MeltQuoteCreateResponse<QuoteId>,
        metadata: FederationMeltQuoteMetadata,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::MeltQuote(
            FederationMeltQuoteOperation {
                quote: quote.to_string(),
                request,
                response: response.into(),
                metadata,
            },
        ))
    }

    /// Validate a federated melt quote before consensus admission.
    ///
    /// This checks only request-local material. Payment attempts and quote
    /// persistence remain in accepted consensus apply.
    pub fn validate_federated_melt_quote_submission(
        &self,
        operation: &FederationMeltQuoteOperation,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&operation.quote)?;
        validate_payment_method_name(&operation.request.method())?;
        if melt_quote_request_string(&operation.request).is_empty() {
            return Err(Error::Custom(
                "federation melt quote request has empty payment request".to_string(),
            ));
        }
        let quote = operation.into_melt_quote(quote_id_from_federation(&operation.quote)?, 1)?;
        ensure_melt_quote_matches_request(&quote, &operation.request)?;

        Ok(())
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
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.melt_quote_payment_observation_with_metadata(
            quote,
            method,
            state,
            amount,
            fee_reserve,
            payment_proof,
            None,
        )
    }

    /// Build the consensus operation for observing an outgoing melt payment
    /// state with optional backend payment metadata.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn melt_quote_payment_observation_with_metadata<Q>(
        &self,
        quote: Q,
        method: PaymentMethod,
        state: cdk_common::MeltQuoteState,
        amount: Amount,
        fee_reserve: Amount,
        payment_proof: Option<String>,
        payment_id: Option<PaymentIdentifier>,
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
                payment_id,
                payment_proof,
            },
        ))
    }

    /// Validate a federated melt quote payment observation before consensus admission.
    ///
    /// This is side-effect free: it verifies structural safety and roster
    /// membership without mutating quote state or attempting external payment.
    pub fn validate_federated_melt_payment_observation_submission(
        &self,
        observation: &FederationMeltQuotePaymentObservation,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&observation.quote)?;
        self.validate_federation_observer(observation.observer)?;
        validate_payment_method_name(&observation.method)?;
        FederationOperation::MeltQuotePayment(observation.clone())
            .balance_audit()
            .map_err(Error::from)?;

        Ok(())
    }

    /// Build the consensus operation for quarantining conflicting payment observations.
    #[must_use]
    pub fn payment_observation_quarantine<Q>(
        &self,
        context: FederationPaymentObservationContext,
        quote: Q,
        reason: FederationPaymentObservationQuarantineReason,
        evidence_operation_ids: Vec<FederationOperationId>,
    ) -> FederationOperationEnvelope
    where
        Q: ToString,
    {
        self.operation_envelope(FederationOperation::PaymentObservationQuarantine(
            FederationPaymentObservationQuarantine {
                context,
                quote: quote.to_string(),
                reason,
                evidence_operation_ids,
            },
        ))
    }

    /// Validate a federated payment-observation quarantine before consensus admission.
    ///
    /// Quarantine is consensus-visible safety state, so this only checks the
    /// quote scope and requires concrete evidence unless the reason is an
    /// operator halt.
    pub fn validate_federated_payment_observation_quarantine_submission(
        &self,
        operation: &FederationPaymentObservationQuarantine,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&operation.quote)?;
        if operation.evidence_operation_ids.is_empty()
            && operation.reason != FederationPaymentObservationQuarantineReason::OperatorHalt
        {
            return Err(Error::Custom(
                "federation payment observation quarantine requires evidence".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate side-effect-free federation operation admission rules.
    ///
    /// This protects consensus submission paths with the same pure checks used
    /// by wallet-facing operation builders. Accepted-state apply remains the
    /// authoritative validity decision against ordered quote and proof state.
    pub fn validate_federated_operation_submission(
        &self,
        operation: &FederationOperation,
    ) -> Result<(), Error> {
        match operation {
            FederationOperation::AddressAllocation(operation) => {
                self.validate_federated_address_allocation_submission(operation)
            }
            FederationOperation::ChainObservation(observation) => {
                observation.validate()?;
                self.validate_federation_observer(observation.observer())
            }
            FederationOperation::MintQuote(operation) => {
                self.validate_federated_mint_quote_submission(operation)
            }
            FederationOperation::MintQuotePayment(observation) => {
                self.validate_federated_mint_payment_observation_submission(observation)
            }
            FederationOperation::MeltQuote(operation) => {
                self.validate_federated_melt_quote_submission(operation)
            }
            FederationOperation::MeltQuotePayment(observation) => {
                self.validate_federated_melt_payment_observation_submission(observation)
            }
            FederationOperation::PaymentObservationQuarantine(operation) => {
                self.validate_federated_payment_observation_quarantine_submission(operation)
            }
            FederationOperation::Mint(operation) => {
                self.validate_federated_mint_operation_submission(operation)
            }
            FederationOperation::Melt(operation) => {
                self.validate_federated_melt_operation_submission(operation)
            }
            FederationOperation::Swap(operation) => {
                self.validate_federated_swap_submission(&operation.request)
            }
            FederationOperation::KeysetRotation(operation) => {
                self.validate_federated_keyset_rotation_submission(operation)
            }
            FederationOperation::FrostRotation(operation) => {
                operation.validate()?;
                Ok(())
            }
            FederationOperation::TransactionProposal(proposal) => {
                proposal.validate()?;
                self.validate_federation_observer(proposal.proposer)
            }
            FederationOperation::TransactionSigned(_) => Ok(()),
            FederationOperation::TransactionBroadcastIntent(intent) => {
                intent.settlement_policy.validate()?;
                Ok(())
            }
            FederationOperation::TransactionBroadcastObservation(observation) => {
                observation.validate()?;
                self.validate_federation_observer(observation.observer)
            }
        }
    }

    /// Validate outputs against the active v3/BLS aggregate keysets.
    pub fn validate_output_keysets(&self, outputs: &[BlindedMessage]) -> Result<(), Error> {
        Mint::check_outputs_unique(outputs)?;
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
            .validate_blinded_restore_outputs(&request.outputs)?)
    }

    /// Validate change outputs from a melt request.
    pub fn validate_melt_change_outputs(
        &self,
        request: &MeltRequest<QuoteId>,
    ) -> Result<(), Error> {
        match request.outputs() {
            Some(outputs) => {
                Mint::check_outputs_unique(outputs)?;
                Ok(self
                    .public_config
                    .validate_blinded_change_outputs(outputs)?)
            }
            None => Ok(()),
        }
    }

    /// Validate proof secret and witness content before consensus admission.
    pub fn validate_proof_content_bounds(proofs: &Proofs) -> Result<(), Error> {
        for proof in proofs {
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

        Ok(())
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
        self.validate_federated_mint_submission(&input)?;
        Ok(
            self.operation_envelope(FederationOperation::Mint(FederationMintOperation {
                request: mint_request_from_input(input),
            })),
        )
    }

    /// Validate a federated mint submission before consensus admission.
    ///
    /// This validation is intentionally side-effect free: it may reject malformed
    /// wallet requests, but it must not persist state or create signature shares.
    pub fn validate_federated_mint_submission(&self, input: &MintInput) -> Result<(), Error> {
        input.validate()?;
        self.validate_output_keysets(input.outputs())
    }

    /// Validate a federated mint operation envelope before consensus admission.
    pub fn validate_federated_mint_operation_submission(
        &self,
        operation: &FederationMintOperation,
    ) -> Result<(), Error> {
        let input = mint_input_from_federation_request(&operation.request)?;
        self.validate_federated_mint_submission(&input)
    }

    /// Sign an accepted mint operation into local threshold BLS signature shares.
    pub fn mint_signature_shares(
        &self,
        operation_id: FederationOperationId,
        input: &MintInput,
    ) -> Result<FederationBlindSignatureShareSet, Error> {
        #[cfg(feature = "prometheus")]
        let started = Instant::now();
        let result = self.private_config.sign_blinded_messages(
            &self.public_config,
            operation_id,
            input.outputs(),
        );
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_signing_share_latency(
            &self.member_id().to_string(),
            "mint",
            started.elapsed().as_secs_f64(),
        );
        Ok(result?)
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
        self.validate_federated_melt_submission(&request)?;
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

    /// Build an authorized consensus operation for a validated on-chain proposal.
    pub fn transaction_proposal_operation(
        &self,
        proposal: FederationTransactionProposal,
    ) -> Result<FederationOperationEnvelope, Error> {
        proposal.validate()?;
        self.validate_federation_observer(proposal.proposer)?;
        if proposal.proposer != self.member_id() {
            return Err(FederationError::OperationAuthorMismatch {
                author: self.member_id(),
                claimed_member: proposal.proposer,
            }
            .into());
        }
        Ok(self.operation_envelope(FederationOperation::TransactionProposal(proposal)))
    }

    /// Build an authenticated reusable-root/application lifecycle operation.
    pub fn frost_rotation_operation(
        &self,
        operation: FederationFrostRotationOperation,
    ) -> Result<FederationOperationEnvelope, Error> {
        operation.validate()?;
        Ok(self.operation_envelope(FederationOperation::FrostRotation(operation)))
    }

    /// Build an authorized consensus operation for one exact signed transaction.
    pub fn transaction_signed_operation(
        &self,
        signed: FederationSignedTransaction,
    ) -> FederationOperationEnvelope {
        self.operation_envelope(FederationOperation::TransactionSigned(signed))
    }

    /// Build an authorized consensus operation that precedes network broadcast.
    pub fn transaction_broadcast_intent_operation(
        &self,
        intent: FederationTransactionBroadcastIntent,
    ) -> FederationOperationEnvelope {
        self.operation_envelope(FederationOperation::TransactionBroadcastIntent(intent))
    }

    /// Build an authenticated local-backend broadcast observation.
    pub fn transaction_broadcast_observation_operation(
        &self,
        observation: FederationTransactionBroadcastObservation,
    ) -> Result<FederationOperationEnvelope, Error> {
        observation.validate()?;
        if observation.observer != self.member_id() {
            return Err(FederationError::OperationAuthorMismatch {
                author: self.member_id(),
                claimed_member: observation.observer,
            }
            .into());
        }
        Ok(
            self.operation_envelope(FederationOperation::TransactionBroadcastObservation(
                observation,
            )),
        )
    }

    /// Validate a federated melt submission before consensus admission.
    ///
    /// This checks only request-local material. Quote state, proof state, payment
    /// attempts, and change signing remain in accepted consensus apply.
    pub fn validate_federated_melt_submission(
        &self,
        request: &MeltRequest<QuoteId>,
    ) -> Result<(), Error> {
        self.validate_melt_change_outputs(request)?;
        Self::validate_proof_content_bounds(request.inputs())?;
        Mint::check_inputs_unique(request.inputs())?;
        self.verify_proofs(request.inputs())?;
        request.verify_spending_conditions()?;
        Ok(())
    }

    /// Validate a federated melt operation envelope before consensus admission.
    pub fn validate_federated_melt_operation_submission(
        &self,
        operation: &FederationMeltOperation,
    ) -> Result<(), Error> {
        let request: MeltRequest<QuoteId> = operation.request.clone().try_into()?;
        self.validate_federated_melt_submission(&request)
    }

    /// Build the consensus operation for signing swap outputs.
    pub fn swap_operation(
        &self,
        request: SwapRequest,
    ) -> Result<FederationOperationEnvelope, Error> {
        self.validate_federated_swap_submission(&request)?;
        Ok(
            self.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
                request,
            })),
        )
    }

    /// Validate a federated swap submission before consensus admission.
    ///
    /// This is a pure admission check. It does not inspect or mutate local proof
    /// state; accepted consensus apply is still the authoritative spend gate.
    pub fn validate_federated_swap_submission(&self, request: &SwapRequest) -> Result<(), Error> {
        self.validate_swap_outputs(request)?;
        Self::validate_proof_content_bounds(request.inputs())?;
        Mint::check_inputs_unique(request.inputs())?;
        self.verify_proofs(request.inputs())?;
        request.verify_spending_conditions()?;
        Ok(())
    }

    /// Validate a federated keyset rotation operation before consensus admission.
    pub fn validate_federated_keyset_rotation_submission(
        &self,
        operation: &FederationKeysetRotationOperation,
    ) -> Result<(), Error> {
        self.apply_keyset_rotation(operation)?;
        Ok(())
    }

    /// Sign an accepted swap operation into local threshold BLS signature shares.
    pub fn swap_signature_shares(
        &self,
        operation_id: FederationOperationId,
        request: &SwapRequest,
    ) -> Result<FederationBlindSignatureShareSet, Error> {
        #[cfg(feature = "prometheus")]
        let started = Instant::now();
        let result = self.private_config.sign_blinded_messages(
            &self.public_config,
            operation_id,
            request.outputs(),
        );
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_signing_share_latency(
            &self.member_id().to_string(),
            "swap",
            started.elapsed().as_secs_f64(),
        );
        Ok(result?)
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

    /// Return a previously ordered mint-quote response for an idempotent
    /// wallet retry, while rejecting reuse of the assigned ID for another
    /// request.
    pub async fn ordered_federation_mint_quote_response(
        &self,
        quote_id: &QuoteId,
        request: &MintQuoteRequest,
    ) -> Result<Option<(MintQuoteResponse<QuoteId>, PaymentIdentifier)>, Error> {
        let quote_id = quote_id.to_string();
        let entries = self.localstore.get_federation_journal_entries().await?;
        let Some(entry) = entries.iter().rev().find(|entry| {
            matches!(
                &entry.envelope.operation,
                FederationOperation::MintQuote(operation) if operation.quote == quote_id
            )
        }) else {
            return Ok(None);
        };
        let FederationOperation::MintQuote(operation) = &entry.envelope.operation else {
            return Ok(None);
        };

        if &operation.request != request {
            return Err(Error::Custom(
                "federation mint quote id was reused with a different request".to_string(),
            ));
        }
        if matches!(
            entry.status(),
            FederationOperationStatus::Rejected | FederationOperationStatus::Failed
        ) {
            return Err(FederationError::RejectedOperation {
                operation_id: entry.operation_id,
                status: entry.status(),
            }
            .into());
        }

        let response = serde_json::from_value(serde_json::to_value(&operation.response)?)?;
        Ok(Some((response, operation.request_lookup_id.clone())))
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

        for payment in payments {
            if payment.payment_amount.value() == 0
                || observed_quote.payment_ids().contains(&&payment.payment_id)
            {
                continue;
            }
            let amount = payment.payment_amount.convert_to(&observed_quote.unit)?;
            payment_id = Some(federation_payment_identifier_from_label(
                &payment.payment_id,
            ));
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
                .last()
                .map(|payment| federation_payment_identifier_from_label(&payment.payment_id));
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
        let is_elected = config
            .public_config()
            .elected_melt_payment_member(operation_id)
            .ok_or(Error::Internal)?
            == config.member_id();
        let _active_payment = if is_elected {
            let Some(active_payment) = self.claim_active_federation_melt_payment(quote_id) else {
                return Ok(None);
            };
            Some(active_payment)
        } else {
            None
        };

        let payment = match saga.state {
            SagaStateEnum::Melt(MeltSagaState::SetupComplete) if is_elected => {
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
                if is_elected => {}
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

        let entries = self.localstore.get_federation_journal_entries().await?;
        let applied_config_transitions = entries
            .iter()
            .filter(|entry| entry.status() == FederationOperationStatus::Applied)
            .filter(|entry| {
                matches!(
                    &entry.envelope.operation,
                    FederationOperation::KeysetRotation(_) | FederationOperation::FrostRotation(_)
                )
            })
            .map(|entry| (entry.index, entry.operation_id))
            .collect::<Vec<_>>();

        let cache = self.federation_effective_config_cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.applied_config_transitions == applied_config_transitions {
                return Ok(Some(cached.config.clone()));
            }
        }
        drop(cache);

        let guard = self.federation_frost_rotation_guard().await?;
        let mut effective = config.clone();
        for entry in entries {
            if entry.status() != FederationOperationStatus::Applied {
                continue;
            }

            match &entry.envelope.operation {
                FederationOperation::KeysetRotation(operation) => {
                    effective = effective.apply_keyset_rotation(operation)?;
                }
                FederationOperation::FrostRotation(operation) => {
                    effective = effective.apply_frost_rotation(operation, &guard)?;
                }
                _ => {}
            }
        }

        let mut cache = self.federation_effective_config_cache.lock().await;
        *cache = Some(super::EffectiveFederationConfigCache {
            applied_config_transitions,
            config: effective.clone(),
        });

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
            FederationOperation::PaymentObservationQuarantine(operation) => {
                self.apply_federation_payment_observation_quarantine(entry, operation)
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
            FederationOperation::FrostRotation(operation) => {
                self.apply_federation_frost_rotation(entry, operation)
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
        }
    }

    async fn federation_payment_observation_is_quarantined_at_or_before(
        &self,
        context: FederationPaymentObservationContext,
        quote: &str,
        max_operation_index: u64,
    ) -> Result<bool, Error> {
        let entries = self.localstore.get_federation_journal_entries().await?;
        Ok(federation_payment_observation_is_quarantined_in_entries(
            &entries,
            context,
            quote,
            max_operation_index,
        ))
    }

    async fn require_prior_applied_melt_quote(
        &self,
        quote: &QuoteId,
        max_operation_index: u64,
    ) -> Result<(), Error> {
        let entries = self.localstore.get_federation_journal_entries().await?;
        let has_prior_quote = entries.iter().any(|entry| {
            entry.index < max_operation_index
                && entry.status() == FederationOperationStatus::Applied
                && matches!(
                    &entry.envelope.operation,
                    FederationOperation::MeltQuote(operation)
                        if operation.quote == quote.to_string()
                )
        });
        if !has_prior_quote {
            return Err(Error::UnknownQuote);
        }
        Ok(())
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
        let mut latest_by_observer = HashMap::new();
        let mut eligible = observations
            .iter()
            .filter(|record| record.operation_index <= max_operation_index)
            .collect::<Vec<_>>();
        eligible.sort_by_key(|record| record.operation_index);
        for record in eligible {
            latest_by_observer.insert(record.observation.observer(), &record.observation);
        }
        let quorum = candidate.quorum(
            latest_by_observer.values().copied(),
            policy.required_matching_observations(),
        );

        match policy.evaluate_quorum(&quorum) {
            Ok(()) => Ok(true),
            Err(
                FederationError::PaymentObservationQuorumNotSatisfied { .. }
                | FederationError::PaymentObservationConflict { .. },
            ) => Ok(false),
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
        let proposed_quote = mint_quote_from_federation_operation(
            operation,
            federation_accepted_logical_time(entry),
        )?;
        let journal_entries = self.localstore.get_federation_journal_entries().await?;
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
                .apply_mint_payment_observations_to_quote(
                    &mut tx,
                    &mut quote,
                    &observations,
                    &journal_entries,
                )
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
        let Some(runtime) = self.effective_federation_config().await? else {
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
            return Err(Error::InvalidPaymentRequest);
        }

        let application = runtime.active_cdk_onchain_application()?;
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
                .map_err(|_| Error::InvalidPaymentRequest)?;
        if response.request != expected_address.to_string() {
            return Err(Error::InvalidPaymentRequest);
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

    async fn apply_federation_payment_observation_quarantine(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationPaymentObservationQuarantine,
    ) -> Result<(), Error> {
        let _quote_id = quote_id_from_federation(&operation.quote)?;
        let transaction = self.localstore.begin_transaction().await?;
        finish_federation_apply_transaction(transaction, entry, Ok(())).await
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
        finish_federation_apply_transaction(transaction, entry, Ok(())).await
    }

    async fn apply_federation_frost_rotation(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationFrostRotationOperation,
    ) -> Result<(), Error> {
        let effective = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        if let FederationFrostRotationTransition::ProposeRoot { root } = &operation.transition {
            let stored_public = self
                .localstore()
                .get_federation_frost_root_public_configs()
                .await?;
            let stored_private = self
                .localstore()
                .get_federation_frost_root_private_configs()
                .await?;
            if !stored_public.iter().any(|candidate| candidate == root) {
                return Err(Error::Custom(
                    "FROST root proposal has no completed local dual-DKG result".to_string(),
                ));
            }
            let private = stored_private
                .iter()
                .find(|candidate| candidate.root_key_epoch == root.root_key_epoch)
                .ok_or_else(|| {
                    Error::Custom(
                        "FROST root proposal has no participant-local root share".to_string(),
                    )
                })?;
            private.validate_metadata(root, effective.member_id())?;
        }
        let guard = self.federation_frost_rotation_guard().await?;
        effective.apply_frost_rotation(operation, &guard)?;
        let transaction = self.localstore.begin_transaction().await?;
        finish_federation_apply_transaction(transaction, entry, Ok(())).await
    }

    async fn apply_federation_mint_quote_payment(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationMintQuotePaymentObservation,
    ) -> Result<(), Error> {
        let quote_id = quote_id_from_federation(&observation.quote)?;
        let record = payment_observation_record_from_entry(entry)?;
        let journal_entries = self.localstore.get_federation_journal_entries().await?;
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
                    .apply_mint_payment_observations_to_quote(
                        &mut tx,
                        &mut quote,
                        &observations,
                        &journal_entries,
                    )
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
        journal_entries: &[FederationJournalEntry],
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
            let is_quarantined = federation_payment_observation_is_quarantined_in_entries(
                journal_entries,
                FederationPaymentObservationContext::MintQuote,
                &observation.quote,
                record.operation_index,
            );

            let observed_paid = observation.amount_paid.with_unit(quote.unit.clone());
            if can_finalize && !is_quarantined && observed_paid > quote.amount_paid() {
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
                quote.add_payment(
                    additional_amount,
                    payment_id,
                    Some(record.operation_index.saturating_add(1)),
                )?;
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
            .ok_or_else(|| FederationError::ChainObservationStateInvalid {
                reason: "on-chain payment observation has no accepted spendable output".to_string(),
            })?;
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
        for quote_id in input.quote_ids() {
            if self
                .federation_payment_observation_is_quarantined_at_or_before(
                    FederationPaymentObservationContext::MintQuote,
                    &quote_id.to_string(),
                    entry.index,
                )
                .await?
            {
                return Err(Error::Custom(
                    "federation mint quote payment observations are quarantined".to_string(),
                ));
            }
        }

        let output_verification =
            self.verify_federation_outputs_with_config(&config, input.outputs())?;
        let outputs_amount = output_verification.amount;
        let blinded_secrets = input
            .outputs()
            .iter()
            .map(|output| output.blinded_secret)
            .collect::<Vec<PublicKey>>();

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
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
            // Use consensus logical time so replay converges across members.
            validate_federation_mint_quote_expiry(
                &quotes,
                federation_accepted_logical_time(entry),
            )?;

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
                    entry.index,
                )? {
                    return Err(Error::UnpaidQuote);
                }
            }

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

            let operation_record = if input.is_batch() {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::BatchMint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    Some(federation_accepted_logical_time(entry)),
                    Some(payment_method.clone()),
                )
            } else {
                Operation::new(
                    federation_operation_uuid(operation_id),
                    OperationKind::Mint,
                    outputs_amount.clone().into(),
                    Amount::ZERO,
                    Amount::ZERO,
                    Some(federation_accepted_logical_time(entry)),
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

        let input_verification =
            self.verify_federation_inputs_with_config(&config, input_proofs)?;
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
        let operation_record = Operation::new(
            federation_operation_uuid(operation_id),
            OperationKind::Swap,
            output_verification.amount.clone().into(),
            input_verification.amount.clone().into(),
            fee_breakdown.total,
            Some(federation_accepted_logical_time(entry)),
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
        let created_time = federation_accepted_logical_time(entry);
        let quote = operation.into_melt_quote(quote_id, created_time)?;
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
        let is_quarantined = self
            .federation_payment_observation_is_quarantined_at_or_before(
                FederationPaymentObservationContext::MeltQuote,
                &observation.quote,
                entry.index,
            )
            .await?;

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
            if can_finalize && !is_quarantined {
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
            .ok_or_else(|| {
                Error::Custom(
                    "federation paid melt observation found no pending melt saga".to_string(),
                )
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
            return Err(Error::InvalidPaymentRequest);
        }
        let receipt = self
            .federation_transaction_state()
            .await?
            .completed_payment_for_quote(&quote.id.to_string())?
            .ok_or(Error::PendingQuote)?;
        let melt_operation_id = match self.localstore.get_melt_saga_by_quote_id(&quote.id).await? {
            Some(saga) => saga
                .federation_operation_id
                .as_deref()
                .ok_or(Error::Internal)?
                .parse::<FederationOperationId>()?,
            None if quote.state == cdk_common::MeltQuoteState::Paid => receipt.melt_operation_id,
            None => return Err(Error::PendingQuote),
        };
        validate_federation_onchain_melt_receipt(quote, melt_operation_id, &receipt)?;
        let expected_proof = receipt.outpoint.to_string();
        if observation.payment_proof.as_deref() != Some(expected_proof.as_str()) {
            return Err(Error::InvalidPaymentRequest);
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
        if self
            .federation_payment_observation_is_quarantined_at_or_before(
                FederationPaymentObservationContext::MeltQuote,
                &request.quote().to_string(),
                entry.index,
            )
            .await?
        {
            return Err(Error::Custom(
                "federation melt quote payment observations are quarantined".to_string(),
            ));
        }

        match self
            .melt_federation_with_accepted_time(
                &request,
                operation_id,
                federation_accepted_logical_time(entry),
            )
            .await
        {
            Ok(_) | Err(Error::PaymentFailed) => {}
            Err(err) => return Err(err),
        }

        let mut transaction = self.localstore.begin_transaction().await?;
        let outcome = async {
            let quote = transaction
                .get_melt_quote(request.quote())
                .await?
                .ok_or(Error::UnknownQuote)?;
            let runtime = self.federation_config().ok_or(Error::Internal)?;
            persist_pending_onchain_intent(
                &mut transaction,
                runtime,
                &quote,
                operation_id,
                federation_accepted_logical_time(entry),
            )
            .await
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }

    /// Verify federated melt or swap inputs against aggregate BLS keysets.
    pub(crate) async fn verify_federation_inputs(
        &self,
        inputs: &Proofs,
    ) -> Result<Verification, Error> {
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        self.verify_federation_inputs_with_config(&config, inputs)
    }

    pub(crate) fn verify_federation_inputs_with_config(
        &self,
        config: &FederationRuntimeConfig,
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

        FederationRuntimeConfig::validate_proof_content_bounds(inputs)?;

        Self::check_inputs_unique(inputs)?;
        config.verify_proofs(inputs)?;
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

fn federation_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn payment_observation_record_from_entry(
    entry: &FederationJournalEntry,
) -> Result<FederationPaymentObservationRecord, Error> {
    FederationPaymentObservationRecord::from_journal_entry(entry).ok_or(Error::Internal)
}

fn federation_payment_identifier_label(payment_id: &PaymentIdentifier) -> String {
    format!("{}:{}", payment_id.kind(), payment_id)
}

fn federation_operation_status_label(status: FederationOperationStatus) -> &'static str {
    match status {
        FederationOperationStatus::Accepted => "accepted",
        FederationOperationStatus::Applying => "applying",
        FederationOperationStatus::Applied => "applied",
        FederationOperationStatus::Rejected => "rejected",
        FederationOperationStatus::Failed => "failed",
    }
}

fn ensure_same_federation_journal_entry_for_retry(
    persisted: &FederationJournalEntry,
    accepted: &FederationJournalEntry,
) -> Result<(), Error> {
    if persisted.index != accepted.index
        || persisted.operation_id != accepted.operation_id
        || persisted.envelope.canonical_bytes()? != accepted.envelope.canonical_bytes()?
        || persisted.conflict_keys != accepted.conflict_keys
        || persisted.previous_order_digest != accepted.previous_order_digest
        || persisted.order_digest != accepted.order_digest
        || persisted.consensus_evidence != accepted.consensus_evidence
    {
        return Err(Error::Custom(format!(
            "federation operation {} has local status {} but does not match the accepted consensus entry",
            persisted.operation_id,
            federation_operation_status_label(persisted.status())
        )));
    }

    Ok(())
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

fn validate_payment_method_name(method: &PaymentMethod) -> Result<(), Error> {
    if method.is_valid_method_name() {
        return Ok(());
    }

    Err(Error::InvalidPaymentMethod)
}

fn mint_quote_from_federation_operation(
    operation: &FederationMintQuoteOperation,
    created_time: u64,
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
        operation.request_lookup_id.clone(),
        operation.request.pubkey(),
        amount_paid,
        amount_issued,
        method,
        created_time,
        created_time,
        Vec::new(),
        Vec::new(),
        extra_json,
    ))
}

fn federation_accepted_logical_time(entry: &FederationJournalEntry) -> u64 {
    // The MVP journal has ordered consensus but no wall-clock timestamp. Use a
    // stable logical time so deterministic replay converges across members.
    entry.index.saturating_add(1)
}

fn federation_payment_observation_is_quarantined_in_entries(
    entries: &[FederationJournalEntry],
    context: FederationPaymentObservationContext,
    quote: &str,
    max_operation_index: u64,
) -> bool {
    entries.iter().any(|entry| {
        entry.index <= max_operation_index
            && FederationPaymentObservationQuarantineRecord::from_journal_entry(entry).is_some_and(
                |record| record.quarantine.context == context && record.quarantine.quote == quote,
            )
    })
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
    match response {
        MintQuoteResponse::Bolt11(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
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
        || existing.request_lookup_id != expected.request_lookup_id
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

fn validate_federation_onchain_melt_receipt(
    quote: &MeltQuote,
    melt_operation_id: FederationOperationId,
    receipt: &FederationOnchainMeltPaymentReceipt,
) -> Result<(), Error> {
    let amount_sat = quote.amount().to_sat()?;
    let maximum_fee_sat = quote.fee_reserve().to_sat()?;
    if receipt.quote != quote.id.to_string()
        || receipt.melt_operation_id != melt_operation_id
        || receipt.amount_sat != amount_sat
        || receipt.fee_sat > maximum_fee_sat
        || receipt.outpoint.txid != receipt.txid
    {
        return Err(Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn melt_quote_payment_response(
    operation_id: FederationOperationId,
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
    onchain_receipt: Option<&FederationOnchainMeltPaymentReceipt>,
) -> Result<MakePaymentResponse, Error> {
    let payment_lookup_id = observation
        .payment_id
        .clone()
        .or_else(|| quote.request_lookup_id.clone())
        .unwrap_or_else(|| {
            PaymentIdentifier::CustomId(format!("federation-melt-observation-{operation_id}"))
        });
    let total_spent = match onchain_receipt {
        Some(receipt) => Amount::new(
            receipt
                .amount_sat
                .checked_add(receipt.fee_sat)
                .ok_or(Error::AmountOverflow)?,
            quote.unit.clone(),
        ),
        None => {
            let amount = observation.amount.with_unit(quote.unit.clone());
            let fee_reserve = observation.fee_reserve.with_unit(quote.unit.clone());
            amount
                .checked_add(&fee_reserve)
                .map_err(|_| Error::AmountOverflow)?
        }
    };

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

fn federation_melt_quote_immutable_terms_match(existing: &MeltQuote, proposed: &MeltQuote) -> bool {
    let fee_selection_matches = match existing.selected_fee_index {
        Some(selected)
            if existing.payment_method
                == PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) =>
        {
            existing
                .fee_options()
                .iter()
                .find(|option| option.fee_index == selected)
                .is_some_and(|option| {
                    existing.fee_reserve() == option.fee_reserve.with_unit(existing.unit.clone())
                        && existing.estimated_blocks == Some(option.estimated_blocks)
                })
        }
        Some(_) => false,
        None => {
            existing.fee_reserve() == proposed.fee_reserve()
                && existing.estimated_blocks == proposed.estimated_blocks
        }
    };

    existing.id == proposed.id
        && existing.unit == proposed.unit
        && existing.request == proposed.request
        && existing.amount() == proposed.amount()
        && fee_selection_matches
        && existing.expiry == proposed.expiry
        && existing.request_lookup_id == proposed.request_lookup_id
        && existing.options == proposed.options
        && existing.created_time == proposed.created_time
        && existing.payment_method == proposed.payment_method
        && existing.extra_json == proposed.extra_json
        && existing.fee_options() == proposed.fee_options()
}

fn federation_payment_identifier_from_label(payment_id: &str) -> PaymentIdentifier {
    match payment_id.split_once(':') {
        Some((kind, identifier)) => PaymentIdentifier::new(kind, identifier)
            .unwrap_or_else(|_| PaymentIdentifier::CustomId(payment_id.to_string())),
        None => PaymentIdentifier::CustomId(payment_id.to_string()),
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

fn validate_federation_mint_quote_expiry(
    quotes: &[database::mint::Acquired<MintQuote>],
    accepted_time: u64,
) -> Result<(), Error> {
    for quote in quotes {
        if quote.expiry != 0 && quote.expiry < accepted_time {
            return Err(Error::ExpiredQuote(quote.expiry, accepted_time));
        }
    }

    Ok(())
}

fn validate_federation_mint_quote_signatures(
    input: &MintInput,
    quotes: &[database::mint::Acquired<MintQuote>],
) -> Result<(), Error> {
    match input {
        MintInput::Single(request) => {
            let quote = quotes.first().ok_or(Error::UnknownQuote)?;
            match quote.pubkey.as_ref() {
                Some(pubkey) => request
                    .verify_signature(*pubkey)
                    .map_err(|_| Error::SignatureMissingOrInvalid),
                None if request.signature.is_some() => Err(Error::SignatureMissingOrInvalid),
                None => Ok(()),
            }
        }
        MintInput::Batch(request) => {
            for (index, quote) in quotes.iter().enumerate() {
                let signature = request
                    .signatures
                    .as_ref()
                    .and_then(|signatures| signatures.get(index))
                    .and_then(Option::as_ref);

                match quote.pubkey.as_ref() {
                    Some(pubkey) => {
                        let signature = signature.ok_or(Error::SignatureMissingOrInvalid)?;
                        let quote_id = request
                            .quotes
                            .get(index)
                            .ok_or(Error::SignatureMissingOrInvalid)?;
                        request
                            .verify_quote_signature(quote_id, signature, pubkey)
                            .map_err(|_| Error::SignatureMissingOrInvalid)?;
                    }
                    None if signature.is_some() => return Err(Error::SignatureMissingOrInvalid),
                    None => {}
                }
            }

            Ok(())
        }
    }
}

fn validate_federation_mint_quote_states_for_signing(
    quotes: &[database::mint::Acquired<MintQuote>],
    expected_amounts: &[Amount<CurrencyUnit>],
    signatures_present: usize,
    expected_signature_count: usize,
) -> Result<(), Error> {
    if quotes.len() != expected_amounts.len() {
        return Err(Error::Internal);
    }

    for (quote, amount_issued) in quotes.iter().zip(expected_amounts) {
        match quote.state() {
            MintQuoteState::Paid => {}
            MintQuoteState::Issued => {
                if quote.amount_issued() < amount_issued.clone() {
                    return Err(Error::IssuedQuote);
                }
                if signatures_present != expected_signature_count {
                    return Err(Error::IssuedQuote);
                }
            }
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
        }
    }

    Ok(())
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
mod tests;
