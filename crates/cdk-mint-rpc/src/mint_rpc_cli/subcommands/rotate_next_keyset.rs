use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::RotateNextKeysetRequest;

#[derive(Args)]
pub struct RotateNextKeysetCommand {
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    #[arg(short, long)]
    max_order: Option<u8>,
    #[arg(short, long)]
    input_fee_ppk: Option<u64>,
}

pub async fn rotate_next_keyset(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &RotateNextKeysetCommand,
) -> Result<()> {
    let response = client
        .rotate_next_keyset(Request::new(RotateNextKeysetRequest {
            unit: sub_command_args.unit.clone(),
            max_order: sub_command_args.max_order.map(|m| m.into()),
            input_fee_ppk: sub_command_args.input_fee_ppk,
        }))
        .await?;

    let response = response.into_inner();

    println!(
        "Rotated to new keyset {} for unit {} with a max order of {} and fee of {}",
        response.id, response.unit, response.max_order, response.input_fee_ppk
    );

    Ok(())
}
