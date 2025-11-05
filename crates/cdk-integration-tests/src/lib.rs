//! Integration Test Library
//!
//! This crate provides shared functionality for CDK integration tests.
//! It includes utilities for setting up test environments, funding wallets,
//! and common test operations across different test scenarios.
//!
//! Test Categories Supported:
//! - Pure in-memory tests (no external dependencies)
//! - Regtest environment tests (with actual Lightning nodes)
//! - Authenticated mint tests
//! - Multi-mint scenarios
//!
//! Key Components:
//! - Test environment initialization
//! - Wallet funding utilities
//! - Lightning Network client helpers
//! - Proof state management utilities

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use cashu::Bolt11Invoice;
use cdk::amount::{Amount, SplitTarget};
use cdk::{StreamExt, Wallet};
use cdk_fake_wallet::create_fake_invoice;
use init_regtest::{get_lnd_dir, LND_RPC_ADDR};
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};

use crate::init_regtest::get_cln_dir;

pub mod cli;
pub mod init_auth_mint;
pub mod init_pure_tests;
pub mod init_regtest;
pub mod shared;

pub async fn fund_wallet(wallet: Arc<Wallet>, amount: Amount) {
    let quote = wallet
        .mint_quote(amount, None)
        .await
        .expect("Could not get mint quote");

    let _proofs = wallet
        .proof_stream(quote, SplitTarget::default(), None)
        .next()
        .await
        .expect("proofs")
        .expect("proofs with no error");
}

pub fn get_mint_url_from_env() -> String {
    match env::var("CDK_TEST_MINT_URL") {
        Ok(url) => url,
        Err(_) => panic!("Mint url not set"),
    }
}

pub fn get_second_mint_url_from_env() -> String {
    match env::var("CDK_TEST_MINT_URL_2") {
        Ok(url) => url,
        Err(_) => panic!("Mint url not set"),
    }
}

// This is the ln wallet we use to send/receive ln payements as the wallet
pub async fn init_lnd_client(work_dir: &Path) -> LndClient {
    let lnd_dir = get_lnd_dir(work_dir, "one");
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");
    LndClient::new(format!("https://{LND_RPC_ADDR}"), cert_file, macaroon_file)
        .await
        .unwrap()
}

/// Pays a Bolt11Invoice if it's on the regtest network, otherwise returns Ok
///
/// This is useful for tests that need to pay invoices in regtest mode but
/// should be skipped in other environments.
pub async fn pay_if_regtest(_work_dir: &Path, invoice: &Bolt11Invoice) -> Result<()> {
    // Check if the invoice is for the regtest network
    if invoice.network() == bitcoin::Network::Regtest {
        let client = get_test_client().await;
        let mut tries = 0;
        while let Err(err) = client.pay_invoice(invoice.to_string()).await {
            println!("Could not pay invoice.retrying {err}");
            tries += 1;
            if tries > 10 {
                bail!("Could not pay invoice");
            }
        }
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
        let client = get_test_client().await;
        client
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

// This is the ln wallet we use to send/receive ln payements as the wallet
async fn _get_lnd_client() -> LndClient {
    let temp_dir = get_work_dir();

    // The LND mint uses the second LND node (LND_TWO_RPC_ADDR = localhost:10010)
    let lnd_dir = get_lnd_dir(&temp_dir, "one");
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");

    println!("Looking for LND cert file: {cert_file:?}");
    println!("Looking for LND macaroon file: {macaroon_file:?}");
    println!("Connecting to LND at: https://{LND_RPC_ADDR}");

    // Connect to LND
    LndClient::new(
        format!("https://{LND_RPC_ADDR}"),
        cert_file.clone(),
        macaroon_file.clone(),
    )
    .await
    .expect("Could not connect to lnd rpc")
}

/// Returns a Lightning client based on the CDK_TEST_LIGHTNING_CLIENT environment variable.
///
/// Reads the CDK_TEST_LIGHTNING_CLIENT environment variable:
/// - "cln" or "CLN": returns a CLN client
/// - Anything else (or unset): returns an LND client (default)
pub async fn get_test_client() -> Box<dyn LightningClient> {
    match env::var("CDK_TEST_LIGHTNING_CLIENT") {
        Ok(val) => {
            let val = val.to_lowercase();
            match val.as_str() {
                "cln" => Box::new(create_cln_client_with_retry().await),
                _ => Box::new(_get_lnd_client().await),
            }
        }
        Err(_) => Box::new(_get_lnd_client().await), // Default to LND
    }
}

fn get_work_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => {
            let path = PathBuf::from(dir);
            println!("Using temp directory from CDK_ITESTS_DIR: {path:?}");
            path
        }
        Err(_) => {
            panic!("Unknown temp dir");
        }
    }
}

// Helper function to create CLN client with retries
async fn create_cln_client_with_retry() -> ClnClient {
    let mut retries = 0;
    let max_retries = 10;

    let cln_dir = get_cln_dir(&get_work_dir(), "one");
    loop {
        match ClnClient::new(cln_dir.clone(), None).await {
            Ok(client) => return client,
            Err(e) => {
                retries += 1;
                if retries >= max_retries {
                    panic!("Could not connect to CLN client after {max_retries} retries: {e}");
                }
                println!(
                    "Failed to connect to CLN (attempt {retries}/{max_retries}): {e}. Retrying in 7 seconds..."
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;
            }
        }
    }
}
