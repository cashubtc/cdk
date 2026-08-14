use anyhow::Result;
use clap::Args;
use tonic::Request;

use crate::payment_method::UpdateDisabledRequest;
use crate::InterceptedPaymentMethodServiceClient;

/// Command to enable or disable minting and melting for the whole mint
///
/// Flags that are not given keep their current value.
#[derive(Args, Debug)]
pub struct UpdateDisabledCommand {
    /// Whether minting is disabled (true) or enabled (false) for every unit and method
    #[arg(long)]
    mint_disabled: Option<bool>,
    /// Whether melting is disabled (true) or enabled (false) for every unit and method
    #[arg(long)]
    melt_disabled: Option<bool>,
}

/// Executes the update_disabled command against the mint server
///
/// This function sends an RPC request to enable or disable minting and melting
/// for the whole mint, keeping the current value of any flag that is not given.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The disabled flags to set
pub async fn update_disabled(
    client: &mut InterceptedPaymentMethodServiceClient,
    sub_command_args: &UpdateDisabledCommand,
) -> Result<()> {
    let response = client
        .update_disabled(Request::new(UpdateDisabledRequest {
            mint_disabled: sub_command_args.mint_disabled,
            melt_disabled: sub_command_args.melt_disabled,
        }))
        .await?
        .into_inner();

    println!("Minting disabled: {}", response.mint_disabled);
    println!("Melting disabled: {}", response.melt_disabled);

    Ok(())
}
