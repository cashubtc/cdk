use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::types::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use cdk::OidcClient;
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Args, Serialize, Deserialize)]
pub struct CatLoginSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Username
    username: String,
    /// Password
    password: String,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    #[arg(short, long)]
    unit: String,
    /// Client ID for OIDC authentication
    #[arg(default_value = "cashu-client")]
    #[arg(long)]
    client_id: String,
}

pub async fn cat_login(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &CatLoginSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None)?;

            multi_mint_wallet.add_wallet(wallet.clone()).await;
            wallet
        }
    };

    let mint_info = wallet.get_mint_info().await?.expect("Mint info not found");

    let openid_discovery = mint_info
        .nuts
        .nut21
        .expect("Nut21 defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery);

    let access_token = oidc_client
        .get_access_token_with_user_password(
            sub_command_args.client_id.clone(),
            sub_command_args.username.clone(),
            sub_command_args.password.clone(),
        )
        .await?;

    println!("access_token: {}", access_token.access_token);
    println!("refresh_token: {:?}", access_token.refresh_token);

    Ok(())
}
