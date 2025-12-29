use anyhow::Result;
use cdk::wallet::MultiMintWallet;
use cdk_common::PublicKey;
use clap::Args;

#[derive(Args)]
pub struct GetPublicKeySubCommand {
    /// Show the latest public key
    #[arg(long, conflicts_with = "hex")]
    pub latest: bool,
    /// Show public key by hex string
    #[arg(long, conflicts_with = "latest")]
    pub hex: Option<String>,
}

pub async fn get_public_keys(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &GetPublicKeySubCommand,
) -> Result<()> {
    let mut latest_public_key = None;

    if sub_command_args.latest {
        let list_public_keys = multi_mint_wallet.p2pk_list().await?;
        for public_key in list_public_keys {
            // check if latest_public_key is none, if so, set it to the first public key
            if latest_public_key.is_none() {
                latest_public_key = Some(public_key.clone());
            } else {
                if public_key.derivation_index > latest_public_key.clone().unwrap().derivation_index
                {
                    latest_public_key = Some(public_key.clone());
                }
            }
        }
    }

    if sub_command_args.hex.is_some() {
        let arg_hex = sub_command_args.hex.clone().unwrap();
        let arg_public_key = PublicKey::from_hex(&arg_hex)?;

        let public_key = multi_mint_wallet.p2pk_public_key(&arg_public_key).await?;
        if public_key.is_some() {
            latest_public_key = public_key.clone();
        }
    }

    if latest_public_key.is_some() {
        println!("\npublic key found! 🎉\n");
        println!("public key: {}", latest_public_key.unwrap().pubkey.to_hex());
    } else {
        println!("\npublic key not found! 🤔\n");
    }
    Ok(())
}
