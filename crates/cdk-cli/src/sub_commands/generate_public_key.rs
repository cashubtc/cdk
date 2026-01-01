use anyhow::Result;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct GeneratePublicKeySubCommand {}

pub async fn generate_public_key(
    multi_mint_wallet: &MultiMintWallet,
    _sub_command_args: &GeneratePublicKeySubCommand,
) -> Result<()> {
    let public_key = multi_mint_wallet.generate_public_key().await?;

    println!("\npublic key generated!\n");
    println!("public key: {}", public_key.to_hex());

    Ok(())
}
