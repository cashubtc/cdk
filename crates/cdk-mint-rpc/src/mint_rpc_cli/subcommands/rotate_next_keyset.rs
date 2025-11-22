use anyhow::Result;
use clap::Args;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_client::CdkMintClient;
use crate::RotateNextKeysetRequest;

/// Command to rotate to the next keyset for the mint
///
/// This command instructs the mint to rotate to a new keyset, which generates new keys
/// for signing tokens of the specified unit type.
#[derive(Args)]
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
    client: &mut CdkMintClient<Channel>,
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
        }))
        .await?;

    let response = response.into_inner();

    println!(
        "Rotated to new keyset {} for unit {} with amounts {:?} and fee of {}",
        response.id, response.unit, response.amounts, response.input_fee_ppk
    );

    Ok(())
}
