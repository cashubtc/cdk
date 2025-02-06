use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateContactRequest;

#[derive(Args)]
pub struct AddContactCommand {
    method: String,
    info: String,
}

pub async fn add_contact(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &AddContactCommand,
) -> Result<()> {
    let _response = client
        .add_contact(Request::new(UpdateContactRequest {
            method: sub_command_args.method.clone(),
            info: sub_command_args.info.clone(),
        }))
        .await?;

    Ok(())
}

#[derive(Args)]
pub struct RemoveContactCommand {
    method: String,
    info: String,
}

pub async fn remove_contact(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &RemoveContactCommand,
) -> Result<()> {
    let _response = client
        .remove_contact(Request::new(UpdateContactRequest {
            method: sub_command_args.method.clone(),
            info: sub_command_args.info.clone(),
        }))
        .await?;

    Ok(())
}
