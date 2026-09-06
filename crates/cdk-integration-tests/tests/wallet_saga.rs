//! Application-facing durability and concurrency tests for wallet workflows.

use std::collections::HashSet;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::nuts::{CurrencyUnit, State};
use cdk::wallet::advanced::{
    ProofQuery, ReissueFeePolicy, ReissueProtection, ReissueRequest, TransactionRecovery,
};
use cdk::wallet::history::HistoryQuery;
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_common::wallet::TransactionDirection;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;

async fn reserved_proof_ys(wallet: &Wallet) -> Result<HashSet<cdk::nuts::PublicKey>> {
    Ok(wallet
        .advanced()
        .proofs(ProofQuery {
            states: vec![State::Reserved],
            conditions: None,
        })
        .await?
        .into_iter()
        .map(|record| record.y)
        .collect())
}

async fn operation_proof_ys(
    wallet: &Wallet,
    operation_id: cdk::wallet::operation::OperationId,
) -> Result<HashSet<cdk::nuts::PublicKey>> {
    Ok(wallet
        .advanced()
        .proofs(ProofQuery::all())
        .await?
        .into_iter()
        .filter(|record| record.used_by_operation == Some(operation_id.as_uuid()))
        .map(|record| record.y)
        .collect())
}

async fn payment_plan(
    wallet: &Wallet,
    amount_msat: u64,
    description: &str,
) -> Result<cdk::wallet::payment::PaymentPlan> {
    let invoice = create_fake_invoice(amount_msat, description.to_string());
    Ok(wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(
            invoice.to_string(),
        )))
        .await?
        .into_single()?
        .prepare()
        .await?)
}

#[tokio::test]
async fn cancelling_send_releases_reserved_funds() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    let initial = Amount::from(1_000);
    fund_wallet(wallet.clone(), initial.into(), None).await?;

    let plan = wallet.plan_send(SendRequest::new(400.into())).await?;
    assert!(!reserved_proof_ys(&wallet).await?.is_empty());
    assert!(wallet.balance().await?.reserved > Amount::ZERO);

    plan.cancel().await?;

    assert!(reserved_proof_ys(&wallet).await?.is_empty());
    let balance = wallet.balance().await?;
    assert_eq!(balance.available, initial);
    assert_eq!(balance.reserved, Amount::ZERO);
    Ok(())
}

#[tokio::test]
async fn prepared_sends_own_disjoint_proofs() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    fund_wallet(wallet.clone(), 600, None).await?;

    let first = wallet.plan_send(SendRequest::new(300.into())).await?;
    let second = wallet.plan_send(SendRequest::new(300.into())).await?;
    let first_ys = operation_proof_ys(&wallet, first.operation_id()).await?;
    let second_ys = operation_proof_ys(&wallet, second.operation_id()).await?;
    assert!(!first_ys.is_empty());
    assert!(!second_ys.is_empty());
    assert!(first_ys.is_disjoint(&second_ys));
    assert!(wallet
        .plan_send(SendRequest::new(100.into()))
        .await
        .is_err());

    first.cancel().await?;
    let third = wallet.plan_send(SendRequest::new(100.into())).await?;
    second.cancel().await?;
    third.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_sends_reserve_and_confirm_independently() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    let initial = Amount::from(2_000);
    fund_wallet(wallet.clone(), initial.into(), None).await?;

    let (first, second) = tokio::join!(
        wallet.plan_send(SendRequest::new(300.into())),
        wallet.plan_send(SendRequest::new(400.into()))
    );
    let first = first?;
    let second = second?;
    let first_ys = operation_proof_ys(&wallet, first.operation_id()).await?;
    let second_ys = operation_proof_ys(&wallet, second.operation_id()).await?;
    assert!(first_ys.is_disjoint(&second_ys));

    let (first_receipt, second_receipt) = tokio::join!(first.execute(), second.execute());
    assert_eq!(first_receipt?.amount, 300.into());
    assert_eq!(second_receipt?.amount, 400.into());
    assert_eq!(
        wallet.balance().await?.available,
        initial - Amount::from(700)
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_payments_are_isolated() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    fund_wallet(wallet.clone(), 2_000, None).await?;

    let first = payment_plan(&wallet, 200_000, "payment 1").await?;
    let second = payment_plan(&wallet, 300_000, "payment 2").await?;
    let first_ys = operation_proof_ys(&wallet, first.operation_id()).await?;
    let second_ys = operation_proof_ys(&wallet, second.operation_id()).await?;
    assert!(first_ys.is_disjoint(&second_ys));

    let (first_receipt, second_receipt) = tokio::join!(first.execute(), second.execute());
    assert_eq!(first_receipt?.amount, 200.into());
    assert_eq!(second_receipt?.amount, 300.into());
    assert!(wallet.balance().await?.available < 1_500.into());
    Ok(())
}

