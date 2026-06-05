use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::{InterceptedCdkMintClient, UpdateTosUrlRequest};

/// Command to update the mint's terms of service URL
#[derive(Args, Debug)]
pub struct UpdateTosUrlCommand {
    /// The URL to the mint's terms of service
    url: String,
}

/// Executes the update_tos_url command against the mint server
pub async fn update_tos_url(
    client: &mut InterceptedCdkMintClient,
    sub_command_args: &UpdateTosUrlCommand,
) -> Result<()> {
    let _response = client
        .update_tos_url(Request::new(UpdateTosUrlRequest {
            tos_url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}
