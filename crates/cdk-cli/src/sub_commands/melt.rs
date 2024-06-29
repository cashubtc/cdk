use std::collections::HashMap;
use std::io::Write;
use std::str::FromStr;
use std::{io, println};

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::wallet::Wallet;
use cdk::{Bolt11Invoice, UncheckedUrl};

use crate::sub_commands::balance::mint_balances;

pub async fn pay(wallets: HashMap<UncheckedUrl, Wallet>) -> Result<()> {
    let mints_amounts = mint_balances(wallets).await?;

    println!("Enter mint number to melt from");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;

    let mint_number: usize = user_input.trim().parse()?;

    if mint_number.gt(&(mints_amounts.len() - 1)) {
        bail!("Invalid mint number");
    }

    let wallet = mints_amounts[mint_number].0.clone();

    println!("Enter bolt11 invoice request");

    let mut user_input = String::new();
    let stdin = io::stdin();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut user_input)?;
    let bolt11 = Bolt11Invoice::from_str(user_input.trim())?;

    if bolt11
        .amount_milli_satoshis()
        .unwrap()
        .gt(&(<cdk::Amount as Into<u64>>::into(mints_amounts[mint_number].1) * 1000_u64))
    {
        bail!("Not enough funds");
    }
    let quote = wallet.melt_quote(bolt11.to_string(), None).await?;

    println!("{:?}", quote);

    let melt = wallet.melt(&quote.id, SplitTarget::default()).await?;

    println!("Paid invoice: {}", melt.state);
    if let Some(preimage) = melt.preimage {
        println!("Payment preimage: {}", preimage);
    }

    Ok(())
}
