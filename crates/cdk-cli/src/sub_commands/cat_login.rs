use std::path::Path;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::MintInfo;
use cdk::wallet::WalletRepository;
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
}

pub async fn cat_login(
    wallet_repository: &WalletRepository,
    sub_command_args: &CatLoginSubCommand,
    work_dir: &Path,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    // Ensure the mint exists
    if !wallet_repository.has_mint(&mint_url).await {
        wallet_repository.add_wallet(mint_url.clone()).await?;
    }

    let mint_info = wallet_repository.fetch_mint_info(&mint_url).await?;

    let (access_token, refresh_token) = get_access_token(
        wallet_repository,
        &mint_url,
        &mint_info,
        &sub_command_args.username,
        &sub_command_args.password,
    )
    .await?;

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
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    mint_info: &MintInfo,
    user: &str,
    password: &str,
) -> Result<(String, String)> {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .ok_or_else(|| anyhow::anyhow!("NUT-21 OIDC settings are not defined"))?
        .openid_discovery;

    let client_id = mint_info
        .nuts
        .nut21
        .clone()
        .ok_or_else(|| anyhow::anyhow!("NUT-21 OIDC settings are not defined"))?
        .client_id;

    let oidc_client = wallet_repository
        .oidc_client_for_mint(mint_url, openid_discovery, None)
        .await;

    // Get the token endpoint from the OIDC configuration
    let token_url = oidc_client.get_oidc_config().await?.token_endpoint;

    // Create the request parameters
    let params = vec![
        ("grant_type".to_string(), "password".to_string()),
        ("client_id".to_string(), client_id),
        ("scope".to_string(), "openid offline_access".to_string()),
        ("username".to_string(), user.to_string()),
        ("password".to_string(), password.to_string()),
    ];

    // Make the token request directly
    let token_response: serde_json::Value = oidc_client.post_form(&token_url, params).await?;

    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access token in response"))?
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No refresh token in response"))?
        .to_string();

    Ok((access_token, refresh_token))
}
