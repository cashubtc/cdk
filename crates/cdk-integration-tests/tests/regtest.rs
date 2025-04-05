use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::ProofsMethods;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::{
    CurrencyUnit, MeltOptions, MeltQuoteState, MintBolt11Request, MintQuoteState, Mpp,
    NotificationPayload, PreMintSecrets,
};
use cdk::wallet::{HttpClient, MintConnector, Wallet, WalletSubscription};
use cdk_integration_tests::init_regtest::{
    get_cln_dir, get_lnd_cert_file_path, get_lnd_dir, get_lnd_macaroon_path, get_mint_port,
    LND_RPC_ADDR, LND_TWO_RPC_ADDR,
};
use cdk_integration_tests::{
    get_mint_url_from_env, get_second_mint_url_from_env, wait_for_mint_to_be_paid,
};
use cdk_sqlite::wallet::{self, memory};
use futures::join;
use lightning_invoice::Bolt11Invoice;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use ln_regtest_rs::InvoiceStatus;
use tokio::time::timeout;

// This is the ln wallet we use to send/receive ln payements as the wallet
async fn init_lnd_client() -> LndClient {
    let lnd_dir = get_lnd_dir("one");
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");
    LndClient::new(
        format!("https://{}", LND_RPC_ADDR),
        cert_file,
        macaroon_file,
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_internal_payment() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let wallet_2 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet_2.mint_quote(10.into(), None).await?;

    let melt = wallet.melt_quote(mint_quote.request.clone(), None).await?;

    assert_eq!(melt.amount, 10.into());

    let _melted = wallet.melt(&melt.id).await.unwrap();

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _wallet_2_mint = wallet_2
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let check_paid = match get_mint_port("0") {
        8085 => {
            let cln_one_dir = get_cln_dir("one");
            let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;

            let payment_hash = Bolt11Invoice::from_str(&mint_quote.request)?;
            cln_client
                .check_incoming_payment_status(&payment_hash.payment_hash().to_string())
                .await
                .expect("Could not check invoice")
        }
        8087 => {
            let lnd_two_dir = get_lnd_dir("two");
            let lnd_client = LndClient::new(
                format!("https://{}", LND_TWO_RPC_ADDR),
                get_lnd_cert_file_path(&lnd_two_dir),
                get_lnd_macaroon_path(&lnd_two_dir),
            )
            .await?;
            let payment_hash = Bolt11Invoice::from_str(&mint_quote.request)?;
            lnd_client
                .check_incoming_payment_status(&payment_hash.payment_hash().to_string())
                .await
                .expect("Could not check invoice")
        }
        _ => panic!("Unknown mint port"),
    };

    match check_paid {
        InvoiceStatus::Unpaid => (),
        _ => {
            bail!("Invoice has incorrect status: {:?}", check_paid);
        }
    }

    let wallet_2_balance = wallet_2.total_balance().await?;

    assert!(wallet_2_balance == 10.into());

    let wallet_1_balance = wallet.total_balance().await?;

    assert!(wallet_1_balance == 90.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_websocket_connection() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(wallet::memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    // Create a small mint quote to test notifications
    let mint_quote = wallet.mint_quote(10.into(), None).await?;

    // Subscribe to notifications for this quote
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![mint_quote
            .id
            .clone()]))
        .await;

    // First check we get the unpaid state
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for unpaid notification")
        .ok_or_else(|| anyhow::anyhow!("No unpaid notification received"))?;

    match msg {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_quote.id);
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        _ => bail!("Unexpected notification type"),
    }

    let lnd_client = init_lnd_client().await;
    lnd_client.pay_invoice(mint_quote.request).await?;

    // Wait for paid notification with 10 second timeout
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for paid notification")
        .ok_or_else(|| anyhow::anyhow!("No paid notification received"))?;

    match msg {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_quote.id);
            assert_eq!(response.state, MintQuoteState::Paid);
            Ok(())
        }
        _ => bail!("Unexpected notification type"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multimint_melt() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let db = Arc::new(memory::empty().await?);
    let wallet1 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let db = Arc::new(memory::empty().await?);
    let wallet2 = Wallet::new(
        &get_second_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    // Fund the wallets
    let quote = wallet1.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request.clone()).await?;
    wait_for_mint_to_be_paid(&wallet1, &quote.id, 60).await?;
    wallet1
        .mint(&quote.id, SplitTarget::default(), None)
        .await?;

    let quote = wallet2.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request.clone()).await?;
    wait_for_mint_to_be_paid(&wallet2, &quote.id, 60).await?;
    wallet2
        .mint(&quote.id, SplitTarget::default(), None)
        .await?;

    // Get an invoice
    let invoice = lnd_client.create_invoice(Some(50)).await?;

    // Get multi-part melt quotes
    let melt_options = MeltOptions::Mpp {
        mpp: Mpp {
            amount: Amount::from(25000),
        },
    };
    let quote_1 = wallet1
        .melt_quote(invoice.clone(), Some(melt_options))
        .await
        .expect("Could not get melt quote");
    let quote_2 = wallet2
        .melt_quote(invoice.clone(), Some(melt_options))
        .await
        .expect("Could not get melt quote");

    // Multimint pay invoice
    let result1 = wallet1.melt(&quote_1.id);
    let result2 = wallet2.melt(&quote_2.id);
    let result = join!(result1, result2);

    // Unpack results
    let result1 = result.0.unwrap();
    let result2 = result.1.unwrap();

    // Check
    assert!(result1.state == result2.state);
    assert!(result1.state == MeltQuoteState::Paid);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cached_mint() -> Result<()> {
    let lnd_client = init_lnd_client().await;
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let quote = wallet.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request.clone()).await?;

    wait_for_mint_to_be_paid(&wallet, &quote.id, 60).await?;

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);
    let premint_secrets =
        PreMintSecrets::random(active_keyset_id, 100.into(), &SplitTarget::default()).unwrap();

    let mut request = MintBolt11Request {
        quote: quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = quote.secret_key;

    request.sign(secret_key.expect("Secret key on quote"))?;

    let response = http_client.post_mint(request.clone()).await?;
    let response1 = http_client.post_mint(request).await?;

    assert!(response == response1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_melt_amountless() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    lnd_client.pay_invoice(mint_quote.request).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let amount = proofs.total_amount()?;

    assert!(mint_amount == amount);

    let invoice = lnd_client.create_invoice(None).await?;

    let options = MeltOptions::new_amountless(5_000);

    let melt_quote = wallet.melt_quote(invoice.clone(), Some(options)).await?;

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    assert!(melt.amount == 5.into());

    Ok(())
}
