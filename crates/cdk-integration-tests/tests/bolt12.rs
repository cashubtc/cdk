use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::amount::SplitTarget;
use cashu::nut23::Amountless;
use cashu::{Amount, CurrencyUnit, MintRequest, MintUrl, PreMintSecrets, ProofsMethods};
use cdk::wallet::{HttpClient, MintConnector, Wallet, WalletBuilder};
use cdk_integration_tests::get_mint_url_from_env;
use cdk_integration_tests::init_regtest::{get_cln_dir, get_temp_dir};
use cdk_sqlite::wallet::memory;
use ln_regtest_rs::ln_client::ClnClient;

// Helper function to get temp directory from environment or fallback
fn get_test_temp_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => get_temp_dir(), // fallback to default
    }
}

// Helper function to create CLN client with retries
async fn create_cln_client_with_retry(cln_dir: PathBuf) -> Result<ClnClient> {
    let mut retries = 0;
    let max_retries = 10;
    loop {
        match ClnClient::new(cln_dir.clone(), None).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                retries += 1;
                if retries >= max_retries {
                    bail!(
                        "Could not connect to CLN client after {} retries: {}",
                        max_retries,
                        e
                    );
                }
                println!(
                    "Failed to connect to CLN (attempt {}/{}): {}. Retrying in 7 seconds...",
                    retries, max_retries, e
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;
            }
        }
    }
}

/// Tests basic BOLT12 minting functionality:
/// - Creates a wallet
/// - Gets a BOLT12 quote for a specific amount (100 sats)
/// - Pays the quote using Core Lightning
/// - Mints tokens and verifies the correct amount is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .unwrap();

    let mint_amount = Amount::from(100);

    let mint_quote = wallet
        .mint_bolt12_quote(Some(mint_amount), None)
        .await
        .unwrap();

    assert_eq!(mint_quote.amount, Some(mint_amount));

    let work_dir = get_test_temp_dir();
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_client = create_cln_client_with_retry(cln_one_dir.clone())
        .await
        .unwrap();
    cln_client
        .pay_bolt12_offer(None, mint_quote.request)
        .await
        .unwrap();

    let proofs = wallet
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), 100.into());
}

/// Tests multiple payments to a single BOLT12 quote:
/// - Creates a wallet and gets a BOLT12 quote without specifying amount
/// - Makes two separate payments (10,000 sats and 11,000 sats) to the same quote
/// - Verifies that each payment can be minted separately and correctly
/// - Tests the functionality of reusing a quote for multiple payments
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_multiple() -> Result<()> {
    let mint_url = MintUrl::from_str(&get_mint_url_from_env())?;

    let wallet = WalletBuilder::new()
        .mint_url(mint_url)
        .unit(CurrencyUnit::Sat)
        .localstore(Arc::new(memory::empty().await?))
        .seed(Mnemonic::generate(12)?.to_seed_normalized(""))
        .target_proof_count(3)
        .use_http_subscription()
        .build()?;

    let mint_quote = wallet.mint_bolt12_quote(None, None).await?;

    let work_dir = get_test_temp_dir();
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_client = create_cln_client_with_retry(cln_one_dir.clone()).await?;
    cln_client
        .pay_bolt12_offer(Some(10000), mint_quote.request.clone())
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await?;

    assert_eq!(proofs.total_amount().unwrap(), 10.into());

    cln_client
        .pay_bolt12_offer(Some(11_000), mint_quote.request.clone())
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await?;

    assert_eq!(proofs.total_amount().unwrap(), 11.into());

    Ok(())
}

