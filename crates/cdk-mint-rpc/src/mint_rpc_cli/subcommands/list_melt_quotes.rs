use cdk_common::terminal::escape_control;
use clap::Args;
use tonic::Request;

use crate::quote::{ListMeltQuotesRequest, MeltQuoteState};
use crate::InterceptedQuoteServiceClient;

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
