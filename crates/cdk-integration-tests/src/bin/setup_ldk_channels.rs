//! Helper binary to setup LDK channels in interactive regtest mode
//!
//! This binary is used after the LDK mint starts in interactive mode to:
//! 1. Get LDK node connection info from the mint API
//! 2. Fund CLN and LND nodes for channel operations
//! 3. Open channels FROM CLN/LND TO the LDK node

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use cdk_integration_tests::ln_regtest::bitcoin_client::BitcoinClient;
use cdk_integration_tests::ln_regtest::ln_client::{ClnClient, LightningClient, LndClient};
use clap::Parser;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "setup-ldk-channels")]
#[command(about = "Setup LDK channels in interactive regtest mode", long_about = None)]
struct Args {
    /// Working directory path
    work_dir: String,

    /// LDK mint port
    #[arg(long, default_value = "8089")]
    ldk_port: u16,
}

#[derive(Deserialize)]
struct MintInfo {
    pub_key: Option<String>,
    version: Option<MintVersion>,
}

#[derive(Deserialize)]
struct MintVersion {
    address: Option<String>,
}

async fn get_ldk_node_info(port: u16) -> Result<(String, String, u16)> {
    tracing::info!("Getting LDK node info from mint API...");

    let url = format!("http://127.0.0.1:{}/v1/info", port);
    let response = reqwest::get(&url).await?;
    let mint_info: MintInfo = response.json().await?;

    let pubkey = mint_info
        .pub_key
        .ok_or_else(|| anyhow::anyhow!("Mint API did not return pub_key"))?;

    let address_port = mint_info
        .version
        .and_then(|v| v.address)
        .ok_or_else(|| anyhow::anyhow!("Mint API did not return LN address"))?;

    // Parse address:port
    let parts: Vec<&str> = address_port.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid address format: {}", address_port);
    }

    let address = parts[0].to_string();
    let port: u16 = parts[1].parse()?;

    tracing::info!("LDK node: pubkey={}, address={}:{}", pubkey, address, port);

    Ok((pubkey, address, port))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let work_dir = PathBuf::from_str(&args.work_dir)?;

    println!("🔧 Setting up LDK channels for interactive regtest mode...");

    // Get LDK node connection info from mint API
    let (ldk_pubkey, ldk_address, ldk_port) = get_ldk_node_info(args.ldk_port).await?;

    // Initialize bitcoin client
    let bitcoin_client = BitcoinClient::new(
        "wallet".to_string(),
        "127.0.0.1:18443".into(),
        None,
        Some("testuser".to_string()),
        Some("testpass".to_string()),
    )?;

    // Initialize CLN client
    let cln_one_dir = work_dir.join("cln").join("one");
    let cln_one = ClnClient::new(cln_one_dir, None).await?;

    // Initialize LND client
    let lnd_one_dir = work_dir.join("lnd").join("one");
    let lnd_one = LndClient::new(
        "https://localhost:10009".to_string(),
        lnd_one_dir.join("tls.cert"),
        lnd_one_dir.join("data/chain/bitcoin/regtest/admin.macaroon"),
    )
    .await?;

    // Fund CLN and LND nodes for opening channels to LDK
    println!("💰 Funding CLN and LND nodes for channel operations...");

    let cln_one_addr = cln_one.get_new_onchain_address().await?;
    bitcoin_client.send_to_address(&cln_one_addr, 5_000_000)?;

    let lnd_one_addr = lnd_one.get_new_onchain_address().await?;
    bitcoin_client.send_to_address(&lnd_one_addr, 5_000_000)?;

    // Mine blocks
    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 6)?;

    // Wait for nodes to sync
    println!("⏳ Waiting for nodes to sync...");
    tokio::time::sleep(Duration::from_secs(3)).await;
    cln_one.wait_chain_sync().await?;
    lnd_one.wait_chain_sync().await?;

    // Wait for CLN to see the funds in its wallet
    cln_one.wait_for_funds(4_000_000).await?;

    println!("📡 Opening channels to LDK node...");

    // Open channel from CLN to LDK
    cln_one
        .connect_peer(ldk_pubkey.clone(), ldk_address.clone(), ldk_port)
        .await?;
    cln_one
        .open_channel(1_500_000, &ldk_pubkey, Some(750_000))
        .await?;
    tracing::info!("Created funding tx: CLN one -> LDK");

    // Open channel from LND to LDK
    lnd_one
        .connect_peer(ldk_pubkey.clone(), ldk_address.clone(), ldk_port)
        .await?;
    lnd_one
        .open_channel(1_500_000, &ldk_pubkey, Some(750_000))
        .await?;
    tracing::info!("Created funding tx: LND one -> LDK");

    // Mine blocks to confirm channels
    println!("⛏️  Mining blocks to confirm channels...");
    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 6)?;

    // Wait for nodes to sync
    println!("⏳ Waiting for channels to become active...");
    tokio::time::sleep(Duration::from_secs(3)).await;
    cln_one.wait_chain_sync().await?;
    lnd_one.wait_chain_sync().await?;

    // Wait for channels to become active
    cln_one.wait_channels_active().await?;
    lnd_one.wait_channels_active().await?;

    // Give extra time for channel state to stabilize
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("✅ LDK channels setup complete!");
    println!("   • CLN one -> LDK: 1.5M sats (750k local balance)");
    println!("   • LND one -> LDK: 1.5M sats (750k local balance)");

    Ok(())
}
