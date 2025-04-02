use std::env;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use cashu::Bolt11Invoice;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::{MintQuoteState, NotificationPayload, State};
use cdk::wallet::WalletSubscription;
use cdk::Wallet;
use cdk_fake_wallet::create_fake_invoice;
use init_regtest::{get_lnd_dir, get_mint_url, LND_RPC_ADDR};
use ln_regtest_rs::ln_client::{LightningClient, LndClient};
use tokio::time::{sleep, timeout, Duration};

pub mod init_auth_mint;
pub mod init_pure_tests;
pub mod init_regtest;

pub async fn fund_wallet(wallet: Arc<Wallet>, amount: Amount) {
    let quote = wallet
        .mint_quote(amount, None)
        .await
        .expect("Could not get mint quote");

    wait_for_mint_to_be_paid(&wallet, &quote.id, 60)
        .await
        .expect("Waiting for mint failed");

    let _proofs = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .expect("Could not mint");
}

// Get all pending from wallet and attempt to swap
// Will panic if there are no pending
// Will return Ok if swap fails as expected
pub async fn attempt_to_swap_pending(wallet: &Wallet) -> Result<()> {
    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(!pending.is_empty());

    let swap = wallet
        .swap(
            None,
            SplitTarget::None,
            pending.into_iter().map(|p| p.proof).collect(),
            None,
            false,
        )
        .await;

    match swap {
        Ok(_swap) => {
            bail!("These proofs should be pending")
        }
        Err(err) => match err {
            cdk::error::Error::TokenPending => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error")
            }
        },
    }

    Ok(())
}

pub async fn wait_for_mint_to_be_paid(
    wallet: &Wallet,
    mint_quote_id: &str,
    timeout_secs: u64,
) -> Result<()> {
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![
            mint_quote_id.to_owned(),
        ]))
        .await;
    // Create the timeout future
    let wait_future = async {
        while let Some(msg) = subscription.recv().await {
            if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
                if response.state == MintQuoteState::Paid {
                    return Ok(());
                }
            }
        }
        Err(anyhow!("Subscription ended without quote being paid"))
    };

    let timeout_future = timeout(Duration::from_secs(timeout_secs), wait_future);

    let check_interval = Duration::from_secs(5);

    let periodic_task = async {
        loop {
            match wallet.mint_quote_state(mint_quote_id).await {
                Ok(result) => {
                    if result.state == MintQuoteState::Paid {
                        tracing::info!("mint quote paid via poll");
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::error!("Could not check mint quote status: {:?}", e);
                }
            }
            sleep(check_interval).await;
        }
    };

    tokio::select! {
        result = timeout_future => {
            match result {
                Ok(payment_result) => payment_result,
                Err(_) => Err(anyhow!("Timeout waiting for mint quote to be paid")),
            }
        }
        result = periodic_task => {
            result // Now propagates the result from periodic checks
        }
    }
}

/// Gets the mint URL from environment variable or falls back to default
///
/// Checks the CDK_TEST_MINT_URL environment variable:
/// - If set, returns that URL
/// - Otherwise falls back to the default URL from get_mint_url("0")
pub fn get_mint_url_from_env() -> String {
    match env::var("CDK_TEST_MINT_URL") {
        Ok(url) => url,
        Err(_) => get_mint_url("0"),
    }
}

/// Gets the second mint URL from environment variable or falls back to default
///
/// Checks the CDK_TEST_MINT_URL_2 environment variable:
/// - If set, returns that URL
/// - Otherwise falls back to the default URL from get_mint_url("1")
pub fn get_second_mint_url_from_env() -> String {
    match env::var("CDK_TEST_MINT_URL_2") {
        Ok(url) => url,
        Err(_) => get_mint_url("1"),
    }
}

// This is the ln wallet we use to send/receive ln payements as the wallet
pub async fn init_lnd_client() -> LndClient {
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
pub async fn pay_if_regtest(invoice: &Bolt11Invoice) -> Result<()> {
    // Check if the invoice is for the regtest network
    if invoice.network() == bitcoin::Network::Regtest {
        println!("Regtest invoice");
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
/// Checks the CDK_TEST_REGTEST environment variable:
/// - If set to "1", "true", or "yes" (case insensitive), returns true
/// - Otherwise returns false
pub fn is_regtest_env() -> bool {
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
/// Uses the is_regtest_env() function to determine whether to
/// create a real regtest invoice or a fake one for testing.
pub async fn create_invoice_for_env(amount_sat: Option<u64>) -> Result<String> {
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
