use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{Wallet, WalletRepository};
use cdk::StreamExt;
use clap::Subcommand;
use nostr_sdk::ToBech32;

/// Helper function to get wallet for a specific mint URL
async fn get_wallet_for_mint(
    wallet_repository: &WalletRepository,
    mint_url_str: &str,
) -> Result<Arc<Wallet>> {
    let mint_url = MintUrl::from_str(mint_url_str)?;

    // Check if wallet exists for this mint
    if !wallet_repository.has_mint(&mint_url).await {
        // Add the mint to the wallet
        wallet_repository.add_wallet(mint_url.clone()).await?;
    }

    match wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
    {
        Ok(wallet) => Ok(Arc::new(wallet)),
        Err(_) => Ok(Arc::new(
            wallet_repository
                .create_wallet(mint_url, CurrencyUnit::Sat, None)
                .await?,
        )),
    }
}

/// Helper function to get or create a Sat wallet without probing the mint.
async fn get_wallet_for_mint_without_probe(
    wallet_repository: &WalletRepository,
    mint_url: MintUrl,
) -> Result<Arc<Wallet>> {
    match wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
    {
        Ok(wallet) => Ok(Arc::new(wallet)),
        Err(_) => Ok(Arc::new(
            wallet_repository
                .create_wallet(mint_url, CurrencyUnit::Sat, None)
                .await?,
        )),
    }
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
    wallet_repository: &WalletRepository,
    mint_url: &str,
    sub_command: &NpubCashSubCommand,
    npubcash_url: Option<String>,
) -> Result<()> {
    // Get default npubcash URL if not provided
    let base_url = npubcash_url.unwrap_or_else(|| "https://npubx.cash".to_string());

    match sub_command {
        NpubCashSubCommand::Sync => sync(wallet_repository, mint_url, &base_url).await,
        NpubCashSubCommand::List { since, format } => {
            list(wallet_repository, mint_url, &base_url, *since, format).await
        }
        NpubCashSubCommand::Subscribe => subscribe(wallet_repository, mint_url, &base_url).await,
        NpubCashSubCommand::SetMint { url } => {
            set_mint(wallet_repository, mint_url, &base_url, url).await
        }
        NpubCashSubCommand::ShowKeys => show_keys(wallet_repository, mint_url).await,
    }
}

/// Helper function to ensure active mint consistency
async fn ensure_active_mint(wallet_repository: &WalletRepository, mint_url: &str) -> Result<()> {
    let mint_url_struct = MintUrl::from_str(mint_url)?;

    match wallet_repository.get_active_npubcash_mint().await? {
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
            wallet_repository
                .set_active_npubcash_mint(mint_url_struct)
                .await?;
            println!("✓ Set {} as active NpubCash mint", mint_url);
        }
    }
    Ok(())
}

async fn sync(wallet_repository: &WalletRepository, mint_url: &str, base_url: &str) -> Result<()> {
    ensure_active_mint(wallet_repository, mint_url).await?;

    println!("Syncing quotes from NpubCash...");

    let wallet = get_wallet_for_mint(wallet_repository, mint_url).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    let quotes = wallet.sync_npubcash_quotes().await?;

    println!("✓ Synced {} quotes successfully", quotes.len());
    Ok(())
}

