use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateIconUrlRequest;

/// Command to update the mint's icon URL
///
/// This command sets a new icon URL for the mint, which is used to visually
/// identify the mint in wallet applications and other client interfaces.
#[derive(Args, Debug)]
pub struct UpdateIconUrlCommand {
    /// The URL to the mint's icon image
    name: String,
}

/// Executes the update_icon_url command against the mint server
///
/// This function sends an RPC request to update the mint's icon URL.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new icon URL to set
pub async fn update_icon_url(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateIconUrlCommand,
) -> Result<()> {
    let _response = client
        .update_icon_url(Request::new(UpdateIconUrlRequest {
            icon_url: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}
