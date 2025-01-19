use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateUrlRequest;

#[derive(Args)]
pub struct AddUrlCommand {
    url: String,
}

pub async fn add_url(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &AddUrlCommand,
) -> Result<()> {
    let _response = client
        .add_url(Request::new(UpdateUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}

#[derive(Args)]
pub struct RemoveUrlCommand {
    url: String,
}

pub async fn remove_url(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &RemoveUrlCommand,
) -> Result<()> {
    let _response = client
        .remove_url(Request::new(UpdateUrlRequest {
            url: sub_command_args.url.clone(),
        }))
        .await?;

    Ok(())
}
