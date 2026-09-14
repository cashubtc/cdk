use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::info::{
    AddContactRequest, AddUrlRequest, GetInfoRequest, RemoveContactRequest, RemoveUrlRequest,
    UpdateIconUrlRequest, UpdateLongDescriptionRequest, UpdateMotdRequest, UpdateNameRequest,
    UpdateShortDescriptionRequest, UpdateTosUrlRequest,
};
use crate::InterceptedMintInfoServiceClient;

/// Executes the get_info command against the mint server
///
/// This function fetches the mint's public metadata and prints it.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
pub async fn get_info(client: &mut InterceptedMintInfoServiceClient) -> Result<()> {
    let response = client.get_info(Request::new(GetInfoRequest {})).await?;
    let info = response.into_inner();

    println!(
        "name:             {}",
        info.name.unwrap_or("None".to_string())
    );
    println!(
        "version:          {}",
        info.version.unwrap_or("None".to_string())
    );
    println!(
        "description:      {}",
        info.description.unwrap_or("None".to_string())
    );
    println!(
        "long description: {}",
        info.long_description.unwrap_or("None".to_string())
    );
    println!("motd: {}", info.motd.unwrap_or("None".to_string()));
    println!("icon_url: {}", info.icon_url.unwrap_or("None".to_string()));
    println!("tos_url: {}", info.tos_url.unwrap_or("None".to_string()));

    for url in info.urls {
        println!("mint_url: {url}");
    }

    for contact in info.contact {
        println!("method: {}, info: {}", contact.method, contact.info);
    }

    Ok(())
}

/// Command to update the mint's name
///
/// This command sets a new display name for the mint, which is used to identify
/// the mint in wallet applications and other client interfaces.
#[derive(Args, Debug)]
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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateNameCommand,
) -> Result<()> {
    let _response = client
        .update_name(Request::new(UpdateNameRequest {
            name: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to update the mint's message of the day
///
/// This command sets a new message of the day (MOTD) for the mint, which can be used
/// to communicate important announcements, updates, or status information to users.
#[derive(Args, Debug)]
pub struct UpdateMotdCommand {
    /// The new message of the day text
    motd: String,
}

/// Executes the update_motd command against the mint server
///
/// This function sends an RPC request to update the mint's message of the day.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new message of the day to set
pub async fn update_motd(
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateMotdCommand,
) -> Result<()> {
    let _response = client
        .update_motd(Request::new(UpdateMotdRequest {
            motd: sub_command_args.motd.clone(),
        }))
        .await?;

    Ok(())
}

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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateShortDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_short_description(Request::new(UpdateShortDescriptionRequest {
            description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to update the mint's long description
///
/// This command sets a new long description for the mint, which provides detailed
/// information about the mint's purpose, operation, and policies.
#[derive(Args, Debug)]
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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateLongDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_long_description(Request::new(UpdateLongDescriptionRequest {
            long_description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}

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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateIconUrlCommand,
) -> Result<()> {
    let _response = client
        .update_icon_url(Request::new(UpdateIconUrlRequest {
            icon_url: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to update the mint's terms of service URL
#[derive(Args, Debug)]
pub struct UpdateTosUrlCommand {
    /// The URL to the mint's terms of service
    url: String,
}

/// Executes the update_tos_url command against the mint server
pub async fn update_tos_url(
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &UpdateTosUrlCommand,
) -> Result<()> {
    let _response = client
        .update_tos_url(Request::new(UpdateTosUrlRequest {
            tos_url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to add a URL to the mint's list of endpoints
///
/// This command adds a new URL to the mint's list of available endpoints.
/// Multiple URLs allow clients to access the mint through different endpoints,
/// providing redundancy and flexibility.
#[derive(Args, Debug)]
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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &AddUrlCommand,
) -> Result<()> {
    let _response = client
        .add_url(Request::new(AddUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}

/// Command to remove a URL from the mint's list of endpoints
///
/// This command removes an existing URL from the mint's list of available endpoints.
/// This can be used to retire endpoints that are no longer in use or available.
#[derive(Args, Debug)]
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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &RemoveUrlCommand,
) -> Result<()> {
    let _response = client
        .remove_url(Request::new(RemoveUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}

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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &AddContactCommand,
) -> Result<()> {
    let _response = client
        .add_contact(Request::new(AddContactRequest {
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
    client: &mut InterceptedMintInfoServiceClient,
    sub_command_args: &RemoveContactCommand,
) -> Result<()> {
    let _response = client
        .remove_contact(Request::new(RemoveContactRequest {
            method: sub_command_args.method.clone(),
            info: sub_command_args.info.clone(),
        }))
        .await?;

    Ok(())
}
