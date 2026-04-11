use std::io::{self, Write};
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;

/// Parse global `--unit`: trim, plural aliases (`sats`→`sat`, …), then [`CurrencyUnit::from_str`].
pub fn parse_cli_currency_unit(s: &str) -> Result<CurrencyUnit> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        bail!("Currency unit must not be empty (omit `--unit` to use the default `sat`)");
    }

    if trimmed.eq_ignore_ascii_case("satt") {
        bail!("Unknown currency unit '{trimmed}'. Did you mean 'sat'?");
    }

    let normalized: &str = match trimmed.to_ascii_lowercase().as_str() {
        "sats" => "sat",
        "msats" => "msat",
        "usds" => "usd",
        "eurs" => "eur",
        "auths" => "auth",
        _ => trimmed,
    };

    let unit = CurrencyUnit::from_str(normalized).map_err(|e| anyhow::anyhow!(e))?;
    if let CurrencyUnit::Custom(ref name) = unit {
        tracing::info!(
            "Using custom currency unit '{name}' (not one of sat, msat, usd, eur, auth)"
        );
    }
    Ok(unit)
}

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

/// Helper function to get an existing wallet or create one if it doesn't exist
pub async fn get_or_create_wallet(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    unit: &CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    match wallet_repository.get_wallet(mint_url, unit).await {
        Ok(wallet) => Ok(wallet),
        Err(_) => wallet_repository
            .create_wallet(mint_url.clone(), unit.clone(), None)
            .await
            .map_err(Into::into),
    }
}
