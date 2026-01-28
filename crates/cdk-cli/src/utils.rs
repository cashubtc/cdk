use std::io::{self, Write};
use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;

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

/// Helper function to create or get a wallet
pub async fn get_or_create_wallet(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    unit: &CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    wallet_repository
        .get_or_create_wallet(mint_url, unit.clone())
        .await
        .map_err(Into::into)
}
