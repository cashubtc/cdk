//! Authenticated operator commands for the federated on-chain wallet.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cdk::mint::FederationRuntimeConfig;
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationConfig, FederationFrostRotationOperation,
    FederationRestMessageKind, FederationSignedRequest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, FederationTransactionProposalId,
    FederationWalletRebroadcastResponse, FederationWalletTransactionRequest,
    FederationWalletTransactionStatus, MemberConfigPrivate, MemberConfigPublic, MemberId,
};
use cdk_http_client::{Async, Transport};

use crate::cli::{
    CLIArgs, FederationFrostRotateArgs, FederationWalletRebroadcastArgs,
    FederationWalletTransactionArgs,
};

pub(crate) async fn run_transaction_status_command(
    cli_args: &CLIArgs,
    args: &FederationWalletTransactionArgs,
) -> Result<()> {
    let (public_config, private_config) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation transaction-status",
    )?;
    let member = target_member(&public_config, &private_config, args.member)?;
    let proposal_id = parse_proposal_id(&args.proposal_id)?;
    let status = query_transaction_status(
        &Async::default(),
        &public_config,
        &private_config,
        &member,
        proposal_id,
    )
    .await?;
    println!("{}", transaction_status_line(member.member_id, &status));
    Ok(())
}

pub(crate) async fn run_rebroadcast_command(
    cli_args: &CLIArgs,
    args: &FederationWalletRebroadcastArgs,
) -> Result<()> {
    ensure_rebroadcast_acknowledged(args.acknowledge_exact_byte_retry)?;
    let (public_config, private_config) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation rebroadcast",
    )?;
    let member = target_member(&public_config, &private_config, args.member)?;
    let proposal_id = parse_proposal_id(&args.proposal_id)?;
    let response = request_rebroadcast(
        &Async::default(),
        &public_config,
        &private_config,
        &member,
        proposal_id,
    )
    .await?;
    println!(
        "member {}: exact_byte_retry_advanced={} {}",
        member.member_id,
        response.advanced,
        transaction_status_line(member.member_id, &response.status)
    );
    Ok(())
}

pub(crate) async fn run_frost_rotate_command(
    cli_args: &CLIArgs,
    args: &FederationFrostRotateArgs,
) -> Result<()> {
    ensure_rotation_acknowledged(args.acknowledge_guarded_rotation)?;
    let (public_config, private_config) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation frost-rotate",
    )?;
    let member = target_member(&public_config, &private_config, args.member)?;
    let statuses = crate::federation_status::query_local_statuses(
        Async::default(),
        &public_config,
        &private_config,
        &[member.member_id.get()],
    )
    .await?;
    let status = statuses
        .first()
        .context("target member returned no federation status")?;
    ensure_expected_active_epochs(
        status.frost_rotation.as_ref(),
        args.expected_root_epoch,
        args.expected_application_epoch,
    )?;

    let operation: FederationFrostRotationOperation =
        crate::read_federation_json(&args.operation, "FROST rotation operation")?;
    let runtime = FederationRuntimeConfig::new(public_config.clone(), private_config.clone())
        .context("private federation config does not match public federation config")?;
    let envelope = runtime.frost_rotation_operation(operation)?;
    let request = FederationSubmitOperationRequest::new(envelope);
    request.ensure_admissible_size()?;
    let signed = FederationSignedRequest::new_signed(
        public_config.federation_id,
        member.member_id,
        operator_sequence(),
        FederationRestMessageKind::SubmitOperation,
        request,
        &private_config,
    )?;
    let url = member
        .federation_api_url
        .join_paths(&["federation", "v1", "operations"])?;
    let response: FederationSubmitOperationResponse =
        Async::default().http_post(url, None, &signed).await?;
    response.ensure_accepted()?;
    println!(
        "member {}: guarded_rotation_operation={} status={}",
        member.member_id,
        response.operation_id,
        response.outcome.entry().status()
    );
    Ok(())
}

async fn query_transaction_status<T>(
    transport: &T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    member: &MemberConfigPublic,
    proposal_id: FederationTransactionProposalId,
) -> Result<FederationWalletTransactionStatus>
where
    T: Transport,
{
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;
    let request = signed_transaction_request(
        public_config,
        private_config,
        member,
        proposal_id,
        FederationRestMessageKind::WalletTransactionStatus,
    )?;
    let url = member.federation_api_url.join_paths(&[
        "federation",
        "v1",
        "wallet",
        "transactions",
        "status",
    ])?;
    Ok(transport.http_post(url, None, &request).await?)
}

