//! This file contains integration tests for the Cashu Development Kit (CDK)
//!
//! These tests verify the interaction between mint and wallet components, simulating real-world usage scenarios.
//! They test the complete flow of operations including wallet funding, token swapping, sending tokens between wallets,
//! and other operations that require client-mint interaction.

use std::assert_eq;
use std::collections::{HashMap, HashSet};
use std::hash::RandomState;
use std::str::FromStr;

use cashu::dhke::construct_proofs;
use cashu::mint_url::MintUrl;
use cashu::{
    CurrencyUnit, Id, MeltBolt11Request, NotificationPayload, PreMintSecrets, ProofState,
    SecretKey, SpendingConditions, State, SwapRequest,
};
use cdk::amount::SplitTarget;
use cdk::mint::Mint;
use cdk::nuts::nut00::ProofsMethods;
use cdk::subscription::{IndexableParams, Params};
use cdk::wallet::SendOptions;
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;

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
    let token = wallet_alice
        .send(prepared_send, None)
        .await
        .expect("Failed to send token");
    assert_eq!(
        Amount::from(40),
        token
            .proofs()
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
        HashSet::<_, RandomState>::from_iter(token.proofs().ys().expect("Failed to get ys")),
        HashSet::from_iter(
            wallet_alice
                .get_pending_spent_proofs()
                .await
                .expect("Failed to get pending spent proofs")
                .ys()
                .expect("Failed to get ys")
        )
    );

    // Alice sends cashu, Carol receives
    let wallet_carol = create_test_wallet_for_mint(mint_bob.clone())
        .await
        .expect("Failed to create Carol's wallet");
    let received_amount = wallet_carol
        .receive_proofs(token.proofs(), SplitTarget::None, &[], &[])
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

    let initial_mint_url = wallet_alice.mint_url.clone();
    let mint_info_before = wallet_alice
        .get_mint_info()
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

    let keys = mint_bob
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .clone()
        .keys;
    let keyset_id = Id::from(&keys);

    let preswap = PreMintSecrets::random(
        keyset_id,
        proofs.total_amount().unwrap(),
        &SplitTarget::default(),
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    let swap = mint_bob.process_swap_request(swap_request).await;
    assert!(swap.is_ok());

    let preswap_two = PreMintSecrets::random(
        keyset_id,
        proofs.total_amount().unwrap(),
        &SplitTarget::default(),
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

    let keys = mint_bob
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .clone()
        .keys;
    let keyset_id = Id::from(&keys);

    let pre_mint_amount =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default()).unwrap();
    let pre_mint_amount_two =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default()).unwrap();

    let mut pre_mint =
        PreMintSecrets::random(keyset_id, 1.into(), &SplitTarget::default()).unwrap();

    pre_mint.combine(pre_mint_amount);
    pre_mint.combine(pre_mint_amount_two);

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    match mint_bob.process_swap_request(swap_request).await {
        Ok(_) => panic!("Swap occurred with overflow"),
        Err(err) => match err {
            cdk::Error::NUT03(cdk::nuts::nut03::Error::Amount(_)) => (),
            cdk::Error::AmountOverflow => (),
            cdk::Error::AmountError(_) => (),
            _ => {
                println!("{:?}", err);
                panic!("Wrong error returned in swap overflow")
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

    // Try to swap for less than the input amount (95 < 100)
    let preswap = PreMintSecrets::random(keyset_id, 95.into(), &SplitTarget::default())
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
    let preswap = PreMintSecrets::random(keyset_id, 101.into(), &SplitTarget::default())
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

    let pre_swap = PreMintSecrets::with_conditions(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &spending_conditions,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let keys = mint_bob
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .cloned()
        .unwrap()
        .keys;

    let post_swap = mint_bob.process_swap_request(swap_request).await.unwrap();

    let mut proofs = construct_proofs(
        post_swap.signatures,
        pre_swap.rs(),
        pre_swap.secrets(),
        &keys,
    )
    .unwrap();

    let pre_swap = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default()).unwrap();

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
        .pubsub_manager
        .try_subscribe::<IndexableParams>(
            Params {
                kind: cdk::nuts::nut17::Kind::ProofState,
                filters: public_keys_to_listen.clone(),
                id: "test".into(),
            }
            .into(),
        )
        .await
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

    let mut msgs = HashMap::new();
    while let Ok((sub_id, msg)) = listener.try_recv() {
        assert_eq!(sub_id, "test".into());
        match msg {
            NotificationPayload::ProofState(ProofState { y, state, .. }) => {
                msgs.entry(y.to_string())
                    .or_insert_with(Vec::new)
                    .push(state);
            }
            _ => panic!("Wrong message received"),
        }
    }

    for keys in public_keys_to_listen {
        let statuses = msgs.remove(&keys).expect("some events");
        // Every input pk receives two state updates, as there are only two state transitions
        assert_eq!(statuses, vec![State::Pending, State::Spent]);
    }

    assert!(listener.try_recv().is_err(), "no other event is happening");
    assert!(msgs.is_empty(), "Only expected key events are received");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_overpay_underpay_fee() {
    setup_tracing();
    let mint_bob = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    mint_bob
        .rotate_keyset(CurrencyUnit::Sat, 1, 32, 1, &HashMap::new())
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

    let keys = mint_bob
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .clone()
        .keys;
    let keyset_id = Id::from(&keys);

    let preswap = PreMintSecrets::random(keyset_id, 9998.into(), &SplitTarget::default()).unwrap();

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

    let preswap = PreMintSecrets::random(keyset_id, 1000.into(), &SplitTarget::default()).unwrap();

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
        .rotate_keyset(CurrencyUnit::Sat, 1, 32, 1, &HashMap::new())
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

    let keys = mint_bob
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .clone()
        .keys;
    let keyset_id = Id::from(&keys);

    let five_proofs: Vec<_> = proofs.drain(..5).collect();

    let preswap = PreMintSecrets::random(keyset_id, 5.into(), &SplitTarget::default()).unwrap();

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

    let preswap = PreMintSecrets::random(keyset_id, 4.into(), &SplitTarget::default()).unwrap();

    let swap_request = SwapRequest::new(five_proofs.clone(), preswap.blinded_messages());

    let res = mint_bob.process_swap_request(swap_request).await;

    assert!(res.is_ok());

    let thousnad_proofs: Vec<_> = proofs.drain(..1001).collect();

    let preswap = PreMintSecrets::random(keyset_id, 1000.into(), &SplitTarget::default()).unwrap();

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

    let preswap = PreMintSecrets::random(keyset_id, 999.into(), &SplitTarget::default()).unwrap();

    let swap_request = SwapRequest::new(thousnad_proofs.clone(), preswap.blinded_messages());

    let _ = mint_bob.process_swap_request(swap_request).await.unwrap();
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

    // Create 3 identical swap requests with the same proofs
    let preswap1 = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())
        .expect("Failed to create preswap");
    let swap_request1 = SwapRequest::new(proofs.clone(), preswap1.blinded_messages());

    let preswap2 = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())
        .expect("Failed to create preswap");
    let swap_request2 = SwapRequest::new(proofs.clone(), preswap2.blinded_messages());

    let preswap3 = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())
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
        .localstore
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

    let melt_request = MeltBolt11Request::new(quote_id.parse().unwrap(), proofs.clone(), None);
    let melt_request2 = melt_request.clone();
    let melt_request3 = melt_request.clone();

    // Spawn 3 concurrent tasks to process the melt requests
    let task1 = tokio::spawn(async move { mint_clone1.melt_bolt11(&melt_request).await });

    let task2 = tokio::spawn(async move { mint_clone2.melt_bolt11(&melt_request2).await });

    let task3 = tokio::spawn(async move { mint_clone3.melt_bolt11(&melt_request3).await });

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
        .localstore
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
    let keys = mint
        .pubkeys()
        .await
        .unwrap()
        .keysets
        .first()
        .unwrap()
        .clone()
        .keys;
    Id::from(&keys)
}
