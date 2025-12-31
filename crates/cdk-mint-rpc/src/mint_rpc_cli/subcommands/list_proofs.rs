use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::ListProofsRequest;

/// Command to list proofs from the mint
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

/// Parses a comma-separated string into a vector of trimmed strings
fn parse_csv(s: &Option<String>) -> Vec<String> {
    s.as_ref()
        .map(|v| v.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Executes the list_proofs command against the mint server
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
