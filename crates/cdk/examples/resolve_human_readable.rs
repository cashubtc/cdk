//! # Resolve Human Readable Payment Example
//!
//! This example shows the two-phase flow for BIP-353 addresses:
//! 1. Resolve `user@domain` into a concrete `bitcoin:` payment instruction
//! 2. Inspect the available methods before deciding how to pay
//!
//! This example demonstrates two wallet-level paths after resolution:
//! - If the instruction contains a Cashu payment request and this wallet's mint is accepted,
//!   pay it directly with `wallet.pay_request(...)`
//! - Otherwise, if the instruction contains a BOLT12 offer, request a melt quote with
//!   `wallet.melt_bip353_quote(...)`
//!
//! For BIP-353 melt flows, CDK only accepts BOLT12 offers.
//!
//! ```bash
//! cargo run --example resolve_human_readable --features="wallet bip353"
//! ```

use std::sync::Arc;

use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{resolve_bip353_payment_instruction, Wallet};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bip353_address = "tsk@thesimplekid.com";
    let seed = random::<[u8; 64]>();
    let localstore = Arc::new(memory::empty().await?);
    let wallet = Wallet::new(
        "https://fake.thesimplekid.dev",
        CurrencyUnit::Sat,
        localstore,
        seed,
        None,
    )?;

    println!("Resolving human-readable address: {}", bip353_address);

    let client = wallet.mint_connector();

    match resolve_bip353_payment_instruction(&client, bip353_address, bitcoin::Network::Bitcoin)
        .await
    {
        Ok(parsed) => {
            println!("Resolved payment instruction:");
            println!(
                "  Description: {}",
                parsed.description.as_deref().unwrap_or("None")
            );
            println!(
                "  Amount (msats): {}",
                parsed
                    .amount_msats
                    .map(|a: u64| a.to_string())
                    .unwrap_or_else(|| "None".to_string())
            );
            println!("  Configurable amount: {}", parsed.is_configurable_amount);
            println!("  Cashu requests: {}", parsed.cashu_requests.len());
            println!("  BOLT11 invoices: {}", parsed.bolt11_invoices.len());
            println!("  BOLT12 offers: {}", parsed.bolt12_offers.len());
            println!("  On-chain addresses: {}", parsed.onchain_addresses.len());

            if let Some(request) = parsed.cashu_requests.first() {
                let accepted_by_wallet =
                    request.mints.is_empty() || request.mints.contains(&wallet.mint_url);

                println!("\nCashu payment request found:");
                println!(
                    "  Payment ID: {}",
                    request.payment_id.as_deref().unwrap_or("None")
                );
                println!(
                    "  Amount: {}",
                    request
                        .amount
                        .as_ref()
                        .map(|a: &Amount| a.to_string())
                        .unwrap_or_else(|| "None".to_string())
                );
                println!(
                    "  Unit: {}",
                    request
                        .unit
                        .as_ref()
                        .map(|u: &CurrencyUnit| u.to_string())
                        .unwrap_or_else(|| "None".to_string())
                );
                println!(
                    "  Accepted mints: {}",
                    request
                        .mints
                        .iter()
                        .map(|m: &MintUrl| m.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );

                if accepted_by_wallet {
                    println!(
                        "\nThis wallet's mint is accepted, proceeding with wallet.pay_request(...)"
                    );
                    wallet.pay_request(request.clone(), None).await?;
                    println!("Cashu payment request paid successfully.");
                } else {
                    println!("\nThis wallet cannot pay the Cashu request.");
                    println!(
                        "The request does not accept wallet mint: {}",
                        wallet.mint_url
                    );
                    println!("Use a wallet for one of the accepted mints or a WalletRepository.");
                }
            } else if let Some(offer) = parsed.bolt12_offers.first() {
                println!(
                    "\nNo Cashu request found. BOLT12 offer found, proceeding to melt quote..."
                );
                let quote = wallet
                    .melt_bip353_quote(
                        bip353_address,
                        Amount::from(100_000_u64),
                        bitcoin::Network::Bitcoin,
                    )
                    .await?;

                println!("Melt quote created:");
                println!("  Quote ID: {}", quote.id);
                println!("  Amount: {}", quote.amount);
                println!("  Fee Reserve: {}", quote.fee_reserve);
                println!("  Payment Method: {}", quote.payment_method);
                println!("  First BOLT12 offer: {}", offer);
            } else {
                println!("\nNo Cashu request or BOLT12 offer found.");
                println!("This example only pays Cashu requests or melts BOLT12 offers.");
            }
        }
        Err(e) => {
            println!("Failed to resolve payment instruction: {}", e);
            println!("This could be because the address has no BIP-353 DNS records,");
            println!("the DNS result is invalid, or the network request failed.");
        }
    }

    Ok(())
}
