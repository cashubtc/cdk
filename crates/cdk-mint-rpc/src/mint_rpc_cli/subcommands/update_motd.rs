use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateMotdRequest;

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
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateMotdCommand,
) -> Result<()> {
    let _response = client
        .update_motd(Request::new(UpdateMotdRequest {
            motd: sub_command_args.motd.clone(),
        }))
        .await?;

    Ok(())
}
