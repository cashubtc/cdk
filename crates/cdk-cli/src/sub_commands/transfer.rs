use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::wallet::transfer::{
    CrossMintTransferAmount, CrossMintTransferOutcome, CrossMintTransferRequest,
};
use cdk::wallet::{WalletIdentity, WalletManager};
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
    /// Amount received at the target; source-side Lightning and input fees are additional
    #[arg(short, long, conflicts_with = "full_balance")]
    amount: Option<u64>,
    /// Transfer all available balance from source mint
    #[arg(long, conflicts_with = "amount")]
    full_balance: bool,
}

/// Helper function to select a mint from available mints
async fn select_mint(
    wallet_manager: &WalletManager,
    prompt: &str,
    exclude_mint: Option<&MintUrl>,
    unit: &cdk::nuts::CurrencyUnit,
) -> Result<MintUrl> {
    let balances = wallet_manager.available_balances().await?;

    // Filter out excluded mint if provided
    let available_mints: Vec<_> = balances
        .iter()
        .filter(|(key, _)| exclude_mint.is_none_or(|excluded| &key.mint_url != excluded))
        .collect();

    if available_mints.is_empty() {
        bail!("No available mints found");
    }

    println!("\nAvailable mints:");
    for (i, (key, balance)) in available_mints.iter().enumerate() {
        println!(
            "  {}: {} ({}) - {} {}",
            i, key.mint_url, key.unit, balance, unit
        );
    }

    let mint_number: usize = get_number_input(prompt)?;
    available_mints
        .get(mint_number)
        .map(|(key, _)| key.mint_url.clone())
        .ok_or_else(|| anyhow::anyhow!("Invalid mint number"))
}

