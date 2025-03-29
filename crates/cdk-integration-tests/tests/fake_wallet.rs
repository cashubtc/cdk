use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CurrencyUnit, MeltBolt11Request, MeltQuoteState, MintBolt11Request, PreMintSecrets, Proofs,
    SecretKey, State, SwapRequest,
};
use cdk::wallet::{HttpClient, MintConnector, Wallet};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_integration_tests::{attempt_to_swap_pending, wait_for_mint_to_be_paid};
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8086";

/// Tests that when both pay and check return pending status, input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_tokens_pending() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await;

    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await?;

    Ok(())
}

/// Tests that if the pay error fails and the check returns unknown or failed,
/// the input proofs should be unset as spending (returned to unspent state)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    // The mint should have unset proofs from pending since payment failed
    let all_proof = wallet.get_unspent_proofs().await?;
    let states = wallet.check_proofs_spent(all_proof).await?;
    for state in states {
        assert!(state.state == State::Unspent);
    }

    let wallet_bal = wallet.total_balance().await?;
    assert_eq!(wallet_bal, 100.into());

    Ok(())
}

/// Tests that when both the pay_invoice and check_invoice both fail,
/// the proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail_and_check() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: true,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(!pending.is_empty());

    Ok(())
}

/// Tests that when the ln backend returns a failed status but does not error,
/// the mint should do a second check, then remove proofs from pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_return_fail_status() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(pending.is_empty());

    Ok(())
}

/// Tests that when the ln backend returns an error with unknown status,
/// the mint should do a second check, then remove proofs from pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_error_unknown() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert_eq!(melt.unwrap_err().to_string(), "Payment failed");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert_eq!(melt.unwrap_err().to_string(), "Payment failed");

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(pending.is_empty());

    Ok(())
}

/// Tests that when the ln backend returns an error but the second check returns paid,
/// proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_err_paid() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await?;

    Ok(())
}

/// Tests that the correct database type is used based on environment variables
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_database_type() -> Result<()> {
    // Get the database type and work dir from environment
    let db_type = std::env::var("CDK_MINTD_DATABASE").expect("MINT_DATABASE env var should be set");
    let work_dir =
        std::env::var("CDK_MINTD_WORK_DIR").expect("CDK_MINTD_WORK_DIR env var should be set");

    // Check that the correct database file exists
    match db_type.as_str() {
        "REDB" => {
            let db_path = std::path::Path::new(&work_dir).join("cdk-mintd.redb");
            assert!(
                db_path.exists(),
                "Expected redb database file to exist at {:?}",
                db_path
            );
        }
        "SQLITE" => {
            let db_path = std::path::Path::new(&work_dir).join("cdk-mintd.sqlite");
            assert!(
                db_path.exists(),
                "Expected sqlite database file to exist at {:?}",
                db_path
            );
        }
        "MEMORY" => {
            // Memory database has no file to check
            println!("Memory database in use - no file to check");
        }
        _ => bail!("Unknown database type: {}", db_type),
    }

    Ok(())
}

/// Tests minting tokens with a valid witness signature
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;
    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    Ok(())
}

/// Tests that minting without a witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_without_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let http_client = HttpClient::new(MINT_URL.parse()?, None);

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let premint_secrets =
        PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::default()).unwrap();

    let request = MintBolt11Request {
        quote: mint_quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let response = http_client.post_mint(request.clone()).await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => Ok(()),
        Err(err) => bail!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => bail!("Minting should not have succeed without a witness"),
    }
}

/// Tests that minting with an incorrect witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_wrong_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let http_client = HttpClient::new(MINT_URL.parse()?, None);

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let premint_secrets =
        PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::default()).unwrap();

    let mut request = MintBolt11Request {
        quote: mint_quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = SecretKey::generate();

    request.sign(secret_key)?;

    let response = http_client.post_mint(request.clone()).await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => Ok(()),
        Err(err) => bail!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => bail!("Minting should not have succeed without a witness"),
    }
}

/// Tests that attempting to mint more tokens than allowed by the quote fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_inflated() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let pre_mint = PreMintSecrets::random(active_keyset_id, 500.into(), &SplitTarget::None)?;

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await?
        .expect("there is a quote");

    let mut mint_request = MintBolt11Request {
        quote: mint_quote.id,
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(secret_key)?;
    }
    let http_client = HttpClient::new(MINT_URL.parse()?, None);

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

