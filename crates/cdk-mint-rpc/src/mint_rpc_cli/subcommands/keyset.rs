use anyhow::Result;
use cdk_common::terminal::escape_control;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::keyset::keyset_service_client::KeysetServiceClient;
use crate::keyset::{GetKeysetTotalsRequest, RotateNextKeysetRequest};

/// Command to rotate to the next keyset for the mint
///
/// This command instructs the mint to rotate to a new keyset, which generates new keys
/// for signing tokens of the specified unit type.
#[derive(Args, Debug)]
pub struct RotateNextKeysetCommand {
    /// The unit type for the keyset (e.g., "sat")
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    /// The amounts that can be minted with this keyset (e.g., "1,2,4,8,16")
    #[arg(short, long)]
    amounts: Option<String>,
    /// The input fee in parts per thousand to apply when minting with this keyset
    #[arg(short, long)]
    input_fee_ppk: Option<u64>,
    /// Use keyset v2
    #[arg(long)]
    use_keyset_v2: Option<bool>,
    /// Final expiry unix timestamp for the keyset
    #[arg(long)]
    final_expiry: Option<u64>,
}

/// Executes the rotate_next_keyset command against the mint server
///
/// This function sends an RPC request to the mint to rotate to a new keyset with the
/// specified parameters and prints the resulting keyset information.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The arguments specifying how the new keyset should be configured
pub async fn rotate_next_keyset(
    client: &mut KeysetServiceClient<Channel>,
    sub_command_args: &RotateNextKeysetCommand,
) -> Result<()> {
    let amounts = if let Some(amounts_str) = &sub_command_args.amounts {
        amounts_str
            .split(',')
            .map(|s| s.trim().parse::<u64>())
            .collect::<Result<Vec<u64>, _>>()?
    } else {
        vec![]
    };

    let response = client
        .rotate_next_keyset(Request::new(RotateNextKeysetRequest {
            unit: sub_command_args.unit.clone(),
            amounts,
            input_fee_ppk: sub_command_args.input_fee_ppk,
            use_keyset_v2: sub_command_args.use_keyset_v2,
            final_expiry: sub_command_args.final_expiry,
        }))
        .await?;

    let response = response.into_inner();

    println!(
        "Rotated to new keyset {} for unit {} with amounts {} and fee of {}",
        escape_control(&response.id),
        escape_control(&response.unit),
        serde_json::to_string(&response.amounts)?,
        response.input_fee_ppk
    );

    Ok(())
}

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
    client: &mut KeysetServiceClient<Channel>,
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
