use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MintInfo};
use cdk::wallet::types::WalletKey;
use cdk::wallet::MultiMintWallet;
use cdk::OidcClient;
use clap::Args;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::token_storage;

#[derive(Args, Serialize, Deserialize)]
pub struct CatDeviceLoginSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    #[arg(short, long)]
    unit: String,
    /// Client ID for OIDC authentication
    #[arg(default_value = "cashu-client")]
    #[arg(long)]
    client_id: String,
}

pub async fn cat_device_login(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &CatDeviceLoginSubCommand,
    work_dir: &Path,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            multi_mint_wallet
                .create_and_add_wallet(&mint_url.to_string(), unit, None)
                .await?
        }
    };

    let mint_info = wallet
        .get_mint_info()
        .await?
        .ok_or(anyhow!("Mint info not found"))?;

    let (access_token, refresh_token) =
        get_device_code_token(&mint_info, &sub_command_args.client_id).await;

    // Save tokens to file in work directory
    if let Err(e) =
        token_storage::save_tokens(work_dir, &mint_url, &access_token, &refresh_token).await
    {
        println!("Warning: Failed to save tokens to file: {}", e);
    } else {
        println!("Tokens saved to work directory");
    }

    // Print a cute ASCII cat
    println!("\nAuthentication successful! 🎉\n");
    println!("\nYour tokens:");
    println!("access_token: {}", access_token);
    println!("refresh_token: {}", refresh_token);

    Ok(())
}

async fn get_device_code_token(mint_info: &MintInfo, client_id: &str) -> (String, String) {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .expect("Nut21 defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery);

    // Get the OIDC configuration
    let oidc_config = oidc_client
        .get_oidc_config()
        .await
        .expect("Failed to get OIDC config");

    // Get the device authorization endpoint
    let device_auth_url = oidc_config.device_authorization_endpoint;

    // Make the device code request
    let client = reqwest::Client::new();
    let device_code_response = client
        .post(device_auth_url)
        .form(&[("client_id", client_id)])
        .send()
        .await
        .expect("Failed to send device code request");

    let device_code_data: serde_json::Value = device_code_response
        .json()
        .await
        .expect("Failed to parse device code response");

    let device_code = device_code_data["device_code"]
        .as_str()
        .expect("No device code in response");

    let user_code = device_code_data["user_code"]
        .as_str()
        .expect("No user code in response");

    let verification_uri = device_code_data["verification_uri"]
        .as_str()
        .expect("No verification URI in response");

    let verification_uri_complete = device_code_data["verification_uri_complete"]
        .as_str()
        .unwrap_or(verification_uri);

    let interval = device_code_data["interval"].as_u64().unwrap_or(5);

    println!("\nTo login, visit: {}", verification_uri);
    println!("And enter code: {}\n", user_code);

    if verification_uri_complete != verification_uri {
        println!(
            "Or visit this URL directly: {}\n",
            verification_uri_complete
        );
    }

    // Poll for the token
    let token_url = oidc_config.token_endpoint;

    loop {
        sleep(Duration::from_secs(interval)).await;

        let token_response = client
            .post(&token_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
                ("client_id", client_id),
            ])
            .send()
            .await
            .expect("Failed to send token request");

        if token_response.status().is_success() {
            let token_data: serde_json::Value = token_response
                .json()
                .await
                .expect("Failed to parse token response");

            let access_token = token_data["access_token"]
                .as_str()
                .expect("No access token in response")
                .to_string();

            let refresh_token = token_data["refresh_token"]
                .as_str()
                .expect("No refresh token in response")
                .to_string();

            return (access_token, refresh_token);
        } else {
            let error_data: serde_json::Value = token_response
                .json()
                .await
                .expect("Failed to parse error response");

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
                panic!("Authentication failed: {}", error);
            }
        }
    }
}
