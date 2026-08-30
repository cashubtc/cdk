use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::{KnownMethod, ProofsMethods};
use cdk::nuts::PaymentMethod;
use cdk::wallet::{MeltConfirmOptions, WalletRepository};
use cdk::Amount;
use cdk_common::wallet::WalletKey;
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

fn fixed_transfer_fee_error(
    source_balance: Amount,
    amount: Amount,
    fee_reserve: Amount,
    required_before_input_fees: Amount,
    unit: &cdk::nuts::CurrencyUnit,
) -> anyhow::Error {
    let maximum_before_input_fees = source_balance
        .checked_sub(fee_reserve)
        .unwrap_or(Amount::ZERO);

    anyhow::anyhow!(
        "Insufficient funds in source mint. Available: {} {}, Transfer amount: {} {}, \
         Lightning fee reserve: {} {}, Minimum before input fees: {} {}. Input fees may \
         increase the total; reduce the amount (maximum before input fees: {} {}) or use \
         --full-balance.",
        source_balance,
        unit,
        amount,
        unit,
        fee_reserve,
        unit,
        required_before_input_fees,
        unit,
        maximum_before_input_fees,
        unit
    )
}

fn ensure_fixed_transfer_fee_reserve(
    source_balance: Amount,
    amount: Amount,
    fee_reserve: Amount,
    unit: &cdk::nuts::CurrencyUnit,
) -> Result<Amount> {
    let required_before_input_fees = amount
        .checked_add(fee_reserve)
        .ok_or(cdk::Error::AmountOverflow)?;

    if source_balance < required_before_input_fees {
        return Err(fixed_transfer_fee_error(
            source_balance,
            amount,
            fee_reserve,
            required_before_input_fees,
            unit,
        ));
    }

    Ok(required_before_input_fees)
}

/// Helper function to select a mint from available mints
async fn select_mint(
    wallet_repository: &WalletRepository,
    prompt: &str,
    exclude_mint: Option<&MintUrl>,
    unit: &cdk::nuts::CurrencyUnit,
) -> Result<MintUrl> {
    let balances = wallet_repository.get_balances().await?;

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
    wallet_repository: &WalletRepository,
    sub_command_args: &TransferSubCommand,
    unit: &cdk::nuts::CurrencyUnit,
) -> Result<()> {
    // Check total balance for the requested unit
    let balances_by_unit = wallet_repository.total_balance().await?;
    let total_balance = balances_by_unit.get(unit).copied().unwrap_or(Amount::ZERO);
    if total_balance == Amount::ZERO {
        bail!("No funds available for unit {}", unit);
    }

    // Get source mint URL either from args or by prompting user
    let source_mint_url = if let Some(source_mint) = &sub_command_args.source_mint {
        let url = MintUrl::from_str(source_mint)?;
        // Verify the mint is in the wallet
        if !wallet_repository.has_mint(&url).await {
            bail!(
                "Source mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints and let user select source
        select_mint(
            wallet_repository,
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
        if !wallet_repository.has_mint(&url).await {
            bail!(
                "Target mint {} is not in the wallet. Please add it first.",
                url
            );
        }
        url
    } else {
        // Show available mints (excluding source) and let user select target
        select_mint(
            wallet_repository,
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
    let balances = wallet_repository.get_balances().await?;
    let source_key = WalletKey::new(source_mint_url.clone(), unit.clone());
    let source_balance = balances.get(&source_key).copied().unwrap_or(Amount::ZERO);

    if source_balance == Amount::ZERO {
        bail!("Source mint has no balance to transfer");
    }

    // Get source and target wallets
    let source_wallet = wallet_repository.get_wallet(&source_mint_url, unit).await?;
    let target_wallet = wallet_repository.get_wallet(&target_mint_url, unit).await?;

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
            let quote = match source_wallet
                .cross_mint_transfer_quote_max(&target_wallet)
                .await
            {
                Ok(quote) => quote,
                Err(cdk::Error::InsufficientFunds) if completed_transfers > 0 => break,
                Err(error) => return Err(error.into()),
            };
            let source_proofs = source_wallet.get_unspent_proofs().await?;
            let prepared = source_wallet
                .prepare_melt_proofs(&quote.melt_quote.id, source_proofs, HashMap::new())
                .await?;
            prepared
                .confirm_with_options(MeltConfirmOptions::skip_swap())
                .await?;
            let received_proofs = target_wallet
                .mint(&quote.mint_quote.id, SplitTarget::default(), None)
                .await?;
            received = received
                .checked_add(received_proofs.total_amount()?)
                .ok_or(cdk::Error::AmountOverflow)?;

            let next_source_balance = source_wallet.total_balance().await?;
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

        let target_balance_after = target_wallet.total_balance().await?;
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

        let mint_quote = target_wallet
            .mint_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                Some(amount),
                None,
                None,
            )
            .await?;
        let melt_quote = source_wallet
            .melt_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &mint_quote.request,
                None,
                None,
            )
            .await?;
        let required_before_input_fees = ensure_fixed_transfer_fee_reserve(
            source_balance,
            amount,
            melt_quote.fee_reserve,
            unit,
        )?;
        let prepared = match source_wallet
            .prepare_melt(&melt_quote.id, HashMap::new())
            .await
        {
            Ok(prepared) => prepared,
            Err(cdk::Error::InsufficientFunds) => {
                return Err(fixed_transfer_fee_error(
                    source_balance,
                    amount,
                    melt_quote.fee_reserve,
                    required_before_input_fees,
                    unit,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        prepared.confirm().await?;
        let received_proofs = target_wallet
            .mint(&mint_quote.id, SplitTarget::default(), None)
            .await?;
        let received = received_proofs.total_amount()?;

        let source_balance_after = source_wallet.total_balance().await?;
        let target_balance_after = target_wallet.total_balance().await?;
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

#[cfg(test)]
mod tests {
    use cdk::nuts::CurrencyUnit;

    use super::*;

    #[test]
    fn fixed_transfer_requires_fee_headroom_beyond_requested_amount() {
        let error = ensure_fixed_transfer_fee_reserve(
            Amount::from(1_000),
            Amount::from(1_000),
            Amount::from(9),
            &CurrencyUnit::Sat,
        )
        .expect_err("fee reserve should make the transfer unaffordable");

        let message = error.to_string();
        assert!(message.contains("Lightning fee reserve: 9 sat"));
        assert!(message.contains("Minimum before input fees: 1009 sat"));
        assert!(message.contains("--full-balance"));
    }

    #[test]
    fn fixed_transfer_accepts_balance_covering_amount_and_fee_reserve() {
        ensure_fixed_transfer_fee_reserve(
            Amount::from(1_009),
            Amount::from(1_000),
            Amount::from(9),
            &CurrencyUnit::Sat,
        )
        .expect("amount and fee reserve should fit");
    }
}
