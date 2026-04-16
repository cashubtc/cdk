use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::{DeactivateKeysetRequest, InterceptedCdkMintClient};

/// Command to deactivate a specific keyset by ID
///
/// This deactivates the specified keyset without creating a replacement.
/// The mint will no longer issue new tokens for this keyset.
/// Existing tokens can still be spent (swapped or melted).
#[derive(Args, Debug)]
pub struct DeactivateKeysetCommand {
    /// The keyset ID to deactivate
    #[arg(short, long)]
    id: String,
}

/// Executes the deactivate_keyset command against the mint server
pub async fn deactivate_keyset(
    client: &mut InterceptedCdkMintClient,
    sub_command_args: &DeactivateKeysetCommand,
) -> Result<()> {
    client
        .deactivate_keyset(Request::new(DeactivateKeysetRequest {
            id: sub_command_args.id.clone(),
        }))
        .await?;

    println!("Deactivated keyset {}", sub_command_args.id);

    Ok(())
}