/// Tests that attempting to mint with multiple currency units in the same request fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_units() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let pre_mint = PreMintSecrets::random(active_keyset_id, 50.into(), &SplitTarget::None)?;

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let active_keyset_id = wallet_usd.get_active_mint_keyset().await?.id;

    let usd_pre_mint = PreMintSecrets::random(active_keyset_id, 50.into(), &SplitTarget::None)?;

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await?
        .expect("there is a quote");

    let mut sat_outputs = pre_mint.blinded_messages();

    let mut usd_outputs = usd_pre_mint.blinded_messages();

    sat_outputs.append(&mut usd_outputs);

    let mut mint_request = MintBolt11Request {
        quote: mint_quote.id,
        outputs: sat_outputs,
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(secret_key)?;
    }
    let http_client = HttpClient::new(MINT_URL.parse()?, None);

    let response = http_client.post_mint(mint_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::MultipleUnits => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed to mint with multiple units");
        }
    }

    Ok(())
}

/// Tests that attempting to swap tokens with multiple currency units fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_swap() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet_usd.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet_usd, &mint_quote.id, 60).await?;

    let usd_proofs = wallet_usd
        .mint(&mint_quote.id, SplitTarget::None, None)
        .await?;

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let pre_mint =
            PreMintSecrets::random(active_keyset_id, inputs.total_amount()?, &SplitTarget::None)?;

        let swap_request = SwapRequest::new(inputs, pre_mint.blinded_messages());

        let http_client = HttpClient::new(MINT_URL.parse()?, None);
        let response = http_client.post_swap(swap_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    bail!("Wrong mint error returned: {}", err.to_string());
                }
            },
            Ok(_) => {
                bail!("Should not have allowed to mint with multiple units");
            }
        }
    }

    {
        let usd_active_keyset_id = wallet_usd.get_active_mint_keyset().await?.id;
        let inputs: Proofs = proofs.into_iter().take(2).collect();

        let total_inputs = inputs.total_amount()?;

        let half = total_inputs / 2.into();
        let usd_pre_mint = PreMintSecrets::random(usd_active_keyset_id, half, &SplitTarget::None)?;
        let pre_mint =
            PreMintSecrets::random(active_keyset_id, total_inputs - half, &SplitTarget::None)?;

        let mut usd_outputs = usd_pre_mint.blinded_messages();
        let mut sat_outputs = pre_mint.blinded_messages();

        usd_outputs.append(&mut sat_outputs);

        let swap_request = SwapRequest::new(inputs, usd_outputs);

        let http_client = HttpClient::new(MINT_URL.parse()?, None);
        let response = http_client.post_swap(swap_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    bail!("Wrong mint error returned: {}", err.to_string());
                }
            },
            Ok(_) => {
                bail!("Should not have allowed to mint with multiple units");
            }
        }
    }

    Ok(())
}

/// Tests that attempting to melt tokens with multiple currency units fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_melt() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::None, None)
        .await
        .unwrap();

    println!("Minted sat");

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet_usd.mint_quote(100.into(), None).await.unwrap();
    println!("Minted quote usd");

    wait_for_mint_to_be_paid(&wallet_usd, &mint_quote.id, 60).await?;

    let usd_proofs = wallet_usd
        .mint(&mint_quote.id, SplitTarget::None, None)
        .await
        .unwrap();

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let input_amount: u64 = inputs.total_amount()?.into();
        let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
        let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

        let melt_request = MeltBolt11Request::new(melt_quote.id, inputs, None);

        let http_client = HttpClient::new(MINT_URL.parse()?, None);
        let response = http_client.post_melt(melt_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    bail!("Wrong mint error returned: {}", err.to_string());
                }
            },
            Ok(_) => {
                bail!("Should not have allowed to melt with multiple units");
            }
        }
    }

    {
        let inputs: Proofs = vec![proofs.first().expect("There is a proof").clone()];

        let input_amount: u64 = inputs.total_amount()?.into();

        let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
        let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
        let usd_active_keyset_id = wallet_usd.get_active_mint_keyset().await?.id;

        let usd_pre_mint = PreMintSecrets::random(
            usd_active_keyset_id,
            inputs.total_amount()? + 100.into(),
            &SplitTarget::None,
        )?;
        let pre_mint = PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::None)?;

        let mut usd_outputs = usd_pre_mint.blinded_messages();
        let mut sat_outputs = pre_mint.blinded_messages();

        usd_outputs.append(&mut sat_outputs);
        let quote = wallet.melt_quote(invoice.to_string(), None).await?;

        let melt_request = MeltBolt11Request::new(quote.id, inputs, Some(usd_outputs));

        let http_client = HttpClient::new(MINT_URL.parse()?, None);

        let response = http_client.post_melt(melt_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    bail!("Wrong mint error returned: {}", err.to_string());
                }
            },
            Ok(_) => {
                bail!("Should not have allowed to melt with multiple units");
            }
        }
    }

    Ok(())
}

