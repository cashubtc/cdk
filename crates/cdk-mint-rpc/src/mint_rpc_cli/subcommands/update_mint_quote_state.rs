use anyhow::{bail, Result};
use clap::Args;
use tonic::Request;

use crate::quote::{MintQuoteState, UpdateMintQuoteStateRequest};
use crate::InterceptedQuoteServiceClient;

/// Command to update the state of a mint quote
///
/// Mint quotes represent pending mint operations. This command allows updating
/// the state of a quote (e.g., marking it as paid) to process the minting of tokens.
#[derive(Args, Debug)]
pub struct UpdateMintQuoteStateCommand {
    /// The ID of the quote to update
    quote_id: String,
    /// The new state to set for the quote (default: "PAID")
    #[arg(default_value = "PAID")]
    state: String,
}

/// Executes the update_mint_quote_state command against the mint server
///
/// This function sends an RPC request to update the state of a mint quote,
/// which can trigger the minting of tokens once a quote is marked as paid.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The quote ID and new state to set
pub async fn update_mint_quote_state(
    client: &mut InterceptedQuoteServiceClient,
    sub_command_args: &UpdateMintQuoteStateCommand,
) -> Result<()> {
    let state = match sub_command_args.state.as_str() {
        "UNPAID" => MintQuoteState::Unpaid,
        "PAID" => MintQuoteState::Paid,
        "ISSUED" => MintQuoteState::Issued,
        state => bail!("Invalid quote state: {state}"),
    };

    let response = client
        .update_mint_quote_state(Request::new(UpdateMintQuoteStateRequest {
            quote_id: sub_command_args.quote_id.clone(),
            state: state.into(),
        }))
        .await?;

    let response = response.into_inner();

    println!(
        "Quote {} updated to {}",
        response.quote_id,
        state_name(response.state())
    );

    Ok(())
}

/// Returns the NUT-04 name of a quote state
fn state_name(state: MintQuoteState) -> &'static str {
    match state {
        MintQuoteState::Unspecified => "UNSPECIFIED",
        MintQuoteState::Unpaid => "UNPAID",
        MintQuoteState::Paid => "PAID",
        MintQuoteState::Issued => "ISSUED",
    }
}
