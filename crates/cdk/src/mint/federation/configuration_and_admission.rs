use std::collections::{HashMap, HashSet};
use std::str::FromStr;
#[cfg(feature = "prometheus")]
use std::time::Instant;

use cdk_common::bitcoin::hashes::{sha256, Hash};
use cdk_common::database;
use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAddressAllocationOperation, FederationAddressAllocationScope,
    FederationAddressKeychain, FederationApplicationState, FederationBarkEvidenceCertificate,
    FederationBarkOperation, FederationBlindSignatureShareSet, FederationChainObservation,
    FederationConfig, FederationConfigDigest, FederationError, FederationFrostApplicationId,
    FederationId, FederationJournalEntry, FederationKeysetRotationOperation,
    FederationMaterializedStateKey, FederationMeltOperation, FederationMeltQuoteMetadata,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOnchainMeltPaymentReceipt, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationSignatureShareRecord,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FederationProofVerificationContext {
    SwapBuilder,
    SwapAdmission,
    SwapApply,
    MeltBuilder,
    MeltAdmission,
    MeltApply,
}

#[cfg(feature = "prometheus")]
impl FederationProofVerificationContext {
    const fn performance_labels(
        self,
    ) -> (
        cdk_prometheus::FederationPerformanceWork,
        cdk_prometheus::FederationPerformanceOperation,
    ) {
        use cdk_prometheus::{FederationPerformanceOperation, FederationPerformanceWork};

        match self {
            Self::SwapBuilder => (
                FederationPerformanceWork::SwapBuilderProofsVerified,
                FederationPerformanceOperation::SwapProofVerificationBuilder,
            ),
            Self::SwapAdmission => (
                FederationPerformanceWork::SwapAdmissionProofsVerified,
                FederationPerformanceOperation::SwapProofVerificationAdmission,
            ),
            Self::SwapApply => (
                FederationPerformanceWork::SwapApplyProofsVerified,
                FederationPerformanceOperation::SwapProofVerificationApply,
            ),
            Self::MeltBuilder => (
                FederationPerformanceWork::MeltBuilderProofsVerified,
                FederationPerformanceOperation::MeltProofVerificationBuilder,
            ),
            Self::MeltAdmission => (
                FederationPerformanceWork::MeltAdmissionProofsVerified,
                FederationPerformanceOperation::MeltProofVerificationAdmission,
            ),
            Self::MeltApply => (
                FederationPerformanceWork::MeltApplyProofsVerified,
                FederationPerformanceOperation::MeltProofVerificationApply,
            ),
        }
    }
}
use crate::fees::calculate_fee;
use crate::types::PaymentProcessorKey;

use self::chain_observation::{
    CHAIN_OBSERVATION_PRIMARY_NAMESPACE, CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
    CHAIN_OBSERVATION_STATE_KEY,
};
use self::transaction_state::persist_pending_onchain_intent;

const FEDERATION_BLS_PROOF_BATCH_MIN: usize = 8;

