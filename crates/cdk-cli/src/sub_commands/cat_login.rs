use std::path::Path;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::MintInfo;
use cdk::wallet::MultiMintWallet;
use cdk::OidcClient;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::token_storage;

#[derive(Args, Serialize, Deserialize)]
pub struct CatLoginSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Username
    username: String,
    /// Password
    password: String,
    /// Client ID for OIDC authentication
    #[arg(default_value = "cashu-client")]
    #[arg(long)]
    client_id: String,
}

pub async fn cat_login(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &CatLoginSubCommand,
    work_dir: &Path,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    // Ensure the mint exists
    if !multi_mint_wallet.has_mint(&mint_url).await {
        multi_mint_wallet.add_mint(mint_url.clone()).await?;
    }

    let mint_info = multi_mint_wallet
        .fetch_mint_info(&mint_url)
        .await?
        .ok_or(anyhow!("Mint info not found"))?;

    let (access_token, refresh_token) = get_access_token(
        &mint_info,
        &sub_command_args.client_id,
        &sub_command_args.username,
        &sub_command_args.password,
    )
    .await;

    // Save tokens to file in work directory
    if let Err(e) =
        token_storage::save_tokens(work_dir, &mint_url, &access_token, &refresh_token).await
    {
        println!("Warning: Failed to save tokens to file: {e}");
    } else {
        println!("Tokens saved to work directory");
    }

    println!("\nAuthentication successful! 🎉\n");
    println!("\nYour tokens:");
    println!("access_token: {access_token}");
    println!("refresh_token: {refresh_token}");

    Ok(())
}

async fn get_access_token(
    mint_info: &MintInfo,
    client_id: &str,
    user: &str,
    password: &str,
) -> (String, String) {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .expect("Nut21 defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery, None);

    // Get the token endpoint from the OIDC configuration
    let token_url = oidc_client
        .get_oidc_config()
        .await
        .expect("Failed to get OIDC config")
        .token_endpoint;

    // Create the request parameters
    let params = [
        ("grant_type", "password"),
        ("client_id", client_id),
        ("username", user),
        ("password", password),
    ];

    // Make the token request directly
    let client = reqwest::Client::new();
    let response = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .expect("Failed to send token request");

    let token_response: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse token response");

    let access_token = token_response["access_token"]
        .as_str()
        .expect("No access token in response")
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .expect("No refresh token in response")
        .to_string();

    (access_token, refresh_token)
}
