//! Example: MultiMint Wallet with NpubCash - Switching Active Mints
//!
//! This example demonstrates:
//! 1. Creating a MultiMintWallet with multiple mints
//! 2. Using NpubCash integration with the MultiMintWallet API
//! 3. Switching the active mint for NpubCash deposits
//! 4. Receiving payments to different mints and verifying balances
//!
//! Key concept: Since all wallets in a MultiMintWallet share the same seed, they all
//! derive the same Nostr keypair. This means your npub.cash address stays the same,
//! but you can change which mint receives the deposits.

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use cdk::StreamExt;
use cdk_sqlite::wallet::memory;
use nostr_sdk::ToBech32;

const NPUBCASH_URL: &str = "https://npubx.cash";
const MINT_URL_1: &str = "https://fake.thesimplekid.dev";
const MINT_URL_2: &str = "https://testnut.cashu.space";
const PAYMENT_AMOUNT_MSATS: u64 = 10000; // 10 sats

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== MultiMint Wallet with NpubCash Example ===\n");

    // -------------------------------------------------------------------------
    // Step 1: Create MultiMintWallet and add mints
    // -------------------------------------------------------------------------
    println!("Step 1: Setting up MultiMintWallet...\n");

    let seed: [u8; 64] = {
        let mut s = [0u8; 64];
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for (i, byte) in s.iter_mut().enumerate() {
            *byte = ((timestamp >> (i % 16)) & 0xFF) as u8;
        }
        s
    };

    let localstore = memory::empty().await?;
    let wallet_repository = WalletRepository::new(Arc::new(localstore), seed).await?;

    let mint_url_1: MintUrl = MINT_URL_1.parse()?;
    let mint_url_2: MintUrl = MINT_URL_2.parse()?;

    wallet_repository.add_mint(mint_url_1.clone()).await?;
    wallet_repository.add_mint(mint_url_2.clone()).await?;
    println!("   Added mints: {}, {}\n", mint_url_1, mint_url_2);

    // -------------------------------------------------------------------------
    // Step 2: Enable NpubCash on mint 1
    // -------------------------------------------------------------------------
    println!("Step 2: Enabling NpubCash on mint 1...\n");
    let wallet = wallet_repository
        .get_wallet(&mint_url_1.clone(), &CurrencyUnit::Sat)
        .await?;
    wallet.enable_npubcash(NPUBCASH_URL.to_string()).await?;

    let keys = wallet.get_npubcash_keys().unwrap();
    let npub = keys.public_key().to_bech32()?;
    let display_url = NPUBCASH_URL.trim_start_matches("https://");

    println!("   Your npub.cash address: {}@{}", npub, display_url);
    println!("   Active mint: {}\n", mint_url_1);

    // -------------------------------------------------------------------------
    // Step 3: Request and receive payment on mint 1
    // -------------------------------------------------------------------------
    println!("Step 3: Receiving payment on mint 1...\n");

    request_invoice(&npub, PAYMENT_AMOUNT_MSATS).await?;
    println!("   Waiting for payment...");
    let mut stream = wallet.npubcash_proof_stream(
        SplitTarget::default(),
        None, // no spending conditions
        Duration::from_secs(1),
    );

    let (_, proofs_1) = stream.next().await.ok_or("Stream ended unexpectedly")??;

    let amount_1: u64 = proofs_1.total_amount()?.into();
    println!("   Received {} sats on mint 1!\n", amount_1);

    // -------------------------------------------------------------------------
    // Step 4: Switch to mint 2 and receive payment
    // -------------------------------------------------------------------------
    println!("Step 4: Switching to mint 2 and receiving payment...\n");
    let wallet = wallet_repository
        .get_wallet(&mint_url_1.clone(), &CurrencyUnit::Sat)
        .await?;

    wallet.enable_npubcash(NPUBCASH_URL.to_string()).await?;
    println!("   Switched to mint: {}", mint_url_2);

    request_invoice(&npub, PAYMENT_AMOUNT_MSATS).await?;
    println!("   Waiting for payment...");

    // The stream is for the multimint wallet so it handles switching mints automatically
    let (_, proofs_2) = stream.next().await.ok_or("Stream ended unexpectedly")??;

    let amount_2: u64 = proofs_2.total_amount()?.into();
    println!("   Received {} sats on mint 2!\n", amount_2);

    // -------------------------------------------------------------------------
    // Step 5: Verify balances
    // -------------------------------------------------------------------------
    println!("Step 5: Verifying balances...\n");

    let balances = wallet_repository.get_balances().await?;
    for (mint, balance) in &balances {
        println!("   {}: {} sats", mint, balance);
    }

    let total = wallet.total_balance().await?;
    let expected_total = amount_1 + amount_2;

    println!(
        "\n   Total: {} sats (expected: {} sats)",
        total, expected_total
    );
    println!(
        "   Status: {}\n",
        if total == expected_total.into() {
            "OK"
        } else {
            "MISMATCH"
        }
    );

    Ok(())
}

/// Request an invoice via LNURL-pay
async fn request_invoice(npub: &str, amount_msats: u64) -> Result<(), Box<dyn std::error::Error>> {
    let http_client = reqwest::Client::new();

    let lnurlp_url = format!("{}/.well-known/lnurlp/{}", NPUBCASH_URL, npub);
    let lnurlp_response: serde_json::Value =
        http_client.get(&lnurlp_url).send().await?.json().await?;

    let callback = lnurlp_response["callback"]
        .as_str()
        .ok_or("No callback URL")?;

    let invoice_url = format!("{}?amount={}", callback, amount_msats);
    let invoice_response: serde_json::Value =
        http_client.get(&invoice_url).send().await?.json().await?;

    let pr = invoice_response["pr"]
        .as_str()
        .ok_or("No payment request")?;
    println!("   Invoice: {}...", &pr[..50.min(pr.len())]);

    Ok(())
}