/// Number of accepted journal items for which the accepted melt author holds
/// exclusive payment-attempt authority before it rotates to the next roster
/// member.
///
/// The lease is measured in consensus-ordered journal positions, not wall
/// time, so every member with the same journal frontier deterministically
/// computes the same single actor. A federation whose frontier never
/// advances has no concurrent work competing for the same payment anyway;
/// the first later operation expires the lease and lets the next member
/// safely continue with the already-durable external identity and recovery state.
pub(crate) const FEDERATION_EXTERNAL_ACTION_ACTOR_LEASE_ITEMS: u64 = 8;

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

    /// Return the federation's immutable CDK on-chain FROST application.
    pub fn cdk_onchain_application(
        &self,
    ) -> Result<&cdk_common::federation::FederationFrostApplicationConfig, Error> {
        let wallet = self.public_config.frost_wallet.as_ref().ok_or_else(|| {
            FederationError::AddressAllocationInvalid {
                reason: "federation has no accepted FROST wallet configuration".to_string(),
            }
        })?;
        Ok(&wallet.cdk_onchain_application)
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
        let application = &wallet.cdk_onchain_application;
        if application.root_key_epoch != scope.root_key_epoch
            || application.application_id != scope.application_id
            || application.application_key_epoch != scope.application_key_epoch
            || application.network != scope.network
            || application.fee_policy_version != scope.policy_version
        {
            return Err(Error::Custom(
                "accepted transaction scope has no FROST application policy".to_string(),
            ));
        }
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
        let application = &wallet.cdk_onchain_application;
        if application.application_id != operation.application_id
            || application.application_key_epoch != operation.application_key_epoch
        {
            return Err(FederationError::AddressAllocationInvalid {
                reason: "allocation references an unknown CDK application epoch".to_string(),
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
            let allocation_operation = FederationAddressAllocationOperation::new(
                observation.allocation.quote.clone(),
                observation.allocation.application_id,
                observation.allocation.application_key_epoch,
                observation.allocation.keychain,
            )?;
            self.validate_federated_address_allocation_submission(&allocation_operation)?;
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
        let quote = operation.into_melt_quote(quote_id_from_federation(&operation.quote)?)?;
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
            FederationOperation::Bark(operation) => {
                operation.validate()?;
                let evidence_certificate = match operation {
                    FederationBarkOperation::ActionTransition(transition) => {
                        transition.evidence_certificate.as_deref()
                    }
                    FederationBarkOperation::EvidenceCertificate(certificate) => Some(certificate),
                    FederationBarkOperation::ReceiveIntent(_)
                    | FederationBarkOperation::ReceivePrepared(_)
                    | FederationBarkOperation::ActionPlan(_)
                    | FederationBarkOperation::MemberObservation(_) => None,
                };
                if let Some(certificate) = evidence_certificate {
                    self.validate_bark_evidence_certificate(certificate)?;
                }
                if let Some(author) = operation.claimed_author() {
                    self.validate_federation_observer(author)?;
                }
                if let FederationBarkOperation::ReceiveIntent(intent) = operation {
                    intent.validate_secret_acknowledgements(&self.public_config)?;
                }
                Ok(())
            }
        }
    }

    fn validate_bark_evidence_certificate(
        &self,
        certificate: &FederationBarkEvidenceCertificate,
    ) -> Result<(), Error> {
        let required = match certificate.binding.phase {
            cdk_common::federation::FederationBarkEvidencePhase::ReceiveHtlcsReady => {
                self.public_config.threshold_params.signature_threshold
            }
            cdk_common::federation::FederationBarkEvidencePhase::ReceiveClaimed => {
                self.public_config.threshold_params.signature_threshold.max(
                    self.public_config
                        .payment_verification_policy(FederationPaymentObservationContext::MintQuote)
                        .required_matching_observations(),
                )
            }
        };
        certificate
            .verify(&self.public_config, required)
            .map_err(Error::from)
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
        if proofs.len() < FEDERATION_BLS_PROOF_BATCH_MIN {
            for proof in proofs {
                cdk_common::dhke::verify_bls_message(
                    self.aggregate_public_key_for_proof(proof)?,
                    proof.c,
                    proof.secret.as_bytes(),
                )?;
            }
            return Ok(());
        }

        let mint_pubkeys = proofs
            .iter()
            .map(|proof| self.aggregate_public_key_for_proof(proof))
            .collect::<Result<Vec<_>, _>>()?;
        let signatures = proofs.iter().map(|proof| proof.c).collect::<Vec<_>>();
        let messages = proofs
            .iter()
            .map(|proof| proof.secret.as_bytes())
            .collect::<Vec<_>>();
        cdk_common::dhke::batch_verify_bls_messages(&mint_pubkeys, &signatures, &messages)?;
        Ok(())
    }

    fn aggregate_public_key_for_proof(
        &self,
        proof: &cdk_common::Proof,
    ) -> Result<PublicKey, Error> {
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

        Ok(PublicKey::from(*aggregate_key))
    }

    fn verify_proofs_with_performance_context(
        &self,
        proofs: &Proofs,
        context: FederationProofVerificationContext,
    ) -> Result<(), Error> {
        #[cfg(feature = "prometheus")]
        let started_at = Instant::now();
        let result = self.verify_proofs(proofs);
        #[cfg(feature = "prometheus")]
        {
            let (work, operation) = context.performance_labels();
            cdk_prometheus::METRICS.record_federation_performance_work(
                work,
                u64::try_from(proofs.len()).unwrap_or(u64::MAX),
            );
            cdk_prometheus::METRICS.record_federation_performance_operation(
                operation,
                cdk_prometheus::FederationPerformanceOutcome::from_success(result.is_ok()),
                started_at.elapsed().as_secs_f64(),
            );
        }
        #[cfg(not(feature = "prometheus"))]
        let _ = context;
        result
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
        self.validate_federated_melt_submission_with_context(
            &request,
            FederationProofVerificationContext::MeltBuilder,
        )?;
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
        self.validate_federated_melt_submission_with_context(
            request,
            FederationProofVerificationContext::MeltAdmission,
        )
    }

    fn validate_federated_melt_submission_with_context(
        &self,
        request: &MeltRequest<QuoteId>,
        context: FederationProofVerificationContext,
    ) -> Result<(), Error> {
        self.validate_melt_change_outputs(request)?;
        Self::validate_proof_content_bounds(request.inputs())?;
        Mint::check_inputs_unique(request.inputs())?;
        self.verify_proofs_with_performance_context(request.inputs(), context)?;
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
        self.validate_federated_swap_submission_with_context(
            &request,
            FederationProofVerificationContext::SwapBuilder,
        )?;
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
        self.validate_federated_swap_submission_with_context(
            request,
            FederationProofVerificationContext::SwapAdmission,
        )
    }

    fn validate_federated_swap_submission_with_context(
        &self,
        request: &SwapRequest,
        context: FederationProofVerificationContext,
    ) -> Result<(), Error> {
        self.validate_swap_outputs(request)?;
        Self::validate_proof_content_bounds(request.inputs())?;
        Mint::check_inputs_unique(request.inputs())?;
        self.verify_proofs_with_performance_context(request.inputs(), context)?;
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
