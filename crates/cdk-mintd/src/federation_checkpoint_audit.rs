use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationCheckpointAuditRequest, FederationCheckpointAuditResponse, FederationConfig,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate, MemberId,
    FEDERATION_REST_API_VERSION,
};
use cdk_http_client::Transport;

use crate::cli::{CLIArgs, FederationCheckpointAuditArgs};

pub(crate) async fn run_checkpoint_audit_command(
    cli_args: &CLIArgs,
    args: &FederationCheckpointAuditArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation checkpoint audit",
    )
    .await?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    private_config
        .validate_against_public(&public_config)
        .context("private federation config does not match public federation config")?;
    let member_id = MemberId::new(args.member)?;
    let member = public_config
        .member(member_id)
        .ok_or_else(|| anyhow::anyhow!("federation public config has no member {member_id}"))?;
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id,
        crate::federation_status::initial_status_sequence(),
        FederationRestMessageKind::CheckpointAudit,
        FederationCheckpointAuditRequest::new(),
        &private_config,
    )?;
    let url =
        member
            .federation_api_url
            .join_paths(&["federation", "v1", "checkpoints", "audit"])?;
    let response: FederationCheckpointAuditResponse =
        crate::federation_operator_transport(cli_args)
            .await?
            .http_post(url, None, &request)
            .await?
            .json_or_status_error()?;
    if response.version != FEDERATION_REST_API_VERSION
        || response.federation_id != public_config.federation_id
        || response.member_id != member_id
    {
        bail!("federation member {member_id} returned a mismatched checkpoint audit response");
    }

    println!(
        "member {}: checkpoint={} journal_next_index={} finalized_next_item_index={} materialized_state_digest={} audit=matched",
        response.member_id,
        response.checkpoint.checkpoint_id,
        response.checkpoint.frontier.journal.next_index,
        response.checkpoint.frontier.finalized.next_item_index,
        response.materialized_state_digest,
    );
    Ok(())
}
