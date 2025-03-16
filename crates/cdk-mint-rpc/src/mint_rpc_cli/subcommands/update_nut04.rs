use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateNut04Request;

#[derive(Args)]
pub struct UpdateNut04Command {
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    #[arg(short, long)]
    #[arg(default_value = "bolt11")]
    method: String,
    #[arg(long)]
    min_amount: Option<u64>,
    #[arg(long)]
    max_amount: Option<u64>,
    #[arg(long)]
    disabled: Option<bool>,
    #[arg(long)]
    description: Option<bool>,
}

pub async fn update_nut04(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateNut04Command,
) -> Result<()> {
    let _response = client
        .update_nut04(Request::new(UpdateNut04Request {
            method: sub_command_args.method.clone(),
            unit: sub_command_args.unit.clone(),
            disabled: sub_command_args.disabled,
            min: sub_command_args.min_amount,
            max: sub_command_args.max_amount,
            description: sub_command_args.description,
        }))
        .await?;

    Ok(())
}
