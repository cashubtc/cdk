use anyhow::{anyhow, Result};
use cdk::nuts::PaymentRequest;
use cdk::wallet::payment_request::{RequestPayment, RequestPaymentLimits};
use cdk::wallet::WalletManager;
use cdk::{Amount, Error};
use clap::Args;

use crate::terminal::escape_control;
use crate::utils::{get_number_input, get_user_input};

#[derive(Args)]
pub struct PayRequestSubCommand {
    payment_request: PaymentRequest,
    /// Amount to pay (required for amountless requests)
    #[arg(short, long)]
    amount: Option<u64>,
    /// Maximum receiver-selected method fee (`mf`) to accept
    #[arg(long)]
    max_method_fee: Option<u64>,
    /// Maximum total wallet debit, including method and mint input fees
    #[arg(long)]
    max_total_amount: Option<u64>,
    /// Confirm without prompting; requires explicit method-fee and total-debit limits
    #[arg(
        long,
        requires_all = ["max_method_fee", "max_total_amount"]
    )]
    yes: bool,
}

pub async fn pay_request(
    wallet_manager: &WalletManager,
    sub_command_args: &PayRequestSubCommand,
) -> Result<()> {
    let payment_request = &sub_command_args.payment_request;

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

    let plan = wallet_manager
        .plan_request_payment(
            RequestPayment::new(payment_request.clone())
                .with_amount(amount)
                .with_limits(RequestPaymentLimits {
                    maximum_method_fee: sub_command_args.max_method_fee.map(Amount::from),
                    maximum_total_amount: sub_command_args.max_total_amount.map(Amount::from),
                }),
        )
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    println!("Payment request:");
    if let Some(description) = &payment_request.description {
        println!("  Description: {}", escape_control(description));
    }
    if let Some(payment_id) = &payment_request.payment_id {
        println!("  Payment ID: {}", escape_control(payment_id));
    }
    println!("  Mint: {}", plan.wallet().mint_url);
    println!(
        "  Requested amount: {} {}",
        plan.requested_amount(),
        plan.wallet().unit
    );
    match plan.method() {
        Some(method) => println!("  Selected method: {method}"),
        None => println!("  Selected method: unrestricted"),
    }
    println!(
        "  Method fee (mf): {} {}",
        plan.method_fee(),
        plan.wallet().unit
    );
    println!(
        "  Mint input fee: {} {}",
        plan.input_fee(),
        plan.wallet().unit
    );
    println!(
        "  Total wallet debit: {} {}",
        plan.total_amount(),
        plan.wallet().unit
    );
    if payment_request.nut10.is_some() {
        println!("  NUT-10 spending condition: required");
    }

    if !sub_command_args.yes {
        let response = match get_user_input("Confirm payment? [y/N]") {
            Ok(response) => response,
            Err(err) => {
                plan.cancel()
                    .await
                    .map_err(|cancel_err| anyhow!(cancel_err.to_string()))?;
                return Err(err);
            }
        };

        if !matches!(response.to_ascii_lowercase().as_str(), "y" | "yes") {
            plan.cancel().await.map_err(|e| anyhow!(e.to_string()))?;
            println!("Payment canceled");
            return Ok(());
        }
    }

    match plan.execute().await {
        Ok(_) => Ok(()),
        Err(Error::PaymentRequestDeliveryFailed {
            operation_id,
            source,
        }) => Err(anyhow!(
            "Payment token was created but delivery failed: {source}. \
             Pending send operation: {operation_id}. Do not pay the request again; \
             reclaim it with Wallet::reclaim_send if the receiver has not claimed it."
        )),
        Err(error) => Err(anyhow!(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk::wallet::WalletManagerBuilder;
    use cdk_sqlite::wallet::memory;

    use super::*;

    #[tokio::test]
    async fn unitless_fixed_amount_request_is_invalid() {
        let seed = [0u8; 64];
        let localstore = Arc::new(memory::empty().await.expect("memory store"));
        let wallet_manager = WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed(seed)
            .build()
            .await
            .expect("wallet manager");

        let payment_request = PaymentRequest {
            payment_id: None,
            amount: Some(Amount::from(0_u64)),
            unit: None,
            single_use: None,
            mints: vec![],
            mint_preferred: None,
            supported_methods: vec![],
            description: None,
            transports: vec![],
            nut10: None,
        };
        let sub_command_args = PayRequestSubCommand {
            payment_request,
            amount: None,
            max_method_fee: None,
            max_total_amount: None,
            yes: false,
        };

        let result = pay_request(&wallet_manager, &sub_command_args)
            .await
            .expect_err("unitless fixed-amount request must be rejected");

        assert!(
            result.to_string().contains("Invalid payment request"),
            "unexpected error: {result}"
        );
    }
}
