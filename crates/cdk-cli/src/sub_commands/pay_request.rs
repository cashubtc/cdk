use anyhow::{anyhow, Result};
use cdk::nuts::{CurrencyUnit, PaymentRequest};
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

    let unit = payment_request.unit.clone().unwrap_or(CurrencyUnit::Sat);

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

        if wallet.unit != unit {
            continue;
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk::mint_url::MintUrl;
    use cdk::wallet::WalletRepositoryBuilder;
    use cdk_sqlite::wallet::memory;

    use super::*;

    #[tokio::test]
    async fn unitless_fixed_amount_request_defaults_to_sat_wallet_selection() {
        let seed = [0u8; 64];
        let localstore = Arc::new(memory::empty().await.expect("memory store"));
        let wallet_repository = WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed(seed)
            .build()
            .await
            .expect("wallet repository");

        let mint_url =
            MintUrl::from_str("https://nonexistent.example.invalid").expect("valid mint url");
        wallet_repository
            .create_wallet(mint_url, CurrencyUnit::Usd, None)
            .await
            .expect("wallet");

        let payment_request = PaymentRequest {
            payment_id: None,
            amount: Some(Amount::from(0_u64)),
            unit: None,
            single_use: None,
            mints: vec![],
            description: None,
            transports: vec![],
            nut10: None,
        };
        let sub_command_args = PayRequestSubCommand {
            payment_request,
            amount: None,
        };

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            pay_request(&wallet_repository, &sub_command_args),
        )
        .await
        .expect("pay_request should not hang")
        .expect_err("usd wallet must not match unitless fixed-amount request");

        assert!(
            result.to_string().contains("No wallet found"),
            "unexpected error: {result}"
        );
    }
}
