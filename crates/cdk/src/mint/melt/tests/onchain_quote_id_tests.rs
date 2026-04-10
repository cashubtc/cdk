//! Tests for the onchain melt quote-id echo contract.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OnchainSettings, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
    SettingsResponse, WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::{Amount, MeltQuoteOnchainRequest, PaymentMethod};
use futures::Stream;

use crate::mint::{Mint, MintBuilder, MintMeltLimits};
use crate::types::QuoteTTL;
use crate::Error;

/// What to put in [`PaymentQuoteResponse::request_lookup_id`] when the test
/// backend is asked for an onchain quote.
#[derive(Debug, Clone)]
enum EchoBehavior {
    /// Echo the mint-supplied `quote_id` verbatim (the contract-compliant
    /// happy path).
    Echo,
    /// Return `None`, simulating a backend that forgot to echo the id.
    None,
    /// Return `Some(PaymentIdentifier::QuoteId(different_id))`.
    Mismatched(QuoteId),
    /// Return a non-`QuoteId` variant (e.g. the shape a bolt11 backend might
    /// return). Uses `PaymentIdentifier::CustomId` as a stand-in.
    WrongVariant(String),
}

/// Minimal `MintPayment` mock that only implements the onchain quote path.
///
/// Everything else (incoming payments, `make_payment`, status polling) is
/// stubbed because `get_melt_onchain_quote_impl` only invokes
/// [`MintPayment::get_payment_quote`] plus a small amount of mint-side
/// bookkeeping.
struct OnchainQuoteMock {
    unit: CurrencyUnit,
    amount: Amount<CurrencyUnit>,
    fee: Amount<CurrencyUnit>,
    confirmations: u32,
    echo: EchoBehavior,
}

impl OnchainQuoteMock {
    fn new(echo: EchoBehavior) -> Self {
        let unit = CurrencyUnit::Sat;
        Self {
            amount: Amount::new(1_000, unit.clone()),
            fee: Amount::new(10, unit.clone()),
            unit,
            confirmations: 1,
            echo,
        }
    }
}

#[async_trait]
impl MintPayment for OnchainQuoteMock {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: self.unit.to_string(),
            bolt11: None,
            bolt12: None,
            onchain: Some(OnchainSettings {
                confirmations: self.confirmations,
                min_receive_amount_sat: 0,
            }),
            custom: std::collections::HashMap::new(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        _options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        Err(payment::Error::UnsupportedPaymentOption)
    }

    async fn get_payment_quote(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let onchain_options = match options {
            OutgoingPaymentOptions::Onchain(o) => o,
            _ => return Err(payment::Error::UnsupportedPaymentOption),
        };

        let request_lookup_id = match &self.echo {
            EchoBehavior::Echo => {
                Some(PaymentIdentifier::QuoteId(onchain_options.quote_id.clone()))
            }
            EchoBehavior::None => None,
            EchoBehavior::Mismatched(other) => Some(PaymentIdentifier::QuoteId(other.clone())),
            EchoBehavior::WrongVariant(label) => Some(PaymentIdentifier::CustomId(label.clone())),
        };

        Ok(PaymentQuoteResponse {
            request_lookup_id,
            amount: self.amount.clone(),
            fee: self.fee.clone(),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: Some(6),
        })
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        _options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Err(payment::Error::UnsupportedPaymentOption)
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        Ok(Box::pin(futures::stream::pending()))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        false
    }

    fn cancel_payment_event_stream(&self) {}

    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Ok(Vec::new())
    }

    async fn check_outgoing_payment(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Err(payment::Error::UnsupportedPaymentOption)
    }
}

async fn create_onchain_test_mint(echo: EchoBehavior) -> Result<Mint, Error> {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(OnchainQuoteMock::new(echo));

    let db = Arc::new(cdk_sqlite::mint::memory::empty().await?);
    let mut mint_builder = MintBuilder::new(db.clone());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Onchain),
            MintMeltLimits::new(1, 100_000),
            backend,
        )
        .await?;

    let mnemonic = bip39::Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;
    let mint = mint_builder
        .with_name("test mint".to_string())
        .with_description("onchain quote-id echo contract tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()])
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
    mint.start().await?;

    Ok(mint)
}

fn onchain_melt_request() -> MeltQuoteRequest {
    MeltQuoteRequest::Onchain(MeltQuoteOnchainRequest {
        request: "bcrt1qexampleaddr0000000000000000000000000000".to_string(),
        unit: CurrencyUnit::Sat,
        amount: Amount::from(1_000),
    })
}

