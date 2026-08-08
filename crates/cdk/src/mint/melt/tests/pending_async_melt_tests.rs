use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::mint::MeltQuote;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState, Proofs};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::{Amount, MeltQuoteBolt11Request, PaymentMethod, ProofsMethods};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
use futures::Stream;

use crate::mint::{Mint, MintBuilder, MintMeltLimits};
use crate::test_helpers::mint::mint_test_proofs;
use crate::types::{FeeReserve, QuoteTTL};
use crate::Error;

struct NoEventPendingBackend {
    inner: FakeWallet,
    status_checks: AtomicUsize,
    settle_after_checks: usize,
    final_status: Option<MeltQuoteState>,
    strip_quote_lookup_id: bool,
}

impl NoEventPendingBackend {
    fn new(settle_after_checks: usize, final_status: Option<MeltQuoteState>) -> Self {
        let fee_reserve = FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };

        Self {
            inner: FakeWallet::new(
                fee_reserve,
                HashMap::default(),
                HashSet::default(),
                2,
                CurrencyUnit::Sat,
            ),
            status_checks: AtomicUsize::new(0),
            settle_after_checks,
            final_status,
            strip_quote_lookup_id: false,
        }
    }

    /// Simulates backends (e.g. bolt12) that cannot provide a lookup id at
    /// quote creation because no invoice exists until `make_payment`.
    fn with_stripped_quote_lookup_id(mut self) -> Self {
        self.strip_quote_lookup_id = true;
        self
    }
}

#[async_trait]
impl MintPayment for NoEventPendingBackend {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let mut response = self.inner.get_payment_quote(unit, options).await?;
        if self.strip_quote_lookup_id {
            response.request_lookup_id = None;
        }
        Ok(response)
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let mut response = self.inner.make_payment(unit, options).await?;
        response.status = MeltQuoteState::Pending;
        response.payment_proof = None;
        Ok(response)
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
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let mut response = self
            .inner
            .check_outgoing_payment(payment_identifier)
            .await?;
        let attempts = self.status_checks.fetch_add(1, Ordering::SeqCst) + 1;
        if attempts < self.settle_after_checks {
            response.status = MeltQuoteState::Pending;
            response.payment_proof = None;
            response.total_spent = Amount::new(0, CurrencyUnit::Sat);
            return Ok(response);
        }

        let Some(final_status) = self.final_status else {
            response.status = MeltQuoteState::Pending;
            response.payment_proof = None;
            response.total_spent = Amount::new(0, CurrencyUnit::Sat);
            return Ok(response);
        };

        response.status = final_status;
        if final_status != MeltQuoteState::Paid {
            response.payment_proof = None;
            response.total_spent = Amount::new(0, CurrencyUnit::Sat);
        }
        Ok(response)
    }
}

async fn create_pending_test_mint(
    backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync>,
) -> Result<Mint, Error> {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await?);
    let mut mint_builder = MintBuilder::new(db.clone());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            backend,
        )
        .await?;

    let mnemonic = bip39::Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;
    let mint = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint for async melt tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()])
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(QuoteTTL::new(10000, 10000)).await?;
    mint.start().await?;

    Ok(mint)
}

async fn create_test_melt_quote(mint: &Mint, amount: Amount) -> MeltQuote {
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let amount_msats: u64 = amount.into();
    let invoice = create_fake_invoice(
        amount_msats,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    );

    let quote_response = mint
        .get_melt_quote(cdk_common::melt::MeltQuoteRequest::Bolt11(
            MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            },
        ))
        .await
        .expect("melt quote created");

    mint.localstore()
        .get_melt_quote(quote_response.quote().expect("single-quote method"))
        .await
        .expect("db read")
        .expect("quote exists")
}

fn create_test_melt_request(
    proofs: &Proofs,
    quote: &MeltQuote,
) -> cdk_common::nuts::MeltRequest<cdk_common::QuoteId> {
    cdk_common::nuts::MeltRequest::new(quote.id.clone(), proofs.clone(), None)
}

#[tokio::test]
async fn pending_melt_wait_completes_via_status_check_without_notification() {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(2, Some(MeltQuoteState::Paid)));
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();
    let response = pending.await.unwrap();

    assert_eq!(response.state(), MeltQuoteState::Paid);

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Paid);
}

#[tokio::test]
async fn pending_melt_wait_rolls_back_via_status_check_without_notification() {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(2, Some(MeltQuoteState::Failed)));
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let input_ys = proofs.ys().unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();
    let response = pending.await.unwrap();

    assert_eq!(response.state(), MeltQuoteState::Unpaid);

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Unpaid);

    let proof_states = mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap();
    assert!(proof_states.iter().all(|state| state.is_none()));
}

