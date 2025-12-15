use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::GetKeysetsRequest;

/// Command to get keysets from the mint
#[derive(Args)]
pub struct GetKeysetsCommand {
    /// Filter by units (comma-separated, e.g., "sat,usd")
    #[arg(short, long)]
    units: Option<String>,
    /// Only show active keysets
    #[arg(short, long)]
    active_only: Option<bool>,
}

/// Executes the get_keysets command against the mint server
pub async fn get_keysets(
    client: &mut CdkMintReportingClient<Channel>,
    sub_command_args: &GetKeysetsCommand,
) -> Result<()> {
    let units = sub_command_args
        .units
        .as_ref()
        .map(|u| u.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let response = client
        .get_keysets(Request::new(GetKeysetsRequest {
            units,
            active_only: sub_command_args.active_only,
        }))
        .await?;

    let keysets = response.into_inner().keysets;

    if keysets.is_empty() {
        println!("No keysets found");
        return Ok(());
    }

    println!("{:<20} {:<10} {}", "ID", "UNIT", "ACTIVE");
    println!("{}", "-".repeat(40));
    for ks in keysets {
        println!("{:<20} {:<10} {}", ks.id, ks.unit, ks.active);
    }

    Ok(())
}
