use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::mint_rpc_cli::utils::parse_csv;
use crate::ListQuotesRequest;

/// Command to list mint quotes from the mint
///
/// This command retrieves mint quote information from the mint with optional filtering
/// by states and units. Supports pagination through offset and limit parameters, and can
/// display results in reverse chronological order. Mint quotes represent requests to mint
/// tokens in exchange for external payments.
#[derive(Args, Debug)]
pub struct ListMintQuotesCommand {
    /// Offset for pagination
    #[arg(long, default_value = "0")]
    offset: i64,
    /// Maximum number of quotes to return
    #[arg(short = 'n', long, default_value = "50")]
    limit: i64,
    /// Reverse order (newest first)
    #[arg(short, long)]
    reversed: bool,
    /// Filter by states (comma-separated: unpaid,paid,issued)
    #[arg(short, long)]
    states: Option<String>,
    /// Filter by units (comma-separated: sat,usd)
    #[arg(short, long)]
    units: Option<String>,
    /// Filter by creation date start (Unix timestamp)
    #[arg(long)]
    from: Option<i64>,
    /// Filter by creation date end (Unix timestamp)
    #[arg(long)]
    to: Option<i64>,
}

/// Executes the list_mint_quotes command against the mint server
///
/// This function sends an RPC request to retrieve mint quote information from the mint
/// and displays the results in a formatted table. Comma-separated filter values for states
/// and units are parsed into vectors. If no quotes are found, it displays an appropriate
/// message. If there are more results available, it indicates pagination is possible.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `args` - The command arguments, including pagination and filtering options
pub async fn list_mint_quotes(
    client: &mut CdkMintReportingClient<Channel>,
    args: &ListMintQuotesCommand,
) -> Result<()> {
    let response = client
        .list_mint_quotes(Request::new(ListQuotesRequest {
            index_offset: args.offset,
            num_max_quotes: args.limit,
            reversed: args.reversed,
            creation_date_start: args.from,
            creation_date_end: args.to,
            states: parse_csv(&args.states),
            units: parse_csv(&args.units),
        }))
        .await?;

    let resp = response.into_inner();
    let quotes = resp.quotes;

    if quotes.is_empty() {
        println!("No mint quotes found");
        return Ok(());
    }

    println!(
        "{:<36} {:>10} {:<6} {:<10} {:<10} {:>12} {:>12} {:>15}",
        "ID", "AMOUNT", "UNIT", "METHOD", "STATE", "PAID", "ISSUED", "CREATED"
    );
    println!("{}", "-".repeat(118));
    for q in &quotes {
        println!(
            "{:<36} {:>10} {:<6} {:<10} {:<10} {:>12} {:>12} {:>15}",
            q.id,
            q.amount.map(|a| a.to_string()).unwrap_or("-".to_string()),
            q.unit,
            q.payment_method,
            q.state,
            q.amount_paid,
            q.amount_issued,
            q.created_time,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
