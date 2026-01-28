use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{Conditions, CurrencyUnit, PublicKey, SpendingConditions};
use cdk::wallet::types::SendKind;
use cdk::wallet::{SendMemo, SendOptions, WalletRepository};
use cdk::Amount;
use clap::Args;

use crate::utils::{get_number_input, get_or_create_wallet};

#[derive(Args)]
pub struct SendSubCommand {
    /// Token Memo
    #[arg(short, long)]
    memo: Option<String>,
    /// Preimage
    #[arg(long, conflicts_with = "hash")]
    preimage: Option<String>,
    /// Hash for HTLC (alternative to preimage)
    #[arg(long, conflicts_with = "preimage")]
    hash: Option<String>,
    /// Required number of signatures
    #[arg(long)]
    required_sigs: Option<u64>,
    /// Locktime before refund keys can be used
    #[arg(short, long)]
    locktime: Option<u64>,
    /// Pubkey to lock proofs to
    #[arg(short, long, action = clap::ArgAction::Append)]
    pubkey: Vec<String>,
    /// Refund keys that can be used after locktime
    #[arg(long, action = clap::ArgAction::Append)]
    refund_keys: Vec<String>,
    /// Token as V3 token
    #[arg(short, long)]
    v3: bool,
    /// Should the send be offline only
    #[arg(short, long)]
    offline: bool,
    /// Include fee to redeem in token
    #[arg(short, long)]
    include_fee: bool,
    /// Amount willing to overpay to avoid a swap
    #[arg(short, long)]
    tolerance: Option<u64>,
    /// Mint URL to use for sending
    #[arg(long)]
    mint_url: Option<String>,
    /// Amount to send
    #[arg(short, long)]
    amount: Option<u64>,
}

pub async fn send(
    wallet_repository: &WalletRepository,
    sub_command_args: &SendSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    // Determine which mint to use for sending BEFORE asking for amount
    let selected_mint = if let Some(mint_url) = &sub_command_args.mint_url {
        MintUrl::from_str(mint_url)?
    } else {
        // Get all mints with their balances
        let balances_map = wallet_repository.get_balances().await?;
        if balances_map.is_empty() {
            return Err(anyhow!("No mints available in the wallet"));
        }

        let balances_vec: Vec<(MintUrl, Amount)> = balances_map.into_iter().collect();

        // If only one mint exists, automatically select it
        if balances_vec.len() == 1 {
            balances_vec[0].0.clone()
        } else {
            // Display all mints with their balances and let user select
            println!("\nAvailable mints and balances:");
            for (index, (mint_url, balance)) in balances_vec.iter().enumerate() {
                println!("  {}: {} - {} {}", index, mint_url, balance, unit);
            }

            loop {
                let selection: usize = get_number_input("Enter mint number to send from")?;

                if let Some((mint_url, _)) = balances_vec.get(selection) {
                    break mint_url.clone();
                }

                println!("Invalid selection, please try again.");
            }
        }
    };

    let token_amount = match sub_command_args.amount {
        Some(amount) => Amount::from(amount),
        None => Amount::from(get_number_input::<u64>(&format!(
            "Enter value of token in {}",
            unit
        ))?),
    };

    // Get or create wallet for the selected mint
    let wallet = get_or_create_wallet(wallet_repository, &selected_mint, unit).await?;

    // Check wallet balance
    let balance = wallet.total_balance().await?;
    if balance < token_amount {
        return Err(anyhow!(
            "Insufficient funds. Wallet balance: {}, Required: {}",
            balance,
            token_amount
        ));
    }

    let conditions = match (&sub_command_args.preimage, &sub_command_args.hash) {
        (Some(_), Some(_)) => {
            // This case shouldn't be reached due to Clap's conflicts_with attribute
            unreachable!("Both preimage and hash were provided despite conflicts_with attribute")
        }
        (Some(preimage), None) => {
            let pubkeys = match sub_command_args.pubkey.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .pubkey
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let conditions = Conditions::new(
                sub_command_args.locktime,
                pubkeys,
                refund_keys,
                sub_command_args.required_sigs,
                None,
                None,
            )?;

            Some(SpendingConditions::new_htlc(
                preimage.clone(),
                Some(conditions),
            )?)
        }
        (None, Some(hash)) => {
            let pubkeys = match sub_command_args.pubkey.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .pubkey
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let conditions = Conditions::new(
                sub_command_args.locktime,
                pubkeys,
                refund_keys,
                sub_command_args.required_sigs,
                None,
                None,
            )?;

            Some(SpendingConditions::new_htlc_hash(hash, Some(conditions))?)
        }
        (None, None) => match sub_command_args.pubkey.is_empty() {
            true => None,
            false => {
                let pubkeys: Vec<PublicKey> = sub_command_args
                    .pubkey
                    .iter()
                    .map(|p| PublicKey::from_str(p))
                    .collect::<Result<Vec<_>, _>>()?;

                let refund_keys: Vec<PublicKey> = sub_command_args
                    .refund_keys
                    .iter()
                    .map(|p| PublicKey::from_str(p))
                    .collect::<Result<Vec<_>, _>>()?;

                let refund_keys = (!refund_keys.is_empty()).then_some(refund_keys);

                let data_pubkey = pubkeys[0];
                let pubkeys = pubkeys[1..].to_vec();
                let pubkeys = (!pubkeys.is_empty()).then_some(pubkeys);

                let conditions = Conditions::new(
                    sub_command_args.locktime,
                    pubkeys,
                    refund_keys,
                    sub_command_args.required_sigs,
                    None,
                    None,
                )?;

                Some(SpendingConditions::P2PKConditions {
                    data: data_pubkey,
                    conditions: Some(conditions),
                })
            }
        },
    };

    let send_kind = match (sub_command_args.offline, sub_command_args.tolerance) {
        (true, Some(amount)) => SendKind::OfflineTolerance(Amount::from(amount)),
        (true, None) => SendKind::OfflineExact,
        (false, Some(amount)) => SendKind::OnlineTolerance(Amount::from(amount)),
        (false, None) => SendKind::OnlineExact,
    };

    let send_options = SendOptions {
        memo: sub_command_args.memo.clone().map(|memo| SendMemo {
            memo,
            include_memo: true,
        }),
        send_kind,
        include_fee: sub_command_args.include_fee,
        conditions,
        ..Default::default()
    };

    // Prepare and confirm the send using the individual wallet
    let prepared = wallet
        .prepare_send(token_amount, send_options.clone())
        .await?;
    let memo = send_options.memo;
    let token = prepared.confirm(memo).await?;

    match sub_command_args.v3 {
        true => {
            println!("{}", token.to_v3_string());
        }
        false => {
            println!("{token}");
        }
    }

    Ok(())
}
