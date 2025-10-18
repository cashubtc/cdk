use std::io::{self, Write};
use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;

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
    multi_mint_wallet: &MultiMintWallet,
    mint_url: &MintUrl,
) -> Result<cdk::wallet::Wallet> {
    match multi_mint_wallet.get_wallet(mint_url).await {
        Some(wallet) => Ok(wallet.clone()),
        None => {
            tracing::debug!("Wallet does not exist creating..");
            multi_mint_wallet.add_mint(mint_url.clone()).await?;
            Ok(multi_mint_wallet
                .get_wallet(mint_url)
                .await
                .expect("Wallet should exist after adding mint")
                .clone())
        }
    }
}
