//! This file contains integration tests for the Cashu Development Kit (CDK)
//!
//! These tests verify the interaction between mint and wallet components, simulating real-world usage scenarios.
//! They test the complete flow of operations including wallet funding, token swapping, sending tokens between wallets,
//! and other operations that require client-mint interaction.
//!
//! Test Environment:
//! - Uses pure in-memory mint instances for fast execution
//! - Tests run concurrently with multi-threaded tokio runtime
//! - No external dependencies (Lightning nodes, databases) required

use std::assert_eq;
use std::collections::{HashMap, HashSet};
use std::hash::RandomState;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cashu::amount::SplitTarget;
use cashu::dhke::construct_proofs;
use cashu::mint_url::MintUrl;
use cashu::{
    CurrencyUnit, Id, MeltRequest, NotificationPayload, PreMintSecrets, ProofState, SecretKey,
    SpendingConditions, State, SwapRequest,
};
use cdk::mint::Mint;
use cdk::nuts::nut00::ProofsMethods;
use cdk::subscription::Params;
use cdk::wallet::types::{TransactionDirection, TransactionId};
use cdk::wallet::{ReceiveOptions, SendMemo, SendOptions};
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;
use tokio::time::sleep;

/// Tests the token swap and send functionality:
/// 1. Alice gets funded with 64 sats
/// 2. Alice prepares to send 40 sats (which requires internal swapping)
/// 3. Alice sends the token
/// 4. Carol receives the token and has the correct balance
#[tokio::test]
async fn test_swap_to_send() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64, None)
        .await
        .expect("Failed to fund wallet");
    let balance_alice = wallet_alice
        .total_balance()
        .await
        .expect("Failed to get balance");
    assert_eq!(Amount::from(64), balance_alice);

    // Alice wants to send 40 sats, which internally swaps
    let prepared_send = wallet_alice
        .prepare_send(Amount::from(40), SendOptions::default())
        .await
        .expect("Failed to prepare send");
    assert_eq!(
        HashSet::<_, RandomState>::from_iter(
            prepared_send.proofs().ys().expect("Failed to get ys")
        ),
        HashSet::from_iter(
            wallet_alice
                .get_reserved_proofs()
                .await
                .expect("Failed to get reserved proofs")
                .ys()
                .expect("Failed to get ys")
        )
    );
    let token = prepared_send
        .confirm(Some(SendMemo::for_token("test_swapt_to_send")))
        .await
        .expect("Failed to send token");
    let keysets_info = wallet_alice.get_mint_keysets().await.unwrap();
    let token_proofs = token.proofs(&keysets_info).unwrap();
    assert_eq!(
        Amount::from(40),
        token_proofs
            .total_amount()
            .expect("Failed to get total amount")
    );
    assert_eq!(
        Amount::from(24),
        wallet_alice
            .total_balance()
            .await
            .expect("Failed to get balance")
    );
    assert_eq!(
        HashSet::<_, RandomState>::from_iter(token_proofs.ys().expect("Failed to get ys")),
        HashSet::from_iter(
            wallet_alice
                .get_pending_spent_proofs()
                .await
                .expect("Failed to get pending spent proofs")
                .ys()
                .expect("Failed to get ys")
        )
    );

    let transaction_id =
        TransactionId::from_proofs(token_proofs.clone()).expect("Failed to get tx id");

    let transaction = wallet_alice
        .get_transaction(transaction_id)
        .await
        .expect("Failed to get transaction")
        .expect("Transaction not found");
    assert_eq!(wallet_alice.mint_url, transaction.mint_url);
    assert_eq!(TransactionDirection::Outgoing, transaction.direction);
    assert_eq!(Amount::from(40), transaction.amount);
    assert_eq!(Amount::from(0), transaction.fee);
    assert_eq!(CurrencyUnit::Sat, transaction.unit);
    assert_eq!(token_proofs.ys().unwrap(), transaction.ys);

    // Alice sends cashu, Carol receives
    let wallet_carol = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create Carol's wallet");
    let received_amount = wallet_carol
        .receive_proofs(
            token_proofs.clone(),
            ReceiveOptions::default(),
            token.memo().clone(),
        )
        .await
        .expect("Failed to receive proofs");

    assert_eq!(Amount::from(40), received_amount);
    assert_eq!(
        Amount::from(40),
        wallet_carol
            .total_balance()
            .await
            .expect("Failed to get Carol's balance")
    );

    let transaction = wallet_carol
        .get_transaction(transaction_id)
        .await
        .expect("Failed to get transaction")
        .expect("Transaction not found");
    assert_eq!(wallet_carol.mint_url, transaction.mint_url);
    assert_eq!(TransactionDirection::Incoming, transaction.direction);
    assert_eq!(Amount::from(40), transaction.amount);
    assert_eq!(Amount::from(0), transaction.fee);
    assert_eq!(CurrencyUnit::Sat, transaction.unit);
    assert_eq!(token_proofs.ys().unwrap(), transaction.ys);
    assert_eq!(token.memo().clone(), transaction.memo);
}

