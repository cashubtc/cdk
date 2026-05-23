use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::{amount_for_offer, Amount, MSAT_IN_SAT};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{CurrencyUnit, MeltOptions, PayjoinV2, PaymentMethod};
use cdk::wallet::WalletRepository;
use cdk::Bolt11Invoice;
use cdk_common::payjoin::{parse_bip21_amount_to_sats, payjoin_v2_from_bip77_endpoint};
use cdk_common::wallet::WalletKey;
use clap::{Args, ValueEnum};
use lightning::offers::offer::Offer;

use crate::utils::{get_number_input, get_or_create_wallet, get_user_input};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum PaymentType {
    /// BOLT11 invoice
    Bolt11,
    /// BOLT12 offer
    Bolt12,
    /// Bip353
    Bip353,
    /// Onchain Bitcoin address
    Onchain,
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

#[derive(Args)]
pub struct MeltSubCommand {
    /// Use Multi-Path Payment (split payment across multiple mints, BOLT11 only)
    #[arg(short, long, conflicts_with = "mint_url")]
    mpp: bool,
    /// Mint URL to use for melting
    #[arg(long, conflicts_with = "mpp")]
    mint_url: Option<String>,
    /// Payment method (bolt11, bolt12, bip353, or onchain)
    #[arg(long, default_value = "bolt11")]
    method: PaymentType,
    /// BOLT11 invoice to pay (for bolt11 method)
    #[arg(long, conflicts_with_all = ["offer", "address"])]
    invoice: Option<String>,
    /// BOLT12 offer to pay (for bolt12 method)
    #[arg(long, conflicts_with_all = ["invoice", "address"])]
    offer: Option<String>,
    /// BIP353 address for --method bip353, or Bitcoin address/full bitcoin: URI for
    /// --method onchain
    #[arg(long, conflicts_with_all = ["invoice", "offer"])]
    address: Option<String>,
    /// Bitcoin network to use for BIP353 (bitcoin, testnet, signet, regtest)
    #[arg(long, default_value = "bitcoin")]
    network: BitcoinNetwork,
    /// Amount in sats for amountless payments or onchain melts
    #[arg(long)]
    amount: Option<u64>,
    /// MPP split entry in the form <mint_url>=<amount_sats>; repeat for multiple mints
    #[arg(long = "mpp-split", value_name = "MINT_URL=AMOUNT", action = clap::ArgAction::Append, requires = "mpp")]
    mpp_split: Vec<String>,
    /// Destination Payjoin instructions as NUT-31 JSON or a bitcoin: Payjoin URI, for onchain melts
    #[arg(long, value_name = "JSON_OR_URI")]
    payjoin: Option<String>,
}

fn validate_args(sub_command_args: &MeltSubCommand) -> Result<()> {
    if sub_command_args.payjoin.is_some()
        && !matches!(sub_command_args.method, PaymentType::Onchain)
    {
        bail!("--payjoin can only be used with --method onchain");
    }

    Ok(())
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
            let user_amount = match cli_amount_sat {
                Some(amount_sat) => amount_sat * MSAT_IN_SAT,
                None => get_number_input::<u64>(prompt)? * MSAT_IN_SAT,
            };

            if user_amount > available_funds {
                bail!("Not enough funds");
            }

            Ok(Some(MeltOptions::new_amountless(user_amount)))
        }
    }
}

fn input_or_prompt(arg: Option<&String>, prompt: &str) -> Result<String> {
    match arg {
        Some(value) => Ok(value.clone()),
        None => get_user_input(prompt),
    }
}

#[derive(Debug, Clone, Default)]
struct OnchainPaymentInput {
    address: Option<String>,
    amount_sat: Option<u64>,
    payjoin: Option<PayjoinV2>,
}

impl OnchainPaymentInput {
    fn merge(self, other: Self) -> Result<Self> {
        Ok(Self {
            address: merge_optional("onchain address", self.address, other.address)?,
            amount_sat: merge_optional("onchain amount", self.amount_sat, other.amount_sat)?,
            payjoin: merge_optional("payjoin", self.payjoin, other.payjoin)?,
        })
    }
}

