//! These tests are to test happy path mint wallet interactions
//!
//!
use std::env;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use bip39::Mnemonic;
use cashu::{MeltOptions, Mpp};
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CurrencyUnit, MeltQuoteState, MintBolt11Request, MintQuoteState, NotificationPayload,
    PreMintSecrets, State,
};
use cdk::wallet::{HttpClient, MintConnector, Wallet, WalletSubscription};
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_regtest::{
    get_cln_dir, get_lnd_cert_file_path, get_lnd_dir, get_lnd_macaroon_path, get_mint_port,
    get_mint_url, get_mint_ws_url, LND_RPC_ADDR, LND_TWO_RPC_ADDR,
};
use cdk_integration_tests::wait_for_mint_to_be_paid;
use cdk_sqlite::wallet::{self, memory};
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

/// Pays a Bolt11Invoice if it's on the regtest network, otherwise returns Ok
///
/// This is useful for tests that need to pay invoices in regtest mode but
/// should be skipped in other environments.
async fn pay_if_regtest(invoice: &Bolt11Invoice) -> Result<()> {
    // Check if the invoice is for the regtest network
    if invoice.network() == bitcoin::Network::Regtest {
        let lnd_client = init_lnd_client().await;
        lnd_client.pay_invoice(invoice.to_string()).await?;
        Ok(())
    } else {
        // Not a regtest invoice, just return Ok
        Ok(())
    }
}

/// Determines if we're running in regtest mode based on environment variable
///
/// Checks the CASHU_TEST_REGTEST environment variable:
/// - If set to "1", "true", or "yes" (case insensitive), returns true
/// - Otherwise returns false
fn is_regtest_env() -> bool {
    match env::var("CDK_TEST_REGTEST") {
        Ok(val) => {
            let val = val.to_lowercase();
            val == "1" || val == "true" || val == "yes"
        }
        Err(_) => false,
    }
}

/// Creates a real invoice if in regtest mode, otherwise returns a fake invoice
///
/// Uses the CASHU_TEST_REGTEST environment variable to determine whether to
/// create a real regtest invoice or a fake one for testing.
async fn create_invoice_for_env(amount_sat: Option<u64>) -> Result<String> {
    if is_regtest_env() {
        // In regtest mode, create a real invoice
        let lnd_client = init_lnd_client().await;
        lnd_client
            .create_invoice(amount_sat)
            .await
            .map_err(|e| anyhow!("Failed to create regtest invoice: {}", e))
    } else {
        // Not in regtest mode, create a fake invoice
        let fake_invoice = create_fake_invoice(
            amount_sat.expect("Amount must be defined") * 1_000,
            "".to_string(),
        );
        Ok(fake_invoice.to_string())
    }
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
async fn test_happy_mint_melt_round_trip() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let (ws_stream, _) = connect_async(get_mint_ws_url("0"))
        .await
        .expect("Failed to connect");
    let (mut write, mut reader) = ws_stream.split();

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let invoice = Bolt11Invoice::from_str(&mint_quote.request)?;
    pay_if_regtest(&invoice).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    let invoice = create_invoice_for_env(Some(50)).await?;

    let melt = wallet.melt_quote(invoice, None).await?;

    write
        .send(Message::Text(
            serde_json::to_string(&json!({
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

            }))?
            .into(),
        ))
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

    assert_eq!(payload.amount + payload.fee_reserve, 50.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Unpaid);

    // get current state
    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(payload.amount + payload.fee_reserve, 50.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Paid);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_happy_mint_melt() -> Result<()> {
    let lnd_client = init_lnd_client().await;

    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
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
