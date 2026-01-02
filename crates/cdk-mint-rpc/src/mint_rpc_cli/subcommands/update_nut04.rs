use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::{MintMethodOptions, UpdateNut04Request};

/// Command to update NUT-04 (mint process) settings for the mint
///
/// NUT-04 defines how tokens are minted in exchange for external payments. This command
/// allows configuring the available token units, payment methods, amounts, and other settings
/// for the minting process.
#[derive(Args, Debug)]
pub struct UpdateNut04Command {
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
    /// Whether this mint method is disabled (true) or enabled (false)
    #[arg(long)]
    disabled: Option<bool>,
    /// Whether the mint should include description fields in Lightning invoices
    #[arg(long)]
    description: Option<bool>,
}

/// Executes the update_nut04 command against the mint server
///
/// This function sends an RPC request to update the mint's NUT-04 settings for token minting.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The NUT-04 configuration parameters to update
pub async fn update_nut04(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateNut04Command,
) -> Result<()> {
    // Create options if description is set
    let options = sub_command_args
        .description
        .map(|description| MintMethodOptions { description });

    let _response = client
        .update_nut04(Request::new(UpdateNut04Request {
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
