use anyhow::{anyhow, Result};
use cdk::nuts::PaymentRequest;
use cdk::wallet::WalletRepository;
use cdk::Amount;
use clap::Args;

use crate::utils::get_number_input;

#[derive(Args)]
pub struct PayRequestSubCommand {
    payment_request: PaymentRequest,
    /// Amount to pay (required for amountless requests)
    #[arg(short, long)]
    amount: Option<u64>,
}

pub async fn pay_request(
    wallet_repository: &WalletRepository,
    sub_command_args: &PayRequestSubCommand,
) -> Result<()> {
    let payment_request = &sub_command_args.payment_request;

    let unit = &payment_request.unit;

    let amount: Amount = match payment_request.amount {
        Some(amount) => amount,
        None => match sub_command_args.amount {
            Some(amt) => amt.into(),
            None => {
                let amount: u64 = get_number_input("Enter the amount you would like to pay")?;
                amount.into()
            }
        },
    };

    let request_mints = &payment_request.mints;

    let wallet_mints = wallet_repository.get_wallets().await;

    // Wallets where unit, balance and mint match request
    let mut matching_wallets = vec![];

    for wallet in wallet_mints.iter() {
        let balance = wallet.total_balance().await?;

        if !request_mints.is_empty() && !request_mints.contains(&wallet.mint_url) {
            continue;
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
