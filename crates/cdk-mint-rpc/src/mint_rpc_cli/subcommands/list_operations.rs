use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::mint_rpc_cli::utils::parse_csv;
use crate::ListOperationsRequest;

/// Command to list operations from the mint
///
/// This command retrieves operation information from the mint with optional filtering
/// by units and operation kinds. Supports pagination through offset and limit parameters,
/// and can display results in reverse chronological order. Operations represent various
/// activities performed by the mint such as minting, melting, and swapping tokens.
#[derive(Args, Debug)]
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

/// Executes the list_operations command against the mint server
///
/// This function sends an RPC request to retrieve operation information from the mint
/// and displays the results in a formatted table. Comma-separated filter values for units
/// and operations are parsed into vectors using the shared parse_csv utility. If no operations
/// are found, it displays an appropriate message. If there are more results available, it
/// indicates pagination is possible.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `args` - The command arguments, including pagination and filtering options
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
        "{:<36} {:<6} {:<6} {:>10} {:>10} {:>8} {:>12} {:>12} {:>10} {:<12}",
        "OP_ID",
        "KIND",
        "UNIT",
        "ISSUED",
        "REDEEMED",
        "FEE",
        "COMPLETED",
        "PAY_AMOUNT",
        "PAY_FEE",
        "PAY_METHOD"
    );
    println!("{}", "-".repeat(128));
    for op in &operations {
        let payment_amount = match op.payment_amount {
            Some(a) => a.to_string(),
            None => "-".to_string(),
        };
        let payment_fee = match op.payment_fee {
            Some(f) => f.to_string(),
            None => "-".to_string(),
        };
        let payment_method = op
            .payment_method
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("-");
        println!(
            "{:<36} {:<6} {:<6} {:>10} {:>10} {:>8} {:>12} {:>12} {:>10} {:<12}",
            op.operation_id,
            op.operation_kind,
            op.unit,
            op.total_issued,
            op.total_redeemed,
            op.fee_collected,
            op.completed_time,
            payment_amount,
            payment_fee,
            payment_method,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
