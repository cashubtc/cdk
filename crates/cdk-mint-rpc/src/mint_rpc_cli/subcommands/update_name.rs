use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateNameRequest;

#[derive(Args)]
pub struct UpdateNameCommand {
    name: String,
}

pub async fn update_name(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateNameCommand,
) -> Result<()> {
    let _response = client
        .update_name(Request::new(UpdateNameRequest {
            name: sub_command_args.name.clone(),
        }))
        .await?;

    Ok(())
}
