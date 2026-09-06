#![allow(missing_docs)]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::{WalletIdentity, WalletManagerBuilder};
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_sqlite::wallet::memory;

/// This example demonstrates the WalletManager API for managing multiple mints.
///
/// It shows:
/// - Creating a WalletManager
/// - Adding a mint
/// - Minting proofs
/// - Sending tokens
/// - Receiving tokens
/// - Melting (paying Lightning invoices)
/// - Querying balances
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let mint_url = MintUrl::from_str("https://testnut.cashudevkit.org")?;
    let unit = CurrencyUnit::Sat;

    // Generate a seed from a mnemonic (in production, store this securely!)
    let mnemonic = Mnemonic::generate(12)?;
    let seed = mnemonic.to_seed_normalized("");
    println!("Generated mnemonic (save this!): {}", mnemonic);

    // Create the WalletManager
    let localstore = Arc::new(memory::empty().await?);
    let wallet = WalletManagerBuilder::new()
        .with_store(localstore)
        .with_seed(seed)
        .build()
        .await?;
    println!("\nCreated WalletManager");

    let identity = WalletIdentity {
        mint_url: mint_url.clone(),
        unit: unit.clone(),
    };
    let mint_wallet = wallet.wallet(identity.clone()).await?;
    println!("Added mint: {}", mint_url);

    // ========================================
    // MINT: Create proofs from Lightning invoice
    // ========================================
    let mint_amount = Amount::from(100);
    println!("\n--- MINT ---");
    println!("Creating mint quote for {} sats...", mint_amount);

    let mint_session = mint_wallet
        .request_mint(MintRequest::bolt11(mint_amount))
        .await?;
    println!(
        "Invoice to pay: {}",
        mint_session.initial_state().payment_request
    );

    // Wait for quote to be paid and mint proofs
    // With the test mint, this happens automatically
    let mint_receipt = mint_session.wait(Duration::from_secs(30)).await?;
    println!("Minted {} sats", mint_receipt.amount);

    // Check balance
    let balance = mint_wallet.balance().await?.available;
    println!("Total balance: {} sats", balance);

    // ========================================
    // SEND: Create a token to send to someone
    // ========================================
    let send_amount = Amount::from(25);
    println!("\n--- SEND ---");
    println!("Preparing to send {} sats...", send_amount);

    let send_plan = mint_wallet.plan_send(SendRequest::new(send_amount)).await?;
    let token = send_plan.execute().await?.token;
    println!("Token created:\n{}", token);

    // Check balance after send
    let balance = mint_wallet.balance().await?.available;
    println!("Balance after send: {} sats", balance);

    // ========================================
    // RECEIVE: Receive a token (using a second wallet)
    // ========================================
    println!("\n--- RECEIVE ---");

    // Create a second wallet to receive the token
    let receiver_seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_wallet = WalletManagerBuilder::new()
        .with_store(receiver_store)
        .with_seed(receiver_seed)
        .build()
        .await?;

    let receiver_mint_wallet = receiver_wallet
        .wallet(WalletIdentity {
            mint_url: mint_url.clone(),
            unit,
        })
        .await?;

    // Receive the token
    let received = receiver_mint_wallet
        .receive(ReceiveRequest::new(token.to_string()))
        .await?
        .amount;
    println!("Receiver got {} sats", received);

    // Check receiver balance
    let receiver_balance = receiver_mint_wallet.balance().await?.available;
    println!("Receiver balance: {} sats", receiver_balance);

    // ========================================
    // MELT: Pay a Lightning invoice
    // ========================================
    let melt_amount_sats: u64 = 10;
    println!("\n--- MELT ---");
    println!("Creating invoice for {} sats to melt...", melt_amount_sats);

    // Create a fake invoice accepted by the test mint
    let invoice = create_fake_invoice(melt_amount_sats * 1000, "test melt".to_string());
    println!("Invoice: {}", invoice);

    let payment_session = mint_wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(
            invoice.to_string(),
        )))
        .await?
        .into_single()?;
    let melt_quote = payment_session.quote();
    println!(
        "Melt quote: {} sats + {} fee reserve",
        melt_quote.amount, melt_quote.fee_reserve
    );

    // Prepare and execute melt
    let payment_plan = payment_session.prepare().await?;
    let payment_receipt = payment_plan.execute().await?;
    println!("Melt completed! Fee paid: {}", payment_receipt.fee_paid);

    // ========================================
    // BALANCE: Query balances
    // ========================================
    println!("\n--- BALANCES ---");

    for (identity, balance) in wallet.balances().await? {
        println!(
            "  {} ({}): {} sats",
            identity.mint_url, identity.unit, balance.available
        );
    }

    // List all mints
    println!("\nMints in wallet:");
    let wallets = wallet.wallets().await;
    for w in wallets {
        let identity = w.identity();
        println!("  - {} ({})", identity.mint_url, identity.unit);
    }

    Ok(())
}
