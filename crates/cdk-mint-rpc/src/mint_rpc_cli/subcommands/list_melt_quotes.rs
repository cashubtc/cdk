use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::ListQuotesRequest;

/// Command to list melt quotes from the mint
#[derive(Args)]
pub struct ListMeltQuotesCommand {
    /// Offset for pagination
    #[arg(long, default_value = "0")]
    offset: i64,
    /// Maximum number of quotes to return
    #[arg(short = 'n', long, default_value = "50")]
    limit: i64,
    /// Reverse order (newest first)
    #[arg(short, long)]
    reversed: bool,
    /// Filter by states (comma-separated: unpaid,pending,paid)
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

/// Executes the list_melt_quotes command against the mint server
pub async fn list_melt_quotes(
    client: &mut CdkMintReportingClient<Channel>,
    args: &ListMeltQuotesCommand,
) -> Result<()> {
    let states = args
        .states
        .as_ref()
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();

    let units = args
        .units
        .as_ref()
        .map(|u| u.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();

    let response = client
        .list_melt_quotes(Request::new(ListQuotesRequest {
            index_offset: args.offset,
            num_max_quotes: args.limit,
            reversed: args.reversed,
            creation_date_start: args.from,
            creation_date_end: args.to,
            states,
            units,
        }))
        .await?;

    let resp = response.into_inner();
    let quotes = resp.quotes;

    if quotes.is_empty() {
        println!("No melt quotes found");
        return Ok(());
    }

    println!(
        "{:<36} {:>10} {:<6} {:<10} {:>12} {:>12}",
        "ID", "AMOUNT", "UNIT", "STATE", "FEE_RESERVE", "CREATED"
    );
    println!("{}", "-".repeat(92));
    for q in &quotes {
        println!(
            "{:<36} {:>10} {:<6} {:<10} {:>12} {:>12}",
            q.id, q.amount, q.unit, q.state, q.fee_reserve, q.created_time,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
