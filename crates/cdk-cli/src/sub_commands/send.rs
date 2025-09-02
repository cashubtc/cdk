use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::nuts::{Conditions, CurrencyUnit, PublicKey, SpendingConditions};
use cdk::wallet::types::SendKind;
use cdk::wallet::{MultiMintWallet, SendMemo, SendOptions};
use cdk::Amount;
use clap::Args;

use crate::utils::get_number_input;

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
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn send(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &SendSubCommand,
) -> Result<()> {
    let _unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let token_amount = Amount::from(get_number_input::<u64>("Enter value of token in sats")?);

    // Check total balance across all wallets for this unit
    let total_balance = multi_mint_wallet.total_balance().await?;
    if total_balance < token_amount {
        return Err(anyhow!(
            "Insufficient funds. Total balance: {}, Required: {}",
            total_balance,
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
                        .map(|p| PublicKey::from_str(p).unwrap())
                        .collect(),
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p).unwrap())
                        .collect(),
                ),
            };

            let conditions = Conditions::new(
                sub_command_args.locktime,
                pubkeys,
                refund_keys,
                sub_command_args.required_sigs,
                None,
                None,
            )
            .unwrap();

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
                        .map(|p| PublicKey::from_str(p).unwrap())
                        .collect(),
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p).unwrap())
                        .collect(),
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
                    .map(|p| PublicKey::from_str(p).unwrap())
                    .collect();

                let refund_keys: Vec<PublicKey> = sub_command_args
                    .refund_keys
                    .iter()
                    .map(|p| PublicKey::from_str(p).unwrap())
                    .collect();

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

    // Create MultiMintSendOptions
    let multi_mint_options = cdk::wallet::multi_mint_wallet::MultiMintSendOptions {
        allow_transfer: false, // Don't automatically transfer between mints
        max_transfer_amount: None,
        allowed_mints: Vec::new(),
        excluded_mints: Vec::new(),
        send_options: send_options.clone(),
    };

    // Use the new unified interface
    let token = if let Some(mint_url) = &sub_command_args.mint_url {
        // User specified a mint, use that specific wallet
        let mint_url = cdk::mint_url::MintUrl::from_str(mint_url)?;
        let prepared = multi_mint_wallet
            .prepare_send(mint_url, token_amount, multi_mint_options)
            .await?;

        // Confirm the prepared send (single mint)
        let memo = send_options.memo.clone();
        prepared.confirm(memo).await?
    } else {
        // Let the wallet automatically select the best mint
        // First, get balances to find a mint with sufficient funds
        let balances = multi_mint_wallet.get_balances().await?;

        // Find a mint with sufficient balance
        let mint_url = balances
            .into_iter()
            .find(|(_, balance)| *balance >= token_amount)
            .map(|(mint_url, _)| mint_url)
            .ok_or_else(|| {
                anyhow::anyhow!("No mint has sufficient balance for the requested amount")
            })?;

        let prepared = multi_mint_wallet
            .prepare_send(mint_url, token_amount, multi_mint_options)
            .await?;

        // Confirm the prepared send (multi mint)
        let memo = send_options.memo.clone();
        prepared.confirm(memo).await?
    };

    match sub_command_args.v3 {
        true => {
            let token = token;

            println!("{}", token.to_v3_string());
        }
        false => {
            println!("{token}");
        }
    }

    Ok(())
}
