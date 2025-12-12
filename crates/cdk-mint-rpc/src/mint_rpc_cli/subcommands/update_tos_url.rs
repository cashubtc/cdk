use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateTosUrlRequest;

/// Command to update the mint's terms of service URL
///
/// This command sets a new terms of service URL for the mint, which is used to
/// provide the location of the terms of service for the mint.
#[derive(Args)]
pub struct UpdateTosUrlCommand {
    /// The URL to the mint's terms of service
    name: String,
}

/// Executes the update_tos_url command against the mint server
///
/// This function sends an RPC request to update the mint's terms of service URL.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new icon URL to set
pub async fn update_tos_url(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateTosUrlCommand,
) -> Result<()> {
    let _response = client
        .update_tos_url(Request::new(UpdateTosUrlRequest {
            tos_url: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}
