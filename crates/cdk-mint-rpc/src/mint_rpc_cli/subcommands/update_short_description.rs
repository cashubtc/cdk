use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateDescriptionRequest;

#[derive(Args)]
pub struct UpdateShortDescriptionCommand {
    description: String,
}

pub async fn update_short_description(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateShortDescriptionCommand,
) -> Result<()> {
    let _response = client
        .update_short_description(Request::new(UpdateDescriptionRequest {
            description: sub_command_args.description.clone(),
        }))
        .await?;

    Ok(())
}
