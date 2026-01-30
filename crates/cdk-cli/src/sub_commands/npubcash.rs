use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::{MultiMintWallet, Wallet};
use cdk::StreamExt;
use clap::Subcommand;
use nostr_sdk::ToBech32;

/// Helper function to get wallet for a specific mint URL
async fn get_wallet_for_mint(
    multi_mint_wallet: &MultiMintWallet,
    mint_url_str: &str,
) -> Result<Arc<Wallet>> {
    let mint_url = MintUrl::from_str(mint_url_str)?;

    // Check if wallet exists for this mint
    if !multi_mint_wallet.has_mint(&mint_url).await {
        // Add the mint to the wallet
        multi_mint_wallet.add_mint(mint_url.clone()).await?;
    }

    multi_mint_wallet
        .get_wallet(&mint_url)
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get wallet for mint: {}", mint_url_str))
}

#[derive(Subcommand)]
pub enum NpubCashSubCommand {
    /// Sync quotes from NpubCash
    Sync,
    /// List all quotes
    List {
        /// Only show quotes since this Unix timestamp
        #[arg(long)]
        since: Option<u64>,
        /// Output format (table/json)
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Subscribe to quote updates and auto-mint paid quotes
    Subscribe,
    /// Set mint URL for NpubCash
    SetMint {
        /// The mint URL to use
        url: String,
    },
    /// Show Nostr keys used for NpubCash authentication
    ShowKeys,
}

pub async fn npubcash(
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &str,
    sub_command: &NpubCashSubCommand,
    npubcash_url: Option<String>,
) -> Result<()> {
    // Get default npubcash URL if not provided
    let base_url = npubcash_url.unwrap_or_else(|| "https://npubx.cash".to_string());

    match sub_command {
        NpubCashSubCommand::Sync => sync(multi_mint_wallet, mint_url, &base_url).await,
        NpubCashSubCommand::List { since, format } => {
            list(multi_mint_wallet, mint_url, &base_url, *since, format).await
        }
        NpubCashSubCommand::Subscribe => subscribe(multi_mint_wallet, mint_url, &base_url).await,
        NpubCashSubCommand::SetMint { url } => {
            set_mint(multi_mint_wallet, mint_url, &base_url, url).await
        }
        NpubCashSubCommand::ShowKeys => show_keys(multi_mint_wallet, mint_url).await,
    }
}

/// Helper function to ensure active mint consistency
async fn ensure_active_mint(multi_mint_wallet: &MultiMintWallet, mint_url: &str) -> Result<()> {
    let mint_url_struct = MintUrl::from_str(mint_url)?;

    match multi_mint_wallet.get_active_npubcash_mint().await? {
        Some(active_mint) => {
            if active_mint != mint_url_struct {
                bail!(
                    "Active NpubCash mint mismatch!\n\
                    Current active mint: {}\n\
                    Requested mint: {}\n\n\
                    You can only have one active mint for NpubCash at a time.\n\
                    Use 'set-mint' command to switch active mint.",
                    active_mint,
                    mint_url
                );
            }
        }
        None => {
            // No active mint set, set this one as active
            multi_mint_wallet
                .set_active_npubcash_mint(mint_url_struct)
                .await?;
            println!("✓ Set {} as active NpubCash mint", mint_url);
        }
    }
    Ok(())
}

async fn sync(multi_mint_wallet: &MultiMintWallet, mint_url: &str, base_url: &str) -> Result<()> {
    ensure_active_mint(multi_mint_wallet, mint_url).await?;

    println!("Syncing quotes from NpubCash...");

    let wallet = get_wallet_for_mint(multi_mint_wallet, mint_url).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    let quotes = wallet.sync_npubcash_quotes().await?;

    println!("✓ Synced {} quotes successfully", quotes.len());
    Ok(())
}

async fn list(
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &str,
    base_url: &str,
    since: Option<u64>,
    format: &str,
) -> Result<()> {
    ensure_active_mint(multi_mint_wallet, mint_url).await?;

    let wallet = get_wallet_for_mint(multi_mint_wallet, mint_url).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    let quotes = if let Some(since_ts) = since {
        wallet.sync_npubcash_quotes_since(since_ts).await?
    } else {
        wallet.sync_npubcash_quotes().await?
    };

    match format {
        "json" => {
            let json = serde_json::to_string_pretty(&quotes)?;
            println!("{}", json);
        }
        "table" => {
            if quotes.is_empty() {
                println!("No quotes found");
            } else {
                println!("\nQuotes:");
                println!("{:-<80}", "");
                for (i, quote) in quotes.iter().enumerate() {
                    println!("{}. ID: {}", i + 1, quote.id);
                    let amount_str = quote
                        .amount
                        .map_or("unknown".to_string(), |a| a.to_string());
                    println!("   Amount: {} {}", amount_str, quote.unit);
                    println!("{:-<80}", "");
                }
                println!("\nTotal: {} quotes", quotes.len());
            }
        }
        _ => bail!("Invalid format '{}'. Use 'table' or 'json'", format),
    }

    Ok(())
}

async fn subscribe(
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &str,
    base_url: &str,
) -> Result<()> {
    ensure_active_mint(multi_mint_wallet, mint_url).await?;

    println!("=== NpubCash Quote Subscription ===\n");

    let wallet = get_wallet_for_mint(multi_mint_wallet, mint_url).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;
    println!("✓ NpubCash integration enabled\n");

    // Display the npub.cash address
    let keys = wallet.get_npubcash_keys()?;
    let display_url = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    println!("Your npub.cash address:");
    println!("   {}@{}\n", keys.public_key().to_bech32()?, display_url);
    println!("Send sats to this address to see them appear!\n");

    println!("Auto-mint is ENABLED - paid quotes will be automatically minted\n");

    println!("Starting quote polling...");
    println!("Press Ctrl+C to stop.\n");

    // Run polling and wait for Ctrl+C
    let mut stream =
        wallet.npubcash_proof_stream(SplitTarget::default(), None, Duration::from_secs(5));

    tokio::select! {
        _ = async {
            while let Some(result) = stream.next().await {
                match result {
                    Ok((quote, proofs)) => {
                        let amount_str = quote.amount.map_or("unknown".to_string(), |a| a.to_string());
                        println!("Received payment for quote {}", quote.id);
                        println!("  ├─ Amount: {} {}", amount_str, quote.unit);

                        match proofs.total_amount() {
                            Ok(amount) => {
                                println!("  └─ Successfully minted {} sats!", amount);
                                if let Ok(balance) = wallet.total_balance().await {
                                    println!("     Wallet balance: {} sats", balance);
                                }
                            }
                            Err(e) => println!("  └─ Failed to calculate amount: {}", e),
                        }
                        println!();
                    }
                    Err(e) => {
                        println!("Error processing payment: {}", e);
                    }
                }
            }
        } => {}
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping quote polling...");
        }
    }

