use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateDescriptionRequest;

/// Command to update the mint's long description
///
/// This command sets a new long description for the mint, which provides detailed
/// information about the mint's purpose, operation, and policies.
#[derive(Args)]
pub struct UpdateLongDescriptionCommand {
    /// The new long description text for the mint
    description: String,
}

/// Executes the update_long_description command against the mint server
///
/// This function sends an RPC request to update the mint's long description.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new long description to set
pub async fn update_long_description(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateLongDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_long_description(Request::new(UpdateDescriptionRequest {
            description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}
