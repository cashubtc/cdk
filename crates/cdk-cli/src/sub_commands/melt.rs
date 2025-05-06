use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{amount_for_offer, MSAT_IN_SAT};
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::Bolt11Invoice;
use clap::{Args, ValueEnum};
use lightning::offers::offer::Offer;
use tokio::task::JoinSet;

use crate::sub_commands::balance::mint_balances;
use crate::utils::{get_number_input, get_user_input, get_wallet_by_index, validate_mint_number};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum PaymentType {
    /// BOLT11 invoice
    Bolt11,
    /// BOLT12 offer
    Bolt12,
}

#[derive(Args)]
pub struct MeltSubCommand {
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Mpp
    #[arg(short, long)]
    mpp: bool,
    /// Payment type (bolt11 or bolt12)
    #[arg(short, long, default_value = "bolt11")]
    payment_type: PaymentType,
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
        // MPP logic only works with BOLT11 currently
        if matches!(sub_command_args.payment_type, PaymentType::Bolt12) {
            bail!("MPP is only supported for BOLT11 invoices, not BOLT12 offers");
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

        // Process BOLT11 MPP payment
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
        let mint_number: usize = get_number_input("Enter mint number to melt from")?;

        let wallet =
            get_wallet_by_index(multi_mint_wallet, &mints_amounts, mint_number, unit.clone())
                .await?;

        let available_funds =
            <cdk::Amount as Into<u64>>::into(mints_amounts[mint_number].1) * MSAT_IN_SAT;

        // Process payment based on payment type
        match sub_command_args.payment_type {
            PaymentType::Bolt11 => {
                // Process BOLT11 payment
                let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice")?)?;

                // Determine payment amount and options
                let options = if bolt11.amount_milli_satoshis().is_none() {
                    // Get user input for amount
                    let prompt = "Enter the amount you would like to pay in sats for this amountless invoice.";
                    let user_amount = get_number_input::<u64>(prompt)? * MSAT_IN_SAT;

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
                println!("Quote ID: {}", quote.id);
                println!("Amount: {}", quote.amount);
                println!("Fee Reserve: {}", quote.fee_reserve);
                println!("State: {}", quote.state);
                println!("Expiry: {}", quote.expiry);

                let melt = wallet.melt(&quote.id).await?;
                println!("Paid invoice: {}", melt.state);

                if let Some(preimage) = melt.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
            PaymentType::Bolt12 => {
                // Process BOLT12 payment (offer)
                let offer_str = get_user_input("Enter BOLT12 offer")?;
                let offer = Offer::from_str(&offer_str)
                    .map_err(|e| anyhow::anyhow!("Invalid BOLT12 offer: {:?}", e))?;

                // Determine if offer has an amount
                let options = match amount_for_offer(&offer, &CurrencyUnit::Msat) {
                    Ok(amount) => {
                        // Offer has an amount
                        let amount_msat = u64::from(amount);
                        if amount_msat > available_funds {
                            bail!("Not enough funds; offer requires {} msats", amount_msat);
                        }
                        None
                    }
                    Err(_) => {
                        // Offer doesn't have an amount, ask user for it
                        let prompt = "Enter the amount you would like to pay in sats for this amountless offer:";
                        let user_amount = get_number_input::<u64>(prompt)? * MSAT_IN_SAT;

                        if user_amount > available_funds {
                            bail!("Not enough funds");
                        }

                        Some(MeltOptions::new_amountless(user_amount))
                    }
                };

                // Get melt quote for BOLT12
                let quote = wallet.melt_bolt12_quote(offer_str, options).await?;
                println!("Quote ID: {}", quote.id);
                println!("Amount: {}", quote.amount);
                println!("Fee Reserve: {}", quote.fee_reserve);
                println!("State: {}", quote.state);
                println!("Expiry: {}", quote.expiry);

                // Execute the payment
                let melt = wallet.melt(&quote.id).await?;
                println!("Paid offer: {}", melt.state);

                if let Some(preimage) = melt.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
        }
    }

    Ok(())
}