fn merge_optional<T>(field: &str, left: Option<T>, right: Option<T>) -> Result<Option<T>>
where
    T: PartialEq + std::fmt::Debug,
{
    match (left, right) {
        (Some(left), Some(right)) if left != right => {
            bail!("Conflicting {field} values: {left:?} and {right:?}")
        }
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn parse_onchain_input_arg(value: &str) -> Result<OnchainPaymentInput> {
    if value.to_ascii_lowercase().starts_with("bitcoin:") {
        parse_bitcoin_payjoin_uri(value)
    } else {
        Ok(OnchainPaymentInput {
            address: Some(value.to_string()),
            ..Default::default()
        })
    }
}

fn parse_payjoin_arg(value: &str) -> Result<OnchainPaymentInput> {
    if value.to_ascii_lowercase().starts_with("bitcoin:") {
        parse_bitcoin_payjoin_uri(value)
    } else {
        Ok(OnchainPaymentInput {
            payjoin: Some(serde_json::from_str::<PayjoinV2>(value)?),
            ..Default::default()
        })
    }
}

fn parse_bitcoin_payjoin_uri(value: &str) -> Result<OnchainPaymentInput> {
    let uri = url::Url::parse(value)?;
    if uri.scheme() != "bitcoin" {
        bail!("Expected a bitcoin: URI");
    }

    let address = normalize_onchain_address(uri.path());
    if address.is_empty() {
        bail!("bitcoin: URI is missing an onchain address");
    }

    let mut amount_sat = None;
    let mut endpoint = None;

    for (key, value) in uri.query_pairs() {
        match key.as_ref() {
            "amount" => amount_sat = Some(parse_bip21_amount_sat(&value)?),
            "pj" => endpoint = Some(value.into_owned()),
            "pjos" if !matches!(value.as_ref(), "0" | "1") => {
                bail!("Invalid pjos value '{}', expected 0 or 1", value);
            }
            "pjos" => {}
            _ => {}
        }
    }

    let payjoin = match endpoint {
        Some(endpoint) => Some(onchain_payjoin_from_endpoint(endpoint)?),
        None => None,
    };

    Ok(OnchainPaymentInput {
        address: Some(address),
        amount_sat,
        payjoin,
    })
}

fn normalize_onchain_address(address: &str) -> String {
    let lowercase = address.to_ascii_lowercase();
    if lowercase.starts_with("bc1")
        || lowercase.starts_with("tb1")
        || lowercase.starts_with("bcrt1")
    {
        lowercase
    } else {
        address.to_string()
    }
}

fn parse_bip21_amount_sat(amount: &str) -> Result<u64> {
    parse_bip21_amount_to_sats(amount).map_err(Into::into)
}

fn onchain_payjoin_from_endpoint(endpoint: String) -> Result<PayjoinV2> {
    payjoin_v2_from_bip77_endpoint(&endpoint).map_err(Into::into)
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
    validate_args(sub_command_args)?;

    // Check total balance for the requested unit
    let balances_by_unit = wallet_repository.total_balance().await?;
    let total_balance = balances_by_unit.get(unit).copied().unwrap_or(Amount::ZERO);
    if total_balance == Amount::ZERO {
        bail!("No funds available for unit {}", unit);
    }

    // Handle MPP mode separately
    if sub_command_args.mpp {
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
                    index, key.mint_url, key.unit, balance, unit
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
    match sub_command_args.method {
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
                let required_amount = bolt11
                    .amount_milli_satoshis()
                    .map(|a| Amount::from(a / MSAT_IN_SAT))
                    .unwrap_or(Amount::ZERO);

                balances
                    .into_iter()
                    .find(|(_, balance)| *balance >= required_amount)
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
            println!("  Quote ID: {}", quote.id);
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
                println!("Payment preimage: {}", preimage);
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
            println!("  Quote ID: {}", quote.id);
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
                println!("Payment preimage: {}", preimage);
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
            println!("  Quote ID: {}", quote.id);
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
                println!("Payment preimage: {}", preimage);
            }
        }
        PaymentType::Onchain => {
            let mut onchain_input = sub_command_args
                .address
                .as_deref()
                .map(parse_onchain_input_arg)
                .transpose()?
                .unwrap_or_default();
            if let Some(payjoin) = sub_command_args.payjoin.as_deref() {
                onchain_input = onchain_input.merge(parse_payjoin_arg(payjoin)?)?;
            }

            let onchain_address = match onchain_input.address {
                Some(address) => address,
                None => get_user_input("Enter onchain address")?,
            };

            let amount_sat = match merge_optional(
                "onchain amount",
                onchain_input.amount_sat,
                sub_command_args.amount,
            )? {
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
                .quote_onchain_melt_options_with_payjoin(
                    &onchain_address,
                    melt_amount,
                    None,
                    onchain_input.payjoin,
                )
                .await?;

            let selected_quote = select_onchain_quote(&quote_options)?;
            let quote = wallet.select_onchain_melt_quote(selected_quote).await?;

            println!("Melt quote selected:");
            println!("  Quote ID: {}", quote.id);
            println!("  Amount: {}", quote.amount);
            println!("  Fee Reserve: {}", quote.fee_reserve);
            println!("  Expiry: {}", quote.expiry);
            if let Some(estimated_blocks) = quote.estimated_blocks {
                println!("  Estimated Blocks: {}", estimated_blocks);
            }
            if let Some(payjoin) = quote.payjoin.as_ref() {
                println!("  Payjoin: {}", serde_json::to_string(payjoin)?);
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
                println!("Payment proof: {}", payment_proof);
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
    if !matches!(sub_command_args.method, PaymentType::Bolt11) {
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
                i, key.mint_url, key.unit, balance, unit
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

        println!("  {} - Quote ID: {}", mint_url, quote.id);
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
            mint_url,
            melted.amount(),
            melted.fee_paid()
        );
        total_paid += melted.amount();
        total_fees += melted.fee_paid();

        if let Some(preimage) = melted.payment_proof() {
            println!("    Preimage: {}", preimage);
        }
    }

    println!("\nTotal paid: {} {}", total_paid, unit);
    println!("Total fees: {} {}", total_fees, unit);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYJOIN_URI: &str = "bitcoin:tb1qhe0dkl8tfkrp9nnufq0v0nl46yramxmzwvs83r?amount=0.00004&pjos=0&pj=HTTPS://PAYJO.IN/E73HSW759WNES%23EX12XHZ26S-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG";

    fn melt_sub_command(method: PaymentType, payjoin: Option<String>) -> MeltSubCommand {
        MeltSubCommand {
            mpp: false,
            mint_url: None,
            method,
            invoice: None,
            offer: None,
            address: None,
            network: BitcoinNetwork::Bitcoin,
            amount: None,
            mpp_split: Vec::new(),
            payjoin,
        }
    }

    #[test]
    fn payjoin_requires_onchain_method() {
        for method in [
            PaymentType::Bolt11,
            PaymentType::Bolt12,
            PaymentType::Bip353,
        ] {
            let args = melt_sub_command(method, Some("{}".to_string()));
            let err = validate_args(&args).expect_err("payjoin should require onchain method");

            assert_eq!(
                err.to_string(),
                "--payjoin can only be used with --method onchain"
            );
        }
    }

    #[test]
    fn onchain_method_allows_payjoin() {
        let args = melt_sub_command(PaymentType::Onchain, Some("{}".to_string()));

        validate_args(&args).expect("onchain payjoin should pass validation");
    }

    #[test]
    fn default_method_rejects_payjoin() {
        let args = melt_sub_command(PaymentType::Bolt11, Some("{}".to_string()));
        let err = validate_args(&args).expect_err("default method should reject payjoin");

        assert_eq!(
            err.to_string(),
            "--payjoin can only be used with --method onchain"
        );
    }

    #[test]
    fn parses_bitcoin_payjoin_uri_for_onchain_melt() {
        let input = parse_bitcoin_payjoin_uri(PAYJOIN_URI).expect("parse payjoin uri");
        let payjoin = input.payjoin.expect("payjoin params");

        assert_eq!(
            input.address.as_deref(),
            Some("tb1qhe0dkl8tfkrp9nnufq0v0nl46yramxmzwvs83r")
        );
        assert_eq!(input.amount_sat, Some(4_000));
        assert_eq!(payjoin.endpoint, "https://payjo.in/E73HSW759WNES");
        assert_eq!(
            payjoin.ohttp_keys.to_string(),
            "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ"
        );
        assert_eq!(
            payjoin.receiver_key.to_string(),
            "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
        assert_eq!(payjoin.expires_at, 1_780_854_353);
    }

    #[test]
    fn onchain_uri_amount_conflicts_are_rejected() {
        let input = parse_bitcoin_payjoin_uri(PAYJOIN_URI).expect("parse payjoin uri");
        let err = merge_optional("onchain amount", input.amount_sat, Some(5_000))
            .expect_err("conflicting amount should fail");

        assert!(err.to_string().contains("Conflicting onchain amount"));
    }
}
