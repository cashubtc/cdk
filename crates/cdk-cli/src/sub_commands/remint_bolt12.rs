use std::sync::Arc;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Args, Serialize, Deserialize)]
pub struct ReMintSubCommand {
    /// Mint url
    mint_url: MintUrl,
    #[arg(long)]
    quote_id: String,
}

pub async fn remint(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &ReMintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let quote_id = sub_command_args.quote_id.clone();

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            let wallet = Wallet::new(
                &mint_url.to_string(),
                CurrencyUnit::Sat,
                localstore,
                seed,
                None,
            )?;

            multi_mint_wallet.add_wallet(wallet.clone()).await;
            wallet
        }
    };

    // TODO: Pubkey
    let receive_amount = wallet
        .mint(&quote_id, SplitTarget::default(), None, None)
        .await?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
