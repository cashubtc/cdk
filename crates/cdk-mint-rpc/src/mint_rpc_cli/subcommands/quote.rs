use anyhow::{bail, Result};
use cdk_common::terminal::escape_control;
use clap::{Args, Subcommand};
use tonic::Request;

use crate::quote::resolve_melt_quote_request::Resolution;
use crate::quote::{
    CompensateMeltQuote, FinalizeMeltQuote, GetQuoteTtlRequest, ListMeltQuotesRequest,
    MeltQuoteState, MintQuoteState, ResolveMeltQuoteRequest, UpdateMintQuoteStateRequest,
    UpdateQuoteTtlRequest,
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

/// Filters and pagination for inspecting stored melt quotes.
#[derive(Debug, Args)]
pub struct ListMeltQuotesCommand {
    /// Look up a quote in any state, unless --state is also supplied.
    #[arg(long)]
    quote_id: Option<String>,
    /// Exact payment lookup ID; all supplied filters must match.
    #[arg(long, visible_alias = "payment-lookup-id")]
    request_lookup_id: Option<String>,
    /// Exact invoice, offer, destination address, or custom payment request.
    #[arg(long)]
    payment_request: Option<String>,
    /// Only return quotes in this state; omit to inspect all states.
    #[arg(long, value_parser = ["unpaid", "pending", "paid", "unknown", "failed"], ignore_case = true)]
    state: Option<String>,
    /// Maximum quotes to return (default 100, maximum 1000).
    #[arg(long, default_value_t = 0)]
    limit: u32,
    /// Matching quotes to skip, oldest first.
    #[arg(long, default_value_t = 0)]
    offset: u32,
}

/// Prints stored melt quotes and recovery details without changing payment state.
pub async fn list_melt_quotes(
    client: &mut InterceptedQuoteServiceClient,
    command: &ListMeltQuotesCommand,
) -> Result<(), tonic::Status> {
    let state = command.state.as_ref().map(|state| {
        let state = match state.to_ascii_lowercase().as_str() {
            "unpaid" => MeltQuoteState::Unpaid,
            "pending" => MeltQuoteState::Pending,
            "paid" => MeltQuoteState::Paid,
            "unknown" => MeltQuoteState::Unknown,
            "failed" => MeltQuoteState::Failed,
            _ => MeltQuoteState::Unspecified,
        };
        i32::from(state)
    });
    let response = client
        .list_melt_quotes(Request::new(ListMeltQuotesRequest {
            quote_id: command.quote_id.clone(),
            request_lookup_id: command.request_lookup_id.clone(),
            limit: command.limit,
            offset: command.offset,
            state,
            payment_request: command.payment_request.clone(),
        }))
        .await?
        .into_inner();

    println!("total: {}", response.total);
    for quote in response.quotes {
        println!(
            "quote_id: {}, state: {}, amount: {}, fee_reserve: {}, unit: {}, method: {}, created_time: {}, expiry: {}, request_lookup_id: {}, request_lookup_id_kind: {}",
            escape_control(&quote.quote_id),
            quote.state().as_str_name().trim_start_matches("MELT_QUOTE_STATE_"),
            quote.amount,
            quote.fee_reserve,
            escape_control(&quote.unit),
            escape_control(&quote.payment_method),
            quote.created_time,
            quote.expiry,
            escape_control(quote.request_lookup_id.as_deref().unwrap_or("none")),
            escape_control(quote.request_lookup_id_kind.as_deref().unwrap_or("none")),
        );
        println!("  payment_request: {}", escape_control(&quote.request));
        println!(
            "  paid_time: {}, payment_proof: {}",
            quote
                .paid_time
                .map(|time| time.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            escape_control(quote.payment_proof.as_deref().unwrap_or("none")),
        );
        match quote.saga {
            Some(saga) => println!(
                "  recovery: {}, operation_id: {}, created_at: {}, updated_at: {}",
                escape_control(&saga.state),
                escape_control(&saga.operation_id),
                saga.created_at,
                saga.updated_at,
            ),
            None => println!("  recovery: no stored saga"),
        }
    }

    Ok(())
}

/// Apply an operator-verified outcome to an inspected melt operation.
#[derive(Debug, Args)]
pub struct ResolveMeltQuoteCommand {
    /// Quote ID to resolve.
    quote_id: String,
    /// Operation ID from list-melt-quotes.
    #[arg(long)]
    operation_id: String,
    /// Recovery stage from list-melt-quotes.
    #[arg(long, value_parser = ["setup_complete", "payment_attempted", "payment_pending", "payment_failed", "finalizing"])]
    expected_saga_state: String,
    /// Reason and external evidence for the decision, retained in the audit record.
    #[arg(long)]
    reason: String,
    #[command(subcommand)]
    action: ResolutionAction,
}

#[derive(Debug, Subcommand)]
enum ResolutionAction {
    /// Record a verified successful payment, spend inputs, and return change.
    Finalize {
        /// Actual total spent including payment fees, in the quote's unit.
        #[arg(long)]
        total_spent: u64,
        /// Quote currency unit, e.g. sat or msat.
        #[arg(long)]
        unit: String,
        /// Actual backend payment identifier.
        #[arg(long)]
        payment_lookup_id: String,
        /// Identifier kind, e.g. payment_hash, payment_id, or quote_id.
        #[arg(long)]
        payment_lookup_id_kind: String,
        /// Payment proof, when available (e.g. preimage or onchain outpoint).
        #[arg(long)]
        payment_proof: Option<String>,
    },
    /// Release inputs after verifying payment cannot complete; does not cancel it.
    Compensate {
        /// Assert payment failed and all backend attempts/retries are stopped.
        #[arg(long, required = true)]
        payment_failure_confirmed: bool,
    },
}

/// Resolve a melt through its normal finalization or compensation path.
pub async fn resolve_melt_quote(
    client: &mut InterceptedQuoteServiceClient,
    command: &ResolveMeltQuoteCommand,
) -> Result<(), tonic::Status> {
    let resolution = match &command.action {
        ResolutionAction::Finalize {
            total_spent,
            unit,
            payment_lookup_id,
            payment_lookup_id_kind,
            payment_proof,
        } => Resolution::Finalize(FinalizeMeltQuote {
            total_spent: *total_spent,
            unit: unit.clone(),
            payment_lookup_id: payment_lookup_id.clone(),
            payment_lookup_id_kind: payment_lookup_id_kind.clone(),
            payment_proof: payment_proof.clone(),
        }),
        ResolutionAction::Compensate {
            payment_failure_confirmed,
        } => Resolution::Compensate(CompensateMeltQuote {
            payment_failure_confirmed: *payment_failure_confirmed,
        }),
    };
    let response = client
        .resolve_melt_quote(ResolveMeltQuoteRequest {
            quote_id: command.quote_id.clone(),
            operation_id: command.operation_id.clone(),
            expected_saga_state: command.expected_saga_state.clone(),
            reason: command.reason.clone(),
            resolution: Some(resolution),
        })
        .await?
        .into_inner();
    println!(
        "quote_id: {}, operation_id: {}, state: {}, cleanup: complete",
        escape_control(&response.quote_id),
        escape_control(&response.operation_id),
        response
            .state()
            .as_str_name()
            .trim_start_matches("MELT_QUOTE_STATE_")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        command: ResolveMeltQuoteCommand,
    }

    #[test]
    fn compensation_requires_explicit_failure_confirmation() {
        let args = [
            "resolve-melt-quote",
            "quote",
            "--operation-id",
            "operation",
            "--expected-saga-state",
            "payment_pending",
            "--reason",
            "verified failure",
            "compensate",
        ];
        assert!(TestCli::try_parse_from(args).is_err());
        let cli = TestCli::try_parse_from(args.into_iter().chain(["--payment-failure-confirmed"]))
            .unwrap();
        assert!(matches!(
            cli.command.action,
            ResolutionAction::Compensate {
                payment_failure_confirmed: true
            }
        ));
    }
}
