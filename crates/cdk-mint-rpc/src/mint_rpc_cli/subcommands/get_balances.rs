use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::GetBalancesRequest;

/// Command to get balances from the mint
///
/// This command retrieves balance information from the mint by unit,
/// issued amounts, redeemed amounts, and fees collected.
///
/// # Arguments
/// * `unit` - Optional filter by unit (e.g., "sat", "usd")
#[derive(Args)]
pub struct GetBalancesCommand {
    /// Filter by unit (e.g., "sat", "usd")
    #[arg(short, long)]
    unit: Option<String>,
}

/// Executes the get_balances command against the mint server
///
/// This function sends an RPC request to retrieve balance information from the mint
/// and displays the results in a formatted table. If no balances are found, it displays
/// an appropriate message.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The command arguments
pub async fn get_balances(
    client: &mut CdkMintReportingClient<Channel>,
    sub_command_args: &GetBalancesCommand,
) -> Result<()> {
    let response = client
        .get_balances(Request::new(GetBalancesRequest {
            unit: sub_command_args.unit.clone(),
        }))
        .await?;

    let balances = response.into_inner().balances;

    if balances.is_empty() {
        println!("No balances found");
        return Ok(());
    }

    println!(
        "{:<10} {:>15} {:>15} {:>15} {:>15}",
        "UNIT", "BALANCE", "ISSUED", "REDEEMED", "FEES"
    );
    println!("{}", "-".repeat(72));
    for bal in balances {
        println!(
            "{:<10} {:>15} {:>15} {:>15} {:>15}",
            bal.unit,
            bal.total_balance,
            bal.total_issued,
            bal.total_redeemed,
            bal.total_fees_collected
        );
    }

    Ok(())
}
