//! Tests for the melt execution lock.
//!
//! Covers:
//! - Arming the lock when a melt quote goes Pending
//! - Status checks leaving a locked quote untouched
//! - Token-holder-only finalization and rollback
//! - Executor release when the payment outcome is in doubt
//! - Startup release of orphaned locks

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::{MeltSagaState, OperationKind, Saga, SagaStateEnum};
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{
    CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MeltRequest, ProofsMethods, State,
};
use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
use cdk_common::{Amount, Bolt11Invoice, PaymentMethod, QuoteId};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};

use crate::mint::melt::melt_saga::MeltSaga;
use crate::mint::melt::shared;
use crate::mint::saga_recovery;
use crate::mint::{MeltQuote, Mint, MintBuilder, MintMeltLimits};
use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
use crate::types::{FeeReserve, QuoteTTL};
use crate::Error;

const QUOTE_AMOUNT: u64 = 9_000;
const INPUT_AMOUNT: u64 = 10_000;

fn fake_invoice(pay_state: MeltQuoteState, check_state: MeltQuoteState) -> Bolt11Invoice {
    let description = FakeInvoiceDescription {
        pay_invoice_state: pay_state,
        check_payment_state: check_state,
        pay_err: false,
        check_err: false,
    };
    let amount_msats: u64 = Amount::from(QUOTE_AMOUNT).into();
    create_fake_invoice(amount_msats, serde_json::to_string(&description).unwrap())
}

/// Creates a test mint whose fake backend reports `check_state` for the given
/// invoice from `check_outgoing_payment`.
async fn create_mint_with_check_state(
    invoice: &Bolt11Invoice,
    check_state: MeltQuoteState,
) -> Mint {
    let payment_states = HashMap::from([(
        invoice.payment_hash().to_string(),
        (
            check_state,
            Amount::from(QUOTE_AMOUNT).with_unit(CurrencyUnit::Sat),
        ),
    )]);

    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    let mut mint_builder = MintBuilder::new(db.clone());
    let backend = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        },
        payment_states,
        std::collections::HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(backend),
        )
        .await
        .unwrap();
    let mnemonic = bip39::Mnemonic::generate(12).unwrap();
    let mint = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint for melt lock tests".to_string())
        .with_urls(vec!["https://test-mint".to_string()])
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();
    mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
        .await
        .unwrap();
    mint.start().await.unwrap();
    mint
}

struct PendingMeltSetup {
    quote: MeltQuote,
    melt_request: MeltRequest<QuoteId>,
    input_ys: Vec<cdk_common::PublicKey>,
    operation_id: uuid::Uuid,
}

/// Runs TX1 of the melt saga and drops the saga, leaving the quote Pending
/// with the execution lock armed, as an interrupted executor would.
async fn setup_interrupted_melt(mint: &Mint, invoice: &Bolt11Invoice) -> PendingMeltSetup {
    let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice.clone(),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let quote_response = mint.get_melt_quote(request).await.unwrap();
    let quote = mint
        .localstore()
        .get_melt_quote(quote_response.quote().unwrap())
        .await
        .unwrap()
        .expect("quote should exist");

    let proofs = mint_test_proofs(mint, Amount::from(INPUT_AMOUNT))
        .await
        .unwrap();
    let input_ys = proofs.ys().unwrap();
    let melt_request = MeltRequest::new(quote.id.clone(), proofs, None);
    let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
    let saga = MeltSaga::new(
        Arc::new(mint.clone()),
        mint.localstore(),
        mint.pubsub_manager(),
    );
    let setup_saga = saga
        .setup_melt(
            &melt_request,
            verification,
            PaymentMethod::Known(KnownMethod::Bolt11),
        )
        .await
        .unwrap();

    let operation_id = single_melt_saga_operation_id(mint).await;
    drop(setup_saga);

    PendingMeltSetup {
        quote,
        melt_request,
        input_ys,
        operation_id,
    }
}

