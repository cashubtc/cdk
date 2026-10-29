//! Integration tests for renewable mint-side melt execution leases.
//!
//! Two mint instances share a database and a controllable payment backend.
//! Recovery respects live ownership, takes over expired leases, and can
//! resolve payments after executors release their leases.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cashu::dhke::construct_proofs;
use cashu::nuts::{MeltRequest, MintQuoteState, PreMintSecrets, Proofs, ProofsMethods, State};
use cashu::{Amount, Bolt11Invoice, CurrencyUnit, PaymentMethod};
use cdk::amount::SplitTarget;
use cdk::mint::{Mint, MintBuilder, MintInput, MintMeltLimits};
use cdk::nuts::MeltQuoteState;
use cdk::types::{FeeReserve, QuoteTTL};
use cdk_common::mint::{MeltSagaState, OperationKind, SagaStateEnum};
use cdk_common::nut00::KnownMethod;
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::{
    MeltQuoteBolt11Request, MeltQuoteRequest, MeltQuoteResponse, MintQuoteBolt11Request,
    MintQuoteBolt11Response,
};
use cdk_fake_wallet::{create_fake_invoice, FakeWallet};
use cdk_integration_tests::init_pure_tests::setup_tracing;
use cdk_signatory::db_signatory::DbSignatory;
use cdk_signatory::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};
use futures::Stream;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

/// How the backend answers `make_payment`.
enum DispatchBehavior {
    /// Block until the gate opens, then report an uncertain dispatch result.
    GatedFailure(Arc<Notify>),
    /// Report the payment as still in flight.
    Pending,
    /// Confirm the payment immediately.
    Paid,
}

/// A bolt11 backend whose dispatch and status-check outcomes the test
/// controls at runtime. Incoming-payment methods delegate to the fake wallet.
struct ControllableBackend {
    inner: FakeWallet,
    dispatch: DispatchBehavior,
    check_status: Mutex<MeltQuoteState>,
    spent_amount: Amount<CurrencyUnit>,
}

impl ControllableBackend {
    fn new(dispatch: DispatchBehavior, spent_amount: Amount<CurrencyUnit>) -> Self {
        Self {
            inner: FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 1.0,
                },
                HashMap::default(),
                HashSet::default(),
                1,
                CurrencyUnit::Sat,
            ),
            dispatch,
            check_status: Mutex::new(MeltQuoteState::Unknown),
            spent_amount,
        }
    }

    fn set_check_status(&self, status: MeltQuoteState) {
        *self.check_status.lock().expect("check status lock") = status;
    }
}

#[async_trait::async_trait]
impl MintPayment for ControllableBackend {
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
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment_lookup_id = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                PaymentIdentifier::PaymentHash(*bolt11_options.bolt11.payment_hash().as_ref())
            }
            _ => PaymentIdentifier::CustomId("controllable".to_string()),
        };

        match &self.dispatch {
            DispatchBehavior::GatedFailure(gate) => {
                gate.notified().await;
                Err(payment::Error::UnknownPaymentState)
            }
            DispatchBehavior::Pending => Ok(MakePaymentResponse {
                payment_lookup_id,
                payment_proof: None,
                status: MeltQuoteState::Pending,
                total_spent: Amount::ZERO.with_unit(CurrencyUnit::Sat),
            }),
            DispatchBehavior::Paid => Ok(MakePaymentResponse {
                payment_lookup_id,
                payment_proof: Some("preimage".to_string()),
                status: MeltQuoteState::Paid,
                total_spent: self.spent_amount.clone(),
            }),
        }
    }

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
        let status = *self.check_status.lock().expect("check status lock");
        let total_spent = match status {
            MeltQuoteState::Paid => self.spent_amount.clone(),
            _ => Amount::ZERO.with_unit(CurrencyUnit::Sat),
        };

        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: Some("preimage".to_string()),
            status,
            total_spent,
        })
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }
}

async fn create_mint_with_backend(backend: Arc<ControllableBackend>) -> Mint {
    let localstore = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    create_mint_with_store(backend, localstore).await
}

async fn create_mint_with_store(
    backend: Arc<ControllableBackend>,
    localstore: Arc<cdk_sqlite::mint::MintSqliteDatabase>,
) -> Mint {
    create_mint_with_store_and_signatory(backend, localstore, None).await
}

