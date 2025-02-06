use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateMotdRequest;

#[derive(Args)]
pub struct UpdateMotdCommand {
    motd: String,
}

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
