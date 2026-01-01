use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::mint_rpc_cli::utils::parse_csv;
use crate::ListProofsRequest;

/// Command to list proofs from the mint
///
/// This command retrieves proof information from the mint with optional filtering by states,
/// units, keyset IDs, and operation kinds. Supports pagination through offset and limit
/// parameters, and can display results in reverse chronological order. Proofs represent
/// individual tokens or token fragments that have been minted and can be spent.
#[derive(Args)]
pub struct ListProofsCommand {
    /// Offset for pagination
    #[arg(long, default_value = "0")]
    offset: i64,
    /// Maximum number of proofs to return
    #[arg(short = 'n', long, default_value = "50")]
    limit: i64,
    /// Reverse order (newest first)
    #[arg(short, long)]
    reversed: bool,
    /// Filter by states (comma-separated: unspent,pending,spent)
    #[arg(short, long)]
    states: Option<String>,
    /// Filter by units (comma-separated: sat,usd)
    #[arg(short, long)]
    units: Option<String>,
    /// Filter by keyset IDs (comma-separated)
    #[arg(short, long)]
    keyset_ids: Option<String>,
    /// Filter by operation kinds (comma-separated: mint,swap_in,swap_out,melt)
    #[arg(short, long)]
    operations: Option<String>,
    /// Filter by creation date start (Unix timestamp)
    #[arg(long)]
    from: Option<i64>,
    /// Filter by creation date end (Unix timestamp)
    #[arg(long)]
    to: Option<i64>,
}

/// Executes the list_proofs command against the mint server
///
/// This function sends an RPC request to retrieve proof information from the mint and displays
/// the results in a formatted table. Comma-separated filter values for states, units, keyset IDs,
/// and operations are parsed into vectors using the shared parse_csv utility. Long keyset IDs are
/// truncated for display purposes. If no proofs are found, it displays an appropriate message.
/// If there are more results available, it indicates pagination is possible.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `args` - The command arguments, including pagination and filtering options
pub async fn list_proofs(
    client: &mut CdkMintReportingClient<Channel>,
    args: &ListProofsCommand,
) -> Result<()> {
    let response = client
        .list_proofs(Request::new(ListProofsRequest {
            index_offset: args.offset,
            num_max_proofs: args.limit,
            reversed: args.reversed,
            creation_date_start: args.from,
            creation_date_end: args.to,
            states: parse_csv(&args.states),
            units: parse_csv(&args.units),
            keyset_ids: parse_csv(&args.keyset_ids),
            operations: parse_csv(&args.operations),
        }))
        .await?;

    let resp = response.into_inner();
    let proofs = resp.proofs;

    if proofs.is_empty() {
        println!("No proofs found");
        return Ok(());
    }

    println!(
        "{:>10} {:<18} {:<10} {:<10} {:<36} {:>12}",
        "AMOUNT", "KEYSET_ID", "STATE", "OP_KIND", "OP_ID", "CREATED"
    );
    println!("{}", "-".repeat(102));
    for p in &proofs {
        let keyset_short = if p.keyset_id.len() > 16 {
            format!("{}...", &p.keyset_id[..16])
        } else {
            p.keyset_id.clone()
        };
        println!(
            "{:>10} {:<18} {:<10} {:<10} {:<36} {:>12}",
            p.amount, keyset_short, p.state, p.operation_kind, p.operation_id, p.created_time,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
