use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::types::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Args, Serialize, Deserialize)]
pub struct MintBlindAuthSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: u64,
    /// Cat
    cat: String,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    #[arg(short, long)]
    unit: String,
}

pub async fn mint_blind_auth(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &MintBlindAuthSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    let _wallet = match multi_mint_wallet
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

    todo!()

    // let proofs = wallet
    //     .mint_blind_auth(Amount::from(sub_command_args.amount))
    //     .await?;

    // println!(
    //     "Received {} from auth proofs for mint {mint_url}",
    //     proofs.len()
    // );

    // Ok(())
}