async fn create_mint_with_store_and_signatory(
    backend: Arc<ControllableBackend>,
    localstore: Arc<cdk_sqlite::mint::MintSqliteDatabase>,
    signatory: Option<Arc<dyn Signatory + Send + Sync>>,
) -> Mint {
    let mut mint_builder = MintBuilder::new(localstore.clone());
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            backend,
        )
        .await
        .unwrap();
    let mint_builder = mint_builder
        .with_name("melt lock test mint".to_string())
        .with_description("melt lock test mint".to_string())
        .with_urls(vec!["https://melt-lock-test".to_string()]);
    let mint = match signatory {
        Some(signatory) => mint_builder.build_with_signatory(signatory).await.unwrap(),
        None => mint_builder
            .build_with_seed(localstore, &[42; 64])
            .await
            .unwrap(),
    };
    mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
        .await
        .unwrap();
    mint.start().await.unwrap();
    mint
}

struct GatedSignatory {
    inner: Arc<dyn Signatory + Send + Sync>,
    enabled: AtomicBool,
    calls: AtomicUsize,
    entered: [Notify; 2],
    resume: [Notify; 2],
}

#[async_trait::async_trait]
impl Signatory for GatedSignatory {
    fn name(&self) -> String {
        self.inner.name()
    }

    async fn blind_sign(
        &self,
        messages: Vec<cdk_common::BlindedMessage>,
    ) -> Result<Vec<cdk_common::BlindSignature>, cdk::Error> {
        if self.enabled.load(Ordering::SeqCst) {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.entered.len() {
                self.entered[call].notify_one();
                self.resume[call].notified().await;
            }
        }
        self.inner.blind_sign(messages).await
    }

    async fn verify_proofs(&self, proofs: Vec<cdk_common::Proof>) -> Result<(), cdk::Error> {
        self.inner.verify_proofs(proofs).await
    }

    async fn keysets(&self) -> Result<SignatoryKeysets, cdk::Error> {
        self.inner.keysets().await
    }

    async fn subscribe_keysets(
        &self,
    ) -> Result<tokio::sync::watch::Receiver<SignatoryKeysets>, cdk::Error> {
        self.inner.subscribe_keysets().await
    }

    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, cdk::Error> {
        self.inner.rotate_keyset(args).await
    }
}

