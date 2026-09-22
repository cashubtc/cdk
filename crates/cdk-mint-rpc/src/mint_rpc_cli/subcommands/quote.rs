use anyhow::{bail, Result};
use clap::Args;
use tonic::Request;

use crate::quote::{
    GetQuoteTtlRequest, MintQuoteState, UpdateMintQuoteStateRequest, UpdateQuoteTtlRequest,
};
use crate::InterceptedQuoteServiceClient;

/// Command to update the time-to-live (TTL) settings for quotes
///
/// This command configures how long mint and melt quotes remain valid before
/// automatically expiring. Quote TTL settings help manage pending operations and
/// resource usage on the mint.
#[derive(Args, Debug)]
pub struct UpdateQuoteTtlCommand {
    /// The TTL (in seconds) for mint quotes
    #[arg(long)]
    mint_ttl: Option<u64>,
    /// The TTL (in seconds) for melt quotes
    #[arg(long)]
    melt_ttl: Option<u64>,
}
/// Executes the update_quote_ttl command against the mint server
///
/// This function sends an RPC request to update the TTL settings for mint and melt quotes.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The new TTL values to set for quotes
pub async fn update_quote_ttl(
    client: &mut InterceptedQuoteServiceClient,
    sub_command_args: &UpdateQuoteTtlCommand,
) -> Result<()> {
    let response = client
        .update_quote_ttl(Request::new(UpdateQuoteTtlRequest {
            mint_ttl: sub_command_args.mint_ttl,
            melt_ttl: sub_command_args.melt_ttl,
        }))
        .await?
        .into_inner();

    println!("Quote TTL Settings:");
    println!("  Mint TTL: {} seconds", response.mint_ttl);
    println!("  Melt TTL: {} seconds", response.melt_ttl);

    Ok(())
}

/// Command to get the current time-to-live (TTL) settings for quotes
///
/// This command retrieves the current TTL settings for mint and melt quotes.
#[derive(Args, Debug)]
pub struct GetQuoteTtlCommand {}

/// Executes the get_quote_ttl command against the mint server
///
/// This function sends an RPC request to retrieve the current TTL settings for mint and melt quotes.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
pub async fn get_quote_ttl(client: &mut InterceptedQuoteServiceClient) -> Result<()> {
    let response = client
        .get_quote_ttl(Request::new(GetQuoteTtlRequest {}))
        .await?
        .into_inner();

    println!("Quote TTL Settings:");
    println!("  Mint TTL: {} seconds", response.mint_ttl);
    println!("  Melt TTL: {} seconds", response.melt_ttl);

    Ok(())
}

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
