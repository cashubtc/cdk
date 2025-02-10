use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::{MeltOptions, Mpp};
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CurrencyUnit, MeltQuoteState, MintBolt11Request, MintQuoteState, NotificationPayload,
    PreMintSecrets, State,
};
use cdk::wallet::client::{HttpClient, MintConnector};
use cdk::wallet::{Wallet, WalletSubscription};
use cdk_integration_tests::init_regtest::{
    get_cln_dir, get_lnd_cert_file_path, get_lnd_dir, get_lnd_macaroon_path, get_mint_port,
    get_mint_url, get_mint_ws_url, LND_RPC_ADDR, LND_TWO_RPC_ADDR,
};
use cdk_integration_tests::wait_for_mint_to_be_paid;
use futures::{join, SinkExt, StreamExt};
use lightning_invoice::Bolt11Invoice;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use ln_regtest_rs::InvoiceStatus;
use serde_json::json;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

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

async fn get_notification<T: StreamExt<Item = Result<Message, E>> + Unpin, E: Debug>(
    reader: &mut T,
    timeout_to_wait: Duration,
) -> (String, NotificationPayload<String>) {
    let msg = timeout(timeout_to_wait, reader.next())
        .await
        .expect("timeout")
        .unwrap()
        .unwrap();

    let mut response: serde_json::Value =
        serde_json::from_str(msg.to_text().unwrap()).expect("valid json");

    let mut params_raw = response
        .as_object_mut()
        .expect("object")
        .remove("params")
        .expect("valid params");

    let params_map = params_raw.as_object_mut().expect("params is object");

    (
        params_map
            .remove("subId")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        serde_json::from_value(params_map.remove("payload").unwrap()).unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint_melt_round_trip() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let (ws_stream, _) = connect_async(get_mint_ws_url("0"))
        .await
        .expect("Failed to connect");
    let (mut write, mut reader) = ws_stream.split();

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await.unwrap();

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    let invoice = lnd_client.create_invoice(Some(50)).await?;

    let melt = wallet.melt_quote(invoice, None).await?;

    write
        .send(Message::Text(serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "subscribe",
                "params": {
                  "kind": "bolt11_melt_quote",
                  "filters": [
                    melt.id.clone(),
                  ],
                  "subId": "test-sub",
                }

        }))?))
        .await?;

    assert_eq!(
        reader.next().await.unwrap().unwrap().to_text().unwrap(),
        r#"{"jsonrpc":"2.0","result":{"status":"OK","subId":"test-sub"},"id":2}"#
    );

    let melt_response = wallet.melt(&melt.id).await.unwrap();
    assert!(melt_response.preimage.is_some());
    assert!(melt_response.state == MeltQuoteState::Paid);

    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    // first message is the current state
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(payload.amount + payload.fee_reserve, 100.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Unpaid);

    // get current state
    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(payload.amount + payload.fee_reserve, 100.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Paid);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint_melt() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    lnd_client.pay_invoice(mint_quote.request).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
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
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    assert!(wallet_2.total_balance().await? == 0.into());

    let restored = wallet_2.restore().await?;
    let proofs = wallet_2.get_unspent_proofs().await?;

    wallet_2
        .swap(None, SplitTarget::default(), proofs, None, false)
        .await?;

    assert!(restored == 100.into());

    assert!(wallet_2.total_balance().await? == 100.into());

    let proofs = wallet.get_unspent_proofs().await?;

    let states = wallet.check_proofs_spent(proofs).await?;

    for state in states {
        if state.state != State::Spent {
            bail!("All proofs should be spent");
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pay_invoice_twice() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("Could not pay invoice");

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert_eq!(mint_amount, 100.into());

    let invoice = lnd_client.create_invoice(Some(10)).await?;

    let melt_quote = wallet.melt_quote(invoice.clone(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    let melt_two = wallet.melt_quote(invoice, None).await?;

    let melt_two = wallet.melt(&melt_two.id).await;

    match melt_two {
        Err(err) => match err {
            cdk::Error::RequestAlreadyPaid => (),
            err => {
                bail!("Wrong invoice already paid: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed second payment");
        }
    }

    let balance = wallet.total_balance().await?;

    assert_eq!(balance, (Amount::from(100) - melt.fee_paid - melt.amount));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_internal_payment() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let wallet_2 = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
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
async fn test_cached_mint() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let quote = wallet.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request).await?;

    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await;

    while let Some(msg) = subscription.recv().await {
        if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
            if response.state == MintQuoteState::Paid {
                break;
            }
        }
    }

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let http_client = HttpClient::new(get_mint_url("0").as_str().parse()?);
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
async fn test_multimint_melt() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet1 = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;
    let wallet2 = Wallet::new(
        &get_mint_url("1"),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    // Fund the wallets
    let quote = wallet1.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request.clone()).await?;
    loop {
        let quote_status = wallet1.mint_quote_state(&quote.id).await?;
        if quote_status.state == MintQuoteState::Paid {
            break;
        }
        tracing::debug!("Quote not yet paid");
    }
    wallet1
        .mint(&quote.id, SplitTarget::default(), None)
        .await?;

    let quote = wallet2.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request.clone()).await?;
    loop {
        let quote_status = wallet2.mint_quote_state(&quote.id).await?;
        if quote_status.state == MintQuoteState::Paid {
            break;
        }
        tracing::debug!("Quote not yet paid");
    }
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
