use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::UpdateNut04QuoteRequest;

#[derive(Args)]
pub struct UpdateNut04QuoteCommand {
    quote_id: String,
    #[arg(default_value = "PAID")]
    state: String,
}

pub async fn update_nut04_quote_state(
    client: &mut CdkMintClient<Channel>,
    sub_command_args: &UpdateNut04QuoteCommand,
) -> Result<()> {
    let response = client
        .update_nut04_quote(Request::new(UpdateNut04QuoteRequest {
            quote_id: sub_command_args.quote_id.clone(),
            state: sub_command_args.state.clone(),
        }))
        .await?;

    let response = response.into_inner();

    println!("Quote {} updated to {}", response.quote_id, response.state);

    Ok(())
}
