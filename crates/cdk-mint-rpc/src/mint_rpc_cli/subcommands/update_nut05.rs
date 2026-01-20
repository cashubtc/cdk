use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::{MeltMethodOptions, UpdateNut05Request};

/// Command to update NUT-05 (melt process) settings for the mint
///
/// NUT-05 defines how tokens are melted (redeemed) in exchange for external payments.
/// This command allows configuring the available token units, payment methods, amounts,
/// and other settings for the melting process.
#[derive(Args, Debug)]
pub struct UpdateNut05Command {
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
    /// Whether this melt method is disabled (true) or enabled (false)
    #[arg(long)]
    disabled: Option<bool>,
    /// Whether amountless bolt11 invoices are allowed
    #[arg(long)]
    amountless: Option<bool>,
}

/// Executes the update_nut05 command against the mint server
///
/// This function sends an RPC request to update the mint's NUT-05 settings for token melting.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The NUT-05 configuration parameters to update
pub async fn update_nut05(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateNut05Command,
) -> Result<()> {
    // Create options if amountless is set
    let options = sub_command_args
        .amountless
        .map(|amountless| MeltMethodOptions { amountless });

    let _response = client
        .update_nut05(Request::new(UpdateNut05Request {
            method: sub_command_args.method.clone(),
            unit: sub_command_args.unit.clone(),
            disabled: sub_command_args.disabled,
            min_amount: sub_command_args.min_amount,
            max_amount: sub_command_args.max_amount,
            options,
        }))
        .await?;

    Ok(())
}