#[tokio::test]
async fn pending_melt_wait_resolves_via_external_successful_event() {
    // Backend stays Pending forever on both pay and check; only the external
    // event delivered via handle_successful_melt_payment_event should resolve
    // the wait loop. Verifies the polling loop observes DB-level settlement
    // and that the finalization path is idempotent under concurrent resolution.
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(usize::MAX, None));
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();

    // Simulate an async event arriving while the wait loop is running.
    let event_mint = Arc::new(mint.clone());
    let event_localstore = mint.localstore();
    let event_pubsub = mint.pubsub_manager();
    let event_quote_id = quote.id.clone();
    let total_spent = quote.amount();
    let lookup_id = PaymentIdentifier::CustomId(quote.id.to_string());
    let event_task = tokio::spawn(async move {
        // Small delay so the wait loop is actually waiting when the event arrives.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let payment_result = MakePaymentResponse {
            payment_lookup_id: lookup_id,
            payment_proof: Some("external_event_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent,
        };
        Mint::handle_successful_melt_payment_event(
            &event_mint,
            &event_localstore,
            &event_pubsub,
            &event_quote_id,
            payment_result,
        )
        .await
    });

    let response = pending.await.unwrap();
    event_task.await.unwrap().unwrap();

    assert_eq!(response.state(), MeltQuoteState::Paid);

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Paid);

    // Saga must be deleted exactly once — racing paths should not leave it orphaned
    // nor double-process.
    let sagas = mint
        .localstore()
        .get_incomplete_sagas(cdk_common::mint::OperationKind::Melt)
        .await
        .unwrap();
    assert!(
        sagas.is_empty(),
        "saga should be deleted after successful finalization"
    );
}

#[tokio::test]
async fn pending_melt_wait_times_out_without_settled_progress() {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(usize::MAX, None));
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();
    assert_eq!(pending.pending_response().state(), MeltQuoteState::Pending);

    let err = pending.await.unwrap_err();
    assert!(matches!(err, Error::PendingMeltTimeout { .. }));

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);

    let saga = mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote.id)
        .await
        .unwrap();
    assert!(
        saga.is_some(),
        "pending melt should remain recoverable after timeout"
    );
}

#[tokio::test]
async fn pending_melt_persists_payment_lookup_id_when_quote_has_none() {
    // Simulates the bolt12 situation: no lookup id exists at quote creation,
    // so the quote is persisted with request_lookup_id: None. When the payment
    // parks as Pending, the saga must persist the lookup id returned by
    // make_payment — it is the only durable handle to the in-flight payment
    // for the pending wait loop and startup recovery.
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(usize::MAX, None).with_stripped_quote_lookup_id());
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    assert!(
        quote.request_lookup_id.is_none(),
        "test premise: quote persisted without a lookup id"
    );
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();
    let err = pending.await.unwrap_err();
    assert!(matches!(err, Error::PendingMeltTimeout { .. }));

    let expected_lookup_id = match &quote.request {
        cdk_common::mint::MeltPaymentRequest::Bolt11 { bolt11 } => {
            PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref())
        }
        request => panic!("expected bolt11 melt payment request, got {request}"),
    };

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);
    assert_eq!(
        stored_quote.request_lookup_id,
        Some(expected_lookup_id),
        "lookup id returned by make_payment must be persisted while pending"
    );
}

#[tokio::test]
async fn pending_melt_without_quote_lookup_id_resolves_via_status_check() {
    // End-to-end regression for the bolt12-style flow: with the quote created
    // without a lookup id, the pending wait loop can only settle the quote by
    // polling the backend with the lookup id persisted when the payment parked
    // as Pending. Without that persistence this times out with
    // PendingMeltTimeout and the quote is stuck.
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> = Arc::new(
        NoEventPendingBackend::new(2, Some(MeltQuoteState::Paid)).with_stripped_quote_lookup_id(),
    );
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    assert!(
        quote.request_lookup_id.is_none(),
        "test premise: quote persisted without a lookup id"
    );
    let melt_request = create_test_melt_request(&proofs, &quote);

    let pending = mint.melt(&melt_request).await.unwrap();
    let response = pending.await.unwrap();

    assert_eq!(response.state(), MeltQuoteState::Paid);

    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Paid);
    assert!(stored_quote.request_lookup_id.is_some());
}

