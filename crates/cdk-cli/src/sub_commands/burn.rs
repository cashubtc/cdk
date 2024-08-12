use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct BurnSubCommand {
    /// Mint Url
    mint_url: Option<MintUrl>,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn burn(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &BurnSubCommand,
) -> Result<()> {
    let mut total_burnt = Amount::ZERO;
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    match &sub_command_args.mint_url {
        Some(mint_url) => {
            let wallet = multi_mint_wallet
                .get_wallet(&WalletKey::new(mint_url.clone(), unit))
                .await
                .unwrap();
            total_burnt = wallet.check_all_pending_proofs().await?;
        }
        None => {
            for wallet in multi_mint_wallet.get_wallets().await {
                let amount_burnt = wallet.check_all_pending_proofs().await?;
                total_burnt += amount_burnt;
            }
        }
    }

    println!("{total_burnt} burned");
    Ok(())
}