/// Tests the NUT-06 functionality (mint discovery):
/// 1. Alice gets funded with 64 sats
/// 2. Verifies the initial mint URL is in the mint info
/// 3. Updates the mint URL to a new value
/// 4. Verifies the wallet balance is maintained after changing the mint URL
#[tokio::test]
async fn test_mint_nut06() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let mut wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64, None)
        .await
        .expect("Failed to fund wallet");
    let balance_alice = wallet_alice
        .total_balance()
        .await
        .expect("Failed to get balance");
    assert_eq!(Amount::from(64), balance_alice);

    let transaction = wallet_alice
        .list_transactions(None)
        .await
        .expect("Failed to list transactions")
        .pop()
        .expect("No transactions found");
    assert_eq!(wallet_alice.mint_url, transaction.mint_url);
    assert_eq!(TransactionDirection::Incoming, transaction.direction);
    assert_eq!(Amount::from(64), transaction.amount);
    assert_eq!(Amount::from(0), transaction.fee);
    assert_eq!(CurrencyUnit::Sat, transaction.unit);

    let initial_mint_url = wallet_alice.mint_url.clone();
    let mint_info_before = wallet_alice
        .fetch_mint_info()
        .await
        .expect("Failed to get mint info")
        .unwrap();
    assert!(mint_info_before
        .urls
        .unwrap()
        .contains(&initial_mint_url.to_string()));

    // Wallet updates mint URL
    let new_mint_url = MintUrl::from_str("https://new-mint-url").expect("Failed to parse mint URL");
    wallet_alice
        .update_mint_url(new_mint_url.clone())
        .await
        .expect("Failed to update mint URL");

    // Check balance after mint URL was updated
    let balance_alice_after = wallet_alice
        .total_balance()
        .await
        .expect("Failed to get balance after URL update");
    assert_eq!(Amount::from(64), balance_alice_after);
}

