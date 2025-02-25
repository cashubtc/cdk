use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CurrencyUnit, MeltBolt11Request, MeltQuoteState, MintBolt11Request, PreMintSecrets, Proofs,
    SecretKey, State, SwapRequest,
};
use cdk::wallet::client::{HttpClient, MintConnector};
use cdk::wallet::Wallet;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_integration_tests::{attempt_to_swap_pending, wait_for_mint_to_be_paid};

const MINT_URL: &str = "http://127.0.0.1:8086";

// If both pay and check return pending input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_tokens_pending() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

// If the pay error fails and the check returns unknown or failed
// The inputs proofs should be unset as spending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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
    assert!(wallet_bal == 100.into());

    Ok(())
}

// When both the pay_invoice and check_invoice both fail
// the proofs should remain as pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail_and_check() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

// In the case that the ln backend returns a failed status but does not error
// The mint should do a second check, then remove proofs from pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_return_fail_status() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

// In the case that the ln backend returns a failed status but does not error
// The mint should do a second check, then remove proofs from pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_error_unknown() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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
    assert!(melt.is_err());

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
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(pending.is_empty());

    Ok(())
}

// In the case that the ln backend returns an err
// The mint should do a second check, that returns paid
// Proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_err_paid() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_change_in_quote() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription::default();

    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    let proofs = wallet.get_unspent_proofs().await?;

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let keyset = wallet.get_active_mint_keyset().await?;

    let premint_secrets = PreMintSecrets::random(keyset.id, 100.into(), &SplitTarget::default())?;

    let client = HttpClient::new(MINT_URL.parse()?);

    let melt_request = MeltBolt11Request {
        quote: melt_quote.id.clone(),
        inputs: proofs.clone(),
        outputs: Some(premint_secrets.blinded_messages()),
    };

    let melt_response = client.post_melt(melt_request).await?;

    assert!(melt_response.change.is_some());

    let check = wallet.melt_quote_status(&melt_quote.id).await?;
    let mut melt_change = melt_response.change.unwrap();
    melt_change.sort_by(|a, b| a.amount.cmp(&b.amount));

    let mut check = check.change.unwrap();
    check.sort_by(|a, b| a.amount.cmp(&b.amount));

    assert_eq!(melt_change, check);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_database_type() -> Result<()> {
    // Get the database type from environment
    let db_type = std::env::var("MINT_DATABASE").expect("MINT_DATABASE env var should be set");
    
    let http_client = HttpClient::new(MINT_URL.parse()?);
    let info = http_client.get_info().await?;
    
    // Check that the database type in the mint info matches what we expect
    match db_type.as_str() {
        "REDB" => assert!(info.database_type.contains("redb"), "Expected redb database"),
        "SQLITE" => assert!(info.database_type.contains("sqlite"), "Expected sqlite database"),
        "MEMORY" => assert!(info.database_type.contains("memory"), "Expected memory database"),
        _ => bail!("Unknown database type: {}", db_type),
    }
    
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_without_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let http_client = HttpClient::new(MINT_URL.parse()?);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_wrong_witness() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let http_client = HttpClient::new(MINT_URL.parse()?);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_inflated() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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
    let http_client = HttpClient::new(MINT_URL.parse()?);

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
async fn test_fake_mint_multiple_units() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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
        Arc::new(WalletMemoryDatabase::default()),
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
    let http_client = HttpClient::new(MINT_URL.parse()?);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_swap() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(WalletMemoryDatabase::default()),
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

        let swap_request = SwapRequest {
            inputs,
            outputs: pre_mint.blinded_messages(),
        };

        let http_client = HttpClient::new(MINT_URL.parse()?);
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

        let swap_request = SwapRequest {
            inputs,
            outputs: usd_outputs,
        };

        let http_client = HttpClient::new(MINT_URL.parse()?);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_melt() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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
        Arc::new(WalletMemoryDatabase::default()),
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

        let melt_request = MeltBolt11Request {
            quote: melt_quote.id,
            inputs,
            outputs: None,
        };

        let http_client = HttpClient::new(MINT_URL.parse()?);
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

        let melt_request = MeltBolt11Request {
            quote: quote.id,
            inputs,
            outputs: Some(usd_outputs),
        };

        let http_client = HttpClient::new(MINT_URL.parse()?);

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

/// Test swap where input unit != output unit
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_input_output_mismatch() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(WalletMemoryDatabase::default()),
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

    let swap_request = SwapRequest {
        inputs,
        outputs: pre_mint.blinded_messages(),
    };

    let http_client = HttpClient::new(MINT_URL.parse()?);
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

/// Test swap where input is less the output
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_swap_inflated() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let pre_mint = PreMintSecrets::random(active_keyset_id, 101.into(), &SplitTarget::None)?;

    let swap_request = SwapRequest {
        inputs: proofs,
        outputs: pre_mint.blinded_messages(),
    };

    let http_client = HttpClient::new(MINT_URL.parse()?);
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

/// Test swap where input unit != output unit
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_swap() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
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

    let swap_request = SwapRequest {
        inputs: inputs.clone(),
        outputs: pre_mint.blinded_messages(),
    };

    let http_client = HttpClient::new(MINT_URL.parse()?);
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

    let swap_request = SwapRequest { inputs, outputs };

    let http_client = HttpClient::new(MINT_URL.parse()?);
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

/// Test duplicate proofs in melt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_melt() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let invoice = create_fake_invoice(7000, "".to_string());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt_request = MeltBolt11Request {
        quote: melt_quote.id,
        inputs,
        outputs: None,
    };

    let http_client = HttpClient::new(MINT_URL.parse()?);
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
