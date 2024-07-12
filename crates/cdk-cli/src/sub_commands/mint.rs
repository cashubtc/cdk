use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use cdk::Amount;
use clap::Args;
use tokio::time::sleep;

#[derive(Args)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: UncheckedUrl,
    /// Amount
    amount: u64,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn mint(
    wallets: HashMap<UncheckedUrl, Wallet>,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let wallet = match wallets.get(&mint_url) {
        Some(wallet) => wallet.clone(),
        None => Wallet::new(
            &mint_url.to_string(),
            CurrencyUnit::Sat,
            localstore,
            seed,
            None,
        ),
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
