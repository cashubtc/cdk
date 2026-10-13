//! Disabling new melt quotes preserves issued quotes and payment recovery.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::{MeltSagaState, SagaStateEnum};
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState, MeltRequest};
use cdk_common::payment::{MintPayment, OutgoingPaymentOptions};
use cdk_common::{Amount, MeltQuoteBolt11Request, PaymentMethod};
use cdk_fake_wallet::{create_fake_invoice, FakeWallet};

use crate::mint::melt::melt_saga::MeltSaga;
use crate::mint::{Mint, MintBuilder, MintMeltLimits};
use crate::test_helpers::mint::mint_test_proofs;
use crate::types::{FeeReserve, QuoteTTL};
use crate::Error;

fn create_backend() -> FakeWallet {
    FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.02,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    )
}

async fn create_mint(backend: Arc<FakeWallet>) -> Mint {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
    let mut builder = MintBuilder::new(db.clone());
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::BOLT11,
            MintMeltLimits::new(1, 10_000),
            backend,
        )
        .await
        .unwrap();

    let mnemonic = bip39::Mnemonic::generate(12).unwrap();
    let mint = builder
        .with_name("melt disable contract test".to_string())
        .build_with_seed(db, &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
        .await
        .unwrap();
    mint.start().await.unwrap();
    mint
}

fn quote_request() -> MeltQuoteRequest {
    MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(9_000, "".to_string()),
        unit: CurrencyUnit::Sat,
        options: None,
    })
}

async fn set_melting_disabled(mint: &Mint, disabled: bool) {
    let mut info = mint.mint_info().await.unwrap();
    info.nuts.nut05.disabled = disabled;
    mint.set_mint_info(info).await.unwrap();
    assert_eq!(
        mint.mint_info().await.unwrap().nuts.nut05.disabled,
        disabled
    );
}

#[tokio::test]
async fn existing_quote_is_executable_while_disabled_and_new_quotes_resume_after_enabling() {
    let backend = Arc::new(create_backend());
    let mint = create_mint(backend).await;
    let proofs = mint_test_proofs(&mint, Amount::from(10)).await.unwrap();
    let quote = mint.get_melt_quote(quote_request()).await.unwrap();
    let quote_id = quote.quote().unwrap().clone();

    set_melting_disabled(&mint, true).await;

    // new quotes request get rejected
    assert!(matches!(
        mint.get_melt_quote(quote_request()).await,
        Err(Error::MeltingDisabled)
    ));

    assert_eq!(mint.melt_quotes().await.unwrap().len(), 1);

    assert_eq!(
        mint.check_melt_quote(&quote_id).await.unwrap().state(),
        MeltQuoteState::Unpaid
    );

    let request = MeltRequest::new(quote_id.clone(), proofs, None);
    let completed = mint.melt(&request).await.unwrap().await.unwrap();
    assert_eq!(completed.state(), MeltQuoteState::Paid);
    assert_eq!(
        mint.check_melt_quote(&quote_id).await.unwrap().state(),
        MeltQuoteState::Paid
    );

    set_melting_disabled(&mint, false).await;

    let new_quote = mint.get_melt_quote(quote_request()).await.unwrap();
    assert_ne!(new_quote.quote().unwrap(), &quote_id);
    assert_eq!(mint.melt_quotes().await.unwrap().len(), 2);

    mint.stop().await.unwrap();
}

#[tokio::test]
async fn dispatched_payment_retains_recovery_and_reconciliation_while_disabled() {
    let backend = Arc::new(create_backend());
    let mint = create_mint(backend.clone()).await;
    let proofs = mint_test_proofs(&mint, Amount::from(10)).await.unwrap();
    let quote = mint.get_melt_quote(quote_request()).await.unwrap();
    let quote_id = quote.quote().unwrap().clone();
    let request = MeltRequest::new(quote_id.clone(), proofs, None);
    let verification = mint.verify_inputs(request.inputs()).await.unwrap();

    let saga = MeltSaga::new(
        Arc::new(mint.clone()),
        mint.localstore(),
        mint.pubsub_manager(),
    );
    let setup = saga
        .setup_melt(&request, verification, PaymentMethod::BOLT11)
        .await
        .unwrap();

    drop(setup);

    // Reproduce a restart after dispatch, before its response is recorded by the mint.
    let store = mint.localstore();
    let stored_saga = store
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .unwrap()
        .unwrap();

    let mut tx = store.begin_transaction().await.unwrap();
    let mut stored_saga = tx
        .get_saga_for_update(&stored_saga.operation_id)
        .await
        .unwrap()
        .unwrap();

    tx.update_acquired_saga(
        &mut stored_saga,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
    )
    .await
    .unwrap();

    tx.commit().await.unwrap();

    let stored_quote = store.get_melt_quote(&quote_id).await.unwrap().unwrap();
    let payment = backend
        .make_payment(
            &CurrencyUnit::Sat,
            OutgoingPaymentOptions::from_melt_quote_with_fee(stored_quote).unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(payment.status, MeltQuoteState::Paid);

    set_melting_disabled(&mint, true).await;

    assert!(matches!(
        mint.get_melt_quote(quote_request()).await,
        Err(Error::MeltingDisabled)
    ));
    assert_eq!(
        mint.localstore()
            .get_melt_quote(&quote_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        MeltQuoteState::Pending
    );

    assert!(mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .unwrap()
        .is_some());

    mint.recover_from_incomplete_melt_sagas().await.unwrap();

    assert_eq!(
        mint.localstore()
            .get_melt_quote(&quote_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        MeltQuoteState::Paid,
        "recovery must persist settlement before the next status check"
    );

    assert_eq!(
        mint.check_melt_quote(&quote_id).await.unwrap().state(),
        MeltQuoteState::Paid
    );

    assert!(mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .unwrap()
        .is_none());

    assert!(mint.mint_info().await.unwrap().nuts.nut05.disabled);
    mint.stop().await.unwrap();
}
