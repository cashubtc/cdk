use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::wallet::multi_mint_wallet::TransferMode;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;
use clap::Args;

use crate::utils::get_number_input;

#[derive(Args)]
pub struct TransferSubCommand {
    /// Source mint URL to transfer from (optional - will prompt if not provided)
    #[arg(long)]
    source_mint: Option<String>,
    /// Target mint URL to transfer to (optional - will prompt if not provided)
    #[arg(long)]
    target_mint: Option<String>,
    /// Amount to transfer (optional - will prompt if not provided)
    #[arg(short, long, conflicts_with = "full_balance")]
    amount: Option<u64>,
    /// Transfer all available balance from source mint
    #[arg(long, conflicts_with = "amount")]
    full_balance: bool,
}

/// Helper function to select a mint from available mints
async fn select_mint(
    multi_mint_wallet: &MultiMintWallet,
    prompt: &str,
    exclude_mint: Option<&MintUrl>,
) -> Result<MintUrl> {
    let balances = multi_mint_wallet.get_balances().await?;

    // Filter out excluded mint if provided
    let available_mints: Vec<_> = balances
        .iter()
        .filter(|(url, _)| exclude_mint.is_none_or(|excluded| url != &excluded))
        .collect();

    if available_mints.is_empty() {
        bail!("No available mints found");
    }

    println!("\nAvailable mints:");
    for (i, (mint_url, balance)) in available_mints.iter().enumerate() {
        println!(
            "  {}: {} - {} {}",
            i,
            mint_url,
            balance,
            multi_mint_wallet.unit()
        );
    }

    let mint_number: usize = get_number_input(prompt)?;
    available_mints
        .get(mint_number)
        .map(|(url, _)| (*url).clone())
        .ok_or_else(|| anyhow::anyhow!("Invalid mint number"))
}

pub async fn transfer(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &TransferSubCommand,
) -> Result<()> {
    // Check total balance across all wallets
    let total_balance = multi_mint_wallet.total_balance().await?;
    if total_balance == Amount::ZERO {
        bail!("No funds available");
    }

    // Get source mint URL either from args or by prompting user
    let source_mint_url = if let Some(source_mint) = &sub_command_args.source_mint {
        let url = MintUrl::from_str(source_mint)?;
        // Verify the mint is in the wallet
        if !multi_mint_wallet.has_mint(&url).await {
            bail!(
                "Source mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints and let user select source
        select_mint(
            multi_mint_wallet,
            "Enter source mint number to transfer from",
            None,
        )
        .await?
    };

    // Get target mint URL either from args or by prompting user
    let target_mint_url = if let Some(target_mint) = &sub_command_args.target_mint {
        let url = MintUrl::from_str(target_mint)?;
        // Verify the mint is in the wallet
        if !multi_mint_wallet.has_mint(&url).await {
            bail!(
                "Target mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints (excluding source) and let user select target
        select_mint(
            multi_mint_wallet,
            "Enter target mint number to transfer to",
            Some(&source_mint_url),
        )
        .await?
    };

    // Ensure source and target are different
    if source_mint_url == target_mint_url {
        bail!("Source and target mints must be different");
    }

    // Check source mint balance
    let balances = multi_mint_wallet.get_balances().await?;
    let source_balance = balances
        .get(&source_mint_url)
        .copied()
        .unwrap_or(Amount::ZERO);

    if source_balance == Amount::ZERO {
        bail!("Source mint has no balance to transfer");
    }

    // Determine transfer mode based on user input
    let transfer_mode = if sub_command_args.full_balance {
        println!(
            "\nTransferring full balance ({} {}) from {} to {}...",
            source_balance,
            multi_mint_wallet.unit(),
            source_mint_url,
            target_mint_url
        );
        TransferMode::FullBalance
    } else {
        let amount = match sub_command_args.amount {
            Some(amt) => Amount::from(amt),
            None => Amount::from(get_number_input::<u64>(&format!(
                "Enter amount to transfer in {}",
                multi_mint_wallet.unit()
            ))?),
        };

        if source_balance < amount {
            bail!(
                "Insufficient funds in source mint. Available: {} {}, Required: {} {}",
                source_balance,
                multi_mint_wallet.unit(),
                amount,
                multi_mint_wallet.unit()
            );
        }

        println!(
            "\nTransferring {} {} from {} to {}...",
            amount,
            multi_mint_wallet.unit(),
            source_mint_url,
            target_mint_url
        );
        TransferMode::ExactReceive(amount)
    };

    // Perform the transfer
    let transfer_result = multi_mint_wallet
        .transfer(&source_mint_url, &target_mint_url, transfer_mode)
        .await?;

    println!("\nTransfer completed successfully!");
    println!(
        "Amount sent: {} {}",
        transfer_result.amount_sent,
        multi_mint_wallet.unit()
    );
    println!(
        "Amount received: {} {}",
        transfer_result.amount_received,
        multi_mint_wallet.unit()
    );
    if transfer_result.fees_paid > Amount::ZERO {
        println!(
            "Fees paid: {} {}",
            transfer_result.fees_paid,
            multi_mint_wallet.unit()
        );
    }
    println!("\nUpdated balances:");
    println!(
        "  Source mint ({}): {} {}",
        source_mint_url,
        transfer_result.source_balance_after,
        multi_mint_wallet.unit()
    );
    println!(
        "  Target mint ({}): {} {}",
        target_mint_url,
        transfer_result.target_balance_after,
        multi_mint_wallet.unit()
    );

    Ok(())
}
