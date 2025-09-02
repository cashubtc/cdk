use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{Amount, MSAT_IN_SAT};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::MultiMintWallet;
use cdk::Bolt11Invoice;
use clap::{Args, ValueEnum};

use crate::utils::{get_number_input, get_user_input};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum PaymentType {
    /// BOLT11 invoice
    Bolt11,
    /// BOLT12 offer
    Bolt12,
    /// Bip353
    Bip353,
}

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
    /// Payment method (bolt11 or bolt12)
    #[arg(long, default_value = "bolt11")]
    method: PaymentType,
}

/// Helper function to check if there are enough funds and create appropriate MeltOptions
fn create_melt_options(
    available_funds: u64,
    payment_amount: Option<u64>,
    prompt: &str,
) -> Result<Option<MeltOptions>> {
    match payment_amount {
        Some(amount) => {
            // Payment has a specified amount
            if amount > available_funds {
                bail!("Not enough funds; payment requires {} msats", amount);
            }
            Ok(None) // Use default options
        }
        None => {
            // Payment doesn't have an amount, ask user for it
            let user_amount = get_number_input::<u64>(prompt)? * MSAT_IN_SAT;

            if user_amount > available_funds {
                bail!("Not enough funds");
            }

            Ok(Some(MeltOptions::new_amountless(user_amount)))
        }
    }
}

pub async fn pay(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MeltSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;

    // Check total balance across all wallets
    let total_balance = multi_mint_wallet.total_balance().await?;
    if total_balance == Amount::ZERO {
        bail!("No funds available for unit: {}", unit);
    }

    if sub_command_args.mpp {
        // For MPP, we'll use the new unified interface which supports MPP internally
        if !matches!(sub_command_args.method, PaymentType::Bolt11) {
            bail!("MPP is only supported for BOLT11 invoices");
        }

        let bolt11 = get_user_input("Enter bolt11 invoice request")?;

        // Use the new unified melt function with MPP support
        match multi_mint_wallet.melt(&bolt11, None, None).await {
            Ok(melted) => {
                println!("Payment successful: {:?}", melted);
                if let Some(preimage) = melted.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
            Err(e) => {
                bail!("MPP payment failed: {}", e);
            }
        }
    } else {
        let available_funds = <cdk::Amount as Into<u64>>::into(total_balance) * MSAT_IN_SAT;

        // Process payment based on payment method using new unified interface
        match sub_command_args.method {
            PaymentType::Bolt11 => {
                // Process BOLT11 payment
                let bolt11_str = get_user_input("Enter bolt11 invoice")?;
                let bolt11 = Bolt11Invoice::from_str(&bolt11_str)?;

                // Determine payment amount and options
                let prompt =
                    "Enter the amount you would like to pay in sats for this amountless invoice.";
                let options =
                    create_melt_options(available_funds, bolt11.amount_milli_satoshis(), prompt)?;

                // Use the new unified interface
                let melted = if let Some(mint_url) = &sub_command_args.mint_url {
                    // User specified a mint
                    let mint_url = MintUrl::from_str(mint_url)?;
                    if let Some(wallet) = multi_mint_wallet.get_wallet(&mint_url).await {
                        // First create a melt quote
                        let quote = wallet.melt_quote(bolt11_str.clone(), options).await?;
                        // Then melt using the quote id
                        wallet.melt(&quote.id).await?
                    } else {
                        bail!("Mint {} not found in wallet", mint_url);
                    }
                } else {
                    // Let the wallet automatically select the best mint
                    multi_mint_wallet.melt(&bolt11_str, options, None).await?
                };

                println!("Payment successful: {:?}", melted);
                if let Some(preimage) = melted.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
            PaymentType::Bolt12 => {
                println!("BOLT12 support with new unified interface is not yet implemented.");
                bail!("Please use the legacy approach for BOLT12 payments");
            }
            PaymentType::Bip353 => {
                println!("BIP353 support with new unified interface is not yet implemented.");
                bail!("Please use the legacy approach for BIP353 payments");
            }
        }
    }

    Ok(())
}
