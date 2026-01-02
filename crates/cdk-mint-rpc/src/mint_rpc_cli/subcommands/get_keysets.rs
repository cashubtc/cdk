use anyhow::Result;
use clap::ArgAction;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::GetKeysetsRequest;

/// Command to get keysets from the mint
///
/// This command retrieves keyset information from the mint, including keyset IDs, units,
/// and active status. The results can be filtered by units and various display options
/// can be configured to include or exclude specific types of keysets.
#[derive(Args)]
pub struct GetKeysetsCommand {
    /// Filter by units (comma-separated, e.g., "sat,usd")
    #[arg(short, long)]
    units: Option<String>,
    /// Only show active keysets
    #[arg(short = 'e', long, action = ArgAction::SetTrue)]
    exclude_inactive: Option<bool>,
    /// Include auth keysets
    #[arg(short = 'a', long, action = ArgAction::SetTrue)]
    include_auth: Option<bool>,
    /// Include proof counts
    #[arg(short = 'b', long, action = ArgAction::SetTrue)]
    include_balances: Option<bool>,
}

/// Executes the get_keysets command against the mint server
///
/// This function sends an RPC request to retrieve keyset information from the mint
/// and displays the results in a formatted table.
/// The units filter is parsed from a comma-separated string
/// into a vector of unit strings.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The command arguments
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
            exclude_inactive: sub_command_args.exclude_inactive,
            include_auth: sub_command_args.include_auth,
            include_balances: sub_command_args.include_balances,
        }))
        .await?;

    let keysets = response.into_inner().keysets;

    if keysets.is_empty() {
        println!("No keysets found");
        return Ok(());
    }

    let show_balances = sub_command_args.include_balances.unwrap_or(false);

    if show_balances {
        println!(
            "{:<20} {:<10} {:<8} {:>12} {:>12} {:>8} {:>6} {:>15} {:>15} {:>15} {:>15}",
            "ID",
            "UNIT",
            "ACTIVE",
            "VALID_FROM",
            "VALID_TO",
            "FEE_PPK",
            "INDEX",
            "BALANCE",
            "ISSUED",
            "REDEEMED",
            "FEES"
        );
        println!("{}", "-".repeat(146));
        for ks in keysets {
            let valid_to = if ks.valid_to == 0 {
                "-".to_string()
            } else {
                ks.valid_to.to_string()
            };
            println!(
                "{:<20} {:<10} {:<8} {:>12} {:>12} {:>8} {:>6} {:>15} {:>15} {:>15} {:>15}",
                ks.id,
                ks.unit,
                ks.active,
                ks.valid_from,
                valid_to,
                ks.input_fee_ppk,
                ks.derivation_path_index,
                ks.total_balance.map(|b| b.to_string()).unwrap_or_default(),
                ks.total_issued.map(|i| i.to_string()).unwrap_or_default(),
                ks.total_redeemed.map(|r| r.to_string()).unwrap_or_default(),
                ks.total_fees_collected
                    .map(|f| f.to_string())
                    .unwrap_or_default(),
            );
        }
    } else {
        println!(
            "{:<20} {:<10} {:<8} {:>12} {:>12} {:>8} {:>6}",
            "ID", "UNIT", "ACTIVE", "VALID_FROM", "VALID_TO", "FEE_PPK", "INDEX"
        );
        println!("{}", "-".repeat(82));
        for ks in keysets {
            let valid_to = if ks.valid_to == 0 {
                "-".to_string()
            } else {
                ks.valid_to.to_string()
            };
            println!(
                "{:<20} {:<10} {:<8} {:>12} {:>12} {:>8} {:>6}",
                ks.id,
                ks.unit,
                ks.active,
                ks.valid_from,
                valid_to,
                ks.input_fee_ppk,
                ks.derivation_path_index
            );
        }
    }

    Ok(())
}
