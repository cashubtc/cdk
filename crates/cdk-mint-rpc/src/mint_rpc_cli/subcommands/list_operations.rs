use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::ListOperationsRequest;

/// Command to list operations from the mint
#[derive(Args)]
pub struct ListOperationsCommand {
    /// Offset for pagination
    #[arg(long, default_value = "0")]
    offset: i64,
    /// Maximum number of operations to return
    #[arg(short = 'n', long, default_value = "50")]
    limit: i64,
    /// Reverse order (newest first)
    #[arg(short, long)]
    reversed: bool,
    /// Filter by units (comma-separated: sat,usd)
    #[arg(short, long)]
    units: Option<String>,
    /// Filter by operation kinds (comma-separated: mint,melt,swap)
    #[arg(short, long)]
    operations: Option<String>,
    /// Filter by creation date start (Unix timestamp)
    #[arg(long)]
    from: Option<i64>,
    /// Filter by creation date end (Unix timestamp)
    #[arg(long)]
    to: Option<i64>,
}

/// Parses a comma-separated string into a vector of trimmed strings
fn parse_csv(s: &Option<String>) -> Vec<String> {
    s.as_ref()
        .map(|v| v.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Executes the list_operations command against the mint server
pub async fn list_operations(
    client: &mut CdkMintReportingClient<Channel>,
    args: &ListOperationsCommand,
) -> Result<()> {
    let response = client
        .list_operations(Request::new(ListOperationsRequest {
            index_offset: args.offset,
            num_max_operations: args.limit,
            reversed: args.reversed,
            creation_date_start: args.from,
            creation_date_end: args.to,
            units: parse_csv(&args.units),
            operations: parse_csv(&args.operations),
        }))
        .await?;

    let resp = response.into_inner();
    let operations = resp.operations;

    if operations.is_empty() {
        println!("No operations found");
        return Ok(());
    }

    println!(
        "{:<36} {:<6} {:<6} {:>10} {:>10} {:>8} {:>12}",
        "OP_ID", "KIND", "UNIT", "ISSUED", "REDEEMED", "FEE", "COMPLETED"
    );
    println!("{}", "-".repeat(94));
    for op in &operations {
        println!(
            "{:<36} {:<6} {:<6} {:>10} {:>10} {:>8} {:>12}",
            op.operation_id,
            op.operation_kind,
            op.unit,
            op.total_issued,
            op.total_redeemed,
            op.fee_collected,
            op.completed_time,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
