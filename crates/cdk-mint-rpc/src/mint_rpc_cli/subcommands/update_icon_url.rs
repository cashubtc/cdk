use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateIconUrlRequest;

#[derive(Args)]
pub struct UpdateIconUrlCommand {
    name: String,
}

pub async fn update_icon_url(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateIconUrlCommand,
) -> Result<()> {
    let _response = client
        .update_icon_url(Request::new(UpdateIconUrlRequest {
            icon_url: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}
