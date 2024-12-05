use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::{
    CurrencyUnit, MeltQuoteState, MintBolt11Request, MintQuoteState, NotificationPayload,
    PreMintSecrets, State,
};
use cdk::wallet::client::{HttpClient, HttpClientMethods};
use cdk::wallet::Wallet;
use cdk_integration_tests::init_regtest::{
    get_mint_url, get_mint_ws_url, init_cln_client, init_lnd_client,
};
use futures::{SinkExt, StreamExt};
use lightning_invoice::Bolt11Invoice;
use ln_regtest_rs::InvoiceStatus;
use serde_json::json;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

async fn get_notification<T: StreamExt<Item = Result<Message, E>> + Unpin, E: Debug>(
    reader: &mut T,
    timeout_to_wait: Duration,
) -> (String, NotificationPayload) {
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
    let lnd_client = init_lnd_client().await.unwrap();

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let (ws_stream, _) = connect_async(get_mint_ws_url())
        .await
        .expect("Failed to connect");
    let (mut write, mut reader) = ws_stream.split();

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(mint_amount == 100.into());

    let invoice = lnd_client.create_invoice(50).await?;

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
    let lnd_client = init_lnd_client().await?;

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(mint_amount == 100.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore() -> Result<()> {
    let lnd_client = init_lnd_client().await?;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let wallet_2 = Wallet::new(
        &get_mint_url(),
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
    let lnd_client = init_lnd_client().await?;
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert_eq!(mint_amount, 100.into());

    let invoice = lnd_client.create_invoice(10).await?;

    let melt_quote = wallet.melt_quote(invoice.clone(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    let melt_two = wallet.melt_quote(invoice, None).await?;

    let melt_two = wallet.melt(&melt_two.id).await;

    match melt_two {
        Err(err) => match err {
            cdk::Error::RequestAlreadyPaid => (),
            _ => {
                bail!("Wrong invoice already paid");
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
    let lnd_client = init_lnd_client().await?;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let wallet_2 = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet_2.mint_quote(10.into(), None).await?;

    let melt = wallet.melt_quote(mint_quote.request.clone(), None).await?;

    assert_eq!(melt.amount, 10.into());

    let _melted = wallet.melt(&melt.id).await.unwrap();

    let _wallet_2_mint = wallet_2
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let cln_client = init_cln_client().await?;
    let payment_hash = Bolt11Invoice::from_str(&mint_quote.request)?;
    let check_paid = cln_client
        .check_incoming_invoice(payment_hash.payment_hash().to_string())
        .await?;

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
    let lnd_client = init_lnd_client().await.unwrap();

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let quote = wallet.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request).await?;

    loop {
        let status = wallet.mint_quote_state(&quote.id).await.unwrap();

        println!("Quote status: {}", status.state);

        if status.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(5)).await;
    }

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let http_client = HttpClient::new();
    let premint_secrets =
        PreMintSecrets::random(active_keyset_id, 31.into(), &SplitTarget::default()).unwrap();

    let request = MintBolt11Request {
        quote: quote.id,
        outputs: premint_secrets.blinded_messages(),
    };

    let response = http_client
        .post_mint(get_mint_url().as_str().parse()?, request.clone())
        .await?;
    let response1 = http_client
        .post_mint(get_mint_url().as_str().parse()?, request)
        .await?;

    assert!(response == response1);
    Ok(())
}
