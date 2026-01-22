use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::{GetQuoteTtlRequest, UpdateQuoteTtlRequest};

/// Command to update the time-to-live (TTL) settings for quotes
///
/// This command configures how long mint and melt quotes remain valid before
/// automatically expiring. Quote TTL settings help manage pending operations and
/// resource usage on the mint.
#[derive(Args, Debug)]
pub struct UpdateQuoteTtlCommand {
    /// The TTL (in seconds) for mint quotes
    #[arg(long)]
    mint_ttl: Option<u64>,
    /// The TTL (in seconds) for melt quotes
    #[arg(long)]
    melt_ttl: Option<u64>,
}
/// Executes the update_quote_ttl command against the mint server
///
/// This function sends an RPC request to update the TTL settings for mint and melt quotes.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new TTL values to set for quotes
pub async fn update_quote_ttl(
    client: &mut CdkMintManagementClient<Channel>,
    sub_command_args: &UpdateQuoteTtlCommand,
) -> Result<()> {
    let _response = client
        .update_quote_ttl(Request::new(UpdateQuoteTtlRequest {
            mint_ttl: sub_command_args.mint_ttl,
            melt_ttl: sub_command_args.melt_ttl,
        }))
        .await?;

    Ok(())
}

/// Command to get the current time-to-live (TTL) settings for quotes
///
/// This command retrieves the current TTL settings for mint and melt quotes.
#[derive(Args, Debug)]
pub struct GetQuoteTtlCommand {}

/// Executes the get_quote_ttl command against the mint server
///
/// This function sends an RPC request to retrieve the current TTL settings for mint and melt quotes.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
pub async fn get_quote_ttl(client: &mut CdkMintManagementClient<Channel>) -> Result<()> {
    let response = client
        .get_quote_ttl(Request::new(GetQuoteTtlRequest {}))
        .await?
        .into_inner();

    println!("Quote TTL Settings:");
    println!("  Mint TTL: {} seconds", response.mint_ttl);
    println!("  Melt TTL: {} seconds", response.melt_ttl);

    Ok(())
}
