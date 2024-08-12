use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use cdk::Amount;
use clap::Args;
use tokio::time::sleep;

#[derive(Args)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: u64,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn mint(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None);

            multi_mint_wallet.add_wallet(wallet.clone()).await;
            wallet
        }
    };

    let quote = wallet
        .mint_quote(Amount::from(sub_command_args.amount))
        .await?;

    println!("Quote: {:#?}", quote);

    println!("Please pay: {}", quote.request);

    loop {
        let status = wallet.mint_quote_state(&quote.id).await?;

        if status.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(2)).await;
    }

    let receive_amount = wallet.mint(&quote.id, SplitTarget::default(), None).await?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
