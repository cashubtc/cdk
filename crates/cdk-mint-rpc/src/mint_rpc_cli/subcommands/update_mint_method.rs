use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::payment_method::{Bolt11MintMethodOptions, UpdateMintMethodRequest};
use crate::InterceptedPaymentMethodServiceClient;

/// Command to update the settings of a mint (NUT-04) payment method
///
/// NUT-04 defines how tokens are minted in exchange for external payments. This command
/// allows configuring the available token units, payment methods, amounts, and other settings
/// for the minting process.
#[derive(Args, Debug)]
pub struct UpdateMintMethodCommand {
    /// The token unit type (e.g., "sat")
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    /// The payment method for minting (e.g., "bolt11" for Lightning payments)
    #[arg(short, long)]
    #[arg(default_value = "bolt11")]
    method: String,
    /// The minimum amount that can be minted in a single transaction
    #[arg(long)]
    min_amount: Option<u64>,
    /// The maximum amount that can be minted in a single transaction
    #[arg(long)]
    max_amount: Option<u64>,
    /// Human-readable name for this payment method
    #[arg(long)]
    method_name: Option<String>,
    /// Whether the mint should include description fields in Lightning invoices
    #[arg(long)]
    description: Option<bool>,
}

/// Executes the update_mint_method command against the mint server
///
/// This function sends an RPC request to update the settings of one of the mint's
/// NUT-04 payment methods.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The NUT-04 configuration parameters to update
pub async fn update_mint_method(
    client: &mut InterceptedPaymentMethodServiceClient,
    sub_command_args: &UpdateMintMethodCommand,
) -> Result<()> {
    // Create options if description is set
    let options = sub_command_args
        .description
        .map(|description| Bolt11MintMethodOptions { description });

    let response = client
        .update_mint_method(Request::new(UpdateMintMethodRequest {
            method: sub_command_args.method.clone(),
            unit: sub_command_args.unit.clone(),
            min_amount: sub_command_args.min_amount,
            max_amount: sub_command_args.max_amount,
            options,
            method_name: sub_command_args.method_name.clone(),
        }))
        .await?
        .into_inner();

    println!("Mint method settings:");
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
        println!("  Bolt11 description: {}", options.description);
    }

    Ok(())
}