/// Happy-path: a contract-compliant backend (echoes `quote_id` verbatim)
/// produces a quote whose persisted `MeltQuote.id` is the echoed value. This
/// pins the fix — the id must flow from the mint outward, not from the
/// backend inward.
#[tokio::test]
async fn onchain_quote_uses_mint_generated_id_when_backend_echoes() {
    let mint = create_onchain_test_mint(EchoBehavior::Echo).await.unwrap();

    let response = mint.get_melt_quote(onchain_melt_request()).await.unwrap();
    let options = match response {
        cdk_common::MeltQuoteCreateResponse::Onchain(o) => o,
        other => panic!("expected onchain quote response, got {:?}", other),
    };
    assert_eq!(options.quotes.len(), 1, "expected single tier-less quote");
    let quote_id = options.quotes[0].quote.clone();

    // The stored quote must be retrievable under that id, and its persisted
    // `request_lookup_id` must be the deterministic echo (so the saga's
    // backend correlation is not dependent on what the backend returned
    // beyond the validated echo).
    let stored = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .expect("quote must be persisted");

    assert_eq!(stored.id, quote_id);
    assert_eq!(
        stored.request_lookup_id,
        Some(PaymentIdentifier::QuoteId(quote_id)),
        "request_lookup_id should be the mint-generated QuoteId, not whatever \
         variant the backend happened to return"
    );
}

/// Backend omits `request_lookup_id` entirely — must reject with
/// `OnchainQuoteLookupIdMismatch { got: None, .. }` and persist no quote.
#[tokio::test]
async fn onchain_quote_rejects_missing_lookup_id() {
    let mint = create_onchain_test_mint(EchoBehavior::None).await.unwrap();

    let err = mint
        .get_melt_quote(onchain_melt_request())
        .await
        .expect_err("missing request_lookup_id must be rejected");

    match err {
        Error::OnchainQuoteLookupIdMismatch { got: None, .. } => {}
        other => panic!("expected OnchainQuoteLookupIdMismatch {{ got: None }}, got {other:?}"),
    }

    // Nothing should have been persisted.
    let quotes = mint.localstore().get_melt_quotes().await.unwrap();
    assert!(
        quotes.is_empty(),
        "no MeltQuote may be persisted on contract-violation reject"
    );
}

/// Backend echoes a *different* `QuoteId` — must reject and surface both
/// `expected` and `got` in the error payload.
#[tokio::test]
async fn onchain_quote_rejects_mismatched_lookup_id() {
    let stray_id = QuoteId::new_uuid();
    let mint = create_onchain_test_mint(EchoBehavior::Mismatched(stray_id.clone()))
        .await
        .unwrap();

    let err = mint
        .get_melt_quote(onchain_melt_request())
        .await
        .expect_err("mismatched request_lookup_id must be rejected");

    match err {
        Error::OnchainQuoteLookupIdMismatch {
            expected,
            got: Some(PaymentIdentifier::QuoteId(returned)),
        } => {
            assert_ne!(
                expected, returned,
                "expected/got must be distinct in a mismatch report"
            );
            assert_eq!(
                returned, stray_id,
                "error should surface the id the backend actually returned"
            );
        }
        other => {
            panic!("expected OnchainQuoteLookupIdMismatch {{ got: Some(QuoteId) }}, got {other:?}")
        }
    }

    let quotes = mint.localstore().get_melt_quotes().await.unwrap();
    assert!(quotes.is_empty());
}

/// Backend returns a non-`QuoteId` `PaymentIdentifier` variant — defence in
/// depth against backends that silently return their own lookup id shape.
#[tokio::test]
async fn onchain_quote_rejects_wrong_identifier_variant() {
    let mint = create_onchain_test_mint(EchoBehavior::WrongVariant(
        "backend-internal-id".to_string(),
    ))
    .await
    .unwrap();

    let err = mint
        .get_melt_quote(onchain_melt_request())
        .await
        .expect_err("non-QuoteId PaymentIdentifier must be rejected");

    match err {
        Error::OnchainQuoteLookupIdMismatch {
            got: Some(PaymentIdentifier::CustomId(label)),
            ..
        } => {
            assert_eq!(label, "backend-internal-id");
        }
        other => {
            panic!("expected OnchainQuoteLookupIdMismatch with CustomId variant, got {other:?}")
        }
    }

    let quotes = mint.localstore().get_melt_quotes().await.unwrap();
    assert!(quotes.is_empty());
}
