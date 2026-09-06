//! Application-level async payment workflow tests.

use std::collections::HashSet;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, State};
use cdk::wallet::advanced::{ProofQuery, WalletBuilder};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::payment::{
    PaymentConfirmation, PaymentPlan, PaymentQuoteRequest, PaymentSession, PaymentState,
    PaymentTarget,
};
use cdk::wallet::Wallet;
use cdk::StreamExt;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8086";

async fn funded_wallet() -> Wallet {
    let wallet = WalletBuilder::new()
        .with_mint_url(MINT_URL.parse().expect("valid mint URL"))
        .with_unit(CurrencyUnit::Sat)
        .with_store(Arc::new(memory::empty().await.expect("in-memory database")))
        .with_seed(
            Mnemonic::generate(12)
                .expect("mnemonic")
                .to_seed_normalized(""),
        )
        .build()
        .expect("wallet");
    let session = wallet
        .request_mint(MintRequest::bolt11(100.into()))
        .await
        .expect("mint session");
    session
        .receipts(Default::default())
        .next()
        .await
        .expect("mint receipt")
        .expect("mint succeeds");
    assert_eq!(
        wallet.balance().await.expect("balance").available,
        100.into()
    );
    wallet
}

fn fake_invoice(pay_state: MeltQuoteState, check_state: MeltQuoteState) -> String {
    create_fake_invoice(
        50_000,
        serde_json::to_string(&FakeInvoiceDescription {
            pay_invoice_state: pay_state,
            check_payment_state: check_state,
            pay_err: false,
            check_err: false,
        })
        .expect("invoice description"),
    )
    .to_string()
}

async fn payment_session(wallet: &Wallet, invoice: String) -> PaymentSession {
    wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .expect("payment quote")
        .into_single()
        .expect("single payment quote")
}

async fn payment_plan(wallet: &Wallet, invoice: String) -> (PaymentSession, PaymentPlan) {
    let session = payment_session(wallet, invoice).await;
    let plan = session.prepare().await.expect("payment plan");
    (session, plan)
}

async fn proof_ys(wallet: &Wallet, states: Vec<State>) -> HashSet<cdk::nuts::PublicKey> {
    wallet
        .advanced()
        .proofs(ProofQuery {
            states,
            conditions: None,
        })
        .await
        .expect("proof records")
        .into_iter()
        .map(|record| record.y)
        .collect()
}

async fn operation_proof_ys(wallet: &Wallet, plan: &PaymentPlan) -> HashSet<cdk::nuts::PublicKey> {
    wallet
        .advanced()
        .proofs(ProofQuery::all())
        .await
        .expect("proof records")
        .into_iter()
        .filter(|record| record.used_by_operation == Some(plan.operation_id().as_uuid()))
        .map(|record| record.y)
        .collect()
}

async fn assert_completed_payment(wallet: &Wallet, spent: &HashSet<cdk::nuts::PublicKey>) {
    assert!(wallet.balance().await.expect("balance").available < 100.into());
    assert!(proof_ys(wallet, vec![State::Pending]).await.is_empty());
    let spent_after = proof_ys(wallet, vec![State::Spent]).await;
    assert!(spent.iter().all(|y| spent_after.contains(y)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn synchronous_payment_returns_a_receipt_and_finalizes_proofs() {
    let wallet = funded_wallet().await;
    let (_session, plan) = payment_plan(
        &wallet,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;
    let spent = operation_proof_ys(&wallet, &plan).await;

    let receipt = plan.execute().await.expect("payment receipt");

    assert_eq!(receipt.amount, 50.into());
    assert_completed_payment(&wallet, &spent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn async_confirmation_returns_pending_without_hiding_reserved_state() {
    let wallet = funded_wallet().await;
    let (_session, plan) = payment_plan(
        &wallet,
        fake_invoice(MeltQuoteState::Pending, MeltQuoteState::Pending),
    )
    .await;

    let confirmation = plan.submit().await.expect("async confirmation");

    assert!(matches!(confirmation, PaymentConfirmation::Pending(_)));
    assert!(!proof_ys(&wallet, vec![State::Pending]).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn pending_payment_handle_can_be_awaited() {
    let wallet = funded_wallet().await;
    let (_session, plan) = payment_plan(
        &wallet,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;
    let spent = operation_proof_ys(&wallet, &plan).await;

    let receipt = match plan.submit().await.expect("async confirmation") {
        PaymentConfirmation::Completed(_) => panic!("fake mint should acknowledge asynchronously"),
        PaymentConfirmation::Pending(pending) => pending.wait().await.expect("payment receipt"),
    };

    assert_eq!(receipt.amount, 50.into());
    assert_completed_payment(&wallet, &spent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn pending_payment_can_be_resumed_by_operation_id() {
    let wallet = funded_wallet().await;
    let (_session, plan) = payment_plan(
        &wallet,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;
    let operation_id = plan.operation_id();
    let spent = operation_proof_ys(&wallet, &plan).await;

    match plan.submit().await.expect("async confirmation") {
        PaymentConfirmation::Completed(_) => panic!("fake mint should acknowledge asynchronously"),
        PaymentConfirmation::Pending(_) => {}
    }

    let receipt = wallet
        .resume_pending_payment(operation_id)
        .await
        .expect("resumed pending payment")
        .wait()
        .await
        .expect("payment receipt");
    assert_eq!(receipt.operation_id, operation_id);
    assert_completed_payment(&wallet, &spent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn payment_quote_can_be_resumed_and_polled_by_typed_id() {
    let wallet = funded_wallet().await;
    let (session, plan) = payment_plan(
        &wallet,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;
    let quote_id = session.id();

    match plan.submit().await.expect("async confirmation") {
        PaymentConfirmation::Completed(_) => panic!("fake mint should acknowledge asynchronously"),
        PaymentConfirmation::Pending(_) => {}
    }

    let resumed = wallet
        .resume_payment_quote(quote_id)
        .await
        .expect("resumed payment session");
    let mut state = PaymentState::Unknown;
    for _ in 0..100 {
        state = resumed.refresh().await.expect("quote refresh").state;
        if matches!(state, PaymentState::Paid | PaymentState::Failed) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(state, PaymentState::Paid);
    assert!(wallet.balance().await.expect("balance").available < 100.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn sync_and_async_confirmation_reach_the_same_terminal_state() {
    let wallet_a = funded_wallet().await;
    let wallet_b = funded_wallet().await;
    let (_session_a, plan_a) = payment_plan(
        &wallet_a,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;
    let (_session_b, plan_b) = payment_plan(
        &wallet_b,
        fake_invoice(MeltQuoteState::Paid, MeltQuoteState::Paid),
    )
    .await;

    let sync_receipt = plan_a.execute().await.expect("sync payment receipt");
    let async_receipt = match plan_b.submit().await.expect("async confirmation") {
        PaymentConfirmation::Completed(receipt) => receipt,
        PaymentConfirmation::Pending(pending) => pending.wait().await.expect("async receipt"),
    };

    assert_eq!(sync_receipt.amount, async_receipt.amount);
    assert!(wallet_a.balance().await.expect("balance").available < 100.into());
    assert!(wallet_b.balance().await.expect("balance").available < 100.into());
    assert!(proof_ys(&wallet_a, vec![State::Pending]).await.is_empty());
    assert!(proof_ys(&wallet_b, vec![State::Pending]).await.is_empty());
}
