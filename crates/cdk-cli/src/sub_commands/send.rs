use std::io::Write;
use std::str::FromStr;
use std::{io, println};

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::{Conditions, CurrencyUnit, PublicKey, SpendingConditions};
use cdk::wallet::Wallet;
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
    /// Publey to lock proofs to
    #[arg(short, long, action = clap::ArgAction::Append)]
    pubkey: Vec<String>,
    /// Publey to lock proofs to
    #[arg(long, action = clap::ArgAction::Append)]
    refund_keys: Vec<String>,
}

pub async fn send(wallet: Wallet, sub_command_args: &SendSubCommand) -> Result<()> {
    let mints_amounts = mint_balances(&wallet).await?;

    println!("Enter mint number to create token");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;

    let mint_number: usize = user_input.trim().parse()?;

    if mint_number.gt(&(mints_amounts.len() - 1)) {
        bail!("Invalid mint number");
    }

    let mint_url = mints_amounts[mint_number].0.clone();

    println!("Enter value of token in sats");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;
    let token_amount = Amount::from(user_input.trim().parse::<u64>()?);

    if token_amount.gt(mints_amounts[mint_number]
        .1
        .get(&CurrencyUnit::Sat)
        .unwrap())
    {
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

                tracing::debug!("{}", data_pubkey.to_string());

                Some(SpendingConditions::P2PKConditions {
                    data: data_pubkey,
                    conditions: Some(conditions),
                })
            }
        },
    };

    let token = wallet
        .send(
            &mint_url,
            CurrencyUnit::Sat,
            token_amount,
            sub_command_args.memo.clone(),
            conditions,
            &SplitTarget::default(),
        )
        .await?;

    println!("{}", token);

    Ok(())
}
