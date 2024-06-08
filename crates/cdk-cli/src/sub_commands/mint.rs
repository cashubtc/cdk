use std::time::Duration;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::nuts::CurrencyUnit;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use cdk::Amount;
use clap::Args;
use tokio::time::sleep;

#[derive(Args)]
pub struct MintSubCommand {
    /// Amount
    #[arg(short, long)]
    amount: u64,
    /// Currency unit e.g. sat
    #[arg(short, long)]
    unit: String,
    /// Mint url
    #[arg(short, long)]
    mint_url: UncheckedUrl,
}

pub async fn mint(wallet: Wallet, sub_command_args: &MintSubCommand) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let quote = wallet
        .mint_quote(
            mint_url.clone(),
            Amount::from(sub_command_args.amount),
            CurrencyUnit::from(&sub_command_args.unit),
        )
        .await?;

    println!("Quote: {:#?}", quote);

    println!("Please pay: {}", quote.request);

    loop {
        let status = wallet
            .mint_quote_status(mint_url.clone(), &quote.id)
            .await?;

        if status.paid {
            break;
        }

        sleep(Duration::from_secs(2)).await;
    }

    let receive_amount = wallet
        .mint(mint_url.clone(), &quote.id, SplitTarget::default(), None)
        .await?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
