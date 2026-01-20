use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateDescriptionRequest;

/// Command to update the mint's short description
///
/// This command sets a new short description for the mint, which provides a brief
/// summary of the mint's purpose or characteristics. The short description is typically
/// displayed in wallets and client interfaces.
#[derive(Args, Debug)]
pub struct UpdateShortDescriptionCommand {
    /// The new short description text for the mint
    description: String,
}

/// Executes the update_short_description command against the mint server
///
/// This function sends an RPC request to update the mint's short description.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new short description to set
pub async fn update_short_description(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateShortDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_short_description(Request::new(UpdateDescriptionRequest {
            description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}
