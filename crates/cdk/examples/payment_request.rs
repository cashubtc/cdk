//! # Payment Request Example (NUT-18)
//!
//! This example demonstrates how to create and receive payments using NUT-18
//! payment requests with the MultiMintWallet. It shows both HTTP and Nostr
//! transport options.
//!
//! ## Payment Request Flow
//!
//! 1. Receiver creates a payment request with desired parameters
//! 2. Receiver shares the encoded payment request string with the payer
//! 3. Payer decodes the request and sends tokens via the specified transport
//! 4. Receiver waits for and receives the payment
//!
//! ## Transport Options
//!
//! - **Nostr**: Privacy-preserving delivery via Nostr relays (gift-wrapped events)
//! - **HTTP**: Direct delivery to a specified callback URL
//! - **None**: Out-of-band delivery (receiver must receive tokens manually)
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example payment_request --features="wallet nostr"
//! ```

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use cdk::amount::SplitTarget;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::payment_request::CreateRequestParams;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("NUT-18 Payment Request Example");
    println!("===============================\n");

    // Generate a random seed for the wallet
    let seed: [u8; 64] = random();

    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let initial_amount = cdk::Amount::from(100);

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new MultiMintWallet
    let wallet = MultiMintWallet::new(localstore, seed, unit.clone()).await?;

    // Add the mint to our wallet
    wallet.add_mint(mint_url.parse()?).await?;

    println!("Step 1: Funding the wallet");
    println!("---------------------------");

    // Get a wallet for our mint to create a mint quote
    let mint_wallet = wallet
        .get_wallet(&mint_url.parse()?)
        .await
        .ok_or_else(|| anyhow!("Wallet not found for mint"))?;
    let mint_quote = mint_wallet.mint_quote(initial_amount, None).await?;

    println!(
        "Pay this invoice to fund the wallet:\n{}",
        mint_quote.request
    );
    println!("\nQuote ID: {}", mint_quote.id);

    // Wait for payment and mint tokens
    println!("\nWaiting for payment...");
    let _proofs = mint_wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(300),
        )
        .await?;

    let balance = wallet.total_balance().await?;
    println!("Wallet funded with {} sats\n", balance);

    // ============================================================================
    // Example 1: Create a Payment Request with Nostr Transport
    // ============================================================================

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Example 1: Payment Request with Nostr Transport               ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("Creating a payment request for 10 sats via Nostr...\n");

    let nostr_params = CreateRequestParams {
        amount: Some(10),
        unit: "sat".to_string(),
        description: Some("Coffee payment".to_string()),
        pubkeys: None,
        num_sigs: 1,
        hash: None,
        preimage: None,
        transport: "nostr".to_string(),
        http_url: None,
        nostr_relays: Some(vec![
            "wss://relay.damus.io".to_string(),
            "wss://nos.lol".to_string(),
        ]),
    };

    let (payment_request, nostr_wait_info) = wallet.create_request(nostr_params).await?;

    println!("Payment Request Created!");
    println!("------------------------");
    println!("Encoded: {}\n", payment_request);

    println!("Request Details:");
    println!("  Amount: {:?}", payment_request.amount);
    println!("  Unit: {:?}", payment_request.unit);
    println!("  Description: {:?}", payment_request.description);
    println!("  Mints: {:?}", payment_request.mints);
    println!("  Transports: {:?}", payment_request.transports);

    if let Some(ref info) = nostr_wait_info {
        println!("\nNostr Wait Info:");
        println!("  Relays: {:?}", info.relays);
        println!("  Pubkey: {}", info.pubkey);

        println!("\nTo receive payment, call:");
        println!("  let amount = wallet.wait_for_nostr_payment(nostr_wait_info).await?;");
        println!("\nThis will:");
        println!("  1. Connect to the specified Nostr relays");
        println!("  2. Subscribe for gift-wrapped payment events");
        println!("  3. Receive and process the first valid payment");
        println!("  4. Return the received amount");

        // Uncomment to actually wait for a payment:
        // println!("\nWaiting for Nostr payment...");
        // let received = wallet.wait_for_nostr_payment(info.clone()).await?;
        // println!("Received {} sats via Nostr!", received);
    }

    // ============================================================================
    // Example 2: Create a Payment Request with HTTP Transport
    // ============================================================================

    println!("\n\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Example 2: Payment Request with HTTP Transport                ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("Creating a payment request for 21 sats via HTTP...\n");

    let http_params = CreateRequestParams {
        amount: Some(21),
        unit: "sat".to_string(),
        description: Some("Tip jar".to_string()),
        pubkeys: None,
        num_sigs: 1,
        hash: None,
        preimage: None,
        transport: "http".to_string(),
        http_url: Some("https://example.com/cashu/callback".to_string()),
        nostr_relays: None,
    };

    let (http_request, _) = wallet.create_request(http_params).await?;

    println!("Payment Request Created!");
    println!("------------------------");
    println!("Encoded: {}\n", http_request);

    println!("Request Details:");
    println!("  Amount: {:?}", http_request.amount);
    println!("  Unit: {:?}", http_request.unit);
    println!("  Description: {:?}", http_request.description);
    println!("  Transports: {:?}", http_request.transports);

    println!("\nWith HTTP transport:");
    println!("  - Payer will POST tokens to: https://example.com/cashu/callback");
    println!("  - Your server receives the token and calls wallet.receive()");

    // ============================================================================
    // Example 3: Create a Payment Request with P2PK Spending Conditions
    // ============================================================================

    println!("\n\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Example 3: Payment Request with P2PK Lock                     ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("Creating a P2PK-locked payment request...\n");

    // Generate a secret key for the spending condition
    let secret = cdk::nuts::SecretKey::generate();
    let pubkey_hex = secret.public_key().to_string();

    let p2pk_params = CreateRequestParams {
        amount: Some(50),
        unit: "sat".to_string(),
        description: Some("Locked payment".to_string()),
        pubkeys: Some(vec![pubkey_hex.clone()]),
        num_sigs: 1,
        hash: None,
        preimage: None,
        transport: "nostr".to_string(),
        http_url: None,
        nostr_relays: Some(vec!["wss://relay.damus.io".to_string()]),
    };

    let (p2pk_request, _) = wallet.create_request(p2pk_params).await?;

    println!("P2PK Payment Request Created!");
    println!("-----------------------------");
    println!("Encoded: {}\n", p2pk_request);

    println!("Security:");
    println!("  - Tokens sent to this request will be locked to pubkey:");
    println!("    {}", pubkey_hex);
    println!("  - Only the holder of the corresponding secret key can spend");

    // ============================================================================
    // Example 4: Paying a Payment Request
    // ============================================================================

    println!("\n\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Example 4: Paying a Payment Request                           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("To pay a payment request from another wallet:\n");

    println!("```rust");
    println!("// Decode the payment request");
    println!("let request = PaymentRequest::from_str(\"creqA...\")?;");
    println!();
    println!("// Pay the request (sends tokens via the specified transport)");
    println!("let result = wallet.pay_request(request).await?;");
    println!();
    println!("println!(\"Sent {{}} sats\", result.amount_sent);");
    println!("```\n");

    println!("The pay_request method will:");
    println!("  1. Select proofs matching the requested amount and unit");
    println!("  2. Apply any spending conditions from the request");
    println!("  3. Deliver the token via the request's transport (Nostr/HTTP)");

    println!("\n✓ Example complete!");

    Ok(())
}
