use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::ListBlindSignaturesRequest;

/// Command to list blind signatures from the mint
#[derive(Args)]
pub struct ListBlindSignaturesCommand {
    /// Offset for pagination
    #[arg(long, default_value = "0")]
    offset: i64,
    /// Maximum number of signatures to return
    #[arg(short = 'n', long, default_value = "50")]
    limit: i64,
    /// Reverse order (newest first)
    #[arg(short, long)]
    reversed: bool,
    /// Filter by units (comma-separated: sat,usd)
    #[arg(short, long)]
    units: Option<String>,
    /// Filter by keyset IDs (comma-separated)
    #[arg(short, long)]
    keyset_ids: Option<String>,
    /// Filter by operation kinds (comma-separated: mint,swap)
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

/// Executes the list_blind_signatures command against the mint server
pub async fn list_blind_signatures(
    client: &mut CdkMintReportingClient<Channel>,
    args: &ListBlindSignaturesCommand,
) -> Result<()> {
    let response = client
        .list_blind_signatures(Request::new(ListBlindSignaturesRequest {
            index_offset: args.offset,
            num_max_signatures: args.limit,
            reversed: args.reversed,
            creation_date_start: args.from,
            creation_date_end: args.to,
            units: parse_csv(&args.units),
            keyset_ids: parse_csv(&args.keyset_ids),
            operations: parse_csv(&args.operations),
        }))
        .await?;

    let resp = response.into_inner();
    let signatures = resp.signatures;

    if signatures.is_empty() {
        println!("No blind signatures found");
        return Ok(());
    }

    println!(
        "{:>10} {:<18} {:<10} {:<36} {:>12} {:>12}",
        "AMOUNT", "KEYSET_ID", "OP_KIND", "OP_ID", "CREATED", "SIGNED"
    );
    println!("{}", "-".repeat(104));
    for s in &signatures {
        let keyset_short = if s.keyset_id.len() > 16 {
            format!("{}...", &s.keyset_id[..16])
        } else {
            s.keyset_id.clone()
        };
        let signed = s
            .signed_time
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:>10} {:<18} {:<10} {:<36} {:>12} {:>12}",
            s.amount, keyset_short, s.operation_kind, s.operation_id, s.created_time, signed,
        );
    }

    if resp.has_more {
        println!("\n... more results available (use --offset to paginate)");
    }

    Ok(())
}
