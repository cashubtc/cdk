//! Authenticated, redacted operator commands for federated Bark.

use crate::federation_operator::{load_configs, operator_sequence, target_member};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationRestMessageKind, FederationSignedRequest, FEDERATION_REST_API_VERSION,
};
use cdk_federation_bark::{
    FederationBarkActionId, FederationBarkReconcileRequest, FederationBarkReconcileResponse,
    FederationBarkRecoveryAddressRequest, FederationBarkRecoveryAddressResponse,
    FederationBarkRetryRequest, FederationBarkRetryResponse,
};
use cdk_http_client::Transport;

use crate::cli::{
    CLIArgs, FederationBarkReconcileArgs, FederationBarkRecoveryAddressArgs,
    FederationBarkRetryArgs,
};

pub(crate) async fn run_reconcile_command(
    cli_args: &CLIArgs,
    args: &FederationBarkReconcileArgs,
) -> Result<()> {
    let (public, private) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation bark-reconcile",
    )
    .await?;
    let member = target_member(&public, &private, args.member)?;
    let request = FederationSignedRequest::new_signed(
        public.federation_id,
        member.member_id,
        operator_sequence()?,
        FederationRestMessageKind::ApplicationReconcile,
        FederationBarkReconcileRequest::default(),
        &private,
    )?;
    let url = member
        .federation_api_url
        .join_paths(&["federation", "v1", "bark", "reconcile"])?;
    let response: FederationBarkReconcileResponse = crate::federation_operator_transport(cli_args)
        .await?
        .http_post(url, None, &request)
        .await?
        .json_or_status_error()?;
    if response.version != FEDERATION_REST_API_VERSION {
        bail!("member returned an unsupported Bark reconcile response version");
    }
    println!(
        "member {} Bark reconcile: accepted_frontier={} local_frontier={}->{} readiness={} degraded_reason={}",
        member.member_id,
        response.accepted_frontier,
        response.local_frontier_before,
        response.local_frontier_after,
        response.readiness,
        response.degraded_reason.as_deref().unwrap_or("none"),
    );
    Ok(())
}

pub(crate) async fn run_retry_command(
    cli_args: &CLIArgs,
    args: &FederationBarkRetryArgs,
) -> Result<()> {
    if !args.acknowledge_bark_side_effects {
        bail!("Bark retry requires --acknowledge-bark-side-effects");
    }
    let action_id = FederationBarkActionId::from_hex(&args.action_id)
        .context("invalid federated Bark action id")?;
    let (public, private) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation bark-retry",
    )
    .await?;
    let member = target_member(&public, &private, args.member)?;
    let request = FederationSignedRequest::new_signed(
        public.federation_id,
        member.member_id,
        operator_sequence()?,
        FederationRestMessageKind::ApplicationActionRetry,
        FederationBarkRetryRequest::new(action_id, true),
        &private,
    )?;
    let url =
        member
            .federation_api_url
            .join_paths(&["federation", "v1", "bark", "actions", "retry"])?;
    let response: FederationBarkRetryResponse = crate::federation_operator_transport(cli_args)
        .await?
        .http_post(url, None, &request)
        .await?
        .json_or_status_error()?;
    if response.version != FEDERATION_REST_API_VERSION || response.action_id != action_id {
        bail!("member returned a mismatched Bark retry response");
    }
    println!(
        "member {} Bark action {} retry: phase={}->{} advanced={}",
        member.member_id,
        response.action_id,
        response.phase_before.as_str(),
        response.phase_after.as_str(),
        response.advanced,
    );
    Ok(())
}

pub(crate) async fn run_recovery_address_command(
    cli_args: &CLIArgs,
    args: &FederationBarkRecoveryAddressArgs,
) -> Result<()> {
    let payload = FederationBarkRecoveryAddressRequest::new(args.reserve_id.clone());
    payload.validate()?;
    let (public, private) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation bark-recovery-address",
    )
    .await?;
    let member = target_member(&public, &private, args.member)?;
    let request = FederationSignedRequest::new_signed(
        public.federation_id,
        member.member_id,
        operator_sequence()?,
        FederationRestMessageKind::ApplicationWalletAddress,
        payload,
        &private,
    )?;
    let url = member.federation_api_url.join_paths(&[
        "federation",
        "v1",
        "bark",
        "recovery",
        "address",
    ])?;
    let response: FederationBarkRecoveryAddressResponse =
        crate::federation_operator_transport(cli_args)
            .await?
            .http_post(url, None, &request)
            .await?
            .json_or_status_error()?;
    if response.version != FEDERATION_REST_API_VERSION || response.reserve_id != args.reserve_id {
        bail!("member returned a mismatched Bark recovery address response");
    }
    println!(
        "member {} Bark recovery reserve {}: address={} application={} key_epoch={} derivation_index={}",
        member.member_id,
        response.reserve_id,
        response.address,
        response.application_id,
        response.application_key_epoch,
        response.derivation_index,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_request_requires_explicit_acknowledgement() {
        let action = FederationBarkActionId::from_bytes([7; 32]).expect("action id");
        assert!(FederationBarkRetryRequest::new(action, false)
            .validate()
            .is_err());
        assert!(FederationBarkRetryRequest::new(action, true)
            .validate()
            .is_ok());
    }
}
