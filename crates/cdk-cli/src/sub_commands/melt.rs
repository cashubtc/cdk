use core::fmt;
use std::collections::HashMap;
use std::convert::Infallible;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{amount_for_offer, Amount, MSAT_IN_SAT};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{CurrencyUnit, MeltOptions, MintInfo, PaymentMethod};
use cdk::wallet::{Wallet, WalletRepository};
use cdk::Bolt11Invoice;
use cdk_common::wallet::WalletKey;
use clap::{Args, ValueEnum};
use lightning::offers::offer::Offer;

use crate::terminal::escape_control;
use crate::utils::{get_number_input, get_or_create_wallet, get_user_input};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaymentType {
    /// BOLT11 invoice
    Bolt11,
    /// BOLT12 offer
    Bolt12,
    /// Bip353
    Bip353,
    /// Onchain Bitcoin address
    Onchain,
    /// Custom payment method
    Custom(String),
}

impl FromStr for PaymentType {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bolt11" => Ok(Self::Bolt11),
            "bolt12" => Ok(Self::Bolt12),
            "bip353" => Ok(Self::Bip353),
            "onchain" => Ok(Self::Onchain),
            custom => Ok(Self::Custom(custom.to_string())),
        }
    }
}

impl fmt::Display for PaymentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bolt11 => write!(f, "bolt11"),
            Self::Bolt12 => write!(f, "bolt12"),
            Self::Bip353 => write!(f, "bip353"),
            Self::Onchain => write!(f, "onchain"),
            Self::Custom(custom) => write!(f, "{custom}"),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum BitcoinNetwork {
    /// Bitcoin Mainnet
    Bitcoin,
    /// Testnet
    Testnet,
    /// Signet
    Signet,
    /// Regtest
    Regtest,
}

impl From<BitcoinNetwork> for bitcoin::Network {
    fn from(network: BitcoinNetwork) -> Self {
        match network {
            BitcoinNetwork::Bitcoin => bitcoin::Network::Bitcoin,
            BitcoinNetwork::Testnet => bitcoin::Network::Testnet,
            BitcoinNetwork::Signet => bitcoin::Network::Signet,
            BitcoinNetwork::Regtest => bitcoin::Network::Regtest,
        }
    }
}

#[derive(Args, Debug)]
pub struct MeltSubCommand {
    /// Use Multi-Path Payment (split payment across multiple mints, BOLT11 only)
    #[arg(short, long, conflicts_with = "mint_url")]
    mpp: bool,
    /// Mint URL to use for melting
    #[arg(long, conflicts_with = "mpp")]
    mint_url: Option<String>,
    /// Payment method (bolt11, bolt12, bip353, onchain, or custom)
    #[arg(long, default_value = "bolt11")]
    method: PaymentType,
    /// BOLT11 invoice to pay (for bolt11 method)
    #[arg(long, conflicts_with_all = ["offer", "address", "request"])]
    invoice: Option<String>,
    /// BOLT12 offer to pay (for bolt12 method)
    #[arg(long, conflicts_with_all = ["invoice", "address", "request"])]
    offer: Option<String>,
    /// BIP353 or onchain address to pay
    #[arg(long, conflicts_with_all = ["invoice", "offer", "request"])]
    address: Option<String>,
    /// Generic payment request (for custom methods)
    #[arg(long, conflicts_with_all = ["invoice", "offer", "address"])]
    request: Option<String>,
    /// Bitcoin network to use for BIP353 (bitcoin, testnet, signet, regtest)
    #[arg(long, default_value = "bitcoin")]
    network: BitcoinNetwork,
    /// Amount to pay. For bolt11, bolt12, bip353, and onchain this is sats; for custom methods this is denominated in the selected wallet unit.
    #[arg(long)]
    amount: Option<u64>,
    /// MPP split entry in the form <mint_url>=<amount_sats>; repeat for multiple mints
    #[arg(long = "mpp-split", value_name = "MINT_URL=AMOUNT", action = clap::ArgAction::Append, requires = "mpp")]
    mpp_split: Vec<String>,
    /// Extra JSON data for custom payment methods (must be a valid JSON object)
    #[arg(long)]
    extra: Option<String>,
}

/// Helper function to check if there are enough funds and create appropriate MeltOptions
fn create_melt_options(
    available_funds: u64,
    payment_amount: Option<u64>,
    cli_amount_sat: Option<u64>,
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
            // Payment doesn't have an amount; use CLI amount if supplied, otherwise prompt.
            let amount_sat = match cli_amount_sat {
                Some(amount_sat) => amount_sat,
                None => get_number_input::<u64>(prompt)?,
            };
            let user_amount = amount_sat
                .checked_mul(MSAT_IN_SAT)
                .ok_or_else(|| anyhow::anyhow!("Amount overflow"))?;

            if user_amount > available_funds {
                bail!("Not enough funds");
            }

            Ok(Some(MeltOptions::new_amountless(user_amount)))
        }
    }
}