/// Mints proofs directly against the test mint (the fake backend pays the
/// mint quote).
async fn mint_proofs(mint: &Mint, amount: Amount) -> Proofs {
    let mint_quote: MintQuoteBolt11Response<_> = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount,
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }
            .into(),
        )
        .await
        .unwrap()
        .into();

    timeout(Duration::from_secs(30), async {
        loop {
            let check: MintQuoteBolt11Response<_> = mint
                .check_mint_quotes(&[QuoteId::from_str(&mint_quote.quote).unwrap()])
                .await
                .unwrap()
                .first()
                .unwrap()
                .clone()
                .into();

            if check.state == MintQuoteState::Paid {
                break;
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("mint quote should be paid");

    let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let keys = mint
        .keyset_pubkeys(&keyset_id)
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .keys
        .clone();
    let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect());
    let premint_secrets =
        PreMintSecrets::random(keyset_id, amount, &SplitTarget::None, &fees.into()).unwrap();

    let mint_res = mint
        .process_mint_request(MintInput::Single(
            cashu::nuts::MintRequest {
                quote: mint_quote.quote,
                outputs: premint_secrets.blinded_messages(),
                signature: None,
            }
            .try_into()
            .unwrap(),
        ))
        .await
        .unwrap();

    construct_proofs(
        mint_res.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &keys,
    )
    .unwrap()
}

async fn create_melt_quote(mint: &Mint, invoice: &Bolt11Invoice) -> QuoteId {
    let response = mint
        .get_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice.clone(),
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .unwrap();
    response.quote().expect("single-quote method").clone()
}

async fn wait_for_saga_state(mint: &Mint, expected: MeltSagaState) {
    timeout(Duration::from_secs(10), async {
        loop {
            let sagas = mint
                .localstore()
                .get_incomplete_sagas(OperationKind::Melt)
                .await
                .unwrap();
            if sagas
                .iter()
                .any(|saga| saga.state == SagaStateEnum::Melt(expected.clone()))
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("saga did not reach state {expected}"));
}

fn melt_quote_state(response: &MeltQuoteResponse<QuoteId>) -> MeltQuoteState {
    match response {
        MeltQuoteResponse::Bolt11(response) => response.state,
        other => panic!("expected bolt11 melt quote response, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_poll_preserves_completion_during_change_signing() {
    setup_tracing();
    let backend = Arc::new(ControllableBackend::new(
        DispatchBehavior::Paid,
        Amount::new(100, CurrencyUnit::Sat),
    ));
    backend.set_check_status(MeltQuoteState::Paid);
    let localstore = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    let signatory = Arc::new(GatedSignatory {
        inner: Arc::new(
            DbSignatory::new(
                localstore.clone(),
                &[42; 64],
                HashMap::new(),
                HashMap::new(),
            )
            .await
            .unwrap(),
        ),
        enabled: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        entered: std::array::from_fn(|_| Notify::new()),
        resume: std::array::from_fn(|_| Notify::new()),
    });
    let mint = create_mint_with_store_and_signatory(
        backend.clone(),
        localstore.clone(),
        Some(signatory.clone()),
    )
    .await;
    let proofs = mint_proofs(&mint, Amount::from(1_000)).await;
    let input_ys = proofs.ys().unwrap();
    let replica =
        create_mint_with_store_and_signatory(backend, localstore, Some(signatory.clone())).await;
    let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let change = PreMintSecrets::blank(keyset_id, Amount::from(1_000)).unwrap();
    let invoice = create_fake_invoice(100_000, String::new());
    let quote_id = create_melt_quote(&mint, &invoice).await;
    signatory.enabled.store(true, Ordering::SeqCst);
    let request = MeltRequest::new(quote_id.clone(), proofs, Some(change.blinded_messages()));
    let pending_melt = mint.melt(&request).await.unwrap();
    timeout(Duration::from_secs(10), signatory.entered[0].notified())
        .await
        .expect("finalizer should reach change signing");
    let during_signing = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();

    let mut poll = tokio::spawn({
        let replica = replica.clone();
        let quote_id = quote_id.clone();
        async move { replica.check_melt_quote(&quote_id).await }
    });
    let response = timeout(Duration::from_secs(10), async {
        tokio::select! {
            response = &mut poll => Some(response.unwrap().unwrap()),
            _ = signatory.entered[1].notified() => None,
        }
    })
    .await
    .expect("poll should finish or reach signing");

    signatory.resume[0].notify_one();
    let completion = timeout(Duration::from_secs(10), pending_melt)
        .await
        .expect("original finalizer should finish");
    signatory.resume[1].notify_one();
    let response = match response {
        Some(response) => response,
        None => timeout(Duration::from_secs(10), poll)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    };
    let finalized = replica.check_melt_quote(&quote_id).await.unwrap();
    let stored = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    let states = mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap();
    let saga = mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .unwrap();
    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    let request_info = tx
        .get_melt_request_and_blinded_messages(&quote_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    mint.stop().await.unwrap();
    replica.stop().await.unwrap();

    let completion = completion.expect("original melt should complete successfully");
    assert_eq!(completion.state(), MeltQuoteState::Paid);
    assert_eq!(response.state(), MeltQuoteState::Pending);
    assert!(response.change().is_none());
    assert!(during_signing.is_locked());
    assert!(during_signing.melt_lock_expires_at > 0);
    assert_eq!(signatory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(finalized.state(), MeltQuoteState::Paid);
    assert_eq!(completion.change(), finalized.change());
    assert_eq!(
        Amount::try_sum(finalized.change().unwrap().iter().map(|s| s.amount)).unwrap(),
        Amount::from(900)
    );
    assert!(!stored.is_locked());
    assert_eq!(stored.melt_lock_expires_at, 0);
    assert!(states.iter().all(|state| *state == Some(State::Spent)));
    assert!(saga.is_none());
    assert!(request_info.is_none());
}

/// Recovery across mint instances respects active ownership and expired leases.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_executor_cannot_override_replica_recovery() {
    setup_tracing();
    let gate = Arc::new(Notify::new());
    let backend = Arc::new(ControllableBackend::new(
        DispatchBehavior::GatedFailure(Arc::clone(&gate)),
        Amount::new(100, CurrencyUnit::Sat),
    ));
    backend.set_check_status(MeltQuoteState::Unknown);
    let localstore = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    let mint = create_mint_with_store(Arc::clone(&backend), localstore.clone()).await;
    let proofs = mint_proofs(&mint, Amount::from(1_000)).await;
    let input_ys = proofs.ys().unwrap();
    let invoice = create_fake_invoice(100_000, String::new());
    let quote_id = create_melt_quote(&mint, &invoice).await;
    let melt_request = MeltRequest::new(quote_id.clone(), proofs, None);
    let pending_melt = mint.melt(&melt_request).await.unwrap();
    wait_for_saga_state(&mint, MeltSagaState::PaymentAttempted).await;

    let owner = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    let replica = create_mint_with_store(Arc::clone(&backend), localstore).await;
    let response = replica.check_melt_quote(&quote_id).await.unwrap();
    assert_eq!(melt_quote_state(&response), MeltQuoteState::Pending);
    assert_eq!(
        mint.localstore()
            .get_melt_quote(&quote_id)
            .await
            .unwrap()
            .unwrap()
            .melt_lock,
        owner.melt_lock
    );
    assert!(replica.melt(&melt_request).await.is_err());

    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    assert!(tx
        .renew_melt_quote_lease(&quote_id, &owner.melt_lock, 0)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let response = replica.check_melt_quote(&quote_id).await.unwrap();
    assert_eq!(melt_quote_state(&response), MeltQuoteState::Pending);
    assert!(mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap()
        .iter()
        .all(|state| *state == Some(State::Pending)));

    gate.notify_one();
    let result = timeout(Duration::from_secs(10), pending_melt)
        .await
        .unwrap();
    assert!(matches!(result, Err(cdk::Error::MeltQuoteLocked)));
    let quote = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(quote.state, MeltQuoteState::Pending);
    assert!(!quote.is_locked());

    backend.set_check_status(MeltQuoteState::Paid);
    let response = replica.check_melt_quote(&quote_id).await.unwrap();
    assert_eq!(melt_quote_state(&response), MeltQuoteState::Paid);
    assert!(mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap()
        .iter()
        .all(|state| *state == Some(State::Spent)));
    mint.stop().await.unwrap();
    replica.stop().await.unwrap();
}

/// An executor that finishes with the payment outcome in doubt releases the
/// execution lock without finalizing: the quote stays Pending and a later
/// poll resolves it once the backend reports the outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_doubt_executor_releases_lock_and_later_poll_resolves() {
    setup_tracing();

    let backend = Arc::new(ControllableBackend::new(
        DispatchBehavior::Pending,
        Amount::new(100, CurrencyUnit::Sat),
    ));
    backend.set_check_status(MeltQuoteState::Pending);
    let mint = create_mint_with_backend(Arc::clone(&backend)).await;

    let proofs = mint_proofs(&mint, Amount::from(1_000)).await;
    let input_ys = proofs.ys().unwrap();
    let invoice = create_fake_invoice(100_000, String::new());
    let quote_id = create_melt_quote(&mint, &invoice).await;
    let melt_request = MeltRequest::new(quote_id.clone(), proofs, None);

    let pending_melt = mint.melt(&melt_request).await.unwrap();
    wait_for_saga_state(&mint, MeltSagaState::PaymentPending).await;

    // Wait for the executor's cleanup after its pending response is persisted.
    timeout(Duration::from_secs(10), async {
        loop {
            if !mint
                .localstore()
                .get_melt_quote(&quote_id)
                .await
                .unwrap()
                .unwrap()
                .is_locked()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let quote = mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .unwrap()
        .expect("quote should exist");
    assert_eq!(quote.state, MeltQuoteState::Pending);
    assert!(!quote.is_locked());

    // While the backend reports the payment in flight, polls keep the quote
    // Pending.
    let response = mint.check_melt_quote(&quote_id).await.unwrap();
    assert_eq!(melt_quote_state(&response), MeltQuoteState::Pending);

    // Once the backend reports the payment settled, a poll finalizes the
    // melt and spends the proofs.
    backend.set_check_status(MeltQuoteState::Paid);
    let response = mint.check_melt_quote(&quote_id).await.unwrap();
    assert_eq!(melt_quote_state(&response), MeltQuoteState::Paid);
    assert!(mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .unwrap()
        .iter()
        .all(|state| *state == Some(State::Spent)));

    // The background waiter observes the finalization too.
    let completion = timeout(Duration::from_secs(10), pending_melt)
        .await
        .expect("melt completion should finish")
        .expect("melt should complete successfully");
    assert_eq!(melt_quote_state(&completion), MeltQuoteState::Paid);
    mint.stop().await.unwrap();
}