/// Tests that swapping tokens where input unit doesn't match output unit fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_input_output_mismatch() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;
    let usd_active_keyset_id = wallet_usd.get_active_mint_keyset().await?.id;

    let inputs = proofs;

    let pre_mint = PreMintSecrets::random(
        usd_active_keyset_id,
        inputs.total_amount()?,
        &SplitTarget::None,
    )?;

    let swap_request = SwapRequest::new(inputs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::UnitMismatch => (),
            err => bail!("Wrong error returned: {}", err),
        },
        Ok(_) => {
            bail!("Should not have allowed to mint with multiple units");
        }
    }

    Ok(())
}

/// Tests that swapping tokens where output amount is greater than input amount fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_swap_inflated() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let pre_mint = PreMintSecrets::random(active_keyset_id, 101.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed to mint with multiple units");
        }
    }

    Ok(())
}

/// Tests that tokens cannot be spent again after a failed swap attempt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_swap_spend_after_fail() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let pre_mint = PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    assert!(response.is_ok());

    let pre_mint = PreMintSecrets::random(active_keyset_id, 101.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => bail!("Wrong mint error returned expected TransactionUnbalanced, got: {err}"),
        },
        Ok(_) => bail!("Should not have allowed swap with unbalanced"),
    }

    let pre_mint = PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed to mint with multiple units");
        }
    }

    Ok(())
}

/// Tests that tokens cannot be melted after a failed swap attempt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_melt_spend_after_fail() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let pre_mint = PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    assert!(response.is_ok());

    let pre_mint = PreMintSecrets::random(active_keyset_id, 101.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => bail!("Wrong mint error returned expected TransactionUnbalanced, got: {err}"),
        },
        Ok(_) => bail!("Should not have allowed swap with unbalanced"),
    }

    let input_amount: u64 = proofs.total_amount()?.into();
    let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt_request = MeltBolt11Request::new(melt_quote.id, proofs, None);

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_melt(melt_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed to melt with multiple units");
        }
    }

    Ok(())
}

/// Tests that attempting to swap with duplicate proofs fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_swap() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let pre_mint =
        PreMintSecrets::random(active_keyset_id, inputs.total_amount()?, &SplitTarget::None)?;

    let swap_request = SwapRequest::new(inputs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateInputs => (),
            err => {
                bail!(
                    "Wrong mint error returned, expected duplicate inputs: {}",
                    err.to_string()
                );
            }
        },
        Ok(_) => {
            bail!("Should not have allowed duplicate inputs");
        }
    }

    let blinded_message = pre_mint.blinded_messages();

    let outputs = vec![blinded_message[0].clone(), blinded_message[0].clone()];

    let swap_request = SwapRequest::new(inputs, outputs);

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateOutputs => (),
            err => {
                bail!(
                    "Wrong mint error returned, expected duplicate outputs: {}",
                    err.to_string()
                );
            }
        },
        Ok(_) => {
            bail!("Should not have allow duplicate inputs");
        }
    }

    Ok(())
}

/// Tests that attempting to melt with duplicate proofs fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_melt() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let invoice = create_fake_invoice(7000, "".to_string());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt_request = MeltBolt11Request::new(melt_quote.id, inputs, None);

    let http_client = HttpClient::new(MINT_URL.parse()?, None);
    let response = http_client.post_melt(melt_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateInputs => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allow duplicate inputs");
        }
    }

    Ok(())
}
