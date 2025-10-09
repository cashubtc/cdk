use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
}

pub async fn restore(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &RestoreSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = match multi_mint_wallet.get_wallet(&mint_url).await {
        Some(wallet) => wallet.clone(),
        None => {
            multi_mint_wallet.add_mint(mint_url.clone()).await?;
            multi_mint_wallet
                .get_wallet(&mint_url)
                .await
                .expect("Wallet should exist after adding mint")
                .clone()
        }
    };

    let amount = wallet.restore().await?;

    println!("Restored {amount}");

    Ok(())
}
