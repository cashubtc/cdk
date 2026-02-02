#![allow(missing_docs)]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::{MultiMintReceiveOptions, SendOptions};
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_sqlite::wallet::memory;

/// This example demonstrates the MultiMintWallet API for managing multiple mints.
///
/// It shows:
/// - Creating a MultiMintWallet
/// - Adding a mint
/// - Minting proofs
/// - Sending tokens
/// - Receiving tokens
/// - Melting (paying Lightning invoices)
/// - Querying balances
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let mint_url = MintUrl::from_str("https://fake.thesimplekid.dev")?;
    let unit = CurrencyUnit::Sat;

    // Generate a seed from a mnemonic (in production, store this securely!)
    let mnemonic = Mnemonic::generate(12)?;
    let seed = mnemonic.to_seed_normalized("");
    println!("Generated mnemonic (save this!): {}", mnemonic);

    // Create the MultiMintWallet
    let localstore = Arc::new(memory::empty().await?);
    let wallet = MultiMintWallet::new(localstore, seed, unit.clone()).await?;
    println!("\nCreated MultiMintWallet");

    // Add a mint to the wallet
    wallet.add_mint(mint_url.clone()).await?;
    println!("Added mint: {}", mint_url);

    // ========================================
    // MINT: Create proofs from Lightning invoice
    // ========================================
    let mint_amount = Amount::from(100);
    println!("\n--- MINT ---");
    println!("Creating mint quote for {} sats...", mint_amount);

    let mint_quote = wallet
        .mint_quote(
            &mint_url,
            PaymentMethod::BOLT11,
            Some(mint_amount),
            None,
            None,
        )
        .await?;
    println!("Invoice to pay: {}", mint_quote.request);

    // Wait for quote to be paid and mint proofs
    // With the fake mint, this happens automatically
    let proofs = wallet
        .wait_for_mint_quote(
            &mint_url,
            &mint_quote.id,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        )
        .await?;

    let minted_amount = proofs.total_amount()?;
    println!("Minted {} sats", minted_amount);

    // Check balance
    let balance = wallet.total_balance().await?;
    println!("Total balance: {} sats", balance);

    // ========================================
    // SEND: Create a token to send to someone
    // ========================================
    let send_amount = Amount::from(25);
    println!("\n--- SEND ---");
    println!("Preparing to send {} sats...", send_amount);

    let prepared_send = wallet
        .prepare_send(mint_url.clone(), send_amount, SendOptions::default())
        .await?;
    let token = prepared_send.confirm(None).await?;
    println!("Token created:\n{}", token);

    // Check balance after send
    let balance = wallet.total_balance().await?;
    println!("Balance after send: {} sats", balance);

    // ========================================
    // RECEIVE: Receive a token (using a second wallet)
    // ========================================
    println!("\n--- RECEIVE ---");

    // Create a second wallet to receive the token
    let receiver_seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_wallet = MultiMintWallet::new(receiver_store, receiver_seed, unit).await?;

    // Add the mint (or use allow_untrusted)
    receiver_wallet.add_mint(mint_url.clone()).await?;

    // Receive the token
    let received = receiver_wallet
        .receive(&token.to_string(), MultiMintReceiveOptions::default())
        .await?;
    println!("Receiver got {} sats", received);

    // Check receiver balance
    let receiver_balance = receiver_wallet.total_balance().await?;
    println!("Receiver balance: {} sats", receiver_balance);

    // ========================================
    // MELT: Pay a Lightning invoice
    // ========================================
    let melt_amount_sats: u64 = 10;
    println!("\n--- MELT ---");
    println!("Creating invoice for {} sats to melt...", melt_amount_sats);

    // Create a fake invoice (works with fake.thesimplekid.dev)
    let invoice = create_fake_invoice(melt_amount_sats * 1000, "test melt".to_string());
    println!("Invoice: {}", invoice);

    // Create melt quote
    let melt_quote = wallet
        .melt_quote(
            &mint_url,
            PaymentMethod::BOLT11,
            invoice.to_string(),
            None,
            None,
        )
        .await?;
    println!(
        "Melt quote: {} sats + {} fee reserve",
        melt_quote.amount, melt_quote.fee_reserve
    );

    // Prepare and execute melt
    let prepared_melt = wallet
        .prepare_melt(&mint_url, &melt_quote.id, HashMap::new())
        .await?;
    let melt_result = prepared_melt.confirm().await?;
    println!("Melt completed! State: {:?}", melt_result.state());

    // ========================================
    // BALANCE: Query balances
    // ========================================
    println!("\n--- BALANCES ---");

    let total = wallet.total_balance().await?;
    println!("Total balance: {} sats", total);

    let per_mint = wallet.get_balances().await?;
    for (url, amount) in per_mint {
        println!("  {}: {} sats", url, amount);
    }

    // List all mints
    println!("\nMints in wallet:");
    let wallets = wallet.get_wallets().await;
    for w in wallets {
        println!("  - {} ({})", w.mint_url, w.unit);
    }

    Ok(())
}
