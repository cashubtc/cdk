use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_management_client::CdkMintManagementClient;
use crate::UpdateNut04QuoteRequest;

/// Command to update the state of a NUT-04 quote
///
/// NUT-04 quotes represent pending mint operations. This command allows updating
/// the state of a quote (e.g., marking it as paid) to process the minting of tokens.
#[derive(Args)]
pub struct UpdateNut04QuoteCommand {
    /// The ID of the quote to update
    quote_id: String,
    /// The new state to set for the quote (default: "PAID")
    #[arg(default_value = "PAID")]
    state: String,
}

/// Executes the update_nut04_quote_state command against the mint server
///
/// This function sends an RPC request to update the state of a NUT-04 quote,
/// which can trigger the minting of tokens once a quote is marked as paid.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The quote ID and new state to set
pub async fn update_nut04_quote_state(
    client: &mut CdkMintManagementClient<Channel>,
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
