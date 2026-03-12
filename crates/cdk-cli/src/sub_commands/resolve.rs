use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{resolve_bip353_payment_instruction, WalletRepository};
use clap::Args;

use crate::sub_commands::melt::BitcoinNetwork;
use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct ResolveSubCommand {
    /// BIP353 address to resolve
    #[arg(long)]
    address: String,
    /// Optional mint URL to select a wallet/connector for resolution
    #[arg(long)]
    mint_url: Option<String>,
    /// Bitcoin network to use for BIP353 (bitcoin, testnet, signet, regtest)
    #[arg(long, default_value = "bitcoin")]
    network: BitcoinNetwork,
}

fn print_list<T: std::fmt::Display>(label: &str, values: &[T]) {
    println!("{label}: {}", values.len());
    for value in values {
        println!("  - {}", value);
    }
}

pub async fn resolve(
    wallet_repository: &WalletRepository,
    sub_command_args: &ResolveSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let wallet = if let Some(mint_url) = &sub_command_args.mint_url {
        let mint_url = MintUrl::from_str(mint_url)?;
        get_or_create_wallet(wallet_repository, &mint_url, unit).await?
    } else {
        wallet_repository
            .get_wallets()
            .await
            .into_iter()
            .find(|wallet| wallet.unit == *unit)
            .ok_or_else(|| anyhow!("No wallet available for unit {}", unit))?
    };

    let client = wallet.mint_connector();
    let parsed = resolve_bip353_payment_instruction(
        &client,
        &sub_command_args.address,
        sub_command_args.network.into(),
    )
    .await?;

    println!(
        "Resolved BIP353 payment instruction for {}",
        sub_command_args.address
    );
    println!(
        "Description: {}",
        parsed.description.as_deref().unwrap_or("None")
    );
    println!(
        "Amount (msats): {}",
        parsed
            .amount_msats
            .map(|a| a.to_string())
            .unwrap_or_else(|| "None".to_string())
    );
    println!("Configurable amount: {}", parsed.is_configurable_amount);

    if parsed.cashu_requests.is_empty() {
        println!("Cashu payment requests: 0");
    } else {
        println!("Cashu payment requests: {}", parsed.cashu_requests.len());
        for (index, request) in parsed.cashu_requests.iter().enumerate() {
            println!("  Request {}", index + 1);
            println!("    Encoded: {}", request.to_bech32_string()?);
            println!(
                "    Payment ID: {}",
                request.payment_id.as_deref().unwrap_or("None")
            );
            println!(
                "    Amount: {}",
                request
                    .amount
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "None".to_string())
            );
            println!(
                "    Unit: {}",
                request
                    .unit
                    .as_ref()
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "None".to_string())
            );
            println!(
                "    Accepted mints: {}",
                if request.mints.is_empty() {
                    "None".to_string()
                } else {
                    request
                        .mints
                        .iter()
                        .map(|m| m.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            println!("    Transports: {}", request.transports.len());
            for transport in &request.transports {
                println!("      - {} ({})", transport.target, transport._type);
            }
            println!(
                "    Next: cdk-cli pay-request '{}'{}",
                request.to_bech32_string()?,
                if request.amount.is_none() {
                    " --amount <amount>"
                } else {
                    ""
                }
            );
        }
    }

    print_list("BOLT12 offers", &parsed.bolt12_offers);
    print_list("BOLT11 invoices", &parsed.bolt11_invoices);
    print_list("On-chain addresses", &parsed.onchain_addresses);

    if !parsed.bolt12_offers.is_empty() {
        println!(
            "Next: cdk-cli melt --method bip353 --address '{}'",
            sub_command_args.address
        );
    }

    Ok(())
}