fn parse_and_validate_extra(extra_str: Option<&str>) -> Result<Option<serde_json::Value>> {
    match extra_str {
        Some(s) => {
            let value: serde_json::Value = serde_json::from_str(s)
                .map_err(|e| anyhow::anyhow!("--extra must be a valid JSON object: {e}"))?;
            if !value.is_object() {
                bail!("--extra must be a valid JSON object");
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn custom_payment_method_supported(
    mint_info: &MintInfo,
    payment_method: &PaymentMethod,
    unit: &CurrencyUnit,
) -> bool {
    !mint_info.nuts.nut05.disabled
        && mint_info
            .nuts
            .nut05
            .methods
            .iter()
            .any(|m| m.method == *payment_method && m.unit == *unit)
}

fn select_mint_for_custom_melt<'a, F>(
    balances: &'a [(WalletKey, Amount)],
    unit: &CurrencyUnit,
    required_amount: Option<Amount>,
    custom_method: &str,
    mut is_supported: F,
) -> Result<&'a MintUrl>
where
    F: FnMut(&MintUrl) -> bool,
{
    let min_balance = required_amount.unwrap_or(Amount::ZERO);
    let mut has_funded_mint = false;

    for (key, balance) in balances {
        if key.unit == *unit && *balance > Amount::ZERO && *balance >= min_balance {
            has_funded_mint = true;
            if is_supported(&key.mint_url) {
                return Ok(&key.mint_url);
            }
        }
    }

    if has_funded_mint {
        bail!(
            "No mint with sufficient balance supports payment method '{}' for unit {}",
            custom_method,
            unit
        );
    } else if let Some(req_amt) = required_amount {
        bail!(
            "No mint with sufficient balance (>= {} {}) for unit {}",
            req_amt,
            unit,
            unit
        );
    } else {
        bail!("No mint with balance for unit {}", unit);
    }
}

fn validate_custom_payment_method_support(
    mint_info: &MintInfo,
    payment_method: &PaymentMethod,
    custom_method: &str,
    unit: &CurrencyUnit,
    mint_url: &MintUrl,
) -> Result<()> {
    if mint_info.nuts.nut05.disabled {
        bail!("Melting is disabled for mint {}", mint_url);
    }

    if !custom_payment_method_supported(mint_info, payment_method, unit) {
        let method_supported_for_other_unit = mint_info
            .nuts
            .nut05
            .supported_methods()
            .contains(&payment_method);
        if method_supported_for_other_unit {
            bail!(
                "Payment method '{}' for unit {} is not supported by mint {}",
                custom_method,
                unit,
                mint_url
            );
        } else {
            bail!(
                "Payment method '{}' is not supported by mint {}",
                custom_method,
                mint_url
            );
        }
    }

    Ok(())
}

fn input_or_prompt(arg: Option<&String>, prompt: &str) -> Result<String> {
    match arg {
        Some(value) => Ok(value.clone()),
        None => get_user_input(prompt),
    }
}

fn parse_mpp_split(entry: &str) -> Result<(MintUrl, Amount)> {
    let (mint, amount) = entry.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("Invalid --mpp-split value '{entry}'. Expected MINT_URL=AMOUNT")
    })?;

    let mint_url = MintUrl::from_str(mint.trim())?;
    let amount_sat: u64 = amount.trim().parse()?;

    if amount_sat == 0 {
        bail!(
            "MPP split amount must be greater than zero for mint {}",
            mint_url
        );
    }

    Ok((mint_url, Amount::from(amount_sat)))
}

fn select_onchain_quote(
    quotes: &[cdk_common::wallet::MeltQuote],
) -> Result<cdk_common::wallet::MeltQuote> {
    if quotes.is_empty() {
        bail!("No onchain melt quotes available");
    }

    if quotes.len() == 1 {
        return Ok(quotes[0].clone());
    }

    println!("\nAvailable onchain melt quotes:");
    for (index, quote) in quotes.iter().enumerate() {
        println!(
            "  {}: amount={} fee={} expiry={} estimated_blocks={}",
            index,
            quote.amount,
            quote.fee_reserve,
            quote.expiry,
            quote.estimated_blocks.unwrap_or_default()
        );
    }

    loop {
        let selection: usize = get_number_input("Enter onchain quote number to use")?;

        if let Some(quote) = quotes.get(selection) {
            return Ok(quote.clone());
        }

        println!("Invalid selection, please try again.");
    }
}

pub async fn pay(
    wallet_repository: &WalletRepository,
    sub_command_args: &MeltSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    // Check total balance for the requested unit
    let balances_by_unit = wallet_repository.total_balance().await?;
    let total_balance = balances_by_unit.get(unit).copied().unwrap_or(Amount::ZERO);
    if total_balance == Amount::ZERO {
        bail!("No funds available for unit {}", unit);
    }

    // Handle MPP mode separately
    if sub_command_args.mpp {
        if sub_command_args.method != PaymentType::Bolt11 {
            bail!("MPP is only supported for BOLT11 invoices");
        }
        return pay_mpp(wallet_repository, sub_command_args, unit).await;
    }

    // Determine which mint to use for melting
    let selected_mint = if let Some(mint_url) = &sub_command_args.mint_url {
        Some(MintUrl::from_str(mint_url)?)
    } else {
        // Display all mints with their balances and let user select
        let balances_map = wallet_repository.get_balances().await?;
        if balances_map.is_empty() {
            bail!("No mints available in the wallet");
        }

        let balances_vec: Vec<(WalletKey, Amount)> = balances_map.into_iter().collect();

        // If only one mint exists, automatically select it
        if balances_vec.len() == 1 {
            Some(balances_vec[0].0.mint_url.clone())
        } else {
            // Display all mints with their balances and let user select
            println!("\nAvailable mints and balances:");
            for (index, (key, balance)) in balances_vec.iter().enumerate() {
                println!(
                    "  {}: {} ({}) - {} {}",
                    index,
                    escape_control(&key.mint_url.to_string()),
                    escape_control(&key.unit.to_string()),
                    balance,
                    unit
                );
            }
            println!("  {}: Any mint (auto-select best)", balances_vec.len());

            let selection = loop {
                let selection: usize =
                    get_number_input("Enter mint number to melt from (or select Any)")?;

                if selection == balances_vec.len() {
                    break None; // "Any" option selected
                }

                if let Some((key, _)) = balances_vec.get(selection) {
                    break Some(key.mint_url.clone());
                }

                println!("Invalid selection, please try again.");
            };

            selection
        }
    };

    let available_funds = <cdk::Amount as Into<u64>>::into(total_balance) * MSAT_IN_SAT;

    // Process payment based on payment method using individual wallets
    match &sub_command_args.method {
        PaymentType::Bolt11 => {
            // Process BOLT11 payment
            let bolt11_str =
                input_or_prompt(sub_command_args.invoice.as_ref(), "Enter bolt11 invoice")?;
            let bolt11 = Bolt11Invoice::from_str(&bolt11_str)?;

            // Determine payment amount and options
            let prompt = format!(
                "Enter the amount you would like to pay in {} for this amountless invoice.",
                unit
            );
            let options = create_melt_options(
                available_funds,
                bolt11.amount_milli_satoshis(),
                sub_command_args.amount,
                &prompt,
            )?;

            // Get or select a mint with sufficient balance
            let mint_url = if let Some(specific_mint) = selected_mint {
                specific_mint
            } else {
                // Auto-select the first mint with sufficient balance
                let balances = wallet_repository.get_balances().await?;
                let required_amount = options
                    .map(|options| options.amount_msat().into())
                    .or_else(|| bolt11.amount_milli_satoshis())
                    .map(|amount| {
                        Amount::new(amount, CurrencyUnit::Msat)
                            .convert_to_ceil(unit)
                            .map(Into::into)
                    })
                    .transpose()?
                    .unwrap_or(Amount::ZERO);

                balances
                    .into_iter()
                    .find(|(key, balance)| &key.unit == unit && *balance >= required_amount)
                    .map(|(key, _)| key.mint_url)
                    .ok_or_else(|| anyhow::anyhow!("No mint with sufficient balance"))?
            };

            let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

            // Get melt quote
            let quote = wallet
                .melt_quote(
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    bolt11_str.clone(),
                    options,
                    None,
                )
                .await?;

            println!("Melt quote created:");
            println!("  Quote ID: {}", escape_control(&quote.id));
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);

            // Execute the melt
            let melted = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;

            println!(
                "Payment successful: state={}, amount={}, fee_paid={}",
                melted.state(),
                melted.amount(),
                melted.fee_paid()
            );
            if let Some(preimage) = melted.payment_proof() {
                println!("Payment preimage: {}", escape_control(preimage));
            }
        }
        PaymentType::Bolt12 => {
            // Process BOLT12 payment (offer)
            let offer_str = input_or_prompt(sub_command_args.offer.as_ref(), "Enter BOLT12 offer")?;
            let offer = Offer::from_str(&offer_str)
                .map_err(|e| anyhow::anyhow!("Invalid BOLT12 offer: {:?}", e))?;

            // Determine if offer has an amount
            let prompt = format!(
                "Enter the amount you would like to pay in {} for this amountless offer:",
                unit
            );
            let amount_msat = match amount_for_offer(&offer, &CurrencyUnit::Msat) {
                Ok(amount) => Some(u64::from(amount)),
                Err(_) => None,
            };

            let options = create_melt_options(
                available_funds,
                amount_msat,
                sub_command_args.amount,
                &prompt,
            )?;

            // Get wallet for BOLT12 using the selected mint
            let mint_url = if let Some(specific_mint) = selected_mint {
                specific_mint
            } else {
                // User selected "Any" - just pick the first mint with any balance
                let balances = wallet_repository.get_balances().await?;

                balances
                    .into_iter()
                    .find(|(_, balance)| *balance > Amount::ZERO)
                    .map(|(key, _)| key.mint_url)
                    .ok_or_else(|| anyhow::anyhow!("No mint available for BOLT12 payment"))?
            };

            let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

            // Get melt quote for BOLT12
            let quote = wallet
                .melt_quote(
                    PaymentMethod::Known(KnownMethod::Bolt12),
                    offer_str,
                    options,
                    None,
                )
                .await?;

            // Display quote info
            println!("Melt quote created:");
            println!("  Quote ID: {}", escape_control(&quote.id));
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);
            println!("  State: {}", quote.state);
            println!("  Expiry: {}", quote.expiry);

            // Execute the melt
            let melted = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;
            println!(
                "Payment successful: Paid {} with fee {}",
                melted.amount(),
                melted.fee_paid()
            );
            if let Some(preimage) = melted.payment_proof() {
                println!("Payment preimage: {}", escape_control(preimage));
            }
        }
        PaymentType::Bip353 => {
            let bip353_addr =
                input_or_prompt(sub_command_args.address.as_ref(), "Enter Bip353 address")?;

            let prompt = format!(
                "Enter the amount you would like to pay in {} for this amountless offer:",
                unit
            );
            // BIP353 payments are always amountless for now
            let options =
                create_melt_options(available_funds, None, sub_command_args.amount, &prompt)?;

            // Get wallet for BIP353 using the selected mint
            let mint_url = if let Some(specific_mint) = selected_mint {
                specific_mint
            } else {
                // User selected "Any" - just pick the first mint with any balance
                let balances = wallet_repository.get_balances().await?;

                balances
                    .into_iter()
                    .find(|(_, balance)| *balance > Amount::ZERO)
                    .map(|(key, _)| key.mint_url)
                    .ok_or_else(|| anyhow::anyhow!("No mint available for BIP353 payment"))?
            };

            let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

            // Get melt quote for BIP353 address (internally resolves and gets BOLT12 quote)
            let quote = wallet
                .melt_bip353_quote(
                    &bip353_addr,
                    options.expect("Amount is required").amount_msat(),
                    sub_command_args.network.into(),
                )
                .await?;

            // Display quote info
            println!("Melt quote created:");
            println!("  Quote ID: {}", escape_control(&quote.id));
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);
            println!("  State: {}", quote.state);
            println!("  Expiry: {}", quote.expiry);

            // Execute the melt
            let melted = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;
            println!(
                "Payment successful: Paid {} with fee {}",
                melted.amount(),
                melted.fee_paid()
            );
            if let Some(preimage) = melted.payment_proof() {
                println!("Payment preimage: {}", escape_control(preimage));
            }
        }
        PaymentType::Onchain => {
            let onchain_address =
                input_or_prompt(sub_command_args.address.as_ref(), "Enter onchain address")?;

            let amount_sat = match sub_command_args.amount {
                Some(amount_sat) if amount_sat > 0 => amount_sat,
                Some(_) => bail!("Onchain melt amount must be greater than zero"),
                None => get_number_input::<u64>("Enter the amount you would like to melt in sats")?,
            };

            let melt_amount = Amount::from(amount_sat);

            // Get wallet for onchain using the selected mint
            let mint_url = if let Some(specific_mint) = selected_mint {
                specific_mint
            } else {
                let balances = wallet_repository.get_balances().await?;

                balances
                    .into_iter()
                    .find(|(_, balance)| *balance >= melt_amount)
                    .map(|(key, _)| key.mint_url)
                    .ok_or_else(|| {
                        anyhow::anyhow!("No mint with sufficient balance for onchain melt")
                    })?
            };

            let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

            let quote_options = wallet
                .quote_onchain_melt_options(&onchain_address, melt_amount, None)
                .await?;

            let selected_quote = select_onchain_quote(&quote_options)?;
            let quote = wallet.select_onchain_melt_quote(selected_quote).await?;

            println!("Melt quote selected:");
            println!("  Quote ID: {}", escape_control(&quote.id));
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);
            println!("  Expiry: {}", quote.expiry);
            if let Some(estimated_blocks) = quote.estimated_blocks {
                println!("  Estimated Blocks: {}", estimated_blocks);
            }

            let melted = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;

            println!(
                "Payment successful: state={}, amount={}, fee_paid={}",
                melted.state(),
                melted.amount(),
                melted.fee_paid()
            );
            if let Some(payment_proof) = melted.payment_proof() {
                println!("Payment proof: {}", escape_control(payment_proof));
            }
        }
        PaymentType::Custom(custom_method) => {
            // Validate --extra as a JSON object before creating the quote
            let validated_extra = parse_and_validate_extra(sub_command_args.extra.as_deref())?;

            let requested_amount = sub_command_args.amount.map(Amount::from);
            if let Some(req_amt) = requested_amount {
                if req_amt > total_balance {
                    bail!(
                        "Not enough funds: balance is {} {}, requested {} {}",
                        total_balance,
                        unit,
                        req_amt,
                        unit
                    );
                }
            }

            let payment_method = PaymentMethod::from_str(custom_method)?;

            let wallet = if let Some(specific_mint) = selected_mint {
                let wallet = get_or_create_wallet(wallet_repository, &specific_mint, unit).await?;
                let mint_info = wallet.load_mint_info().await?;
                validate_custom_payment_method_support(
                    &mint_info,
                    &payment_method,
                    custom_method,
                    unit,
                    &specific_mint,
                )?;
                wallet
            } else {
                let balances_map = wallet_repository.get_balances().await?;
                let balances_vec: Vec<(WalletKey, Amount)> = balances_map.into_iter().collect();

                let min_balance = requested_amount.unwrap_or(Amount::ZERO);
                let mut supported_candidate: Option<(MintUrl, Wallet)> = None;

                for (key, balance) in &balances_vec {
                    if key.unit == *unit && *balance > Amount::ZERO && *balance >= min_balance {
                        let candidate_wallet = match get_or_create_wallet(
                            wallet_repository,
                            &key.mint_url,
                            unit,
                        )
                        .await
                        {
                            Ok(w) => w,
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to get wallet for mint {}: {e}",
                                    key.mint_url
                                );
                                continue;
                            }
                        };

                        if let Ok(mint_info) = candidate_wallet.load_mint_info().await {
                            if custom_payment_method_supported(&mint_info, &payment_method, unit) {
                                supported_candidate =
                                    Some((key.mint_url.clone(), candidate_wallet));
                                break;
                            }
                        }
                    }
                }

                select_mint_for_custom_melt(
                    &balances_vec,
                    unit,
                    requested_amount,
                    custom_method,
                    |url| {
                        supported_candidate
                            .as_ref()
                            .map(|(u, _)| u == url)
                            .unwrap_or(false)
                    },
                )?;

                let (_, wallet) = supported_candidate.expect("candidate was selected");
                wallet
            };

            // For custom methods, explicitly use --request
            let request_str = input_or_prompt(
                sub_command_args.request.as_ref(),
                &format!("Enter payment request for {custom_method}"),
            )?;

            let mut extra_map = match validated_extra {
                Some(serde_json::Value::Object(map)) => map,
                _ => serde_json::Map::new(),
            };
            if let Some(amount) = sub_command_args.amount {
                extra_map.insert("amount".to_string(), serde_json::Value::from(amount));
            }
            let extra_str = if extra_map.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&serde_json::Value::Object(
                    extra_map,
                ))?)
            };

            let quote = wallet
                .melt_quote(payment_method, request_str, None, extra_str)
                .await?;

            println!("Melt quote created:");
            println!("  Quote ID: {}", escape_control(&quote.id));
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);
            println!("  State: {}", quote.state);
            println!("  Expiry: {}", quote.expiry);

            let required_total = quote
                .amount
                .checked_add(quote.fee_reserve)
                .ok_or_else(|| anyhow::anyhow!("Amount overflow in quote calculation"))?;

            let current_wallet_balance = wallet.total_balance().await?;
            if current_wallet_balance < required_total {
                bail!(
                    "Insufficient funds in selected mint: balance is {} {}, but quote requires {} (amount: {}, fee reserve: {})",
                    current_wallet_balance,
                    unit,
                    required_total,
                    quote.amount,
                    quote.fee_reserve
                );
            }

            let melted = wallet
                .prepare_melt(&quote.id, HashMap::new())
                .await?
                .confirm()
                .await?;

            println!(
                "Payment successful: state={}, amount={}, fee_paid={}",
                melted.state(),
                melted.amount(),
                melted.fee_paid()
            );
            if let Some(payment_proof) = melted.payment_proof() {
                println!("Payment proof: {}", escape_control(payment_proof));
            }
        }
    }

    Ok(())
}

