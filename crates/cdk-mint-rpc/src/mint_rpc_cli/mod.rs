//! Command-line interface helpers for the mint management RPC client.

/// Subcommands for cli
pub mod subcommands;

use anyhow::Result;
use clap::Subcommand;

use crate::{GetInfoRequest, InterceptedCdkMintClient};

/// Immediate mint-management operations exposed by the management RPC CLI.
#[derive(Debug, Subcommand)]
pub enum ManagementCommand {
    /// Get public mint information and issuance totals.
    GetInfo,
    /// Update the mint's message of the day.
    UpdateMotd(subcommands::UpdateMotdCommand),
    /// Update the mint's short description.
    UpdateShortDescription(subcommands::UpdateShortDescriptionCommand),
    /// Update the mint's long description.
    UpdateLongDescription(subcommands::UpdateLongDescriptionCommand),
    /// Update the mint's name.
    UpdateName(subcommands::UpdateNameCommand),
    /// Update the mint's icon URL.
    UpdateIconUrl(subcommands::UpdateIconUrlCommand),
    /// Update the mint's terms-of-service URL.
    UpdateTosUrl(subcommands::UpdateTosUrlCommand),
    /// Add a public mint URL.
    AddUrl(subcommands::AddUrlCommand),
    /// Remove a public mint URL.
    RemoveUrl(subcommands::RemoveUrlCommand),
    /// Add mint contact information.
    AddContact(subcommands::AddContactCommand),
    /// Remove mint contact information.
    RemoveContact(subcommands::RemoveContactCommand),
    /// Update NUT-04 mint method settings.
    UpdateNut04(subcommands::UpdateNut04Command),
    /// Update NUT-05 melt method settings.
    UpdateNut05(subcommands::UpdateNut05Command),
    /// Update quote time-to-live settings.
    UpdateQuoteTtl(subcommands::UpdateQuoteTtlCommand),
    /// Get quote time-to-live settings.
    GetQuoteTtl,
    /// Update the state of a NUT-04 quote.
    UpdateNut04QuoteState(subcommands::UpdateNut04QuoteCommand),
    /// Rotate to the next mint keyset.
    RotateNextKeyset(subcommands::RotateNextKeysetCommand),
}

/// Dispatches a management RPC CLI command against a connected client.
pub async fn dispatch(
    client: &mut InterceptedCdkMintClient,
    command: &ManagementCommand,
) -> Result<()> {
    match command {
        ManagementCommand::GetInfo => {
            let info = client.get_info(GetInfoRequest {}).await?.into_inner();
            println!(
                "name:             {}",
                info.name.as_deref().unwrap_or("None")
            );
            println!(
                "version:          {}",
                info.version.as_deref().unwrap_or("None")
            );
            println!(
                "description:      {}",
                info.description.as_deref().unwrap_or("None")
            );
            println!(
                "long description: {}",
                info.long_description.as_deref().unwrap_or("None")
            );
            println!("motd: {}", info.motd.as_deref().unwrap_or("None"));
            println!("icon_url: {}", info.icon_url.as_deref().unwrap_or("None"));
            println!("tos_url: {}", info.tos_url.as_deref().unwrap_or("None"));
            for url in info.urls {
                println!("mint_url: {url}");
            }
            for contact in info.contact {
                println!("method: {}, info: {}", contact.method, contact.info);
            }
            println!("total issued:     {} sat", info.total_issued);
            println!("total redeemed:   {} sat", info.total_redeemed);
        }
        ManagementCommand::UpdateMotd(arguments) => {
            subcommands::update_motd(client, arguments).await?;
        }
        ManagementCommand::UpdateShortDescription(arguments) => {
            subcommands::update_short_description(client, arguments).await?;
        }
        ManagementCommand::UpdateLongDescription(arguments) => {
            subcommands::update_long_description(client, arguments).await?;
        }
        ManagementCommand::UpdateName(arguments) => {
            subcommands::update_name(client, arguments).await?;
        }
        ManagementCommand::UpdateIconUrl(arguments) => {
            subcommands::update_icon_url(client, arguments).await?;
        }
        ManagementCommand::UpdateTosUrl(arguments) => {
            subcommands::update_tos_url(client, arguments).await?;
        }
        ManagementCommand::AddUrl(arguments) => {
            subcommands::add_url(client, arguments).await?;
        }
        ManagementCommand::RemoveUrl(arguments) => {
            subcommands::remove_url(client, arguments).await?;
        }
        ManagementCommand::AddContact(arguments) => {
            subcommands::add_contact(client, arguments).await?;
        }
        ManagementCommand::RemoveContact(arguments) => {
            subcommands::remove_contact(client, arguments).await?;
        }
        ManagementCommand::UpdateNut04(arguments) => {
            subcommands::update_nut04(client, arguments).await?;
        }
        ManagementCommand::UpdateNut05(arguments) => {
            subcommands::update_nut05(client, arguments).await?;
        }
        ManagementCommand::UpdateQuoteTtl(arguments) => {
            subcommands::update_quote_ttl(client, arguments).await?;
        }
        ManagementCommand::GetQuoteTtl => {
            subcommands::get_quote_ttl(client).await?;
        }
        ManagementCommand::UpdateNut04QuoteState(arguments) => {
            subcommands::update_nut04_quote_state(client, arguments).await?;
        }
        ManagementCommand::RotateNextKeyset(arguments) => {
            subcommands::rotate_next_keyset(client, arguments).await?;
        }
    }

    Ok(())
}
