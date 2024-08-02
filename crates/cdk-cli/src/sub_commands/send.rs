use std::io;
use std::io::Write;
use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::{Conditions, CurrencyUnit, PublicKey, SpendingConditions};
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::types::SendKind;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;
use clap::Args;

use crate::sub_commands::balance::mint_balances;

#[derive(Args)]
pub struct SendSubCommand {
    /// Token Memo
    #[arg(short, long)]
    memo: Option<String>,
    /// Preimage
    #[arg(long)]
    preimage: Option<String>,
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
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn send(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &SendSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let mints_amounts = mint_balances(multi_mint_wallet, &unit).await?;

    println!("Enter mint number to create token");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;

    let mint_number: usize = user_input.trim().parse()?;

    if mint_number.gt(&(mints_amounts.len() - 1)) {
        bail!("Invalid mint number");
    }

    println!("Enter value of token in sats");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;
    let token_amount = Amount::from(user_input.trim().parse::<u64>()?);

    if token_amount.gt(&mints_amounts[mint_number].1) {
        bail!("Not enough funds");
    }

    let conditions = match &sub_command_args.preimage {
        Some(preimage) => {
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
            )
            .unwrap();

            Some(SpendingConditions::new_htlc(
                preimage.clone(),
                Some(conditions),
            )?)
        }
        None => match sub_command_args.pubkey.is_empty() {
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
                )
                .unwrap();

                Some(SpendingConditions::P2PKConditions {
                    data: data_pubkey,
                    conditions: Some(conditions),
                })
            }
        },
    };

    let wallet = mints_amounts[mint_number].0.clone();
    let wallet = multi_mint_wallet
        .get_wallet(&WalletKey::new(wallet, unit))
        .await
        .expect("Known wallet");

    let send_kind = match (sub_command_args.offline, sub_command_args.tolerance) {
        (true, Some(amount)) => SendKind::OfflineTolerance(Amount::from(amount)),
        (true, None) => SendKind::OfflineExact,
        (false, Some(amount)) => SendKind::OnlineTolerance(Amount::from(amount)),
        (false, None) => SendKind::OnlineExact,
    };

    let token = wallet
        .send(
            token_amount,
            sub_command_args.memo.clone(),
            conditions,
            &SplitTarget::default(),
            &send_kind,
            sub_command_args.include_fee,
        )
        .await?;

    match sub_command_args.v3 {
        true => {
            let token = token;

            println!("{}", token.to_v3_string());
        }
        false => {
            println!("{}", token);
        }
    }

    Ok(())
}
