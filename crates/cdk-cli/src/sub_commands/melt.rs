use std::io;
use std::io::Write;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::{MultiMintWallet, WalletKey};
use cdk::Bolt11Invoice;
use clap::Args;

use crate::sub_commands::balance::mint_balances;

#[derive(Args)]
pub struct MeltSubCommand {
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn pay(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MeltSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let mints_amounts = mint_balances(multi_mint_wallet, &unit).await?;

    println!("Enter mint number to melt from");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;

    let mint_number: usize = user_input.trim().parse()?;

    if mint_number.gt(&(mints_amounts.len() - 1)) {
        bail!("Invalid mint number");
    }

    let wallet = mints_amounts[mint_number].0.clone();

    let wallet = multi_mint_wallet
        .get_wallet(&WalletKey::new(wallet, unit))
        .await
        .expect("Known wallet");

    println!("Enter bolt11 invoice request");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;
    let bolt11 = Bolt11Invoice::from_str(user_input.trim())?;

    if bolt11
        .amount_milli_satoshis()
        .unwrap()
        .gt(&(<cdk::Amount as Into<u64>>::into(mints_amounts[mint_number].1) * 1000_u64))
    {
        bail!("Not enough funds");
    }
    let quote = wallet.melt_quote(bolt11.to_string(), None).await?;

    println!("{:?}", quote);

    let melt = wallet.melt(&quote.id).await?;

    println!("Paid invoice: {}", melt.state);
    if let Some(preimage) = melt.preimage {
        println!("Payment preimage: {}", preimage);
    }

    Ok(())
}
