use anyhow::{anyhow, Result};
use cdk::nuts::CurrencyUnit;
use cdk::url::UncheckedUrl;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: UncheckedUrl,
}

pub async fn restore(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &RestoreSubCommand,
) -> Result<()> {
    let wallet = multi_mint_wallet
        .get_wallet(&WalletKey::new(
            sub_command_args.mint_url.clone(),
            CurrencyUnit::Sat,
        ))
        .await
        .ok_or(anyhow!("Unknown mint url"))?;

    let amount = wallet.restore().await?;

    println!("Restored {}", amount);

    Ok(())
}
