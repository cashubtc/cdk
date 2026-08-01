//! Authenticated, redacted operator commands for federated Bark.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationBarkActionId, FederationBarkReconcileRequest, FederationBarkReconcileResponse,
    FederationBarkRetryRequest, FederationBarkRetryResponse, FederationConfig,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate, MemberConfigPublic,
    MemberId, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::Transport;

use crate::cli::{CLIArgs, FederationBarkReconcileArgs, FederationBarkRetryArgs};

pub(crate) async fn run_reconcile_command(
    cli_args: &CLIArgs,
    args: &FederationBarkReconcileArgs,
) -> Result<()> {
    let (public, private) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation bark-reconcile",
    )?;
    let member = target_member(&public, &private, args.member)?;
    let request = FederationSignedRequest::new_signed(
        public.federation_id,
        member.member_id,
        operator_sequence(),
        FederationRestMessageKind::BarkReconcile,
        FederationBarkReconcileRequest::default(),
        &private,
    )?;
    let url = member
        .federation_api_url
        .join_paths(&["federation", "v1", "bark", "reconcile"])?;
    let response: FederationBarkReconcileResponse = crate::federation_operator_transport(cli_args)
        .await?
        .http_post(url, None, &request)
        .await?;
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
    )?;
    let member = target_member(&public, &private, args.member)?;
    let request = FederationSignedRequest::new_signed(
        public.federation_id,
        member.member_id,
        operator_sequence(),
        FederationRestMessageKind::BarkActionRetry,
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
        .await?;
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

fn load_configs(
    cli_args: &CLIArgs,
    public_config: &Option<std::path::PathBuf>,
    private_config: &Option<std::path::PathBuf>,
    action: &str,
) -> Result<(FederationConfig, MemberConfigPrivate)> {
    let (public_path, private_path) =
        crate::federation_config_paths(cli_args, public_config, private_config, action)?;
    let public: FederationConfig =
        crate::read_federation_json(&public_path, "public federation config")?;
    let private: MemberConfigPrivate =
        crate::read_federation_json(&private_path, "private federation config")?;
    private
        .validate_against_public(&public)
        .context("private federation config does not match public federation config")?;
    Ok((public, private))
}

fn target_member(
    public: &FederationConfig,
    private: &MemberConfigPrivate,
    requested: Option<u16>,
) -> Result<MemberConfigPublic> {
    let member_id = match requested {
        Some(member_id) => MemberId::new(member_id)?,
        None => private.member_id,
    };
    public
        .member(member_id)
        .cloned()
        .with_context(|| format!("federation public config has no member {member_id}"))
}

fn operator_sequence() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .ok()
        .and_then(|millis| u64::try_from(millis).ok())
        .unwrap_or_default()
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