pub async fn transfer(
    wallet_manager: &WalletManager,
    sub_command_args: &TransferSubCommand,
    unit: &cdk::nuts::CurrencyUnit,
) -> Result<()> {
    // Check total balance for the requested unit
    let balances_by_unit = wallet_manager.balance_totals().await?;
    let total_balance = balances_by_unit.get(unit).copied().unwrap_or(Amount::ZERO);
    if total_balance == Amount::ZERO {
        bail!("No funds available for unit {}", unit);
    }

    // Get source mint URL either from args or by prompting user
    let source_mint_url = if let Some(source_mint) = &sub_command_args.source_mint {
        let url = MintUrl::from_str(source_mint)?;
        // Verify the mint is in the wallet
        if !wallet_manager.contains_mint(&url).await {
            bail!(
                "Source mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints and let user select source
        select_mint(
            wallet_manager,
            "Enter source mint number to transfer from",
            None,
            unit,
        )
        .await?
    };

    // Get target mint URL either from args or by prompting user
    let target_mint_url = if let Some(target_mint) = &sub_command_args.target_mint {
        let url = MintUrl::from_str(target_mint)?;
        // Verify the mint is in the wallet
        if !wallet_manager.contains_mint(&url).await {
            bail!(
                "Target mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints (excluding source) and let user select target
        select_mint(
            wallet_manager,
            "Enter target mint number to transfer to",
            Some(&source_mint_url),
            unit,
        )
        .await?
    };

    // Ensure source and target are different
    if source_mint_url == target_mint_url {
        bail!("Source and target mints must be different");
    }

    // Check source mint balance
    let balances = wallet_manager.available_balances().await?;
    let source_key = WalletIdentity::new(source_mint_url.clone(), unit.clone());
    let source_balance = balances.get(&source_key).copied().unwrap_or(Amount::ZERO);

    if source_balance == Amount::ZERO {
        bail!("Source mint has no balance to transfer");
    }

    // Get source and target wallets
    let source_identity = WalletIdentity {
        mint_url: source_mint_url.clone(),
        unit: unit.clone(),
    };
    let target_identity = WalletIdentity {
        mint_url: target_mint_url.clone(),
        unit: unit.clone(),
    };
    let source_wallet = wallet_manager.wallet(source_identity.clone()).await?;
    let target_wallet = wallet_manager.wallet(target_identity.clone()).await?;

    // Determine transfer mode and execute
    if sub_command_args.full_balance {
        println!(
            "\nTransferring full balance ({} {}) from {} to {}...",
            source_balance, unit, source_mint_url, target_mint_url
        );

        let mut source_balance_after = source_balance;
        let mut received = Amount::ZERO;
        let mut completed_transfers = 0_u64;

        loop {
            let plan = match wallet_manager
                .plan_cross_mint_transfer(CrossMintTransferRequest {
                    source: source_identity.clone(),
                    destination: target_identity.clone(),
                    amount: CrossMintTransferAmount::Maximum,
                    metadata: Default::default(),
                })
                .await
            {
                Ok(plan) => plan,
                Err(cdk::Error::InsufficientFunds) if completed_transfers > 0 => break,
                Err(error) => return Err(error.into()),
            };
            let receipt = match plan.execute().await? {
                CrossMintTransferOutcome::Completed(receipt) => receipt,
                CrossMintTransferOutcome::ClaimPending(pending) => {
                    bail!(
                        "Source payment completed, but destination claim is pending for operation {}: {}",
                        pending.operation_id,
                        pending.error_message
                    );
                }
            };
            received = received
                .checked_add(receipt.amount)
                .ok_or(cdk::Error::AmountOverflow)?;

            let next_source_balance = source_wallet.balance().await?.available;
            if next_source_balance >= source_balance_after {
                bail!(
                    "Full-balance transfer made no progress; source balance remains {} {}",
                    next_source_balance,
                    unit
                );
            }

            source_balance_after = next_source_balance;
            completed_transfers += 1;

            if source_balance_after == Amount::ZERO {
                break;
            }
        }

        let target_balance_after = target_wallet.balance().await?.available;
        let amount_sent = source_balance
            .checked_sub(source_balance_after)
            .unwrap_or(Amount::ZERO);

        println!("\nTransfer completed successfully!");
        println!("Amount sent: {} {}", amount_sent, unit);
        println!("Amount received: {} {}", received, unit);
        let fees_paid = amount_sent.checked_sub(received).unwrap_or(Amount::ZERO);
        if fees_paid > Amount::ZERO {
            println!("Fees paid: {} {}", fees_paid, unit);
        }
        if source_balance_after > Amount::ZERO {
            println!(
                "Remaining balance below transfer limits: {} {}",
                source_balance_after, unit
            );
        }
        println!("\nUpdated balances:");
        println!(
            "  Source mint ({}): {} {}",
            source_mint_url, source_balance_after, unit
        );
        println!(
            "  Target mint ({}): {} {}",
            target_mint_url, target_balance_after, unit
        );
    } else {
        let amount = match sub_command_args.amount {
            Some(amt) => Amount::from(amt),
            None => Amount::from(get_number_input::<u64>(&format!(
                "Enter amount to transfer in {}",
                unit
            ))?),
        };

        if source_balance < amount {
            bail!(
                "Insufficient funds in source mint. Available: {} {}, Required: {} {}",
                source_balance,
                unit,
                amount,
                unit
            );
        }

        println!(
            "\nTransferring {} {} from {} to {}...",
            amount, unit, source_mint_url, target_mint_url
        );

        let plan = match wallet_manager
            .plan_cross_mint_transfer(CrossMintTransferRequest {
                source: source_identity,
                destination: target_identity,
                amount: CrossMintTransferAmount::Exact(amount),
                metadata: Default::default(),
            })
            .await
        {
            Ok(plan) => plan,
            Err(cdk::Error::InsufficientFunds) => {
                bail!(
                    "Insufficient funds in source mint after Lightning and input fees. Available: {} {}, destination amount: {} {}. Reduce the amount or use --full-balance.",
                    source_balance,
                    unit,
                    amount,
                    unit
                );
            }
            Err(error) => return Err(error.into()),
        };
        println!("Maximum source-side fee: {} {}", plan.maximum_fee(), unit);
        let received = match plan.execute().await? {
            CrossMintTransferOutcome::Completed(receipt) => receipt.amount,
            CrossMintTransferOutcome::ClaimPending(pending) => {
                bail!(
                    "Source payment completed, but destination claim is pending for operation {}: {}",
                    pending.operation_id,
                    pending.error_message
                );
            }
        };

        let source_balance_after = source_wallet.balance().await?.available;
        let target_balance_after = target_wallet.balance().await?.available;
        let amount_sent = source_balance
            .checked_sub(source_balance_after)
            .unwrap_or(Amount::ZERO);

        println!("\nTransfer completed successfully!");
        println!("Amount sent: {} {}", amount_sent, unit);
        println!("Amount received: {} {}", received, unit);
        let fees_paid = amount_sent.checked_sub(received).unwrap_or(Amount::ZERO);
        if fees_paid > Amount::ZERO {
            println!("Fees paid: {} {}", fees_paid, unit);
        }
        println!("\nUpdated balances:");
        println!(
            "  Source mint ({}): {} {}",
            source_mint_url, source_balance_after, unit
        );
        println!(
            "  Target mint ({}): {} {}",
            target_mint_url, target_balance_after, unit
        );
    }

    Ok(())
}
