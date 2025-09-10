//! Example demonstrating how to use the OHTTP mint connector
//! This provides privacy by routing requests through an OHTTP gateway or relay

use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::{MintConnector, OhttpClient};
use tracing::{error, info};
use tracing_subscriber::fmt::init as tracing_init;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init();

    info!("Starting OHTTP mint connector example");

    // Example mint URL (replace with your actual mint)
    let mint_url = MintUrl::from_str("https://mint.coinos.io")?;

    // Example OHTTP gateway URL (replace with your actual gateway)
    let gateway_url = Url::parse("https://gateway.example.com")?;

    // Create OHTTP client with gateway
    #[cfg(feature = "auth")]
    let ohttp_client = OhttpClient::new_with_gateway(mint_url.clone(), gateway_url, None);
    #[cfg(not(feature = "auth"))]
    let ohttp_client = OhttpClient::new_with_gateway(mint_url.clone(), gateway_url);

    info!("Created OHTTP client, testing connection...");

    // Try to get mint info through OHTTP
    match ohttp_client.get_mint_info().await {
        Ok(mint_info) => {
            info!("Successfully retrieved mint info through OHTTP:");
            info!("  Name: {}", mint_info.name.unwrap_or_default());
            info!("  Version: {:?}", mint_info.version);
            info!(
                "  Description: {}",
                mint_info.description.unwrap_or_default()
            );
            info!("  Supported NIPs: {:?}", mint_info.nuts);
        }
        Err(e) => {
            error!("Failed to get mint info: {}", e);
            return Err(e.into());
        }
    }

    // Try to get mint keys through OHTTP
    match ohttp_client.get_mint_keys().await {
        Ok(keysets) => {
            info!(
                "Successfully retrieved {} keysets through OHTTP",
                keysets.len()
            );
            for (i, keyset) in keysets.iter().enumerate() {
                info!("  Keyset {}: {}", i + 1, keyset.id);
            }
        }
        Err(e) => {
            error!("Failed to get mint keys: {}", e);
            return Err(e.into());
        }
    }

    info!("OHTTP mint connector example completed successfully");
    Ok(())
}
