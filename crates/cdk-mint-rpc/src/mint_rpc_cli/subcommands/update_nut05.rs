use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateNut05Request;

#[derive(Args)]
pub struct UpdateNut05Command {
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
}

pub async fn update_nut05(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateNut05Command,
) -> Result<()> {
    let _response = client
        .update_nut05(Request::new(UpdateNut05Request {
            method: sub_command_args.method.clone(),
            unit: sub_command_args.unit.clone(),
            disabled: sub_command_args.disabled,
            min: sub_command_args.min_amount,
            max: sub_command_args.max_amount,
        }))
        .await?;

    Ok(())
}