/// Attempt to double spend proofs on swap
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_double_spend() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keys = mint_bob.pubkeys().keysets.first().unwrap().clone();
    let keyset_id = keys.id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let preswap = PreMintSecrets::random(
        keyset_id,
        proofs.total_amount().unwrap(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    let swap = mint_bob.process_swap_request(swap_request).await;
    assert!(swap.is_ok());

    let preswap_two = PreMintSecrets::random(
        keyset_id,
        proofs.total_amount().unwrap(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_two_request = SwapRequest::new(proofs, preswap_two.blinded_messages());

    match mint_bob.process_swap_request(swap_two_request).await {
        Ok(_) => panic!("Proofs double spent"),
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            _ => panic!("Wrong error returned"),
        },
    }
}

/// This attempts to swap for more outputs then inputs.
/// This will work if the mint does not check for outputs amounts overflowing
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_swap_by_overflowing() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let amount = 2_u64.pow(63);

    let keys = mint_bob.pubkeys().keysets.first().unwrap().clone();
    let keyset_id = keys.id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint_amount = PreMintSecrets::random(
        keyset_id,
        amount.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();
    let pre_mint_amount_two = PreMintSecrets::random(
        keyset_id,
        amount.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let mut pre_mint = PreMintSecrets::random(
        keyset_id,
        1.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    pre_mint.combine(pre_mint_amount);
    pre_mint.combine(pre_mint_amount_two);

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap occurred with overflow"),
        Err(err) => match err {
            cdk::Error::NUT03(cdk::nuts::nut03::Error::Amount(_)) => (),
            cdk::Error::AmountOverflow => (),
            cdk::Error::AmountError(_) => (),
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                panic!("Wrong error returned in swap overflow {:?}", err);
            }
        },
    }
}

/// Tests that the mint correctly rejects unbalanced swap requests:
/// 1. Attempts to swap for less than the input amount (95 < 100)
/// 2. Attempts to swap for more than the input amount (101 > 100)
/// 3. Both should fail with TransactionUnbalanced error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_unbalanced() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(wallet_alice.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint_bob).await;

    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Try to swap for less than the input amount (95 < 100)
    let preswap = PreMintSecrets::random(
        keyset_id,
        95.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => panic!("Wrong error returned"),
        },
    }

    // Try to swap for more than the input amount (101 > 100)
    let preswap = PreMintSecrets::random(
        keyset_id,
        101.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => panic!("Wrong error returned"),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_p2pk_swap() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(wallet_alice.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint_bob).await;

    let secret = SecretKey::generate();

    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_swap = PreMintSecrets::with_conditions(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &spending_conditions,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let keys = mint_bob.pubkeys().keysets.first().cloned().unwrap().keys;

    let post_swap = mint_bob.process_swap_request(swap_request).await.unwrap();

    let mut proofs = construct_proofs(
        post_swap.signatures,
        pre_swap.rs(),
        pre_swap.secrets(),
        &keys,
    )
    .unwrap();

    let pre_swap = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    // Listen for status updates on all input proof pks
    let public_keys_to_listen: Vec<_> = swap_request
        .inputs()
        .ys()
        .unwrap()
        .iter()
        .map(|pk| pk.to_string())
        .collect();

    let mut listener = mint_bob
        .pubsub_manager()
        .subscribe(Params {
            kind: cdk::nuts::nut17::Kind::ProofState,
            filters: public_keys_to_listen.clone(),
            id: Arc::new("test".into()),
        })
        .expect("valid subscription");

    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Proofs spent without sig"),
        Err(err) => match err {
            cdk::Error::NUT11(cdk::nuts::nut11::Error::SignaturesNotProvided) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned")
            }
        },
    }

    for proof in &mut proofs {
        proof.sign_p2pk(secret.clone()).unwrap();
    }

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let attempt_swap = mint_bob.process_swap_request(swap_request).await;

    assert!(attempt_swap.is_ok());

    sleep(Duration::from_secs(1)).await;

    let mut msgs = HashMap::new();
    while let Some(msg) = listener.try_recv() {
        match msg.into_inner() {
            NotificationPayload::ProofState(ProofState { y, state, .. }) => {
                msgs.entry(y.to_string())
                    .or_insert_with(Vec::new)
                    .push(state);
            }
            _ => panic!("Wrong message received"),
        }
    }

    for (i, key) in public_keys_to_listen.into_iter().enumerate() {
        let statuses = msgs.remove(&key).expect("some events");
        // Every input pk receives two state updates, as there are only two state transitions
        assert_eq!(
            statuses,
            vec![State::Pending, State::Spent],
            "failed to test key {:?} (pos {})",
            key,
            i,
        );
    }

    assert!(listener.try_recv().is_none(), "no other event is happening");
    assert!(msgs.is_empty(), "Only expected key events are received");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_overpay_underpay_fee() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    mint_bob
        .rotate_keyset(CurrencyUnit::Sat, 32, 1)
        .await
        .unwrap();

    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(wallet_alice.clone(), 1000, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keys = mint_bob.pubkeys().keysets.first().unwrap().clone().keys;
    let keyset_id = Id::v1_from_keys(&keys);
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let preswap = PreMintSecrets::random(
        keyset_id,
        9998.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Attempt to swap overpaying fee
    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(
        keyset_id,
        1000.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned")
            }
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_enforce_fee() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    mint_bob
        .rotate_keyset(CurrencyUnit::Sat, 32, 1)
        .await
        .unwrap();

    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(
        wallet_alice.clone(),
        1010,
        Some(SplitTarget::Value(Amount::ONE)),
    )
    .await
    .expect("Failed to fund wallet");

    let mut proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keys = mint_bob.pubkeys().keysets.first().unwrap().clone();
    let keyset_id = keys.id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let five_proofs: Vec<_> = proofs.drain(..5).collect();

    let preswap = PreMintSecrets::random(
        keyset_id,
        5.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(five_proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(
        keyset_id,
        4.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(five_proofs.clone(), preswap.blinded_messages());

    let res = mint_bob.process_swap_request(swap_request).await;

    assert!(res.is_ok());

    let thousnad_proofs: Vec<_> = proofs.drain(..1001).collect();

    let preswap = PreMintSecrets::random(
        keyset_id,
        1000.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(thousnad_proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(
        keyset_id,
        999.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(thousnad_proofs.clone(), preswap.blinded_messages());

    let _ = mint_bob.process_swap_request(swap_request).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_change_with_fee_melt() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    mint_bob
        .rotate_keyset(CurrencyUnit::Sat, 32, 1)
        .await
        .unwrap();

    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(
        wallet_alice.clone(),
        100,
        Some(SplitTarget::Value(Amount::ONE)),
    )
    .await
    .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let fake_invoice = create_fake_invoice(1000, "".to_string());

    let melt_quote = wallet_alice
        .melt_quote(fake_invoice.to_string(), None)
        .await
        .unwrap();

    let w = wallet_alice
        .melt_proofs(&melt_quote.id, proofs)
        .await
        .unwrap();

    assert_eq!(w.change.unwrap().total_amount().unwrap(), 97.into());
}
/// Tests concurrent double-spending attempts by trying to use the same proofs
/// in 3 swap transactions simultaneously using tokio tasks
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn test_concurrent_double_spend_swap() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(wallet_alice.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint_bob).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create 3 identical swap requests with the same proofs
    let preswap1 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");
    let swap_request1 = SwapRequest::new(proofs.clone(), preswap1.blinded_messages());

    let preswap2 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");
    let swap_request2 = SwapRequest::new(proofs.clone(), preswap2.blinded_messages());

    let preswap3 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");
    let swap_request3 = SwapRequest::new(proofs.clone(), preswap3.blinded_messages());

    // Spawn 3 concurrent tasks to process the swap requests
    let mint_clone1 = mint_bob.clone();
    let mint_clone2 = mint_bob.clone();
    let mint_clone3 = mint_bob.clone();

    let task1 = tokio::spawn(async move { mint_clone1.process_swap_request(swap_request1).await });

    let task2 = tokio::spawn(async move { mint_clone2.process_swap_request(swap_request2).await });

    let task3 = tokio::spawn(async move { mint_clone3.process_swap_request(swap_request3).await });

    // Wait for all tasks to complete
    let results = tokio::try_join!(task1, task2, task3).expect("Tasks failed to complete");

    // Count successes and failures
    let mut success_count = 0;
    let mut token_already_spent_count = 0;

    for result in [results.0, results.1, results.2] {
        match result {
            Ok(_) => success_count += 1,
            Err(err) => match err {
                cdk::Error::TokenAlreadySpent | cdk::Error::TokenPending => {
                    token_already_spent_count += 1
                }
                other_err => panic!("Unexpected error: {:?}", other_err),
            },
        }
    }

    // Only one swap should succeed, the other two should fail with TokenAlreadySpent
    assert_eq!(1, success_count, "Expected exactly one successful swap");
    assert_eq!(
        2, token_already_spent_count,
        "Expected exactly two TokenAlreadySpent errors"
    );

    // Verify that all proofs are marked as spent in the mint
    let states = mint_bob
        .localstore()
        .get_proofs_states(&proofs.iter().map(|p| p.y().unwrap()).collect::<Vec<_>>())
        .await
        .expect("Failed to get proof state");

    for state in states {
        assert_eq!(
            State::Spent,
            state.expect("Known state"),
            "Expected proof to be marked as spent, but got {:?}",
            state
        );
    }
}

/// Tests concurrent double-spending attempts by trying to use the same proofs
/// in 3 melt transactions simultaneously using tokio tasks
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn test_concurrent_double_spend_melt() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create test wallet");

    // Alice gets 100 sats
    fund_wallet(wallet_alice.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet_alice
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    // Create a Lightning invoice for the melt
    let invoice = create_fake_invoice(1000, "".to_string());

    // Create a melt quote
    let melt_quote = wallet_alice
        .melt_quote(invoice.to_string(), None)
        .await
        .expect("Failed to create melt quote");

    // Get the quote ID and payment request
    let quote_id = melt_quote.id.clone();

    // Create 3 identical melt requests with the same proofs
    let mint_clone1 = mint_bob.clone();
    let mint_clone2 = mint_bob.clone();
    let mint_clone3 = mint_bob.clone();

    let melt_request = MeltRequest::new(quote_id.parse().unwrap(), proofs.clone(), None);
    let melt_request2 = melt_request.clone();
    let melt_request3 = melt_request.clone();

    // Spawn 3 concurrent tasks to process the melt requests
    let task1 = tokio::spawn(async move { mint_clone1.melt(&melt_request).await });

    let task2 = tokio::spawn(async move { mint_clone2.melt(&melt_request2).await });

    let task3 = tokio::spawn(async move { mint_clone3.melt(&melt_request3).await });

    // Wait for all tasks to complete
    let results = tokio::try_join!(task1, task2, task3).expect("Tasks failed to complete");

    // Count successes and failures
    let mut success_count = 0;
    let mut token_already_spent_count = 0;

    for result in [results.0, results.1, results.2] {
        match result {
            Ok(_) => success_count += 1,
            Err(err) => match err {
                cdk::Error::TokenAlreadySpent | cdk::Error::TokenPending => {
                    token_already_spent_count += 1;
                    println!("Got expected error: {:?}", err);
                }
                other_err => {
                    println!("Got unexpected error: {:?}", other_err);
                    token_already_spent_count += 1;
                }
            },
        }
    }

    // Only one melt should succeed, the other two should fail
    assert_eq!(1, success_count, "Expected exactly one successful melt");
    assert_eq!(
        2, token_already_spent_count,
        "Expected exactly two TokenAlreadySpent errors"
    );

    // Verify that all proofs are marked as spent in the mint
    let states = mint_bob
        .localstore()
        .get_proofs_states(&proofs.iter().map(|p| p.y().unwrap()).collect::<Vec<_>>())
        .await
        .expect("Failed to get proof state");

    for state in states {
        assert_eq!(
            State::Spent,
            state.expect("Known state"),
            "Expected proof to be marked as spent, but got {:?}",
            state
        );
    }
}

async fn get_keyset_id(mint: &Mint) -> Id {
    let keys = mint.pubkeys().keysets.first().unwrap().clone();
    keys.verify_id()
        .expect("Keyset ID generation is successful");
    keys.id
}
