use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateQuoteTtlRequest;

#[derive(Args)]
pub struct UpdateQuoteTtlCommand {
    #[arg(long)]
    mint_ttl: Option<u64>,
    #[arg(long)]
    melt_ttl: Option<u64>,
}
pub async fn update_quote_ttl(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateQuoteTtlCommand,
) -> Result<()> {
    let _response = client
        .update_quote_ttl(Request::new(UpdateQuoteTtlRequest {
            mint_ttl: sub_command_args.mint_ttl,
            melt_ttl: sub_command_args.melt_ttl,
        }))
        .await?;

    Ok(())
}
