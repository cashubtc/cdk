use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::payment_method::{Bolt11MeltMethodOptions, UpdateMeltMethodRequest};
use crate::InterceptedPaymentMethodServiceClient;

/// Command to update the settings of a melt (NUT-05) payment method
///
/// NUT-05 defines how tokens are melted (redeemed) for external payments. This command
/// allows configuring the available token units, payment methods, amounts, and other settings
/// for the melting process.
#[derive(Args, Debug)]
pub struct UpdateMeltMethodCommand {
    /// The token unit type (e.g., "sat")
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    /// The payment method for melting (e.g., "bolt11" for Lightning payments)
    #[arg(short, long)]
    #[arg(default_value = "bolt11")]
    method: String,
    /// The minimum amount that can be melted in a single transaction
    #[arg(long)]
    min_amount: Option<u64>,
    /// The maximum amount that can be melted in a single transaction
    #[arg(long)]
    max_amount: Option<u64>,
    /// Human-readable name for this payment method
    #[arg(long)]
    method_name: Option<String>,
    /// Whether the mint supports paying amountless Lightning invoices
    #[arg(long)]
    amountless: Option<bool>,
}

/// Executes the update_melt_method command against the mint server
///
/// This function sends an RPC request to update the settings of one of the mint's
/// NUT-05 payment methods.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The NUT-05 configuration parameters to update
pub async fn update_melt_method(
    client: &mut InterceptedPaymentMethodServiceClient,
    sub_command_args: &UpdateMeltMethodCommand,
) -> Result<()> {
    // Create options if amountless is set
    let options = sub_command_args
        .amountless
        .map(|amountless| Bolt11MeltMethodOptions { amountless });

    let response = client
        .update_melt_method(Request::new(UpdateMeltMethodRequest {
            method: sub_command_args.method.clone(),
            unit: sub_command_args.unit.clone(),
            min_amount: sub_command_args.min_amount,
            max_amount: sub_command_args.max_amount,
            options,
            method_name: sub_command_args.method_name.clone(),
        }))
        .await?
        .into_inner();

    println!("Melt method settings:");
    println!("  Unit: {}", response.unit);
    println!("  Method: {}", response.method);
    println!(
        "  Min amount: {}",
        response
            .min_amount
            .map_or("none".to_string(), |a| a.to_string())
    );
    println!(
        "  Max amount: {}",
        response
            .max_amount
            .map_or("none".to_string(), |a| a.to_string())
    );
    println!(
        "  Method name: {}",
        response.method_name.unwrap_or_else(|| "none".to_string())
    );
    if let Some(options) = response.options {
        println!("  Bolt11 amountless: {}", options.amountless);
    }

    Ok(())
}
