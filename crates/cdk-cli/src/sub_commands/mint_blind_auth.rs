use std::path::Path;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MintInfo};
use cdk::wallet::WalletRepository;
use cdk::{Amount, Wallet};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::token_storage;
use crate::utils::get_or_create_wallet;

#[derive(Args, Serialize, Deserialize)]
pub struct MintBlindAuthSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: Option<u64>,
    /// Cat (access token)
    #[arg(long)]
    cat: Option<String>,
}

pub async fn mint_blind_auth(
    wallet_repository: &WalletRepository,
    sub_command_args: &MintBlindAuthSubCommand,
    work_dir: &Path,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    // Ensure the mint exists
    if !wallet_repository.has_mint(&mint_url).await {
        wallet_repository.add_wallet(mint_url.clone()).await?;
    }

    wallet_repository.fetch_mint_info(&mint_url).await?;

    // Get a wallet for this mint
    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    // Try to get the token from the provided argument or from the stored file
    let cat = match &sub_command_args.cat {
        Some(token) => token.clone(),
        None => {
            // Try to load from file
            match token_storage::get_token_for_mint(work_dir, &mint_url).await {
                Ok(Some(token_data)) => {
                    println!("Using access token from cashu_tokens.json");
                    token_data.access_token
                }
                Ok(None) => {
                    return Err(anyhow::anyhow!(
                        "No access token provided and no token found in cashu_tokens.json for this mint"
                    ));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to read token from cashu_tokens.json: {}",
                        e
                    ));
                }
            }
        }
    };

    // Try to set the access token
    if let Err(err) = wallet.set_cat(cat.clone()).await {
        tracing::error!("Could not set cat: {}", err);

        // Try to refresh the token if we have a refresh token
        if let Ok(Some(token_data)) = token_storage::get_token_for_mint(work_dir, &mint_url).await {
            println!("Attempting to refresh the access token...");

            // Get the mint info to access OIDC configuration
            let mint_info = wallet_repository.fetch_mint_info(&mint_url).await?;
            match refresh_access_token(&wallet, &mint_info, &token_data.refresh_token).await {
                Ok((new_access_token, new_refresh_token)) => {
                    println!("Successfully refreshed access token");

                    // Save the new tokens
                    if let Err(e) = token_storage::save_tokens(
                        work_dir,
                        &mint_url,
                        &new_access_token,
                        &new_refresh_token,
                    )
                    .await
                    {
                        println!("Warning: Failed to save refreshed tokens: {e}");
                    }

                    // Try setting the new access token
                    if let Err(err) = wallet.set_cat(new_access_token).await {
                        tracing::error!("Could not set refreshed cat: {}", err);
                        return Err(anyhow::anyhow!(
                            "Authentication failed even after token refresh"
                        ));
                    }

                    // Set the refresh token
                    wallet.set_refresh_token(new_refresh_token).await?;
                }
                Err(e) => {
                    tracing::error!("Failed to refresh token: {}", e);
                    return Err(anyhow::anyhow!("Failed to refresh access token: {}", e));
                }
            }
        } else {
            return Err(anyhow::anyhow!(
                "Authentication failed and no refresh token available"
            ));
        }
    } else {
        // If we have a refresh token, set it
        if let Ok(Some(token_data)) = token_storage::get_token_for_mint(work_dir, &mint_url).await {
            tracing::info!("Attempting to use refresh access token to refresh auth token");
            wallet.set_refresh_token(token_data.refresh_token).await?;
            wallet.refresh_access_token().await?;
        }
    }

    println!("Attempting to mint blind auth");

    let amount = match sub_command_args.amount {
        Some(amount) => amount,
        None => {
            let mint_info = wallet_repository.fetch_mint_info(&mint_url).await?;
            mint_info
                .bat_max_mint()
                .ok_or(anyhow!("Unknown max bat mint"))?
        }
    };

    let proofs = wallet.mint_blind_auth(Amount::from(amount)).await?;

    println!("Received {} auth proofs for mint {mint_url}", proofs.len());

    Ok(())
}

async fn refresh_access_token(
    wallet: &Wallet,
    mint_info: &MintInfo,
    refresh_token: &str,
) -> Result<(String, String)> {
    let openid_discovery = mint_info
        .openid_discovery()
        .ok_or_else(|| anyhow::anyhow!("OIDC discovery information not available"))?;
    let client_id = mint_info
        .client_id()
        .ok_or_else(|| anyhow::anyhow!("OIDC client ID is not available"))?;
    let oidc_client = wallet.oidc_client(openid_discovery, None);
    let token_response = oidc_client
        .refresh_access_token(client_id, refresh_token.to_string())
        .await?;

    let access_token = token_response.access_token;

    // Get the new refresh token or use the old one if not provided
    let new_refresh_token = token_response
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());

    Ok((access_token, new_refresh_token))
}
