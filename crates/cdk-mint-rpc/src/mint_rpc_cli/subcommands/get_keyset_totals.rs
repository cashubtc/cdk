use anyhow::Result;
use cdk_common::terminal::escape_control;
use clap::Args;
use tonic::Request;

use crate::keyset::GetKeysetTotalsRequest;
use crate::InterceptedKeysetServiceClient;

/// Command to retrieve ecash issued and redeemed totals by unit and keyset
#[derive(Args, Debug)]
pub struct GetKeysetTotalsCommand {
    /// Filter totals by currency unit (e.g., "sat", "usd")
    #[arg(short, long)]
    unit: Option<String>,

    /// Filter totals by keyset ID
    #[arg(short, long)]
    keyset_id: Option<String>,
}

/// Executes the get_keyset_totals command against the mint server
///
/// Fetches aggregated ecash issued and redeemed totals by unit and per keyset.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The command arguments containing optional unit and keyset filters
pub async fn get_keyset_totals(
    client: &mut InterceptedKeysetServiceClient,
    sub_command_args: &GetKeysetTotalsCommand,
) -> Result<()> {
    let response = client
        .get_keyset_totals(Request::new(GetKeysetTotalsRequest {
            unit: sub_command_args.unit.clone(),
            keyset_id: sub_command_args.keyset_id.clone(),
        }))
        .await?;

    let response = response.into_inner();

    if response.unit_totals.is_empty() && response.keyset_totals.is_empty() {
        println!("No keyset totals found.");
        return Ok(());
    }

    println!("Unit totals:");
    for unit_total in response.unit_totals {
        println!("  Unit: {}", escape_control(&unit_total.unit));
        println!("    Total issued:   {}", unit_total.total_issued);
        println!("    Total redeemed: {}", unit_total.total_redeemed);
    }

    println!("\nKeyset totals:");
    for keyset_total in response.keyset_totals {
        println!(
            "  Keyset: {} ({})",
            escape_control(&keyset_total.keyset_id),
            escape_control(&keyset_total.unit)
        );
        println!("    Total issued:   {}", keyset_total.total_issued);
        println!("    Total redeemed: {}", keyset_total.total_redeemed);
    }

    Ok(())
}
