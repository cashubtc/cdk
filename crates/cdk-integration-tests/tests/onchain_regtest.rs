//! Onchain Regtest Integration Tests
//!
//! This file contains tests for NUT-26 onchain payments against a regtest environment.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::nuts::{CurrencyUnit, NotificationPayload, PaymentMethod, Proofs, ProofsMethods};
use cdk::wallet::{MeltOutcome, Wallet, WalletSubscription};
use cdk_integration_tests::init_regtest::init_bitcoin_client;
use cdk_integration_tests::get_mint_url_from_env;
use cdk_sqlite::wallet::memory;
use futures::StreamExt;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    assert!(mint_quote.request.starts_with("bcrt1"));
    println!("Mint address: {}", mint_quote.request);

    // 2. Subscribe to notifications for this quote
    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![
            mint_quote.id.clone()
        ]))
        .await
        .expect("failed to subscribe");

    // 3. Send bitcoin to the mint address
    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("Failed to send bitcoin");

    // 4. Mine a block to confirm the transaction
    let mine_addr = bitcoin_client.get_new_address().expect("Failed to get address");
    bitcoin_client.generate_blocks(&mine_addr, 1).expect("Failed to mine block");

    // 5. Wait for paid notification
    // The mint checks for confirmations. Since we set num_confs=1 in settings, 1 block should be enough.
    let mut paid_amount = cdk::amount::Amount::from(0);
    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        paid_amount = response.amount_paid;
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");

    assert_eq!(paid_amount, mint_amount.into());

    // 6. Mint the tokens
    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        )
        .await
        .expect("Failed to mint");

    assert_eq!(proofs.total_amount().unwrap(), mint_amount.into());
    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 50_000;

    // 1. Fund the wallet via onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![
            mint_quote.id.clone()
        ]))
        .await
        .expect("failed to subscribe");

    let mut paid_amount = cdk::amount::Amount::from(0);
    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        paid_amount = response.amount_paid;
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");

    assert_eq!(paid_amount, mint_amount.into());

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Request onchain melt options
    let dest_addr = bitcoin_client.get_new_address().unwrap();
    let melt_amount = 20_000;

    let melt_quotes = wallet
        .quote_onchain_melt_options(&dest_addr.to_string(), melt_amount.into(), None)
        .await
        .expect("Failed to get melt quotes");

    assert!(!melt_quotes.is_empty());
    let melt_quote = wallet
        .select_onchain_melt_quote(melt_quotes[0].clone())
        .await
        .expect("Failed to select melt quote");

    println!("Melt quote selected: {:?}", melt_quote);

    // 3. Prepare and confirm melt
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("Failed to prepare melt");

    // We need to mine blocks for the transaction to confirm.
    // We will generate a block every second until the confirm future completes.
    let _melt_result = timeout(Duration::from_secs(60), async {
        let confirm_future = prepared.confirm();
        tokio::pin!(confirm_future);
        loop {
            tokio::select! {
                res = &mut confirm_future => {
                    return res.expect("Failed to confirm melt");
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for melt confirmation");

    // Check balance
    let remaining_balance = wallet.total_balance().await.unwrap();
    // Balance should be mint_amount - melt_amount - fee
    assert!(remaining_balance < (mint_amount - melt_amount).into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_restore() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 20_000;

    // 1. Fund the wallet via onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Create a new wallet instance with the same seed
    let wallet_2 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    assert_eq!(wallet_2.total_balance().await.unwrap(), 0.into());

    // 3. Restore the wallet
    let restored = wallet_2.restore().await.unwrap();
    assert_eq!(restored.unspent, mint_amount.into());
    assert_eq!(wallet_2.total_balance().await.unwrap(), mint_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_multiple_payments() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let total_amount = 30_000;
    let payment_1 = 10_000;
    let payment_2 = 20_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(total_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    // 2. Send first bitcoin payment
    bitcoin_client
        .send_to_address(&mint_quote.request, payment_1)
        .expect("Failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // 3. Send second bitcoin payment
    bitcoin_client
        .send_to_address(&mint_quote.request, payment_2)
        .expect("Failed to send bitcoin");

    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let mut stream = wallet.proof_stream(mint_quote, SplitTarget::default(), None);
    let mut proofs = Proofs::new();

    while proofs.total_amount().unwrap() < total_amount.into() {
        if let Some(Ok(p)) = stream.next().await {
            proofs.extend(p);
        }
    }

    assert_eq!(proofs.total_amount().unwrap(), total_amount.into());
    assert_eq!(wallet.total_balance().await.unwrap(), total_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt_prefer_async() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 50_000;

    // Fund wallet
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .unwrap();

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    // 2. Request onchain melt
    let dest_addr = bitcoin_client.get_new_address().unwrap();
    let melt_amount = 20_000;

    let melt_quotes = wallet
        .quote_onchain_melt_options(&dest_addr.to_string(), melt_amount.into(), None)
        .await
        .unwrap();

    let melt_quote = wallet
        .select_onchain_melt_quote(melt_quotes[0].clone())
        .await
        .unwrap();

    // 3. Confirm with prefer async
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    let outcome = prepared.confirm_prefer_async().await.unwrap();

    match outcome {
        MeltOutcome::Pending(pending) => {
            // We need to mine blocks for the transaction to confirm.
            // We will generate a block every second until the finalized future completes.
            let finalized = timeout(Duration::from_secs(60), async {
                let mut finalized_future = Box::pin(std::future::IntoFuture::into_future(pending));
                loop {
                    tokio::select! {
                        res = &mut finalized_future => break res.unwrap(),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            let mine_addr = bitcoin_client.get_new_address().unwrap();
                            bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                        }
                    }
                }
            })
            .await
            .expect("Melt timed out");

            assert_eq!(finalized.state(), cdk::nuts::MeltQuoteState::Paid);
        }
        MeltOutcome::Paid(_) => panic!("Expected pending outcome for onchain melt"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_underpaid() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let quote_amount = 20_000;
    let actual_paid = 15_000;

    // 1. Request a mint quote
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(quote_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    // 2. Underpay the quote
    bitcoin_client
        .send_to_address(&mint_quote.request, actual_paid)
        .unwrap();

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // 3. Mint what was paid
    // Let's poll until amount_paid is correct
    let mut quote = mint_quote.clone();
    for _ in 0..30 {
        quote = wallet.check_mint_quote_status(&quote.id).await.unwrap();
        if quote.amount_paid >= actual_paid.into() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    assert!(quote.amount_paid >= actual_paid.into());

    let proofs = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), actual_paid.into());
    assert_eq!(wallet.total_balance().await.unwrap(), actual_paid.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_unique_addresses() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request first mint quote
    let mint_quote_1 = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get first mint quote");

    // 2. Request second mint quote
    let mint_quote_2 = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get second mint quote");

    // 3. Verify addresses are unique
    assert_ne!(
        mint_quote_1.request, mint_quote_2.request,
        "Mint quotes should have unique addresses"
    );

    assert!(mint_quote_1.request.starts_with("bcrt1"));
    assert!(mint_quote_2.request.starts_with("bcrt1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_mint_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount_1 = 10_000;
    let mint_amount_2 = 20_000;
    let mint_amount_3 = 30_000;

    // Request 3 quotes concurrently
    let (quote_1, quote_2, quote_3) = tokio::try_join!(
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_1.into()),
            None,
            None,
        ),
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_2.into()),
            None,
            None,
        ),
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_3.into()),
            None,
            None,
        ),
    )
    .expect("Failed to get mint quotes");

    // Pay all 3
    bitcoin_client
        .send_to_address(&quote_1.request, mint_amount_1)
        .expect("failed to send bitcoin 1");
    bitcoin_client
        .send_to_address(&quote_2.request, mint_amount_2)
        .expect("failed to send bitcoin 2");
    bitcoin_client
        .send_to_address(&quote_3.request, mint_amount_3)
        .expect("failed to send bitcoin 3");

    // Mine 1 block to confirm all 3 transactions together
    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // Mint all 3 concurrently
    let (proofs_1, proofs_2, proofs_3) = tokio::try_join!(
        wallet.wait_and_mint_quote(
            quote_1,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
        wallet.wait_and_mint_quote(
            quote_2,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
        wallet.wait_and_mint_quote(
            quote_3,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
    )
    .expect("Failed to mint concurrently");

    assert_eq!(proofs_1.total_amount().unwrap(), mint_amount_1.into());
    assert_eq!(proofs_2.total_amount().unwrap(), mint_amount_2.into());
    assert_eq!(proofs_3.total_amount().unwrap(), mint_amount_3.into());

    let total_expected = mint_amount_1 + mint_amount_2 + mint_amount_3;
    assert_eq!(wallet.total_balance().await.unwrap(), total_expected.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_melt_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 200_000;

    // 1. Fund the wallet with a large onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Prepare 3 destinations and request options
    let dest_1 = bitcoin_client.get_new_address().unwrap();
    let dest_2 = bitcoin_client.get_new_address().unwrap();
    let dest_3 = bitcoin_client.get_new_address().unwrap();
    
    let melt_amount_1 = 20_000;
    let melt_amount_2 = 30_000;
    let melt_amount_3 = 40_000;

    // Request options sequentially because quote creation isn't the complex state mutation
    let options_1 = wallet
        .quote_onchain_melt_options(&dest_1.to_string(), melt_amount_1.into(), None)
        .await
        .unwrap();
    let options_2 = wallet
        .quote_onchain_melt_options(&dest_2.to_string(), melt_amount_2.into(), None)
        .await
        .unwrap();
    let options_3 = wallet
        .quote_onchain_melt_options(&dest_3.to_string(), melt_amount_3.into(), None)
        .await
        .unwrap();

    let melt_quote_1 = wallet.select_onchain_melt_quote(options_1[0].clone()).await.unwrap();
    let melt_quote_2 = wallet.select_onchain_melt_quote(options_2[0].clone()).await.unwrap();
    let melt_quote_3 = wallet.select_onchain_melt_quote(options_3[0].clone()).await.unwrap();

    // 3. Prepare melts concurrently to stress input selection
    let (prep_1, prep_2, prep_3) = tokio::try_join!(
        wallet.prepare_melt(&melt_quote_1.id, std::collections::HashMap::new()),
        wallet.prepare_melt(&melt_quote_2.id, std::collections::HashMap::new()),
        wallet.prepare_melt(&melt_quote_3.id, std::collections::HashMap::new()),
    ).expect("Failed to prepare melts concurrently");

    // 4. Confirm concurrently
    timeout(Duration::from_secs(120), async {
        let conf_1 = prep_1.confirm();
        let conf_2 = prep_2.confirm();
        let conf_3 = prep_3.confirm();
        
        tokio::pin!(conf_1);
        tokio::pin!(conf_2);
        tokio::pin!(conf_3);
        
        let mut confirmed_1 = false;
        let mut confirmed_2 = false;
        let mut confirmed_3 = false;
        
        loop {
            if confirmed_1 && confirmed_2 && confirmed_3 {
                break;
            }
            
            tokio::select! {
                res = &mut conf_1, if !confirmed_1 => {
                    res.expect("Failed conf 1");
                    confirmed_1 = true;
                }
                res = &mut conf_2, if !confirmed_2 => {
                    res.expect("Failed conf 2");
                    confirmed_2 = true;
                }
                res = &mut conf_3, if !confirmed_3 => {
                    res.expect("Failed conf 3");
                    confirmed_3 = true;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for multiple melts");

    // Balance should be reduced by the melts and their fees
    let final_balance = wallet.total_balance().await.unwrap();
    let total_melted = melt_amount_1 + melt_amount_2 + melt_amount_3;
    assert!(final_balance < (mint_amount - total_melted).into());
}