async fn list(
    wallet_repository: &WalletRepository,
    mint_url: &str,
    base_url: &str,
    since: Option<u64>,
    format: &str,
) -> Result<()> {
    ensure_active_mint(wallet_repository, mint_url).await?;

    let wallet = get_wallet_for_mint(wallet_repository, mint_url).await?;

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
    wallet_repository: &WalletRepository,
    mint_url: &str,
    base_url: &str,
) -> Result<()> {
    ensure_active_mint(wallet_repository, mint_url).await?;

    println!("=== NpubCash Quote Subscription ===\n");

    let wallet = get_wallet_for_mint(wallet_repository, mint_url).await?;

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
    wallet_repository: &WalletRepository,
    _mint_url: &str,
    base_url: &str,
    url: &str,
) -> Result<()> {
    println!("Setting NpubCash mint URL to: {}", url);

    let mint_url_struct = MintUrl::from_str(url)?;
    let wallet =
        get_wallet_for_mint_without_probe(wallet_repository, mint_url_struct.clone()).await?;

    // Enable NpubCash if not already enabled
    wallet.enable_npubcash(base_url.to_string()).await?;

    // Try to set the mint URL on the NpubCash server
    match wallet.set_npubcash_mint_url(url).await {
        Ok(_) => {
            wallet_repository
                .set_active_npubcash_mint(mint_url_struct)
                .await?;
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
                println!("  • Your local active NpubCash mint was not changed");
                println!("\nNote: The official npubx.cash server supports this feature.");
                println!("      Custom servers may not have it implemented.");
            } else {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

async fn show_keys(wallet_repository: &WalletRepository, mint_url: &str) -> Result<()> {
    let wallet = get_wallet_for_mint(wallet_repository, mint_url).await?;

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk::mint_url::MintUrl;
    use cdk::wallet::WalletRepositoryBuilder;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::set_mint;

    #[tokio::test]
    async fn set_mint_persists_requested_url_after_server_update() {
        let localstore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("memory db"),
        );
        let wallet_repository = WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository builds");

        let global_mint = "https://global-mint.invalid";
        let requested_mint = "https://requested-mint.invalid";
        let response_body = r#"{"error":false,"data":{"user":{"pubkey":"test","mintUrl":"https://requested-mint.invalid","lockQuote":false}}}"#;
        let (npubcash_url, server) =
            start_npubcash_settings_server("HTTP/1.1 200 OK", response_body, 2).await;

        tokio::time::timeout(
            Duration::from_secs(10),
            set_mint(
                &wallet_repository,
                global_mint,
                &npubcash_url,
                requested_mint,
            ),
        )
        .await
        .expect("set-mint does not hang on the requested mint")
        .expect("set-mint succeeds");

        let active = wallet_repository
            .get_active_npubcash_mint()
            .await
            .expect("active npubcash mint is readable");

        assert_eq!(
            active,
            Some(MintUrl::from_str(requested_mint).expect("valid mint URL"))
        );

        let requests = server.await.expect("server task completes");
        assert_eq!(requests.len(), 2);
        assert!(requests
            .iter()
            .all(|request| request.starts_with("PATCH /api/v2/user/mint HTTP/1.1")));
        assert!(requests
            .iter()
            .all(|request| request.contains(requested_mint)));
    }

    #[tokio::test]
    async fn set_mint_keeps_local_active_mint_when_server_update_fails() {
        let localstore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("memory db"),
        );
        let wallet_repository = WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository builds");

        let previous_mint =
            MintUrl::from_str("https://previous-mint.invalid").expect("previous mint URL is valid");
        wallet_repository
            .set_active_npubcash_mint(previous_mint.clone())
            .await
            .expect("active mint can be set");

        let response_body = r#"{"error":true,"message":"temporary failure"}"#;
        let (npubcash_url, server) =
            start_npubcash_settings_server("HTTP/1.1 500 Internal Server Error", response_body, 2)
                .await;

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            set_mint(
                &wallet_repository,
                "https://global-mint.invalid",
                &npubcash_url,
                "https://requested-mint.invalid",
            ),
        )
        .await
        .expect("set-mint does not hang on the requested mint");

        assert!(result.is_err());

        let active = wallet_repository
            .get_active_npubcash_mint()
            .await
            .expect("active npubcash mint is readable");

        assert_eq!(active, Some(previous_mint));

        let requests = server.await.expect("server task completes");
        assert_eq!(requests.len(), 2);
    }

    async fn start_npubcash_settings_server(
        status_line: &'static str,
        response_body: &'static str,
        request_count: usize,
    ) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("test server has local addr");
        let base_url = format!("http://{}", addr);

        let server = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(request_count);

            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().await.expect("connection accepted");
                let request = read_http_request(&mut stream).await;
                requests.push(request);

                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response is written");
            }

            requests
        });

        (base_url, server)
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];

        loop {
            let read = stream.read(&mut chunk).await.expect("request is readable");
            if read == 0 {
                break;
            }

            buffer.extend_from_slice(&chunk[..read]);

            if let Some(header_end) = find_header_end(&buffer) {
                let content_length = parse_content_length(&buffer[..header_end]);
                let request_len = header_end + 4 + content_length;
                if buffer.len() >= request_len {
                    break;
                }
            }
        }

        String::from_utf8_lossy(&buffer).to_string()
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }

    fn parse_content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }
}