/// Handle Multi-Path Payment (MPP) - split a BOLT11 payment across multiple mints
async fn pay_mpp(
    wallet_repository: &WalletRepository,
    sub_command_args: &MeltSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    if sub_command_args.method != PaymentType::Bolt11 {
        bail!("MPP is only supported for BOLT11 invoices");
    }

    let bolt11_str = input_or_prompt(sub_command_args.invoice.as_ref(), "Enter bolt11 invoice")?;
    // Validate invoice format
    let _bolt11 = Bolt11Invoice::from_str(&bolt11_str)?;

    // Show available mints and balances
    let balances = wallet_repository.get_balances().await?;
    let balances_vec: Vec<(WalletKey, Amount)> = balances.into_iter().collect();

    // Collect mint selections and amounts from CLI when provided, otherwise prompt interactively.
    let mint_amounts: Vec<(MintUrl, Amount)> = if sub_command_args.mpp_split.is_empty() {
        println!("\nAvailable mints and balances:");
        for (i, (key, balance)) in balances_vec.iter().enumerate() {
            println!(
                "  {}: {} ({}) - {} {}",
                i,
                escape_control(&key.mint_url.to_string()),
                escape_control(&key.unit.to_string()),
                balance,
                unit
            );
        }

        let mut selected = Vec::new();
        loop {
            let mint_input = get_user_input("Enter mint number to use (or 'done' to finish)")?;

            if mint_input.to_lowercase() == "done" || mint_input.is_empty() {
                break;
            }

            let mint_index: usize = mint_input.parse()?;
            let (key, _) = balances_vec
                .get(mint_index)
                .ok_or_else(|| anyhow::anyhow!("Invalid mint index"))?;

            let amount: u64 =
                get_number_input(&format!("Enter amount to use from this mint ({})", unit))?;
            selected.push((key.mint_url.clone(), Amount::from(amount)));
        }

        selected
    } else {
        let mut selected = Vec::new();
        for split in &sub_command_args.mpp_split {
            selected.push(parse_mpp_split(split)?);
        }
        selected
    };

    if mint_amounts.is_empty() {
        bail!("No mints selected for MPP payment");
    }

    for (mint_url, amount) in &mint_amounts {
        if !wallet_repository.has_mint(mint_url).await {
            bail!("MPP split mint {} is not in the wallet", mint_url);
        }

        let key = WalletKey::new(mint_url.clone(), unit.clone());
        let mint_balance = balances_vec
            .iter()
            .find(|(wallet_key, _)| *wallet_key == key)
            .map(|(_, balance)| *balance)
            .unwrap_or(Amount::ZERO);

        if *amount > mint_balance {
            bail!(
                "MPP split exceeds balance for mint {}. Available: {} {}, requested: {} {}",
                mint_url,
                mint_balance,
                unit,
                amount,
                unit
            );
        }
    }

    // Get quotes from each mint with MPP options
    println!("\nGetting melt quotes...");
    let mut quotes = Vec::new();
    for (mint_url, amount) in &mint_amounts {
        let wallet = get_or_create_wallet(wallet_repository, mint_url, unit).await?;

        // Convert amount to millisats for MPP
        let amount_msat = u64::from(*amount) * MSAT_IN_SAT;
        let options = Some(MeltOptions::new_mpp(amount_msat));

        let quote = wallet
            .melt_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                bolt11_str.clone(),
                options,
                None,
            )
            .await?;

        println!(
            "  {} - Quote ID: {}",
            escape_control(&mint_url.to_string()),
            escape_control(&quote.id)
        );
        println!("    Amount: {}, Fee: {}", quote.amount, quote.fee_reserve);
        quotes.push((mint_url.clone(), wallet, quote));
    }

    // Execute all melts
    println!("\nExecuting MPP payment...");
    let mut total_paid = Amount::ZERO;
    let mut total_fees = Amount::ZERO;

    for (mint_url, wallet, quote) in quotes {
        let melted = wallet
            .prepare_melt(&quote.id, HashMap::new())
            .await?
            .confirm()
            .await?;

        println!(
            "  {} - Paid: {}, Fee: {}",
            escape_control(&mint_url.to_string()),
            melted.amount(),
            melted.fee_paid()
        );
        total_paid += melted.amount();
        total_fees += melted.fee_paid();

        if let Some(preimage) = melted.payment_proof() {
            println!("    Preimage: {}", escape_control(preimage));
        }
    }

    println!(
        "\nTotal paid: {} {}",
        total_paid,
        escape_control(&unit.to_string())
    );
    println!(
        "Total fees: {} {}",
        total_fees,
        escape_control(&unit.to_string())
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use cdk::nuts::nut00::PaymentMethod;
    use cdk::nuts::nut05::{MeltMethodSettings, Settings as Nut05Settings};
    use cdk::nuts::{CurrencyUnit, MintInfo, Nuts};
    use clap::Parser;

    use super::*;

    #[derive(Parser, Debug)]
    struct TestMeltCli {
        #[command(flatten)]
        melt: MeltSubCommand,
    }

    #[test]
    fn parses_default_method_as_bolt11() {
        let cli = TestMeltCli::try_parse_from(["test"]).expect("parse test");
        assert_eq!(cli.melt.method, PaymentType::Bolt11);
        assert_eq!(cli.melt.method.to_string(), "bolt11");
    }

    #[test]
    fn parses_standard_methods() {
        let cases = [
            ("bolt11", PaymentType::Bolt11),
            ("bolt12", PaymentType::Bolt12),
            ("bip353", PaymentType::Bip353),
            ("onchain", PaymentType::Onchain),
        ];

        for (input, expected) in cases {
            let cli = TestMeltCli::try_parse_from(["test", "--method", input])
                .unwrap_or_else(|e| panic!("failed to parse {input}: {e}"));
            assert_eq!(cli.melt.method, expected);
            assert_eq!(cli.melt.method.to_string(), input);
        }
    }

    #[test]
    fn parses_custom_payment_methods() {
        let custom_methods = ["branch", "fake", "strike", "custom_pay"];

        for method in custom_methods {
            let cli = TestMeltCli::try_parse_from(["test", "--method", method])
                .unwrap_or_else(|e| panic!("failed to parse custom method {method}: {e}"));
            assert_eq!(cli.melt.method, PaymentType::Custom(method.to_string()));
            assert_eq!(cli.melt.method.to_string(), method);
        }
    }

    #[test]
    fn parses_custom_melt_args() {
        let cli = TestMeltCli::try_parse_from([
            "test",
            "--method",
            "branch",
            "--request",
            "branch_req_123",
            "--amount",
            "500",
            "--extra",
            r#"{"branch_id":"abc"}"#,
        ])
        .expect("parse full custom melt");

        assert_eq!(cli.melt.method, PaymentType::Custom("branch".to_string()));
        assert_eq!(cli.melt.request.as_deref(), Some("branch_req_123"));
        assert_eq!(cli.melt.amount, Some(500));
        assert_eq!(cli.melt.extra.as_deref(), Some(r#"{"branch_id":"abc"}"#));
    }

    #[test]
    fn request_conflicts_with_invoice_offer_and_address() {
        assert!(TestMeltCli::try_parse_from([
            "test",
            "--request",
            "req123",
            "--invoice",
            "lnbc123",
        ])
        .is_err());

        assert!(
            TestMeltCli::try_parse_from(["test", "--request", "req123", "--offer", "lno123",])
                .is_err()
        );

        assert!(TestMeltCli::try_parse_from([
            "test",
            "--request",
            "req123",
            "--address",
            "bc1q123",
        ])
        .is_err());
    }

    #[test]
    fn test_parse_and_validate_extra_valid_object() {
        let extra_str = r#"{"branch_id":"abc","nested":{"foo":123}}"#;
        let parsed = parse_and_validate_extra(Some(extra_str)).expect("valid object");
        assert!(parsed.is_some());
        let val = parsed.unwrap();
        assert_eq!(val["branch_id"], "abc");
        assert_eq!(val["nested"]["foo"], 123);
    }

    #[test]
    fn test_parse_and_validate_extra_none() {
        let parsed = parse_and_validate_extra(None).expect("none is ok");
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_and_validate_extra_malformed() {
        let extra_str = r#"{"branch_id":"abc""#;
        let err = parse_and_validate_extra(Some(extra_str)).expect_err("malformed must fail");
        assert!(err
            .to_string()
            .contains("--extra must be a valid JSON object"));
    }

    #[test]
    fn test_parse_and_validate_extra_non_object_rejected() {
        for invalid in ["[1, 2, 3]", "\"hello\"", "123", "true", "null"] {
            let err = parse_and_validate_extra(Some(invalid)).expect_err("non-object must fail");
            assert!(err
                .to_string()
                .contains("--extra must be a valid JSON object"));
        }
    }

    #[test]
    fn test_select_mint_for_custom_melt_filters_by_amount_and_unit() {
        let mint_a = MintUrl::from_str("https://mint-a.com").unwrap();
        let mint_b = MintUrl::from_str("https://mint-b.com").unwrap();
        let mint_c = MintUrl::from_str("https://mint-c.com").unwrap();

        let ora_unit = CurrencyUnit::Custom("ora".into());
        let sat_unit = CurrencyUnit::Sat;

        let balances = vec![
            (
                WalletKey::new(mint_a.clone(), ora_unit.clone()),
                Amount::from(10),
            ),
            (
                WalletKey::new(mint_b.clone(), sat_unit.clone()),
                Amount::from(1000),
            ),
            (
                WalletKey::new(mint_c.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
        ];

        // Should select mint_c because mint_a has only 10 (< 500) and mint_b has sat unit (!= ora)
        let selected = select_mint_for_custom_melt(
            &balances,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |_| true,
        )
        .unwrap();
        assert_eq!(selected, &mint_c);

        // When amount exceeds all matching mint balances, returns error with required amount
        let err_exceed = select_mint_for_custom_melt(
            &balances,
            &ora_unit,
            Some(Amount::from(2000)),
            "branch",
            |_| true,
        )
        .unwrap_err();
        assert_eq!(
            err_exceed.to_string(),
            "No mint with sufficient balance (>= 2000 ora) for unit ora"
        );

        // When amount is None, selects first mint with matching unit and non-zero balance
        let selected_first =
            select_mint_for_custom_melt(&balances, &ora_unit, None, "branch", |_| true).unwrap();
        assert_eq!(selected_first, &mint_a);
    }

    #[test]
    fn test_select_mint_for_custom_melt_capabilities() {
        let mint_a = MintUrl::from_str("https://mint-a.com").unwrap();
        let mint_b = MintUrl::from_str("https://mint-b.com").unwrap();

        let ora_unit = CurrencyUnit::Custom("ora".into());
        let usd_unit = CurrencyUnit::Usd;
        let branch_method = PaymentMethod::from_str("branch").unwrap();

        let branch_ora_settings = MeltMethodSettings {
            method: branch_method.clone(),
            unit: ora_unit.clone(),
            method_name: None,
            min_amount: None,
            max_amount: None,
            options: None,
        };

        let branch_usd_settings = MeltMethodSettings {
            method: branch_method.clone(),
            unit: usd_unit.clone(),
            method_name: None,
            min_amount: None,
            max_amount: None,
            options: None,
        };

        let mint_info_empty = MintInfo::default();

        let mint_info_ora = MintInfo {
            nuts: Nuts {
                nut05: Nut05Settings {
                    methods: vec![branch_ora_settings.clone()],
                    disabled: false,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let mint_info_usd = MintInfo {
            nuts: Nuts {
                nut05: Nut05Settings {
                    methods: vec![branch_usd_settings],
                    disabled: false,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let mint_info_disabled = MintInfo {
            nuts: Nuts {
                nut05: Nut05Settings {
                    methods: vec![branch_ora_settings],
                    disabled: true,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // TEST A: Two matching-unit mints. Mint A unsupported, Mint B supported.
        // Auto-selection must choose Mint B.
        let balances_a_b = vec![
            (
                WalletKey::new(mint_a.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
            (
                WalletKey::new(mint_b.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
        ];
        let selected_a = select_mint_for_custom_melt(
            &balances_a_b,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |url| {
                if url == &mint_a {
                    custom_payment_method_supported(&mint_info_empty, &branch_method, &ora_unit)
                } else if url == &mint_b {
                    custom_payment_method_supported(&mint_info_ora, &branch_method, &ora_unit)
                } else {
                    false
                }
            },
        )
        .unwrap();
        assert_eq!(selected_a, &mint_b);

        // TEST B: Both mints support branch, but Mint A has balance 10, Mint B has 1000.
        // Requested 500 must choose Mint B.
        let balances_b = vec![
            (
                WalletKey::new(mint_a.clone(), ora_unit.clone()),
                Amount::from(10),
            ),
            (
                WalletKey::new(mint_b.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
        ];
        let selected_b = select_mint_for_custom_melt(
            &balances_b,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |url| {
                if url == &mint_a || url == &mint_b {
                    custom_payment_method_supported(&mint_info_ora, &branch_method, &ora_unit)
                } else {
                    false
                }
            },
        )
        .unwrap();
        assert_eq!(selected_b, &mint_b);

        // TEST C: Mint A has branch for usd, Mint B has branch for ora.
        // Requested unit ora must choose Mint B.
        let balances_c = vec![
            (
                WalletKey::new(mint_a.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
            (
                WalletKey::new(mint_b.clone(), ora_unit.clone()),
                Amount::from(1000),
            ),
        ];
        let selected_c = select_mint_for_custom_melt(
            &balances_c,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |url| {
                if url == &mint_a {
                    custom_payment_method_supported(&mint_info_usd, &branch_method, &ora_unit)
                } else if url == &mint_b {
                    custom_payment_method_supported(&mint_info_ora, &branch_method, &ora_unit)
                } else {
                    false
                }
            },
        )
        .unwrap();
        assert_eq!(selected_c, &mint_b);

        // TEST D: Matching unit and balance exist, but no mint advertises branch.
        // Must return the capability-specific error.
        let err_d = select_mint_for_custom_melt(
            &balances_a_b,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |url| {
                if url == &mint_a || url == &mint_b {
                    custom_payment_method_supported(&mint_info_empty, &branch_method, &ora_unit)
                } else {
                    false
                }
            },
        )
        .unwrap_err();
        assert_eq!(
            err_d.to_string(),
            "No mint with sufficient balance supports payment method 'branch' for unit ora"
        );

        // TEST E: Explicitly selected mint does not support branch.
        // Must fail on that mint with specific error, must NOT silently switch to another mint.
        let err_e = validate_custom_payment_method_support(
            &mint_info_empty,
            &branch_method,
            "branch",
            &ora_unit,
            &mint_a,
        )
        .unwrap_err();
        assert_eq!(
            err_e.to_string(),
            "Payment method 'branch' is not supported by mint https://mint-a.com"
        );

        // TEST F: NUT-05 disabled mint is not a valid auto-selection candidate.
        assert!(!custom_payment_method_supported(
            &mint_info_disabled,
            &branch_method,
            &ora_unit
        ));
        let selected_f = select_mint_for_custom_melt(
            &balances_a_b,
            &ora_unit,
            Some(Amount::from(500)),
            "branch",
            |url| {
                if url == &mint_a {
                    custom_payment_method_supported(&mint_info_disabled, &branch_method, &ora_unit)
                } else if url == &mint_b {
                    custom_payment_method_supported(&mint_info_ora, &branch_method, &ora_unit)
                } else {
                    false
                }
            },
        )
        .unwrap();
        assert_eq!(selected_f, &mint_b);
    }

    #[test]
    fn test_validate_custom_payment_method_support() {
        let mint_url = MintUrl::from_str("https://mint.example.com").unwrap();
        let branch_method = PaymentMethod::from_str("branch").unwrap();
        let bolt11_method = PaymentMethod::from_str("bolt11").unwrap();
        let ora_unit = CurrencyUnit::Custom("ora".into());
        let sat_unit = CurrencyUnit::Sat;
        let usd_unit = CurrencyUnit::Usd;

        let mut mint_info = MintInfo {
            nuts: Nuts {
                nut05: Nut05Settings {
                    methods: vec![
                        MeltMethodSettings {
                            method: branch_method.clone(),
                            unit: ora_unit.clone(),
                            method_name: None,
                            min_amount: None,
                            max_amount: None,
                            options: None,
                        },
                        MeltMethodSettings {
                            method: bolt11_method.clone(),
                            unit: sat_unit.clone(),
                            method_name: None,
                            min_amount: None,
                            max_amount: None,
                            options: None,
                        },
                    ],
                    disabled: false,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // 1. Advertised custom method and matching unit succeeds
        assert!(validate_custom_payment_method_support(
            &mint_info,
            &branch_method,
            "branch",
            &ora_unit,
            &mint_url
        )
        .is_ok());

        // 2. Typo in payment method fails with clean error
        let typo_method = PaymentMethod::from_str("bbolt11").unwrap();
        let typo_err = validate_custom_payment_method_support(
            &mint_info,
            &typo_method,
            "bbolt11",
            &sat_unit,
            &mint_url,
        )
        .unwrap_err();
        assert_eq!(
            typo_err.to_string(),
            "Payment method 'bbolt11' is not supported by mint https://mint.example.com"
        );

        // 3. Supported method for different unit fails with unit-specific message
        let wrong_unit_err = validate_custom_payment_method_support(
            &mint_info,
            &branch_method,
            "branch",
            &usd_unit,
            &mint_url,
        )
        .unwrap_err();
        assert_eq!(
            wrong_unit_err.to_string(),
            "Payment method 'branch' for unit usd is not supported by mint https://mint.example.com"
        );

        // 4. Disabled NUT-05 fails
        mint_info.nuts.nut05.disabled = true;
        let disabled_err = validate_custom_payment_method_support(
            &mint_info,
            &branch_method,
            "branch",
            &ora_unit,
            &mint_url,
        )
        .unwrap_err();
        assert_eq!(
            disabled_err.to_string(),
            "Melting is disabled for mint https://mint.example.com"
        );
    }

    #[test]
    fn test_create_melt_options_checked_arithmetic_overflow() {
        let err = create_melt_options(1_000_000, None, Some(u64::MAX), "prompt").unwrap_err();
        assert_eq!(err.to_string(), "Amount overflow");
    }

    #[test]
    fn test_create_melt_options_amountless_converts_sat_to_msat() {
        let options = create_melt_options(1_000_000, None, Some(500), "prompt")
            .unwrap()
            .unwrap();
        assert_eq!(options.amount_msat(), Amount::from(500_000));
    }
}