async fn set_saga_state(mint: &Mint, operation_id: &uuid::Uuid, state: MeltSagaState) {
    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    let mut saga = tx
        .get_saga_for_update(operation_id)
        .await
        .unwrap()
        .expect("saga should exist");
    tx.update_acquired_saga(&mut saga, SagaStateEnum::Melt(state))
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn stored_quote(mint: &Mint, quote_id: &QuoteId) -> MeltQuote {
    mint.localstore()
        .get_melt_quote(quote_id)
        .await
        .unwrap()
        .expect("quote should exist")
}

async fn proofs_state(mint: &Mint, ys: &[cdk_common::PublicKey]) -> Vec<Option<State>> {
    mint.localstore().get_proofs_states(ys).await.unwrap()
}

async fn single_melt_saga_operation_id(mint: &Mint) -> uuid::Uuid {
    let sagas = mint
        .localstore()
        .get_incomplete_sagas(OperationKind::Melt)
        .await
        .unwrap();
    assert_eq!(sagas.len(), 1, "expected exactly one melt saga");
    sagas[0].operation_id
}

/// A status check on a locked Pending quote must return it as-is: the payment
/// backend is not consulted and the reserved proofs are not released, even
/// when the backend would report an authoritative failure. Once the lock
/// holder releases the lock, a later status check resolves the quote.
#[tokio::test]
async fn status_poll_leaves_locked_quote_untouched_until_unlocked() {
    let invoice = fake_invoice(MeltQuoteState::Failed, MeltQuoteState::Failed);
    let mint = create_mint_with_check_state(&invoice, MeltQuoteState::Failed).await;
    let setup = setup_interrupted_melt(&mint, &invoice).await;

    // The lock is armed with the saga operation id when the quote goes Pending.
    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert!(stored.is_locked());
    assert_eq!(stored.melt_lock, setup.operation_id.to_string());

    // Simulate an interrupted dispatcher.
    set_saga_state(&mint, &setup.operation_id, MeltSagaState::PaymentPending).await;

    // A status poll must not consult the backend: if it did, the
    // authoritative failure would roll the melt back.
    let mut quote = stored_quote(&mint, &setup.quote.id).await;
    mint.handle_pending_melt_quote(&mut quote).await.unwrap();

    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Pending);
    assert!(stored.is_locked());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| *state == Some(State::Pending)));

    // The reserved proofs cannot be melted again.
    let err = mint.melt(&setup.melt_request).await.unwrap_err();
    assert!(matches!(err, Error::TokenPending | Error::PendingQuote));

    // The lock holder releases the lock without finalizing; the quote stays
    // Pending and a later poll resolves it.
    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    assert!(tx
        .unlock_melt_quote(&setup.quote.id, &setup.operation_id.to_string())
        .await
        .unwrap());
    tx.commit().await.unwrap();

    let mut quote = stored_quote(&mint, &setup.quote.id).await;
    mint.handle_pending_melt_quote(&mut quote).await.unwrap();

    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    assert!(!stored.is_locked());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| state.is_none()));
}

/// Finalizing or rolling back with a lock token that does not match the held
/// lock is refused and leaves the melt untouched; the token holder succeeds.
#[tokio::test]
async fn finalize_and_rollback_with_wrong_lock_token_are_refused() {
    let invoice = fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid);
    let mint = create_test_mint().await.unwrap();
    let setup = setup_interrupted_melt(&mint, &invoice).await;

    let wrong_operation_id = uuid::Uuid::now_v7();
    let stored = stored_quote(&mint, &setup.quote.id).await;

    // Finalization by a non-holder is refused.
    let mut foreign_quote = stored.clone();
    foreign_quote.melt_lock = wrong_operation_id.to_string();
    let err = shared::finalize_melt_quote(
        &mint,
        &mint.localstore(),
        &mint.pubsub_manager(),
        &foreign_quote,
        stored.amount(),
        None,
        &stored
            .request_lookup_id
            .clone()
            .expect("bolt11 quote should have a lookup id"),
        Some(wrong_operation_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::MeltQuoteLocked));

    // Rollback by a non-holder is refused, even with a saga record for the
    // wrong token.
    let foreign_saga = Saga::new_melt(
        wrong_operation_id,
        MeltSagaState::PaymentFailed,
        setup.quote.id.to_string(),
    );
    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    tx.add_saga(&foreign_saga).await.unwrap();
    tx.commit().await.unwrap();

    let err = shared::rollback_failed_melt_quote(
        &mint.localstore(),
        &mint.pubsub_manager(),
        &setup.quote.id,
        &setup.input_ys,
        &[],
        &wrong_operation_id,
        &wrong_operation_id.to_string(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::MeltQuoteLocked));

    // The refused attempts changed nothing.
    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Pending);
    assert_eq!(stored.melt_lock, setup.operation_id.to_string());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| *state == Some(State::Pending)));

    // The lock holder rolls the melt back and the lock is released with the
    // state change.
    set_saga_state(&mint, &setup.operation_id, MeltSagaState::PaymentFailed).await;
    shared::rollback_failed_melt_quote(
        &mint.localstore(),
        &mint.pubsub_manager(),
        &setup.quote.id,
        &setup.input_ys,
        &[],
        &setup.operation_id,
        &setup.operation_id.to_string(),
    )
    .await
    .unwrap();

    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    assert!(!stored.is_locked());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| state.is_none()));
}

