use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateContactRequest;

/// Command to add a contact method to the mint
///
/// This command adds a new contact method with associated information to the mint.
/// Contact methods allow users to reach the mint operators through various channels.
#[derive(Args, Debug)]
pub struct AddContactCommand {
    /// The contact method type (e.g., "email", "twitter", "telegram")
    method: String,
    /// The contact information for the specified method
    info: String,
}

/// Executes the add_contact command against the mint server
///
/// This function sends an RPC request to add a new contact method to the mint.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The contact method and information to add
pub async fn add_contact(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &AddContactCommand,
) -> Result<()> {
    let _response = client
        .add_contact(Request::new(UpdateContactRequest {
            method: sub_command_args.method.clone(),
            info: sub_command_args.info.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to remove a contact method from the mint
///
/// This command removes an existing contact method and its associated information
/// from the mint's available contact methods.
#[derive(Args, Debug)]
pub struct RemoveContactCommand {
    /// The contact method type to remove (e.g., "email", "twitter", "telegram")
    method: String,
    /// The specific contact information to remove for the specified method
    info: String,
}

/// Executes the remove_contact command against the mint server
///
/// This function sends an RPC request to remove an existing contact method from the mint.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The contact method and information to remove
pub async fn remove_contact(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &RemoveContactCommand,
) -> Result<()> {
    let _response = client
        .remove_contact(Request::new(UpdateContactRequest {
            method: sub_command_args.method.clone(),
            info: sub_command_args.info.clone(),
        }))
        .await?;

    Ok(())
}
