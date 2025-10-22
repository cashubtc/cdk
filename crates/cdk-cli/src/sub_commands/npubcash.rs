use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::MintQuoteState;
use cdk::wallet::MultiMintWallet;
use clap::Subcommand;
use nostr_sdk::ToBech32;

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
    Subscribe {
        /// Automatically mint paid quotes (default: true)
        #[arg(long, default_value = "true")]
        auto_mint: bool,
    },
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
    sub_command: &NpubCashSubCommand,
    npubcash_url: Option<String>,
) -> Result<()> {
    // Get default npubcash URL if not provided
    let base_url = npubcash_url.unwrap_or_else(|| "https://npubx.cash".to_string());

    match sub_command {
        NpubCashSubCommand::Sync => sync(multi_mint_wallet, &base_url).await,
        NpubCashSubCommand::List { since, format } => {
            list(multi_mint_wallet, &base_url, *since, format).await
        }
        NpubCashSubCommand::Subscribe { auto_mint } => {
            subscribe(multi_mint_wallet, &base_url, *auto_mint).await
        }
        NpubCashSubCommand::SetMint { url } => set_mint(multi_mint_wallet, &base_url, url).await,
        NpubCashSubCommand::ShowKeys => show_keys(multi_mint_wallet).await,
    }
}

async fn sync(multi_mint_wallet: &MultiMintWallet, base_url: &str) -> Result<()> {
    println!("Syncing quotes from NpubCash...");

    let wallets = multi_mint_wallet.get_wallets().await;
    let wallet = wallets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No wallet available. Please add a mint first."))?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    let quotes = wallet.sync_npubcash_quotes().await?;

    println!("✓ Synced {} quotes successfully", quotes.len());
    Ok(())
}

async fn list(
    multi_mint_wallet: &MultiMintWallet,
    base_url: &str,
    since: Option<u64>,
    format: &str,
) -> Result<()> {
    let wallets = multi_mint_wallet.get_wallets().await;
    let wallet = wallets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No wallet available. Please add a mint first."))?;

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
    base_url: &str,
    auto_mint: bool,
) -> Result<()> {
    println!("=== NpubCash Quote Subscription ===\n");

    let wallets = multi_mint_wallet.get_wallets().await;
    let wallet = wallets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No wallet available. Please add a mint first."))?;

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

    if auto_mint {
        println!("Auto-mint is ENABLED - paid quotes will be automatically minted\n");
    } else {
        println!("Auto-mint is DISABLED - quotes will only be displayed\n");
    }

    println!("Starting quote polling...");
    println!("Press Ctrl+C to stop.\n");

    // Clone wallet for use in callback
    let wallet_clone = wallet.clone();

    let _handle = wallet
        .subscribe_npubcash_updates(move |quotes| {
            let wallet = wallet_clone.clone();

            println!("Received {} new quote(s)", quotes.len());

            for quote in quotes {
                let amount_str = quote
                    .amount
                    .map_or("unknown".to_string(), |a| a.to_string());
                println!("  ├─ Quote ID: {}", quote.id);
                println!("  ├─ Amount: {} {}", amount_str, quote.unit);
                println!("  ├─ State: {:?}", quote.state);

                if auto_mint && matches!(quote.state, MintQuoteState::Paid) {
                    println!("  └─ Auto-minting...");

                    let wallet_mint = wallet.clone();
                    let quote_id = quote.id.clone();

                    tokio::spawn(async move {
                        match wallet_mint
                            .mint(&quote_id, SplitTarget::default(), None)
                            .await
                        {
                            Ok(proofs) => match proofs.total_amount() {
                                Ok(amount) => {
                                    println!("     Successfully minted {} sats!", amount);

                                    if let Ok(balance) = wallet_mint.total_balance().await {
                                        println!("     Wallet balance: {} sats", balance);
                                    }
                                }
                                Err(e) => {
                                    println!("     Failed to calculate amount: {}", e);
                                }
                            },
                            Err(e) => {
                                println!("     Failed to mint: {}", e);
                            }
                        }
                    });
                } else {
                    println!("  └─ Added to database");
                }
            }
            println!();
        })
        .await?;

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("\nStopping quote polling...");

    // Show final wallet balance
    let balance = wallet.total_balance().await?;
    println!("Final wallet balance: {} sats\n", balance);

    Ok(())
}

async fn set_mint(multi_mint_wallet: &MultiMintWallet, base_url: &str, url: &str) -> Result<()> {
    println!("Setting NpubCash mint URL to: {}", url);

    let wallets = multi_mint_wallet.get_wallets().await;
    let wallet = wallets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No wallet available. Please add a mint first."))?;

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

            if error_msg.contains("404") || error_msg.contains("API error") {
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

async fn show_keys(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    let wallets = multi_mint_wallet.get_wallets().await;
    let wallet = wallets
        .first()
        .ok_or_else(|| anyhow::anyhow!("No wallet available. Please add a mint first."))?;

    let keys = wallet.get_npubcash_keys()?;

    println!("\n╔═══════════════════════════════════════════════════════════════════════════╗");
    println!("║                         NpubCash Nostr Keys                               ║");
    println!("╠═══════════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                           ║");
    println!("║  These keys are automatically derived from your wallet seed and are      ║");
    println!("║  used for authenticating with the NpubCash service.                      ║");
    println!("║                                                                           ║");
    println!("╠═══════════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                           ║");
    println!("║  Public Key (npub):                                                       ║");
    println!("║  {}  ║", keys.public_key().to_bech32()?);
    println!("║                                                                           ║");
    println!("║  NpubCash Address:                                                        ║");
    println!("║  {}@npubx.cash         ║", keys.public_key().to_bech32()?);
    println!("║                                                                           ║");
    println!("╠═══════════════════════════════════════════════════════════════════════════╣");
    println!("║                                                                           ║");
    println!("║  Secret Key (nsec):                                                       ║");
    println!("║  {}  ║", keys.secret_key().to_bech32()?);
    println!("║                                                                           ║");
    println!("║  ⚠️  KEEP THIS SECRET! Anyone with this key can access your npubcash     ║");
    println!("║      account and authenticate as you.                                    ║");
    println!("║                                                                           ║");
    println!("╚═══════════════════════════════════════════════════════════════════════════╝");
    println!();

    Ok(())
}
