use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateUrlRequest;

/// Command to add a URL to the mint's list of endpoints
///
/// This command adds a new URL to the mint's list of available endpoints.
/// Multiple URLs allow clients to access the mint through different endpoints,
/// providing redundancy and flexibility.
#[derive(Args)]
pub struct AddUrlCommand {
    /// The URL to add to the mint's endpoints
    url: String,
}

/// Executes the add_url command against the mint server
///
/// This function sends an RPC request to add a new URL to the mint's list of endpoints.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The URL to add to the mint
pub async fn add_url(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &AddUrlCommand,
) -> Result<()> {
    let _response = client
        .add_url(Request::new(UpdateUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to remove a URL from the mint's list of endpoints
///
/// This command removes an existing URL from the mint's list of available endpoints.
/// This can be used to retire endpoints that are no longer in use or available.
#[derive(Args)]
pub struct RemoveUrlCommand {
    /// The URL to remove from the mint's endpoints
    url: String,
}

/// Executes the remove_url command against the mint server
///
/// This function sends an RPC request to remove an existing URL from the mint's list of endpoints.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The URL to remove from the mint
pub async fn remove_url(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &RemoveUrlCommand,
) -> Result<()> {
    let _response = client
        .remove_url(Request::new(UpdateUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}
