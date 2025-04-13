use std::io;
use std::io::Write;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::MSAT_IN_SAT;
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::Bolt11Invoice;
use clap::Args;

use crate::sub_commands::balance::mint_balances;

#[derive(Args)]
pub struct MeltSubCommand {
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Mpp
    #[arg(short, long)]
    mpp: bool,
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

    let available_funds =
        <cdk::Amount as Into<u64>>::into(mints_amounts[mint_number].1) * MSAT_IN_SAT;

    // Determine payment amount and options
    let options = if sub_command_args.mpp || bolt11.amount_milli_satoshis().is_none() {
        // Get user input for amount
        println!(
            "Enter the amount you would like to pay in sats for a {} payment.",
            if sub_command_args.mpp {
                "MPP"
            } else {
                "amountless invoice"
            }
        );

        let mut user_input = String::new();
        io::stdout().flush()?;
        io::stdin().read_line(&mut user_input)?;

        let user_amount = user_input.trim_end().parse::<u64>()? * MSAT_IN_SAT;

        if user_amount > available_funds {
            bail!("Not enough funds");
        }

        Some(if sub_command_args.mpp {
            MeltOptions::new_mpp(user_amount)
        } else {
            MeltOptions::new_amountless(user_amount)
        })
    } else {
        // Check if invoice amount exceeds available funds
        let invoice_amount = bolt11.amount_milli_satoshis().unwrap();
        if invoice_amount > available_funds {
            bail!("Not enough funds");
        }
        None
    };

    // Process payment
    let quote = wallet.melt_quote(bolt11.to_string(), options).await?;
    println!("{:?}", quote);

    let melt = wallet.melt(&quote.id).await?;
    println!("Paid invoice: {}", melt.state);

    if let Some(preimage) = melt.preimage {
        println!("Payment preimage: {}", preimage);
    }

    Ok(())
}
