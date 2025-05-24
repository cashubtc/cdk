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
use crate::utils::{
    get_number_input, get_user_input, get_wallet_by_index, get_wallet_by_mint_url,
    validate_mint_number,
};

#[derive(Args)]
pub struct MeltSubCommand {
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Mpp
    #[arg(short, long, conflicts_with = "mint_url")]
    mpp: bool,
    /// Mint URL to use for melting
    #[arg(long, conflicts_with = "mpp")]
    mint_url: Option<String>,
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
        // MPP functionality expects multiple mints, so mint_url flag doesn't fully apply here,
        // but we can offer to use the specified mint as the first one if provided
        if let Some(mint_url) = &sub_command_args.mint_url {
            println!("Using mint URL {mint_url} as the first mint for MPP payment.");

            // Check if the mint exists
            if let Ok(_wallet) =
                get_wallet_by_mint_url(multi_mint_wallet, mint_url, unit.clone()).await
            {
                // Find the index of this mint in the mints_amounts list
                if let Some(mint_index) = mints_amounts
                    .iter()
                    .position(|(url, _)| url.to_string() == *mint_url)
                {
                    mints.push(mint_index);
                    let melt_amount: u64 =
                        get_number_input("Enter amount to mint from this mint in sats.")?;
                    mint_amounts.push(melt_amount);
                } else {
                    println!("Warning: Mint URL exists but no balance found. Continuing with manual selection.");
                }
            } else {
                println!("Warning: Could not find wallet for the specified mint URL. Continuing with manual selection.");
            }
        }
        loop {
            let mint_number: String =
                get_user_input("Enter mint number to melt from and -1 when done.")?;

            if mint_number == "-1" || mint_number.is_empty() {
                break;
            }

            let mint_number: usize = mint_number.parse()?;
            validate_mint_number(mint_number, mints_amounts.len())?;

            mints.push(mint_number);
            let melt_amount: u64 =
                get_number_input("Enter amount to mint from this mint in sats.")?;
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
            let options = MeltOptions::new_mpp(amount * 1000);

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
                let quote = quote.as_ref().unwrap();
                println!(
                    "Melt quote {} for mint {} of amount {} with fee {}.",
                    quote.id, wallet.mint_url, quote.amount, quote.fee_reserve
                );
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

        let mut error = false;

        for (wallet, melt) in melts {
            match melt {
                Ok(melt) => {
                    println!(
                        "Melt for {} paid {} with fee of {} ",
                        wallet.mint_url, melt.amount, melt.fee_paid
                    );
                }
                Err(err) => {
                    println!("Melt for {} failed with {}", wallet.mint_url, err);
                    error = true;
                }
            }
        }

        if error {
            bail!("Could not complete all melts");
        }
    } else {
        // Get wallet either by mint URL or by index
        let wallet = if let Some(mint_url) = &sub_command_args.mint_url {
            // Use the provided mint URL
            get_wallet_by_mint_url(multi_mint_wallet, mint_url, unit.clone()).await?
        } else {
            // Fallback to the index-based selection
            let mint_number: usize = get_number_input("Enter mint number to melt from")?;
            get_wallet_by_index(multi_mint_wallet, &mints_amounts, mint_number, unit.clone())
                .await?
        };

        // Find the mint amount for the selected wallet to check available funds
        let mint_url = &wallet.mint_url;
        let mint_amount = mints_amounts
            .iter()
            .find(|(url, _)| url == mint_url)
            .map(|(_, amount)| *amount)
            .ok_or_else(|| anyhow::anyhow!("Could not find balance for mint: {}", mint_url))?;

        let available_funds = <cdk::Amount as Into<u64>>::into(mint_amount) * MSAT_IN_SAT;

        let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice request")?)?;

        // Determine payment amount and options
        let options = if bolt11.amount_milli_satoshis().is_none() {
            // Get user input for amount
            let prompt = format!(
                "Enter the amount you would like to pay in sats for a {} payment.",
                if sub_command_args.mpp {
                    "MPP"
                } else {
                    "amountless invoice"
                }
            );

            let user_amount = get_number_input::<u64>(&prompt)? * MSAT_IN_SAT;

            if user_amount > available_funds {
                bail!("Not enough funds");
            }

            Some(MeltOptions::new_amountless(user_amount))
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
        println!("{quote:?}");

        let melt = wallet.melt(&quote.id).await?;
        println!("Paid invoice: {}", melt.state);

        if let Some(preimage) = melt.preimage {
            println!("Payment preimage: {preimage}");
        }
    }

    Ok(())
}