/// Internally-settled melts never touch the backend, so a quote without a
/// lookup id must still be finalized by on-demand checks rather than waiting
/// for the next restart.
#[tokio::test]
async fn internal_settlement_without_lookup_id_finalizes_on_demand() {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(usize::MAX, None).with_stripped_quote_lookup_id());
    let mint = create_pending_test_mint(backend).await.unwrap();

    // A mint quote on THIS mint; its invoice makes the melt below an internal
    // settlement.
    let mint_quote_response = mint
        .get_mint_quote(
            cdk_common::MintQuoteBolt11Request {
                amount: Amount::from(4_000),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }
            .into(),
        )
        .await
        .unwrap();
    let mint_quote = mint
        .localstore()
        .get_mint_quote(mint_quote_response.quote())
        .await
        .unwrap()
        .expect("mint quote should exist");

    let melt_quote_response = mint
        .get_melt_quote(cdk_common::melt::MeltQuoteRequest::Bolt11(
            MeltQuoteBolt11Request {
                request: mint_quote.request.to_string().parse().unwrap(),
                unit: CurrencyUnit::Sat,
                options: None,
            },
        ))
        .await
        .unwrap();
    let quote = mint
        .localstore()
        .get_melt_quote(melt_quote_response.quote().expect("single-quote method"))
        .await
        .unwrap()
        .expect("melt quote should exist");
    assert!(
        quote.request_lookup_id.is_none(),
        "test premise: quote persisted without a lookup id"
    );

    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let input_ys = proofs.ys().unwrap();
    let melt_request = create_test_melt_request(&proofs, &quote);

    let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
    let saga = crate::mint::melt::melt_saga::MeltSaga::new(
        Arc::new(mint.clone()),
        mint.localstore(),
        mint.pubsub_manager(),
    );
    let setup = saga
        .setup_melt(
            &melt_request,
            verification,
            PaymentMethod::Known(KnownMethod::Bolt11),
        )
        .await
        .unwrap();
    let (payment_saga, _decision) = setup
        .attempt_internal_settlement(&melt_request)
        .await
        .unwrap();

    // Simulate a crash before finalize: mint quote credited, proofs pending.
    drop(payment_saga);
    assert_eq!(
        mint.localstore()
            .get_mint_quote(mint_quote_response.quote())
            .await
            .unwrap()
            .expect("mint quote should exist")
            .state(),
        cdk_common::MintQuoteState::Paid
    );

    // On-demand check finalizes instead of requiring a restart.
    let mut quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    mint.handle_pending_melt_quote(&mut quote).await.unwrap();

    assert_eq!(quote.state, MeltQuoteState::Paid);
    let states = mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap();
    assert!(
        states.iter().all(|s| *s == Some(cdk_common::State::Spent)),
        "internally-settled proofs must be consumed"
    );
}

/// Regression test (Loupe #96): startup recovery must NOT return a user's
/// reserved proofs for a `PaymentAttempted` melt saga whose quote has no
/// `request_lookup_id`.
///
/// Once a saga reaches `PaymentAttempted` the payment may already have
/// succeeded. bolt12/custom backends create quotes without a lookup id, and a
/// crash after dispatch but before the handle is persisted leaves the saga
/// unverifiable. Recovery must keep the quote pending (manual intervention)
/// rather than compensate.
#[tokio::test]
async fn payment_attempted_without_lookup_id_is_not_compensated_at_startup() {
    let backend: Arc<dyn MintPayment<Err = payment::Error> + Send + Sync> =
        Arc::new(NoEventPendingBackend::new(usize::MAX, None).with_stripped_quote_lookup_id());
    let mint = create_pending_test_mint(backend).await.unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
    let input_ys = proofs.ys().unwrap();
    let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
    assert!(
        quote.request_lookup_id.is_none(),
        "test premise: quote persisted without a lookup id"
    );
    let melt_request = create_test_melt_request(&proofs, &quote);

    let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
    let saga = crate::mint::melt::melt_saga::MeltSaga::new(
        Arc::new(mint.clone()),
        mint.localstore(),
        mint.pubsub_manager(),
    );
    let setup = saga
        .setup_melt(
            &melt_request,
            verification,
            PaymentMethod::Known(KnownMethod::Bolt11),
        )
        .await
        .unwrap();
    drop(setup);

    let operation_id = mint
        .localstore()
        .get_incomplete_sagas(cdk_common::mint::OperationKind::Melt)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("saga should exist")
        .operation_id;

    // Simulate the payment having been dispatched: the state advances to
    // PaymentAttempted before make_payment runs, and no lookup id has been
    // persisted to the quote yet.
    {
        let mut tx = mint.localstore().begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga(
            &mut saga,
            cdk_common::mint::SagaStateEnum::Melt(
                cdk_common::mint::MeltSagaState::PaymentAttempted,
            ),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // Inputs are reserved (Pending) at the moment of the crash.
    let states = mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap();
    assert!(states
        .iter()
        .all(|s| *s == Some(cdk_common::State::Pending)));

    // Simulate the crash: run startup recovery.
    mint.recover_from_incomplete_melt_sagas()
        .await
        .expect("recovery should succeed");

    // SECURITY: recovery could not verify payment status (no lookup id), so it
    // must NOT return the proofs; the quote stays pending for manual
    // intervention.
    let states = mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap();
    assert!(
        states
            .iter()
            .all(|s| *s == Some(cdk_common::State::Pending)),
        "unverifiable PaymentAttempted saga must not return the reserved proofs"
    );
    let stored_quote = mint
        .localstore()
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);
}