    // Show final wallet balance
    let balance = wallet.total_balance().await?;
    println!("Final wallet balance: {} sats\n", balance);

    Ok(())
}

async fn set_mint(
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &str,
    base_url: &str,
    url: &str,
) -> Result<()> {
    println!("Setting NpubCash mint URL to: {}", url);

    // Update active mint in KV store
    let mint_url_struct = MintUrl::from_str(mint_url)?;
    multi_mint_wallet
        .set_active_npubcash_mint(mint_url_struct)
        .await?;

    let wallet = get_wallet_for_mint(multi_mint_wallet, mint_url).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    // Try to set the mint URL on the NpubCash server
    match wallet.set_npubcash_mint_url(url).await {
        Ok(_) => {
            println!("✓ Mint URL updated successfully on NpubCash server");
            println!("\nThe NpubCash server will now include this mint URL");
            println!("when creating quotes for your npub address.");
        }
        Err(e) => {
            let error_msg = e.to_string();

            // Check if the error is a 404 (endpoint not supported)
            if error_msg.contains("API error (404)") {
                println!("⚠️  Warning: NpubCash server does not support setting mint URL");
                println!("\nThis means:");
                println!(
                    "  • The server at '{}' does not have the settings endpoint",
                    base_url
                );
                println!("  • Quotes will use whatever mint URL the server defaults to");
                println!("  • You can still mint using your local wallet's mint configuration");
                println!("\nNote: The official npubx.cash server supports this feature.");
                println!("      Custom servers may not have it implemented.");
            } else {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

async fn show_keys(multi_mint_wallet: &MultiMintWallet, mint_url: &str) -> Result<()> {
    let wallet = get_wallet_for_mint(multi_mint_wallet, mint_url).await?;

    let keys = wallet.get_npubcash_keys()?;
    let npub = keys.public_key().to_bech32()?;
    let nsec = keys.secret_key().to_bech32()?;

    println!(
        r#"
╔═══════════════════════════════════════════════════════════════════════════╗
║                         NpubCash Nostr Keys                               ║
╠═══════════════════════════════════════════════════════════════════════════╣
║                                                                           ║
║  These keys are automatically derived from your wallet seed and are      ║
║  used for authenticating with the NpubCash service.                      ║
║                                                                           ║
╠═══════════════════════════════════════════════════════════════════════════╣
║                                                                           ║
║  Public Key (npub):                                                       ║
║  {}  ║
║                                                                           ║
║  NpubCash Address:                                                        ║
║  {}@npubx.cash         ║
║                                                                           ║
╠═══════════════════════════════════════════════════════════════════════════╣
║                                                                           ║
║  Secret Key (nsec):                                                       ║
║  {}  ║
║                                                                           ║
║  ⚠️  KEEP THIS SECRET! Anyone with this key can access your npubcash     ║
║      account and authenticate as you.                                    ║
║                                                                           ║
╚═══════════════════════════════════════════════════════════════════════════╝
"#,
        npub, npub, nsec
    );

    Ok(())
}
