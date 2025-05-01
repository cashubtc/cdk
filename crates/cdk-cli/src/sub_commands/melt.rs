use std::io::{self, Write};
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::MSAT_IN_SAT;
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::Bolt11Invoice;
use clap::Args;
use tokio::task::JoinSet;

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

/// Helper function to get user input with a prompt
fn get_user_input(prompt: &str) -> Result<String> {
    println!("{}", prompt);
    let mut user_input = String::new();
    io::stdout().flush()?;
    io::stdin().read_line(&mut user_input)?;
    Ok(user_input.trim().to_string())
}

pub async fn pay(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MeltSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let mints_amounts = mint_balances(multi_mint_wallet, &unit).await?;

    let mut mints = vec![];
    let mut mint_amounts = vec![];
    if sub_command_args.mpp {
        loop {
            let mint_number: String = get_user_input("Enter mint number to melt from")?;

            if mint_number == "-1" {
                break;
            }

            let mint_number: usize = mint_number.parse()?;
            if mint_number.gt(&(mints_amounts.len() - 1)) {
                bail!("Invalid mint number");
            }

            mints.push(mint_number);
            let melt_amount: u64 =
                get_user_input("Enter amount to mint from this mint.")?.parse()?;
            mint_amounts.push(melt_amount);
        }

        let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice request")?)?;

        let mut quotes = JoinSet::new();

        for (mint, amount) in mints.iter().zip(mint_amounts) {
            let wallet = mints_amounts[*mint].0.clone();

            let wallet = multi_mint_wallet
                .get_wallet(&WalletKey::new(wallet, unit.clone()))
                .await
                .expect("Known wallet");
            let options = MeltOptions::new_mpp(amount);

            let bolt11_clone = bolt11.clone();

            quotes.spawn(async move {
                let quote = wallet
                    .melt_quote(bolt11_clone.to_string(), Some(options))
                    .await;

                (wallet, quote)
            });
        }

        let quotes = quotes.join_all().await;

        for (wallet, quote) in quotes.iter() {
            if let Err(quote) = quote {
                tracing::error!("Could not get quote for {}: {:?}", wallet.mint_url, quote);
                bail!("Could not get melt quote for {}", wallet.mint_url);
            } else {
                println!("{:?}", quote.as_ref().expect("checked"));
            }
        }

        let mut melts = JoinSet::new();

        for (wallet, quote) in quotes {
            let quote = quote.expect("Errors checked above");

            melts.spawn(async move {
                let melt = wallet.melt(&quote.id).await;
                (wallet, melt)
            });
        }

        let melts = melts.join_all().await;

        for (wallet, melt) in melts {
            match melt {
                Ok(melt) => {
                    println!("Melt for {} complete {:?}", wallet.mint_url, melt);
                }
                Err(err) => {
                    println!("Melt for {} failed with {}", wallet.mint_url, err);
                }
            }
        }

        bail!("Could not complete all melts");
    } else {
        let mint_number: usize = get_user_input("Enter mint number to melt from")?.parse()?;

        if mint_number.gt(&(mints_amounts.len() - 1)) {
            bail!("Invalid mint number");
        }

        let wallet = mints_amounts[mint_number].0.clone();

        let wallet = multi_mint_wallet
            .get_wallet(&WalletKey::new(wallet, unit))
            .await
            .expect("Known wallet");

        let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice request")?)?;

        let available_funds =
            <cdk::Amount as Into<u64>>::into(mints_amounts[mint_number].1) * MSAT_IN_SAT;

        // Determine payment amount and options
        let options = if sub_command_args.mpp || bolt11.amount_milli_satoshis().is_none() {
            // Get user input for amount
            let prompt = format!(
                "Enter the amount you would like to pay in sats for a {} payment.",
                if sub_command_args.mpp {
                    "MPP"
                } else {
                    "amountless invoice"
                }
            );

            let user_amount = get_user_input(&prompt)?.parse::<u64>()? * MSAT_IN_SAT;

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
    }

    Ok(())
}
