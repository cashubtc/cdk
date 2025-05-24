use std::io::{self, Write};
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::Amount;

/// Helper function to get user input with a prompt
pub fn get_user_input(prompt: &str) -> Result<String> {
    println!("{prompt}");
    let mut user_input = String::new();
    io::stdout().flush()?;
    io::stdin().read_line(&mut user_input)?;
    Ok(user_input.trim().to_string())
}

/// Helper function to get a number from user input with a prompt
pub fn get_number_input<T>(prompt: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let input = get_user_input(prompt)?;
    let number = input.parse::<T>()?;
    Ok(number)
}

/// Helper function to validate a mint number against available mints
pub fn validate_mint_number(mint_number: usize, mint_count: usize) -> Result<()> {
    if mint_number >= mint_count {
        bail!("Invalid mint number");
    }
    Ok(())
}

/// Helper function to check if there are enough funds for an operation
pub fn check_sufficient_funds(available: Amount, required: Amount) -> Result<()> {
    if required.gt(&available) {
        bail!("Not enough funds");
    }
    Ok(())
}

/// Helper function to get a wallet from the multi-mint wallet by mint URL
pub async fn get_wallet_by_mint_url(
    multi_mint_wallet: &MultiMintWallet,
    mint_url_str: &str,
    unit: CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    let mint_url = MintUrl::from_str(mint_url_str)?;

    let wallet_key = WalletKey::new(mint_url.clone(), unit);
    let wallet = multi_mint_wallet
        .get_wallet(&wallet_key)
        .await
        .ok_or_else(|| anyhow::anyhow!("Wallet not found for mint URL: {}", mint_url_str))?;

    Ok(wallet.clone())
}

/// Helper function to get a wallet from the multi-mint wallet
pub async fn get_wallet_by_index(
    multi_mint_wallet: &MultiMintWallet,
    mint_amounts: &[(MintUrl, Amount)],
    mint_number: usize,
    unit: CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    validate_mint_number(mint_number, mint_amounts.len())?;

    let wallet_key = WalletKey::new(mint_amounts[mint_number].0.clone(), unit);
    let wallet = multi_mint_wallet
        .get_wallet(&wallet_key)
        .await
        .ok_or_else(|| anyhow::anyhow!("Wallet not found"))?;

    Ok(wallet.clone())
}

/// Helper function to create or get a wallet
pub async fn get_or_create_wallet(
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &MintUrl,
    unit: CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
    {
        Some(wallet) => Ok(wallet.clone()),
        None => {
            tracing::debug!("Wallet does not exist creating..");
            multi_mint_wallet
                .create_and_add_wallet(&mint_url.to_string(), unit, None)
                .await
        }
    }
}
