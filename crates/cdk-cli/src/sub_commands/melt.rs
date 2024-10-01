use std::io;
use std::io::Write;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::Amount;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::multi_mint_wallet::{MultiMintWallet, WalletKey};
// use cdk::Bolt11Invoice;
use clap::Args;

use crate::sub_commands::balance::mint_balances;

#[derive(Args)]
pub struct MeltSubCommand {
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Payment method
    #[arg(short, long, default_value = "bolt11")]
    method: String,
    /// Amount
    #[arg(short, long)]
    amount: Option<u64>,
}

pub async fn pay(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MeltSubCommand,
) -> Result<()> {
    println!("{}", sub_command_args.unit);
    let unit = CurrencyUnit::from_str(&sub_command_args.unit).unwrap();
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

    let method = PaymentMethod::from_str(&sub_command_args.method)?;
    match method {
        PaymentMethod::Bolt11 => {
            println!("Enter bolt11 invoice request");
        }
        PaymentMethod::Bolt12 => {
            println!("Enter bolt12 invoice request");
        }
        _ => panic!("Unknown payment method"),
    }

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;

    let quote = match method {
        PaymentMethod::Bolt11 => {
            wallet
                .melt_quote(user_input.trim().to_string(), None)
                .await?
        }
        PaymentMethod::Bolt12 => {
            let amount = sub_command_args.amount.map(Amount::from);
            wallet
                .melt_bolt12_quote(user_input.trim().to_string(), amount)
                .await?
        }
        _ => panic!("Unsupported payment methof"),
    };

    println!("{:?}", quote);

    let melt = wallet.melt(&quote.id).await?;

    println!("Paid invoice: {}", melt.state);
    if let Some(preimage) = melt.preimage {
        println!("Payment preimage: {}", preimage);
    }

    Ok(())
}
