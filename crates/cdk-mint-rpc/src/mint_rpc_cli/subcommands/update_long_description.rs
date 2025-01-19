use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateDescriptionRequest;

#[derive(Args)]
pub struct UpdateLongDescriptionCommand {
    description: String,
}

pub async fn update_long_description(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateLongDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_long_description(Request::new(UpdateDescriptionRequest {
            description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}
