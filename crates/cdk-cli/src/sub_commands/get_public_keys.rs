use anyhow::Result;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct GetPublicKeysSubCommand {
    /// Show the latest public key
    #[arg(long)]
    pub latest: bool,
}

pub async fn get_public_keys(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &GetPublicKeysSubCommand,
) -> Result<()> {
    if sub_command_args.latest {
        let latest_public_key = multi_mint_wallet.get_latest_public_key().await?;

        match latest_public_key {
            Some(key) => {
                println!("\npublic key found!\n");

                println!("public key: {}", key.pubkey.to_hex());
                println!("derivation path: {}", key.derivation_path);
            }
            None => {
                println!("\npublic key not found!\n");
            }
        }

        return Ok(());
    }

    let list_public_keys = multi_mint_wallet.get_public_keys().await?;
    if list_public_keys.is_empty() {
        println!("\npublic not found!\n");
    }
    println!("\npublic keys found:\n");
    for public_key in list_public_keys {
        println!("public key: {}", public_key.pubkey.to_hex());
        println!("derivation path: {}", public_key.derivation_path);
    }
    Ok(())
}
