//! Federation mode scaffolding for CDK mints.
//!
//! This module is intentionally small at first. Later federation milestones will
//! attach consensus, peer networking, and threshold signing behavior to member
//! mode. Until then, the default standalone mode preserves current mint
//! behavior.

use cdk_common::error::Error;
use cdk_common::federation::{
    FederationBlindSignatureShareSet, FederationConfig, FederationId, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOperation, FederationOperationEnvelope, FederationOperationId, MemberConfigPrivate,
    MemberId,
};
use cdk_common::{
    Amount, BatchMintRequest, BlindedMessage, MeltRequest, MintQuoteRequest, MintQuoteResponse,
    MintQuoteState, MintRequest, PaymentMethod, QuoteId, RestoreRequest, SwapRequest,
};

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
                method,
                state,
                amount_paid,
                amount_issued,
                payment_proof,
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
        self.validate_output_keysets(&request.outputs)
    }

    /// Validate change outputs from a melt request.
    pub fn validate_melt_change_outputs(
        &self,
        request: &MeltRequest<QuoteId>,
    ) -> Result<(), Error> {
        match request.outputs() {
            Some(outputs) => self.validate_output_keysets(outputs),
            None => Ok(()),
        }
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
    use cdk_common::{
        Amount, BatchMintRequest, BlindedMessage, CurrencyUnit, Id, MeltRequest,
        MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse,
        MintQuoteState, MintRequest, PaymentMethod, PublicKey, RestoreRequest, SecretKey,
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
    }
}
