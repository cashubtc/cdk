use std::collections::HashMap;
use std::io::Write;
use std::{io, println};

use anyhow::{bail, Result};
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;

pub async fn check_spent(wallet: Wallet) -> Result<()> {
    let mints_amounts: Vec<(UncheckedUrl, HashMap<_, _>)> =
        wallet.mint_balances().await?.into_iter().collect();

    for (i, (mint, amount)) in mints_amounts.iter().enumerate() {
        println!("{}: {}, {:?} sats", i, mint, amount);
    }

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

    let proofs = wallet.get_proofs(mint_url.clone()).await?.unwrap();

    let send_proofs = wallet.check_proofs_spent(mint_url, proofs.to_vec()).await?;

    for proof in send_proofs {
        println!("{:#?}", proof);
    }

    Ok(())
}