async fn request_rebroadcast<T>(
    transport: &T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    member: &MemberConfigPublic,
    proposal_id: FederationTransactionProposalId,
) -> Result<FederationWalletRebroadcastResponse>
where
    T: Transport,
{
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;
    let request = signed_transaction_request(
        public_config,
        private_config,
        member,
        proposal_id,
        FederationRestMessageKind::WalletRebroadcast,
    )?;
    let url = member.federation_api_url.join_paths(&[
        "federation",
        "v1",
        "wallet",
        "transactions",
        "rebroadcast",
    ])?;
    Ok(transport.http_post(url, None, &request).await?)
}

fn signed_transaction_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    member: &MemberConfigPublic,
    proposal_id: FederationTransactionProposalId,
    kind: FederationRestMessageKind,
) -> Result<FederationSignedRequest<FederationWalletTransactionRequest>> {
    Ok(FederationSignedRequest::new_signed(
        public_config.federation_id,
        member.member_id,
        operator_sequence(),
        kind,
        FederationWalletTransactionRequest::new(proposal_id),
        private_config,
    )?)
}

fn load_configs(
    cli_args: &CLIArgs,
    public_config: &Option<std::path::PathBuf>,
    private_config: &Option<std::path::PathBuf>,
    action: &str,
) -> Result<(FederationConfig, MemberConfigPrivate)> {
    let (public_path, private_path) =
        crate::federation_config_paths(cli_args, public_config, private_config, action)?;
    let public_config = crate::read_federation_json(&public_path, "public federation config")?;
    let private_config = crate::read_federation_json(&private_path, "private federation config")?;
    Ok((public_config, private_config))
}

fn target_member(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    requested: Option<u16>,
) -> Result<MemberConfigPublic> {
    let member_id = match requested {
        Some(member_id) => MemberId::new(member_id)?,
        None => private_config.member_id,
    };
    public_config
        .member(member_id)
        .cloned()
        .with_context(|| format!("federation public config has no member {member_id}"))
}

fn parse_proposal_id(value: &str) -> Result<FederationTransactionProposalId> {
    FederationTransactionProposalId::from_str(value)
        .with_context(|| format!("invalid transaction proposal id {value}"))
}

fn ensure_rebroadcast_acknowledged(acknowledged: bool) -> Result<()> {
    if !acknowledged {
        bail!("rebroadcast requires --acknowledge-exact-byte-retry");
    }
    Ok(())
}

fn ensure_rotation_acknowledged(acknowledged: bool) -> Result<()> {
    if !acknowledged {
        bail!("FROST rotation requires --acknowledge-guarded-rotation");
    }
    Ok(())
}

fn ensure_expected_active_epochs(
    status: Option<&cdk_common::federation::FederationFrostRotationStatus>,
    expected_root_epoch: u64,
    expected_application_epoch: u64,
) -> Result<()> {
    let status = status.context("target member has no FROST lifecycle status")?;
    if !status
        .roots
        .iter()
        .any(|root| root.active && root.root_key_epoch == expected_root_epoch)
    {
        bail!("active FROST root epoch does not match the explicit expectation");
    }
    if !status.applications.iter().any(|application| {
        application.active
            && application.root_key_epoch == expected_root_epoch
            && application.application_key_epoch == expected_application_epoch
    }) {
        bail!("active FROST application epoch does not match the explicit expectation");
    }
    Ok(())
}

fn transaction_status_line(member: MemberId, status: &FederationWalletTransactionStatus) -> String {
    format!(
        "member {} transaction {}: phase={} state={} root_epoch={} application_epoch={} signed={} broadcast_authorized={} broadcast_observations={} successful_broadcast_observations={} confirmation_observations={}",
        member,
        status.proposal_id,
        status.phase,
        transaction_state_name(status.state),
        status.root_key_epoch,
        status.application_key_epoch,
        status.signed,
        status.broadcast_authorized,
        status.broadcast_observations,
        status.successful_broadcast_observations,
        status.confirmation_observations
    )
}

fn transaction_state_name(state: FederationAcceptedTransactionState) -> &'static str {
    match state {
        FederationAcceptedTransactionState::Accepted => "accepted",
        FederationAcceptedTransactionState::Signed => "signed",
        FederationAcceptedTransactionState::BroadcastPending => "broadcast_pending",
        FederationAcceptedTransactionState::Broadcast => "broadcast",
        FederationAcceptedTransactionState::Confirmed => "confirmed",
        FederationAcceptedTransactionState::Abandoned => "abandoned",
        FederationAcceptedTransactionState::Replaced { .. } => "replaced",
    }
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
    use super::{ensure_rebroadcast_acknowledged, ensure_rotation_acknowledged};

    #[test]
    fn destructive_adjacent_commands_require_explicit_acknowledgements() {
        assert!(ensure_rebroadcast_acknowledged(false).is_err());
        assert!(ensure_rebroadcast_acknowledged(true).is_ok());
        assert!(ensure_rotation_acknowledged(false).is_err());
        assert!(ensure_rotation_acknowledged(true).is_ok());
    }
}
