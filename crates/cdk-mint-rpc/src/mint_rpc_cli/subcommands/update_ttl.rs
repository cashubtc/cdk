use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateQuoteTtlRequest;

/// Command to update the time-to-live (TTL) settings for quotes
///
/// This command configures how long mint and melt quotes remain valid before
/// automatically expiring. Quote TTL settings help manage pending operations and
/// resource usage on the mint.
#[derive(Args)]
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
    client: &mut CdkMintClient<Channel>,
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
