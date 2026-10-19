//! Authenticated operator commands for the federated on-chain wallet.

use crate::federation_operator::{load_configs, operator_sequence, target_member};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationConfig, FederationRestMessageKind,
    FederationSignedRequest, FederationTransactionProposalId, FederationWalletRebroadcastResponse,
    FederationWalletTransactionRequest, FederationWalletTransactionStatus, MemberConfigPrivate,
    MemberConfigPublic, MemberId,
};
use cdk_http_client::Transport;

use crate::cli::{CLIArgs, FederationWalletRebroadcastArgs, FederationWalletTransactionArgs};

pub(crate) async fn run_transaction_status_command(
    cli_args: &CLIArgs,
    args: &FederationWalletTransactionArgs,
) -> Result<()> {
    let (public_config, private_config) = load_configs(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation transaction-status",
    )
    .await?;
    let member = target_member(&public_config, &private_config, args.member)?;
    let proposal_id = parse_proposal_id(&args.proposal_id)?;
    let transport = crate::federation_operator_transport(cli_args).await?;
    let status = query_transaction_status(
        &transport,
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
    )
    .await?;
    let member = target_member(&public_config, &private_config, args.member)?;
    let proposal_id = parse_proposal_id(&args.proposal_id)?;
    let transport = crate::federation_operator_transport(cli_args).await?;
    let response = request_rebroadcast(
        &transport,
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
    Ok(transport
        .http_post(url, None, &request)
        .await?
        .json_or_status_error()?)
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
    Ok(transport
        .http_post(url, None, &request)
        .await?
        .json_or_status_error()?)
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
        operator_sequence()?,
        kind,
        FederationWalletTransactionRequest::new(proposal_id),
        private_config,
    )?)
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

#[cfg(test)]
mod tests {
    use super::ensure_rebroadcast_acknowledged;

    #[test]
    fn destructive_adjacent_commands_require_explicit_acknowledgements() {
        assert!(ensure_rebroadcast_acknowledged(false).is_err());
        assert!(ensure_rebroadcast_acknowledged(true).is_ok());
    }
}
