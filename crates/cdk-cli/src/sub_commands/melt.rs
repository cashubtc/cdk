use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{amount_for_offer, Amount, MSAT_IN_SAT};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::MultiMintWallet;
use cdk::Bolt11Invoice;
use clap::{Args, ValueEnum};
use lightning::offers::offer::Offer;

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
    /// Mpp
    #[arg(short, long, conflicts_with = "mint_url")]
    mpp: bool,
    /// Mint URL to use for melting
    #[arg(long, conflicts_with = "mpp")]
    mint_url: Option<String>,
    /// Payment method (bolt11, bolt12, or bip353)
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
    // Check total balance across all wallets
    let total_balance = multi_mint_wallet.total_balance().await?;
    if total_balance == Amount::ZERO {
        bail!("No funds available");
    }

    // Determine which mint to use for melting BEFORE processing payment (unless using MPP)
    let selected_mint = if sub_command_args.mpp {
        None // MPP mode handles mint selection differently
    } else if let Some(mint_url) = &sub_command_args.mint_url {
        Some(MintUrl::from_str(mint_url)?)
    } else {
        // Display all mints with their balances and let user select
        let balances_map = multi_mint_wallet.get_balances().await?;
        if balances_map.is_empty() {
            bail!("No mints available in the wallet");
        }

        let balances_vec: Vec<(MintUrl, Amount)> = balances_map.into_iter().collect();

        println!("\nAvailable mints and balances:");
        for (index, (mint_url, balance)) in balances_vec.iter().enumerate() {
            println!(
                "  {}: {} - {} {}",
                index,
                mint_url,
                balance,
                multi_mint_wallet.unit()
            );
        }
        println!("  {}: Any mint (auto-select best)", balances_vec.len());

        let selection = loop {
            let selection: usize =
                get_number_input("Enter mint number to melt from (or select Any)")?;

            if selection == balances_vec.len() {
                break None; // "Any" option selected
            }

            if let Some((mint_url, _)) = balances_vec.get(selection) {
                break Some(mint_url.clone());
            }

            println!("Invalid selection, please try again.");
        };

        selection
    };

    if sub_command_args.mpp {
        // Manual MPP - user specifies which mints and amounts to use
        if !matches!(sub_command_args.method, PaymentType::Bolt11) {
            bail!("MPP is only supported for BOLT11 invoices");
        }

        let bolt11_str = get_user_input("Enter bolt11 invoice")?;
        let _bolt11 = Bolt11Invoice::from_str(&bolt11_str)?; // Validate invoice format

        // Show available mints and balances
        let balances = multi_mint_wallet.get_balances().await?;
        println!("\nAvailable mints and balances:");
        for (i, (mint_url, balance)) in balances.iter().enumerate() {
            println!(
                "  {}: {} - {} {}",
                i,
                mint_url,
                balance,
                multi_mint_wallet.unit()
            );
        }

        // Collect mint selections and amounts
        let mut mint_amounts = Vec::new();
        loop {
            let mint_input = get_user_input("Enter mint number to use (or 'done' to finish)")?;

            if mint_input.to_lowercase() == "done" || mint_input.is_empty() {
                break;
            }

            let mint_index: usize = mint_input.parse()?;
            let mint_url = balances
                .iter()
                .nth(mint_index)
                .map(|(url, _)| url.clone())
                .ok_or_else(|| anyhow::anyhow!("Invalid mint index"))?;

            let amount: u64 = get_number_input(&format!(
                "Enter amount to use from this mint ({})",
                multi_mint_wallet.unit()
            ))?;
            mint_amounts.push((mint_url, Amount::from(amount)));
        }

        if mint_amounts.is_empty() {
            bail!("No mints selected for MPP payment");
        }

        // Get quotes for each mint
        println!("\nGetting melt quotes...");
        let quotes = multi_mint_wallet
            .mpp_melt_quote(bolt11_str, mint_amounts)
            .await?;

        // Display quotes
        println!("\nMelt quotes obtained:");
        for (mint_url, quote) in &quotes {
            println!("  {} - Quote ID: {}", mint_url, quote.id);
            println!("    Amount: {}, Fee: {}", quote.amount, quote.fee_reserve);
        }

        // Execute the melts
        let quotes_to_execute: Vec<(MintUrl, String)> = quotes
            .iter()
            .map(|(url, quote)| (url.clone(), quote.id.clone()))
            .collect();

        println!("\nExecuting MPP payment...");
        let results = multi_mint_wallet.mpp_melt(quotes_to_execute).await?;

        // Display results
        println!("\nPayment results:");
        let mut total_paid = Amount::ZERO;
        let mut total_fees = Amount::ZERO;

        for (mint_url, melted) in results {
            println!(
                "  {} - Paid: {}, Fee: {}",
                mint_url, melted.amount, melted.fee_paid
            );
            total_paid += melted.amount;
            total_fees += melted.fee_paid;

            if let Some(preimage) = melted.preimage {
                println!("    Preimage: {}", preimage);
            }
        }

        println!("\nTotal paid: {} {}", total_paid, multi_mint_wallet.unit());
        println!("Total fees: {} {}", total_fees, multi_mint_wallet.unit());
    } else {
        let available_funds = <cdk::Amount as Into<u64>>::into(total_balance) * MSAT_IN_SAT;

        // Process payment based on payment method using new unified interface
        match sub_command_args.method {
            PaymentType::Bolt11 => {
                // Process BOLT11 payment
                let bolt11_str = get_user_input("Enter bolt11 invoice")?;
                let bolt11 = Bolt11Invoice::from_str(&bolt11_str)?;

                // Determine payment amount and options
                let prompt = format!(
                    "Enter the amount you would like to pay in {} for this amountless invoice.",
                    multi_mint_wallet.unit()
                );
                let options =
                    create_melt_options(available_funds, bolt11.amount_milli_satoshis(), &prompt)?;

                // Use selected mint or auto-select
                let melted = if let Some(mint_url) = selected_mint {
                    // User selected a specific mint - use the new mint-specific functions
                    let quote = multi_mint_wallet
                        .melt_quote(&mint_url, bolt11_str.clone(), options)
                        .await?;

                    println!("Melt quote created:");
                    println!("  Quote ID: {}", quote.id);
                    println!("  Amount: {}", quote.amount);
                    println!("  Fee Reserve: {}", quote.fee_reserve);

                    // Execute the melt
                    multi_mint_wallet
                        .melt_with_mint(&mint_url, &quote.id)
                        .await?
                } else {
                    // User selected "Any" - let the wallet auto-select the best mint
                    multi_mint_wallet.melt(&bolt11_str, options, None).await?
                };

                println!("Payment successful: {:?}", melted);
                if let Some(preimage) = melted.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
            PaymentType::Bolt12 => {
                // Process BOLT12 payment (offer)
                let offer_str = get_user_input("Enter BOLT12 offer")?;
                let offer = Offer::from_str(&offer_str)
                    .map_err(|e| anyhow::anyhow!("Invalid BOLT12 offer: {:?}", e))?;

                // Determine if offer has an amount
                let prompt = format!(
                    "Enter the amount you would like to pay in {} for this amountless offer:",
                    multi_mint_wallet.unit()
                );
                let amount_msat = match amount_for_offer(&offer, &CurrencyUnit::Msat) {
                    Ok(amount) => Some(u64::from(amount)),
                    Err(_) => None,
                };

                let options = create_melt_options(available_funds, amount_msat, &prompt)?;

                // Get wallet for BOLT12 using the selected mint
                let mint_url = if let Some(specific_mint) = selected_mint {
                    specific_mint
                } else {
                    // User selected "Any" - just pick the first mint with any balance
                    let balances = multi_mint_wallet.get_balances().await?;

                    balances
                        .into_iter()
                        .find(|(_, balance)| *balance > Amount::ZERO)
                        .map(|(mint_url, _)| mint_url)
                        .ok_or_else(|| anyhow::anyhow!("No mint available for BOLT12 payment"))?
                };

                let wallet = multi_mint_wallet
                    .get_wallet(&mint_url)
                    .await
                    .ok_or_else(|| anyhow::anyhow!("Mint {} not found", mint_url))?;

                // Get melt quote for BOLT12
                let quote = wallet.melt_bolt12_quote(offer_str, options).await?;

                // Display quote info
                println!("Melt quote created:");
                println!("  Quote ID: {}", quote.id);
                println!("  Amount: {}", quote.amount);
                println!("  Fee Reserve: {}", quote.fee_reserve);
                println!("  State: {}", quote.state);
                println!("  Expiry: {}", quote.expiry);

                // Execute the melt
                let melted = wallet.melt(&quote.id).await?;
                println!(
                    "Payment successful: Paid {} with fee {}",
                    melted.amount, melted.fee_paid
                );
                if let Some(preimage) = melted.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
            PaymentType::Bip353 => {
                let bip353_addr = get_user_input("Enter Bip353 address")?;

                let prompt = format!(
                    "Enter the amount you would like to pay in {} for this amountless offer:",
                    multi_mint_wallet.unit()
                );
                // BIP353 payments are always amountless for now
                let options = create_melt_options(available_funds, None, &prompt)?;

                // Get wallet for BIP353 using the selected mint
                let mint_url = if let Some(specific_mint) = selected_mint {
                    specific_mint
                } else {
                    // User selected "Any" - just pick the first mint with any balance
                    let balances = multi_mint_wallet.get_balances().await?;

                    balances
                        .into_iter()
                        .find(|(_, balance)| *balance > Amount::ZERO)
                        .map(|(mint_url, _)| mint_url)
                        .ok_or_else(|| anyhow::anyhow!("No mint available for BIP353 payment"))?
                };

                let wallet = multi_mint_wallet
                    .get_wallet(&mint_url)
                    .await
                    .ok_or_else(|| anyhow::anyhow!("Mint {} not found", mint_url))?;

                // Get melt quote for BIP353 address (internally resolves and gets BOLT12 quote)
                let quote = wallet
                    .melt_bip353_quote(
                        &bip353_addr,
                        options.expect("Amount is required").amount_msat(),
                    )
                    .await?;

                // Display quote info
                println!("Melt quote created:");
                println!("  Quote ID: {}", quote.id);
                println!("  Amount: {}", quote.amount);
                println!("  Fee Reserve: {}", quote.fee_reserve);
                println!("  State: {}", quote.state);
                println!("  Expiry: {}", quote.expiry);

                // Execute the melt
                let melted = wallet.melt(&quote.id).await?;
                println!(
                    "Payment successful: Paid {} with fee {}",
                    melted.amount, melted.fee_paid
                );
                if let Some(preimage) = melted.preimage {
                    println!("Payment preimage: {}", preimage);
                }
            }
        }
    }

    Ok(())
}
