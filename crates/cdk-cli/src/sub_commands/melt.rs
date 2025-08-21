use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{amount_for_offer, Amount, MSAT_IN_SAT};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MeltOptions};
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::wallet::{MeltQuote, Wallet};
use cdk::Bolt11Invoice;
use clap::{Args, ValueEnum};
use lightning::offers::offer::Offer;
use tokio::task::JoinSet;

use crate::sub_commands::balance::mint_balances;
use crate::utils::{
    get_number_input, get_user_input, get_wallet_by_index, get_wallet_by_mint_url,
    validate_mint_number,
};

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

/// Helper function to process a melt quote and execute the payment
async fn process_payment(wallet: &Wallet, quote: MeltQuote) -> Result<()> {
    // Display quote information
    println!("Quote ID: {}", quote.id);
    println!("Amount: {}", quote.amount);
    println!("Fee Reserve: {}", quote.fee_reserve);
    println!("State: {}", quote.state);
    println!("Expiry: {}", quote.expiry);

    // Execute the payment
    let melt = wallet.melt(&quote.id).await?;
    println!("Paid: {}", melt.state);

    if let Some(preimage) = melt.preimage {
        println!("Payment preimage: {preimage}");
    }

    Ok(())
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
    let mints_amounts = mint_balances(multi_mint_wallet, &unit).await?;

    if sub_command_args.mpp {
        // MPP logic only works with BOLT11 currently
        if !matches!(sub_command_args.method, PaymentType::Bolt11) {
            bail!("MPP is only supported for BOLT11 invoices");
        }

        // Collect mint numbers and amounts for MPP
        let (mints, mint_amounts) = collect_mpp_inputs(&mints_amounts, &sub_command_args.mint_url)?;

        // Process BOLT11 MPP payment
        let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice request")?)?;

        // Get quotes from all mints
        let quotes = get_mpp_quotes(
            multi_mint_wallet,
            &mints_amounts,
            &mints,
            &mint_amounts,
            &unit,
            &bolt11,
        )
        .await?;

        // Execute all melts
        execute_mpp_melts(quotes).await?;
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

        // Process payment based on payment method
        match sub_command_args.method {
            PaymentType::Bolt11 => {
                // Process BOLT11 payment
                let bolt11 = Bolt11Invoice::from_str(&get_user_input("Enter bolt11 invoice")?)?;

                // Determine payment amount and options
                let prompt =
                    "Enter the amount you would like to pay in sats for this amountless invoice.";
                let options =
                    create_melt_options(available_funds, bolt11.amount_milli_satoshis(), prompt)?;

                // Process payment
                let quote = wallet.melt_quote(bolt11.to_string(), options).await?;
                process_payment(&wallet, quote).await?;
            }
            PaymentType::Bolt12 => {
                // Process BOLT12 payment (offer)
                let offer_str = get_user_input("Enter BOLT12 offer")?;
                let offer = Offer::from_str(&offer_str)
                    .map_err(|e| anyhow::anyhow!("Invalid BOLT12 offer: {:?}", e))?;

                // Determine if offer has an amount
                let prompt =
                    "Enter the amount you would like to pay in sats for this amountless offer:";
                let amount_msat = match amount_for_offer(&offer, &CurrencyUnit::Msat) {
                    Ok(amount) => Some(u64::from(amount)),
                    Err(_) => None,
                };

                let options = create_melt_options(available_funds, amount_msat, prompt)?;

                // Get melt quote for BOLT12
                let quote = wallet.melt_bolt12_quote(offer_str, options).await?;
                process_payment(&wallet, quote).await?;
            }
            PaymentType::Bip353 => {
                let bip353_addr = get_user_input("Enter Bip353 address.")?;

                let prompt =
                    "Enter the amount you would like to pay in sats for this amountless offer:";
                // BIP353 payments are always amountless for now
                let options = create_melt_options(available_funds, None, prompt)?;

                // Get melt quote for BIP353 address (internally resolves and gets BOLT12 quote)
                let quote = wallet
                    .melt_bip353_quote(
                        &bip353_addr,
                        options.expect("Amount is required").amount_msat(),
                    )
                    .await?;
                process_payment(&wallet, quote).await?;
            }
        }
    }

    Ok(())
}

/// Collect mint numbers and amounts for MPP payments
fn collect_mpp_inputs(
    mints_amounts: &[(MintUrl, Amount)],
    mint_url_opt: &Option<String>,
) -> Result<(Vec<usize>, Vec<u64>)> {
    let mut mints = Vec::new();
    let mut mint_amounts = Vec::new();

    // If a specific mint URL was provided, try to use it as the first mint
    if let Some(mint_url) = mint_url_opt {
        println!("Using mint URL {mint_url} as the first mint for MPP payment.");

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
            println!(
                "Warning: Mint URL not found or no balance. Continuing with manual selection."
            );
        }
    }

    // Continue with regular mint selection
    loop {
        let mint_number: String =
            get_user_input("Enter mint number to melt from and -1 when done.")?;

        if mint_number == "-1" || mint_number.is_empty() {
            break;
        }

        let mint_number: usize = mint_number.parse()?;
        validate_mint_number(mint_number, mints_amounts.len())?;

        mints.push(mint_number);
        let melt_amount: u64 = get_number_input("Enter amount to mint from this mint in sats.")?;
        mint_amounts.push(melt_amount);
    }

    if mints.is_empty() {
        bail!("No mints selected for MPP payment");
    }

    Ok((mints, mint_amounts))
}

/// Get quotes from all mints for MPP payment
async fn get_mpp_quotes(
    multi_mint_wallet: &MultiMintWallet,
    mints_amounts: &[(MintUrl, Amount)],
    mints: &[usize],
    mint_amounts: &[u64],
    unit: &CurrencyUnit,
    bolt11: &Bolt11Invoice,
) -> Result<Vec<(Wallet, MeltQuote)>> {
    let mut quotes = JoinSet::new();

    for (mint, amount) in mints.iter().zip(mint_amounts) {
        let wallet = mints_amounts[*mint].0.clone();

        let wallet = multi_mint_wallet
            .get_wallet(&WalletKey::new(wallet, unit.clone()))
            .await
            .expect("Known wallet");
        let options = MeltOptions::new_mpp(*amount * 1000);

        let bolt11_clone = bolt11.clone();

        quotes.spawn(async move {
            let quote = wallet
                .melt_quote(bolt11_clone.to_string(), Some(options))
                .await;

            (wallet, quote)
        });
    }

    let quotes_results = quotes.join_all().await;

    // Validate all quotes succeeded
    let mut valid_quotes = Vec::new();
    for (wallet, quote_result) in quotes_results {
        match quote_result {
            Ok(quote) => {
                println!(
                    "Melt quote {} for mint {} of amount {} with fee {}.",
                    quote.id, wallet.mint_url, quote.amount, quote.fee_reserve
                );
                valid_quotes.push((wallet, quote));
            }
            Err(err) => {
                tracing::error!("Could not get quote for {}: {:?}", wallet.mint_url, err);
                bail!("Could not get melt quote for {}", wallet.mint_url);
            }
        }
    }

    Ok(valid_quotes)
}

/// Execute all melts for MPP payment
async fn execute_mpp_melts(quotes: Vec<(Wallet, MeltQuote)>) -> Result<()> {
    let mut melts = JoinSet::new();

    for (wallet, quote) in quotes {
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

    Ok(())
}
