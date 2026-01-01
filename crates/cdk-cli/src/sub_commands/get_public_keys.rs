use anyhow::{Ok, Result};
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
    let list_public_keys = multi_mint_wallet.p2pk_list().await?;
    if sub_command_args.latest {
        // keys are ordered by creation time, so the first one is the latest
        let latest_public_key = list_public_keys.first().cloned();

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

    println!("\npublic keys found:\n");
    for public_key in list_public_keys {
        println!("public key: {}", public_key.pubkey.to_hex());
        println!("derivation path: {}", public_key.derivation_path);
    }
    Ok(())
}
