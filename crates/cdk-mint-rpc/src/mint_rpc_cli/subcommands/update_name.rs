use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateNameRequest;

/// Command to update the mint's name
///
/// This command sets a new display name for the mint, which is used to identify
/// the mint in wallet applications and other client interfaces.
#[derive(Args)]
pub struct UpdateNameCommand {
    /// The new name for the mint
    name: String,
}

/// Executes the update_name command against the mint server
///
/// This function sends an RPC request to update the mint's display name.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new name to set for the mint
pub async fn update_name(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateNameCommand,
) -> Result<()> {
    let _response = client
        .update_name(Request::new(UpdateNameRequest {
            name: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}