#[tokio::test]
async fn payment_plan_accounts_for_input_fees() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        1_000,
        true,
        None,
    )
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fund_wallet(wallet.clone(), 500, None).await?;

    let initial = wallet.balance().await?.available;
    let receipt = payment_plan(&wallet, 100_000, "input fee regression")
        .await?
        .execute()
        .await?;

    assert_eq!(receipt.amount, 100.into());
    assert!(wallet.balance().await?.available < initial);
    Ok(())
}

#[tokio::test]
async fn payment_plan_handles_many_non_optimal_proofs() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        100,
        true,
        None,
    )
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fund_wallet(wallet.clone(), 200, Some(SplitTarget::Value(Amount::ONE))).await?;
    assert!(wallet.advanced().proofs(ProofQuery::default()).await?.len() > 50);

    let receipt = payment_plan(&wallet, 100_000, "non-optimal proofs")
        .await?
        .execute()
        .await?;

    assert_eq!(receipt.amount, 100.into());
    assert!(wallet.balance().await?.available < 100.into());
    Ok(())
}

#[tokio::test]
async fn prepared_payment_survives_sync_until_explicit_cancel() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    let initial = Amount::from(500);
    fund_wallet(wallet.clone(), initial.into(), None).await?;

    let plan = payment_plan(&wallet, 100_000, "durable plan").await?;
    let operation_id = plan.operation_id();
    assert!(wallet.balance().await?.reserved > Amount::ZERO);

    let report = wallet.synchronize(SyncPolicy::Online).await?;
    assert!(report.pending_operations >= 1);
    let resumed = wallet.resume_payment(operation_id).await?;
    assert_eq!(resumed.operation_id(), operation_id);

    resumed.cancel().await?;
    assert!(wallet.resume_payment(operation_id).await.is_err());
    let balance = wallet.balance().await?;
    assert_eq!(balance.available, initial);
    assert_eq!(balance.reserved, Amount::ZERO);
    Ok(())
}

#[tokio::test]
async fn send_with_internal_reissue_succeeds() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    let initial = Amount::from(1_000);
    fund_wallet(
        wallet.clone(),
        initial.into(),
        Some(SplitTarget::Value(Amount::from(64))),
    )
    .await?;

    let receipt = wallet
        .plan_send(SendRequest::new(100.into()))
        .await?
        .execute()
        .await?;

    assert_eq!(receipt.amount, 100.into());
    assert!(!receipt.token.to_string().is_empty());
    assert!(wallet.balance().await?.available < initial);
    Ok(())
}

#[tokio::test]
async fn send_with_internal_reissue_can_be_received() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let sender = create_test_wallet_for_mint(mint.clone()).await?;
    let receiver = create_test_wallet_for_mint(mint).await?;
    fund_wallet(
        sender.clone(),
        1_000,
        Some(SplitTarget::Value(Amount::from(64))),
    )
    .await?;

    let token = sender
        .plan_send(SendRequest::new(100.into()))
        .await?
        .execute()
        .await?
        .token;
    let receipt = receiver
        .receive(ReceiveRequest::new(token.to_string()))
        .await?;

    assert_eq!(receipt.amount, 100.into());
    assert_eq!(receiver.balance().await?.available, 100.into());
    Ok(())
}

#[tokio::test]
async fn unclaimed_send_can_be_inspected_and_recovered_by_transaction() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    let initial = Amount::from(500);
    fund_wallet(wallet.clone(), initial.into(), None).await?;

    let send = wallet
        .plan_send(SendRequest::new(100.into()))
        .await?
        .execute()
        .await?;
    let history = wallet
        .history(HistoryQuery {
            direction: Some(TransactionDirection::Outgoing),
            limit: Some(1),
        })
        .await?;
    let transaction = history.first().expect("confirmed send has history");
    assert_eq!(transaction.operation_id, Some(send.operation_id));

    let details = wallet
        .advanced()
        .transaction_details(transaction.id)
        .await?
        .expect("transaction details remain inspectable");
    assert_eq!(details.transaction.amount, send.amount);
    assert!(!details.proofs.is_empty());

    let recovery = wallet
        .advanced()
        .recover_transaction(transaction.id)
        .await?;
    assert!(matches!(
        recovery,
        TransactionRecovery::SendReclaimed { amount } if amount >= send.amount
    ));
    assert_eq!(wallet.balance().await?.available, initial);
    Ok(())
}

#[tokio::test]
async fn expert_reissue_remains_available_without_exposing_swap_sagas() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint).await?;
    fund_wallet(wallet.clone(), 100, None).await?;
    let proofs = wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await?
        .into_iter()
        .map(|record| record.proof)
        .collect::<Vec<_>>();

    let receipt = wallet
        .advanced()
        .reissue(ReissueRequest {
            proofs,
            amount: None,
            amount_split_target: SplitTarget::default(),
            conditions: None,
            fee_policy: ReissueFeePolicy::Deduct,
            protection: ReissueProtection::Plain,
        })
        .await?;

    assert!(receipt.proofs.is_none());
    assert_eq!(wallet.balance().await?.available, 100.into());
    Ok(())
}