/// When the backend leaves the payment outcome in doubt, the executor
/// releases the execution lock without finalizing: the quote stays Pending
/// and a later status resolution can finalize it.
#[tokio::test]
async fn in_doubt_payment_releases_lock_and_later_poll_resolves() {
    let invoice = fake_invoice(MeltQuoteState::Pending, MeltQuoteState::Pending);
    let mint = create_mint_with_check_state(&invoice, MeltQuoteState::Pending).await;

    let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice.clone(),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let quote_response = mint.get_melt_quote(request).await.unwrap();
    let quote = mint
        .localstore()
        .get_melt_quote(quote_response.quote().unwrap())
        .await
        .unwrap()
        .expect("quote should exist");
    let proofs = mint_test_proofs(&mint, Amount::from(INPUT_AMOUNT))
        .await
        .unwrap();
    let input_ys = proofs.ys().unwrap();
    let melt_request = MeltRequest::new(quote.id.clone(), proofs, None);
    let pending = mint.melt(&melt_request).await.unwrap();
    assert!(matches!(
        pending.await,
        Err(Error::PendingMeltTimeout { .. })
    ));

    // The executor released the lock without finalizing.
    let stored = stored_quote(&mint, &quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Pending);
    assert!(!stored.is_locked());

    // While the backend stays pending, so does the quote.
    let mut quote = stored_quote(&mint, &quote.id).await;
    mint.handle_pending_melt_quote(&mut quote).await.unwrap();
    assert_eq!(quote.state, MeltQuoteState::Pending);

    // A later paid outcome resolves the unlocked quote.
    let saga = mint
        .localstore()
        .get_incomplete_sagas(OperationKind::Melt)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("saga should exist");
    let paid_response = MakePaymentResponse {
        payment_lookup_id: PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref()),
        payment_proof: Some("preimage".to_string()),
        status: MeltQuoteState::Paid,
        total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
    };
    saga_recovery::process_melt_saga_outcome(
        &saga,
        &mut quote,
        &paid_response,
        &mint.localstore(),
        &mint.pubsub_manager(),
        &mint,
    )
    .await
    .unwrap();

    let stored = stored_quote(&mint, &quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Paid);
    assert!(!stored.is_locked());
    assert!(proofs_state(&mint, &input_ys)
        .await
        .iter()
        .all(|state| *state == Some(State::Spent)));
}

/// Startup respects live ownership and reconciles only after its deadline.
#[tokio::test]
async fn startup_recovers_only_expired_leases() {
    let invoice = fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid);
    let mint = create_test_mint().await.unwrap();
    let setup = setup_interrupted_melt(&mint, &invoice).await;

    mint.recover_from_incomplete_melt_sagas().await.unwrap();
    let current = stored_quote(&mint, &setup.quote.id).await;
    assert!(current.is_locked());
    assert_eq!(current.melt_lock, setup.operation_id.to_string());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| *state == Some(State::Pending)));

    let mut tx = mint.localstore().begin_transaction().await.unwrap();
    assert!(tx
        .renew_melt_quote_lease(&setup.quote.id, &setup.operation_id.to_string(), 0)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    mint.recover_from_incomplete_melt_sagas().await.unwrap();
    let stored = stored_quote(&mint, &setup.quote.id).await;
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    assert!(!stored.is_locked());
    assert!(proofs_state(&mint, &setup.input_ys)
        .await
        .iter()
        .all(|state| state.is_none()));
}
