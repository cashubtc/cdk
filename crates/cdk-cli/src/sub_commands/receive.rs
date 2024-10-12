use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::nuts::{SecretKey, Token};
use cdk::wallet::multi_mint_wallet::{MultiMintWallet, WalletKey};
use cdk::wallet::Wallet;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct ReceiveSubCommand {
    /// Cashu Token
    token: Option<String>,
    /// Signing Key
    #[arg(short, long, action = clap::ArgAction::Append)]
    signing_key: Vec<String>,
}

pub async fn receive(
    multi_mint_wallet: &MultiMintWallet,
    localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
    seed: &[u8],
    sub_command_args: &ReceiveSubCommand,
) -> Result<()> {
    let token_str = sub_command_args.token.clone().unwrap();
    let amount = receive_token(multi_mint_wallet, localstore, seed, &token_str, &[], &[]).await?;
    println!("Received: {}", amount);

    Ok(())
}

async fn receive_token(
    multi_mint_wallet: &MultiMintWallet,
    localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
    seed: &[u8],
    token_str: &str,
    signing_keys: &[SecretKey],
    preimage: &[String],
) -> Result<Amount> {
    let token: Token = Token::from_str(token_str)?;

    let mint_url = token.proofs().into_keys().next().expect("Mint in token");

    let wallet_key = WalletKey::new(mint_url.clone(), token.unit().unwrap_or_default());

    if multi_mint_wallet.get_wallet(&wallet_key).await.is_none() {
        let wallet = Wallet::new(
            &mint_url.to_string(),
            token.unit().unwrap_or_default(),
            localstore,
            seed,
            None,
        )?;
        multi_mint_wallet.add_wallet(wallet).await;
    }

    let amount = multi_mint_wallet
        .receive(token_str, signing_keys, preimage)
        .await?;
    Ok(amount)
}
