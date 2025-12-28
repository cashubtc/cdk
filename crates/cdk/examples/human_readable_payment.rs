//! # Human Readable Payment Example
//!
//! This example demonstrates how to use both BIP-353 and Lightning Address (LNURL-pay)
//! with the CDK wallet. Both allow users to share simple email-like addresses instead
//! of complex Bitcoin addresses or Lightning invoices.
//!
//! ## BIP-353 (Bitcoin URI Payment Instructions)
//!
//! BIP-353 uses DNS TXT records to resolve human-readable addresses to BOLT12 offers.
//! 1. Parse a human-readable address like `user@domain.com`
//! 2. Query DNS TXT records at `user.user._bitcoin-payment.domain.com`
//! 3. Extract Lightning offers (BOLT12) from the TXT records
//! 4. Use the offer to create a melt quote
//!
//! ## Lightning Address (LNURL-pay)
//!
//! Lightning Address uses HTTPS to fetch BOLT11 invoices.
//! 1. Parse a Lightning address like `user@domain.com`
//! 2. Query HTTPS endpoint at `https://domain.com/.well-known/lnurlp/user`
//! 3. Get callback URL and amount constraints
//! 4. Request BOLT11 invoice with the specified amount
//!
//! ## Unified API
//!
//! The `melt_human_readable_quote()` method automatically tries BIP-353 first
//! (if the mint supports BOLT12), then falls back to Lightning Address if needed.
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example human_readable_payment --features="wallet bip353"
//! ```

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Human Readable Payment Example");
    println!("================================\n");

    // Example addresses
    let bip353_address = "tsk@thesimplekid.com";
    let lnurl_address =
        "npub1qjgcmlpkeyl8mdkvp4s0xls4ytcux6my606tgfx9xttut907h0zs76lgjw@npubx.cash";

    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let initial_amount = Amount::from(2000); // Start with 2000 sats (enough for both payments)

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    println!("Step 1: Funding the wallet");
    println!("---------------------------");

    // First, we need to fund the wallet
    println!("Requesting mint quote for {} sats...", initial_amount);
    let mint_quote = wallet.mint_quote(initial_amount, None).await?;
    println!(
        "Pay this invoice to fund the wallet:\n{}",
        mint_quote.request
    );
    println!("\nQuote ID: {}", mint_quote.id);

    // Wait for payment and mint tokens automatically
    println!("\nWaiting for payment... (in real use, pay the above invoice)");
    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(300), // 5 minutes timeout
        )
        .await?;

    let received_amount = proofs.total_amount()?;
    println!("✓ Successfully minted {} sats\n", received_amount);

    // ============================================================================
    // Part 1: BIP-353 Payment
    // ============================================================================

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Part 1: BIP-353 Payment (BOLT12 Offer via DNS)                ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let bip353_amount_sats = 100; // Example: paying 100 sats
    println!("BIP-353 Address: {}", bip353_address);
    println!("Payment Amount: {} sats", bip353_amount_sats);
    println!("\nHow BIP-353 works:");
    println!("1. Parse address into user@domain");
    println!("2. Query DNS TXT records at: tsk.user._bitcoin-payment.thesimplekid.com");
    println!("3. Extract BOLT12 offer from DNS records");
    println!("4. Create melt quote with the offer\n");

    // Use the specific BIP353 method
    println!("Attempting BIP-353 payment...");
    match wallet
        .melt_bip353_quote(bip353_address, bip353_amount_sats * 1_000)
        .await
    {
        Ok(melt_quote) => {
            println!("✓ BIP-353 melt quote received:");
            println!("  Quote ID: {}", melt_quote.id);
            println!("  Amount: {} sats", melt_quote.amount);
            println!("  Fee Reserve: {} sats", melt_quote.fee_reserve);
            println!("  State: {}", melt_quote.state);
            println!("  Payment Method: {}", melt_quote.payment_method);

            // Prepare the payment - shows fees before confirming
            println!("\nPreparing payment...");
            match wallet
                .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
                .await
            {
                Ok(prepared) => {
                    println!("✓ Prepared melt:");
                    println!("  Amount: {} sats", prepared.amount());
                    println!("  Total Fee: {} sats", prepared.total_fee());

                    // Execute the payment
                    println!("\nExecuting payment...");
                    match prepared.confirm().await {
                        Ok(confirmed) => {
                            println!("✓ BIP-353 payment successful!");
                            println!("  State: {:?}", confirmed.state());
                            println!("  Amount paid: {} sats", confirmed.amount());
                            println!("  Fee paid: {} sats", confirmed.fee_paid());

                            if let Some(preimage) = confirmed.payment_proof() {
                                println!("  Payment preimage: {}", preimage);
                            }
                        }
                        Err(e) => {
                            println!("✗ BIP-353 payment failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("✗ Failed to prepare melt: {}", e);
                }
            }
        }
        Err(e) => {
            println!("✗ Failed to get BIP-353 melt quote: {}", e);
            println!("\nPossible reasons:");
            println!("  • DNS resolution failed or no DNS records found");
            println!("  • No Lightning offer (BOLT12) in DNS TXT records");
            println!("  • DNSSEC validation failed");
            println!("  • Mint doesn't support BOLT12");
            println!("  • Network connectivity issues");
        }
    }

    // ============================================================================
    // Part 2: Lightning Address (LNURL-pay) Payment
    // ============================================================================

    println!("\n\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Part 2: Lightning Address Payment (BOLT11 via LNURL-pay)      ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let lnurl_amount_sats = 100; // Example: paying 100 sats
    println!("Lightning Address: {}", lnurl_address);
    println!("Payment Amount: {} sats", lnurl_amount_sats);
    println!("\nHow Lightning Address works:");
    println!("1. Parse address into user@domain");
    println!("2. Query HTTPS: https://npubx.cash/.well-known/lnurlp/npub1qj...");
    println!("3. Get callback URL and amount constraints");
    println!("4. Request BOLT11 invoice for the amount");
    println!("5. Create melt quote with the invoice\n");

    // Use the specific Lightning Address method
    println!("Attempting Lightning Address payment...");
    match wallet
        .melt_lightning_address_quote(lnurl_address, lnurl_amount_sats * 1_000)
        .await
    {
        Ok(melt_quote) => {
            println!("✓ Lightning Address melt quote received:");
            println!("  Quote ID: {}", melt_quote.id);
            println!("  Amount: {} sats", melt_quote.amount);
            println!("  Fee Reserve: {} sats", melt_quote.fee_reserve);
            println!("  State: {}", melt_quote.state);
            println!("  Payment Method: {}", melt_quote.payment_method);

            // Prepare the payment - shows fees before confirming
            println!("\nPreparing payment...");
            match wallet
                .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
                .await
            {
                Ok(prepared) => {
                    println!("✓ Prepared melt:");
                    println!("  Amount: {} sats", prepared.amount());
                    println!("  Total Fee: {} sats", prepared.total_fee());

                    // Execute the payment
                    println!("\nExecuting payment...");
                    match prepared.confirm().await {
                        Ok(confirmed) => {
                            println!("✓ Lightning Address payment successful!");
                            println!("  State: {:?}", confirmed.state());
                            println!("  Amount paid: {} sats", confirmed.amount());
                            println!("  Fee paid: {} sats", confirmed.fee_paid());

                            if let Some(preimage) = confirmed.payment_proof() {
                                println!("  Payment preimage: {}", preimage);
                            }
                        }
                        Err(e) => {
                            println!("✗ Lightning Address payment failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("✗ Failed to prepare melt: {}", e);
                }
            }
        }
        Err(e) => {
            println!("✗ Failed to get Lightning Address melt quote: {}", e);
            println!("\nPossible reasons:");
            println!("  • HTTPS request to .well-known/lnurlp failed");
            println!("  • Invalid Lightning Address format");
            println!("  • Amount outside min/max constraints");
            println!("  • Service unavailable or network issues");
        }
    }

    // ============================================================================
    // Part 3: Unified Human Readable API (Smart Fallback)
    // ============================================================================

    println!("\n\n╔════════════════════════════════════════════════════════════════╗");
    println!("║ Part 3: Unified API (Automatic BIP-353 → LNURL Fallback)      ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("The `melt_human_readable_quote()` method intelligently chooses:");
    println!("1. If mint supports BOLT12 AND address has BIP-353 DNS: Use BIP-353");
    println!("2. If BIP-353 DNS fails OR address has no DNS: Fall back to LNURL");
    println!("3. If mint doesn't support BOLT12: Use LNURL directly\n");

    // Test 1: Address with BIP-353 support (has DNS records)
    let unified_amount_sats = 50;
    println!("Test 1: Address with BIP-353 DNS support");
    println!("Address: {}", bip353_address);
    println!("Payment Amount: {} sats", unified_amount_sats);
    println!("Expected: BIP-353 (BOLT12) via DNS resolution\n");

    println!("Attempting unified payment...");
    match wallet
        .melt_human_readable_quote(bip353_address, unified_amount_sats * 1_000)
        .await
    {
        Ok(melt_quote) => {
            println!("✓ Unified melt quote received:");
            println!("  Quote ID: {}", melt_quote.id);
            println!("  Amount: {} sats", melt_quote.amount);
            println!("  Fee Reserve: {} sats", melt_quote.fee_reserve);
            println!("  Payment Method: {}", melt_quote.payment_method);

            let method_str = melt_quote.payment_method.to_string().to_lowercase();
            let used_method = if method_str.contains("bolt12") {
                "BIP-353 (BOLT12)"
            } else if method_str.contains("bolt11") {
                "Lightning Address (LNURL-pay)"
            } else {
                "Unknown method"
            };
            println!("\n  → Used: {}", used_method);
        }
        Err(e) => {
            println!("✗ Failed to get unified melt quote: {}", e);
            println!("  Both BIP-353 and Lightning Address resolution failed");
        }
    }

    // Test 2: Address without BIP-353 support (LNURL only)
    println!("\n\nTest 2: Address without BIP-353 (LNURL-only)");
    println!("Address: {}", lnurl_address);
    println!("Payment Amount: {} sats", unified_amount_sats);
    println!("Expected: Lightning Address (LNURL-pay) fallback\n");

    println!("Attempting unified payment...");
    match wallet
        .melt_human_readable_quote(lnurl_address, unified_amount_sats * 1_000)
        .await
    {
        Ok(melt_quote) => {
            println!("✓ Unified melt quote received:");
            println!("  Quote ID: {}", melt_quote.id);
            println!("  Amount: {} sats", melt_quote.amount);
            println!("  Fee Reserve: {} sats", melt_quote.fee_reserve);
            println!("  Payment Method: {}", melt_quote.payment_method);

            let method_str = melt_quote.payment_method.to_string().to_lowercase();
            let used_method = if method_str.contains("bolt12") {
                "BIP-353 (BOLT12)"
            } else if method_str.contains("bolt11") {
                "Lightning Address (LNURL-pay)"
            } else {
                "Unknown method"
            };
            println!("\n  → Used: {}", used_method);
            println!("\n  Note: This address doesn't have BIP-353 DNS records,");
            println!("        so it automatically fell back to LNURL-pay.");
        }
        Err(e) => {
            println!("✗ Failed to get unified melt quote: {}", e);
            println!("  Both BIP-353 and Lightning Address resolution failed");
        }
    }

    Ok(())
}
