use std::io::{self, Write};

use anyhow::{anyhow, Result};
use cdk::nuts::PaymentRequest;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct PayRequestSubCommand {
    payment_request: PaymentRequest,
}

pub async fn pay_request(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &PayRequestSubCommand,
) -> Result<()> {
    let payment_request = &sub_command_args.payment_request;

    let unit = &payment_request.unit;

    // Determine amount: use from request or prompt user
    let amount: Amount = match payment_request.amount {
        Some(amount) => amount,
        None => {
            println!("Enter the amount you would like to pay");

            let mut user_input = String::new();
            let stdin = io::stdin();
            io::stdout().flush().unwrap();
            stdin.read_line(&mut user_input)?;

            let amount: u64 = user_input.trim().parse()?;

            amount.into()
        }
    };

    let request_mints = &payment_request.mints;

    let wallet_mints = multi_mint_wallet.get_wallets().await;

    // Wallets where unit, balance and mint match request
    let mut matching_wallets = vec![];

    for wallet in wallet_mints.iter() {
        let balance = wallet.total_balance().await?;

        if let Some(request_mints) = request_mints {
            if !request_mints.contains(&wallet.mint_url) {
                continue;
            }
        }

        if let Some(unit) = unit {
            if &wallet.unit != unit {
                continue;
            }
        }

        if balance >= amount {
            matching_wallets.push(wallet);
        }
    }

    let matching_wallet = matching_wallets
        .first()
        .ok_or_else(|| anyhow!("No wallet found that can pay this request"))?;

    matching_wallet
        .pay_request(payment_request.clone(), Some(amount))
        .await
        .map_err(|e| anyhow!(e.to_string()))
}
