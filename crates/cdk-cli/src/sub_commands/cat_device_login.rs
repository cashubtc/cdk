use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::MintInfo;
use cdk::wallet::WalletRepository;
use clap::Args;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::token_storage;

#[derive(Args, Serialize, Deserialize)]
pub struct CatDeviceLoginSubCommand {
    /// Mint url
    mint_url: MintUrl,
}

pub async fn cat_device_login(
    wallet_repository: &WalletRepository,
    sub_command_args: &CatDeviceLoginSubCommand,
    work_dir: &Path,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    // Ensure the mint exists
    if !wallet_repository.has_mint(&mint_url).await {
        wallet_repository.add_wallet(mint_url.clone()).await?;
    }

    let mint_info = wallet_repository.fetch_mint_info(&mint_url).await?;

    let (access_token, refresh_token) =
        get_device_code_token(wallet_repository, &mint_url, &mint_info).await?;

    // Save tokens to file in work directory
    if let Err(e) =
        token_storage::save_tokens(work_dir, &mint_url, &access_token, &refresh_token).await
    {
        println!("Warning: Failed to save tokens to file: {e}");
    } else {
        println!("Tokens saved to work directory");
    }

    // Print a cute ASCII cat
    println!("\nAuthentication successful! 🎉\n");
    println!("\nYour tokens:");
    println!("access_token: {access_token}");
    println!("refresh_token: {refresh_token}");

    Ok(())
}

async fn get_device_code_token(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    mint_info: &MintInfo,
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

    // Get the OIDC configuration
    let oidc_config = oidc_client.get_oidc_config().await?;

    // Get the device authorization endpoint
    let device_auth_url = oidc_config.device_authorization_endpoint;

    // Make the device code request
    let device_code_data: serde_json::Value = oidc_client
        .post_form(
            &device_auth_url,
            vec![
                ("client_id".to_string(), client_id.clone()),
                ("scope".to_string(), "openid offline_access".to_string()),
            ],
        )
        .await?;

    let device_code = device_code_data["device_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No device code in response"))?;

    let user_code = device_code_data["user_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No user code in response"))?;

    let verification_uri = device_code_data["verification_uri"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No verification URI in response"))?;

    let verification_uri_complete = device_code_data["verification_uri_complete"]
        .as_str()
        .unwrap_or(verification_uri);

    let interval = device_code_data["interval"].as_u64().unwrap_or(5);

    println!("\nTo login, visit: {verification_uri}");
    println!("And enter code: {user_code}\n");

    if verification_uri_complete != verification_uri {
        println!("Or visit this URL directly: {verification_uri_complete}\n");
    }

    // Poll for the token
    let token_url = oidc_config.token_endpoint;

    loop {
        sleep(Duration::from_secs(interval)).await;

        let token_response = oidc_client
            .post_form_response(
                &token_url,
                vec![
                    (
                        "grant_type".to_string(),
                        "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                    ),
                    ("device_code".to_string(), device_code.to_string()),
                    ("client_id".to_string(), client_id.clone()),
                ],
            )
            .await?;

        if token_response.is_success() {
            let token_data: serde_json::Value = token_response.json()?;

            let access_token = token_data["access_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No access token in response"))?
                .to_string();

            let refresh_token = token_data["refresh_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No refresh token in response"))?
                .to_string();

            return Ok((access_token, refresh_token));
        } else {
            let status = token_response.status();
            let error_data: serde_json::Value = token_response.json()?;

            let error = error_data["error"].as_str().unwrap_or("unknown_error");

            // If the user hasn't completed the flow yet, continue polling
            if error == "authorization_pending" || error == "slow_down" {
                if error == "slow_down" {
                    // If we're polling too fast, slow down
                    sleep(Duration::from_secs(interval + 5)).await;
                }
                println!("Waiting for user to complete authentication...");
                continue;
            } else {
                // For other errors, exit with an error message
                return Err(anyhow::anyhow!(
                    "Authentication failed with status {}: {}",
                    status,
                    error
                ));
            }
        }
    }
}