/// Tests that multiple wallets can pay the same BOLT12 offer:
/// - Creates a BOLT12 offer through CLN that both wallets will pay
/// - Creates two separate wallets with different minting amounts
/// - Has each wallet get their own quote and make payments
/// - Verifies both wallets can successfully mint their tokens
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_multiple_wallets() -> Result<()> {
    // Create first wallet
    let wallet_one = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    // Create second wallet
    let wallet_two = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    // Create a BOLT12 offer that both wallets will use
    let work_dir = get_test_temp_dir();
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_client = create_cln_client_with_retry(cln_one_dir.clone()).await?;
    // First wallet payment
    let quote_one = wallet_one
        .mint_bolt12_quote(Some(10_000.into()), None)
        .await?;
    cln_client
        .pay_bolt12_offer(None, quote_one.request.clone())
        .await?;

    let proofs_one = wallet_one
        .wait_and_mint_quote(
            quote_one.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await?;

    assert_eq!(proofs_one.total_amount()?, 10_000.into());

    // Second wallet payment
    let quote_two = wallet_two
        .mint_bolt12_quote(Some(15_000.into()), None)
        .await?;
    cln_client
        .pay_bolt12_offer(None, quote_two.request.clone())
        .await?;

    let proofs_two = wallet_two
        .wait_and_mint_quote(
            quote_two.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await?;

    assert_eq!(proofs_two.total_amount()?, 15_000.into());

    let offer = cln_client
        .get_bolt12_offer(None, false, "test_multiple_wallets".to_string())
        .await?;

    let wallet_one_melt_quote = wallet_one
        .melt_bolt12_quote(
            offer.to_string(),
            Some(cashu::MeltOptions::Amountless {
                amountless: Amountless {
                    amount_msat: 1500.into(),
                },
            }),
        )
        .await?;

    let wallet_two_melt_quote = wallet_two
        .melt_bolt12_quote(
            offer.to_string(),
            Some(cashu::MeltOptions::Amountless {
                amountless: Amountless {
                    amount_msat: 1000.into(),
                },
            }),
        )
        .await?;

    let melted = wallet_one.melt(&wallet_one_melt_quote.id).await?;

    assert!(melted.preimage.is_some());

    let melted_two = wallet_two.melt(&wallet_two_melt_quote.id).await?;

    assert!(melted_two.preimage.is_some());

    Ok(())
}

/// Tests the BOLT12 melting (spending) functionality:
/// - Creates a wallet and mints 20,000 sats using BOLT12
/// - Creates a BOLT12 offer for 10,000 sats
/// - Tests melting (spending) tokens using the BOLT12 offer
/// - Verifies the correct amount is melted
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_melt() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(20_000);

    // Create a single-use BOLT12 quote
    let mint_quote = wallet.mint_bolt12_quote(Some(mint_amount), None).await?;

    assert_eq!(mint_quote.amount, Some(mint_amount));
    // Pay the quote
    let work_dir = get_test_temp_dir();
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_client = create_cln_client_with_retry(cln_one_dir.clone()).await?;
    cln_client
        .pay_bolt12_offer(None, mint_quote.request.clone())
        .await?;

    let _proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await?;

    let offer = cln_client
        .get_bolt12_offer(Some(10_000), true, "hhhhhhhh".to_string())
        .await?;

    let quote = wallet.melt_bolt12_quote(offer.to_string(), None).await?;

    let melt = wallet.melt(&quote.id).await?;

    assert_eq!(melt.amount, 10.into());

    Ok(())
}

/// Tests security validation for BOLT12 minting to prevent overspending:
/// - Creates a wallet and gets an open-ended BOLT12 quote
/// - Makes a payment of 10,000 millisats
/// - Attempts to mint more tokens (500 sats) than were actually paid for
/// - Verifies that the mint correctly rejects the oversized mint request
/// - Ensures proper error handling with TransactionUnbalanced error
/// This test is crucial for ensuring the economic security of the minting process
/// by preventing users from minting more tokens than they have paid for.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_extra() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    // Create a single-use BOLT12 quote
    let mint_quote = wallet.mint_bolt12_quote(None, None).await?;

    let state = wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    assert_eq!(state.amount_paid, Amount::ZERO);
    assert_eq!(state.amount_issued, Amount::ZERO);

    let active_keyset_id = wallet.fetch_active_keyset().await?.id;

    let pay_amount_msats = 10_000;

    let work_dir = get_test_temp_dir();
    let cln_one_dir = get_cln_dir(&work_dir, "one");
    let cln_client = create_cln_client_with_retry(cln_one_dir.clone()).await?;
    cln_client
        .pay_bolt12_offer(Some(pay_amount_msats), mint_quote.request.clone())
        .await?;

    let payment = wallet
        .wait_for_payment(&mint_quote, tokio::time::Duration::from_secs(15))
        .await?
        .unwrap();

    let state = wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    assert_eq!(payment, state.amount_paid);
    assert_eq!(state.amount_paid, (pay_amount_msats / 1_000).into());
    assert_eq!(state.amount_issued, Amount::ZERO);

    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        500.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )?;

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await?
        .expect("there is a quote");

    let mut mint_request = MintRequest {
        quote: mint_quote.id,
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(secret_key)?;
    }

    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    let response = http_client.post_mint(mint_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed second payment");
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_mint_unpaid() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let mint_quote = wallet
        .mint_bolt12_quote(Some(mint_amount), None)
        .await
        .unwrap();

    assert_eq!(mint_quote.amount, Some(mint_amount));

    let proofs = wallet
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await;

    match proofs {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }

    let mint_quote = wallet
        .mint_bolt12_quote(Some(mint_amount), None)
        .await
        .unwrap();

    let state = wallet
        .mint_bolt12_quote_state(&mint_quote.id)
        .await
        .unwrap();

    assert!(state.amount_paid == Amount::ZERO);

    let proofs = wallet
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await;

    match proofs {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }
}
