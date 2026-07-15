use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
pub(crate) use cdk_common::federation::FederationLocalStatusResponse;
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationId,
    FederationPaymentObservationReportAction, FederationPaymentObservationReportStatus,
    FederationPaymentObservationReportSummary, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate, MemberConfigPublic,
    MemberId, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};

use crate::cli::{CLIArgs, FederationStatusArgs};

pub(crate) async fn run_status_command(
    cli_args: &CLIArgs,
    args: &FederationStatusArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation status",
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    let statuses = query_local_statuses(
        Async::default(),
        &public_config,
        &private_config,
        &args.members,
    )
    .await?;

    print_statuses(&public_config, &private_config, &statuses);
    if args.require_ready {
        ensure_statuses_ready(&statuses)?;
    }
    if args.require_no_digest_alerts {
        ensure_statuses_no_digest_alerts(&statuses)?;
    }
    if args.require_converged {
        ensure_statuses_converged(&statuses)?;
    }
    if let Some(max_operation_lag) = args.max_operation_lag {
        ensure_statuses_operation_lag(&statuses, max_operation_lag)?;
    }
    if args.require_catch_up_certificate_quorum {
        ensure_statuses_catch_up_certificate_quorum(&public_config, &statuses)?;
    }
    if args.require_no_payment_observation_alerts {
        ensure_statuses_no_payment_observation_alerts(&statuses)?;
    }
    if args.require_no_invalid_operation_alerts {
        ensure_statuses_no_invalid_operation_alerts(&statuses)?;
    }

    Ok(())
}

pub(crate) async fn query_local_statuses<T>(
    transport: T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    selected_members: &[u16],
) -> Result<Vec<FederationLocalStatusResponse>>
where
    T: Transport,
{
    query_local_statuses_with_materialized_rows(
        transport,
        public_config,
        private_config,
        selected_members,
        false,
    )
    .await
}

pub(crate) async fn query_local_statuses_with_materialized_rows<T>(
    transport: T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    selected_members: &[u16],
    include_materialized_rows: bool,
) -> Result<Vec<FederationLocalStatusResponse>>
where
    T: Transport,
{
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;

    query_local_statuses_prevalidated(
        transport,
        public_config,
        private_config,
        selected_members,
        include_materialized_rows,
    )
    .await
}

pub(crate) async fn query_local_statuses_prevalidated<T>(
    transport: T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    selected_members: &[u16],
    include_materialized_rows: bool,
) -> Result<Vec<FederationLocalStatusResponse>>
where
    T: Transport,
{
    let targets = status_targets(public_config, selected_members)?;
    let mut sequence = initial_status_sequence();
    let mut statuses = Vec::with_capacity(targets.len());
    for member in targets {
        sequence = sequence.saturating_add(1);
        let status = query_local_status(
            &transport,
            public_config,
            private_config,
            &member,
            sequence,
            include_materialized_rows,
        )
        .await?;
        statuses.push(status);
    }

    Ok(statuses)
}

async fn query_local_status<T>(
    transport: &T,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    member: &MemberConfigPublic,
    sequence: u64,
    include_materialized_rows: bool,
) -> Result<FederationLocalStatusResponse>
where
    T: Transport,
{
    let payload = if include_materialized_rows {
        FederationPeerStatusRequest::with_materialized_rows()
    } else {
        FederationPeerStatusRequest::default()
    };
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        member.member_id,
        sequence,
        FederationRestMessageKind::Status,
        payload,
        private_config,
    )?;
    let url = member
        .federation_api_url
        .join_paths(&["federation", "v1", "local-status"])?;
    let response = transport.http_post(url, None, &request).await?;
    validate_local_status(public_config.federation_id, member, &response)?;
    Ok(response)
}

fn status_targets(
    public_config: &FederationConfig,
    selected_members: &[u16],
) -> Result<Vec<MemberConfigPublic>> {
    if selected_members.is_empty() {
        return Ok(public_config.members.values().cloned().collect());
    }

    selected_members
        .iter()
        .map(|member_id| {
            let member_id = MemberId::new(*member_id)?;
            public_config.member(member_id).cloned().ok_or_else(|| {
                anyhow::anyhow!("federation public config has no member {}", member_id)
            })
        })
        .collect()
}

fn validate_local_status(
    expected_federation_id: FederationId,
    member: &MemberConfigPublic,
    response: &FederationLocalStatusResponse,
) -> Result<()> {
    if response.version != FEDERATION_REST_API_VERSION {
        bail!(
            "federation member {} returned unsupported local status version {}",
            member.member_id,
            response.version
        );
    }
    if response.peer_status.version != FEDERATION_REST_API_VERSION {
        bail!(
            "federation member {} returned unsupported peer status version {}",
            member.member_id,
            response.peer_status.version
        );
    }
    if response.peer_status.federation_id != expected_federation_id {
        bail!(
            "federation member {} returned status for federation {}, expected {}",
            member.member_id,
            response.peer_status.federation_id,
            expected_federation_id
        );
    }
    if response.peer_status.member_id != member.member_id {
        bail!(
            "federation member status mismatch: expected {}, found {}",
            member.member_id,
            response.peer_status.member_id
        );
    }

    Ok(())
}

fn print_statuses(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    statuses: &[FederationLocalStatusResponse],
) {
    println!(
        "Federation {} status signed by member {}",
        public_config.federation_id, private_config.member_id
    );
    for status in statuses {
        println!("{}", member_status_line(status));
        for source in &status.onchain_wallets {
            println!(
                "  chain_source source={} scope={} id={} correlated={} health={} tip_height={} tip_hash={} failures={}",
                source.source,
                source.source_scope,
                source.source_id,
                source.correlated_source_warning,
                source.health,
                source.local_tip_height,
                source.local_tip_hash,
                source.consecutive_failures
            );
        }
        if let Some(wallet) = &status.wallet_operator {
            println!("{}", wallet_operator_status_line(wallet));
        }
        for report in &status.payment_observation_reports {
            print_payment_observation_report(report);
        }
    }
}

fn wallet_operator_status_line(
    wallet: &cdk_common::federation::FederationWalletOperatorStatus,
) -> String {
    let utxos = wallet
        .utxos
        .iter()
        .map(|status| format!("{}:{}/{}sat", status.state, status.count, status.value_sat))
        .collect::<Vec<_>>()
        .join(",");
    let phases = wallet
        .operation_phases
        .iter()
        .map(|status| format!("{}:{}", status.phase, status.count))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "  wallet root_epoch={} application_epoch={} descriptor_commitment={} network={} confirmations={} fee_policy_version={} external_revealed={} internal_revealed={} utxos={} reserved_value_sat={} phases={} pending_batches={} signing_attempts={} signing_failures={} broadcast_health={} broadcast_failures={}",
        wallet.root_key_epoch,
        wallet.application_key_epoch,
        encode_commitment(&wallet.descriptor_commitment),
        wallet.network,
        wallet.minimum_confirmations,
        wallet.fee_policy_version,
        wallet.external_revealed_count,
        wallet.internal_revealed_count,
        utxos,
        wallet.reserved_value_sat,
        phases,
        wallet.pending_batches,
        wallet.signing_attempts,
        wallet.signing_failures,
        wallet.broadcast_health,
        wallet.broadcast_failures
    )
}

fn encode_commitment(commitment: &[u8; 32]) -> String {
    commitment
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn member_status_line(status: &FederationLocalStatusResponse) -> String {
    let peer = &status.peer_status;
    let latest_index = peer
        .latest_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "none".to_string());
    let catch_up_certificate_quorum_operations = status
        .catch_up_certificate_quorum_operation_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "none".to_string());
    let catch_up_certificate_quorum_from =
        option_u64(status.catch_up_certificate_quorum_from_index);
    let catch_up_certificate_quorum_latest =
        option_u64(status.catch_up_certificate_quorum_latest_index);
    let catch_up_certificate_quorum_signers =
        member_list(&status.catch_up_certificate_quorum_signers);
    let sync_peer = status
        .sync_peer
        .map(|peer| peer.to_string())
        .unwrap_or_else(|| "none".to_string());
    let sync_local_operation_count = option_u64(status.sync_local_operation_count);
    let sync_peer_operation_count = option_u64(status.sync_peer_operation_count);
    let sync_operation_count = option_u64(status.sync_operation_count);
    let (payment_reports, payment_conflicts, payment_unresolved, payment_stale, payment_quarantine) =
        payment_report_counts(status);
    let invalid_operation_alerts = invalid_operation_alert_count(status);
    let aleph_bft = aleph_bft_status_fields(status);
    let materialized_state_digest = peer
        .materialized_state_digest
        .map(|digest| digest.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "member {}: sync={} ready={} sync_peer={} sync_local_operations={} sync_peer_operations={} sync_operations={} operations={} latest_index={} order_digest={} materialized_state_digest={} materialized_rows={} pending_mempool_items={} pending_response_waiters={} response_cache_items={} response_cache_hits={}/{} response_cache_hit_rate_bps={} duplicate_submissions={} duplicate_pending_waiter_joins={} catch_up_certificate_quorum_operations={} catch_up_certificate_quorum_from={} catch_up_certificate_quorum_latest={} catch_up_certificate_quorum_signers={} payment_reports={} payment_conflicts={} payment_unresolved={} payment_stale={} payment_quarantine={} invalid_operation_alerts={} aleph_bft_live_session={} aleph_bft_network_inbound={} aleph_bft={} message={}",
        peer.member_id,
        status.sync_state,
        status.sync_ready,
        sync_peer,
        sync_local_operation_count,
        sync_peer_operation_count,
        sync_operation_count,
        peer.operation_count,
        latest_index,
        peer.latest_order_digest,
        materialized_state_digest,
        status.materialized_state_rows.len(),
        status.pending_mempool_items,
        status.pending_response_waiters,
        status.response_cache_items,
        status.response_cache_hits,
        status.response_cache_lookups,
        status.response_cache_hit_rate_bps,
        status.duplicate_submissions,
        status.duplicate_pending_waiter_joins,
        catch_up_certificate_quorum_operations,
        catch_up_certificate_quorum_from,
        catch_up_certificate_quorum_latest,
        catch_up_certificate_quorum_signers,
        payment_reports,
        payment_conflicts,
        payment_unresolved,
        payment_stale,
        payment_quarantine,
        invalid_operation_alerts,
        status.aleph_bft_live_session_active,
        status.aleph_bft_network_inbound_active,
        aleph_bft,
        redact_federation_operator_message(&status.sync_message)
    )
}

fn option_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn aleph_bft_status_fields(status: &FederationLocalStatusResponse) -> String {
    match &status.aleph_bft_status {
        Some(aleph) => format!(
            "local_member:{} session:{} queued:{} pending_finalization:{} finalized:{} accepted:{} next_unit:{} next_finalization_unit:{} next_item:{} latest_accepted:{} members:{}",
            aleph.local_member,
            aleph.session,
            aleph.queued_item_count,
            aleph.pending_finalization_item_count,
            aleph.finalized_item_count,
            aleph.accepted_operation_count,
            aleph.next_unit_index,
            aleph.next_unit_finalization_index,
            aleph.next_item_index,
            option_u64(aleph.latest_accepted_operation_index),
            option_u64(aleph.allowed_member_count)
        ),
        None => "none".to_string(),
    }
}

fn invalid_operation_alert_count(status: &FederationLocalStatusResponse) -> u64 {
    status
        .invalid_operation_alerts
        .iter()
        .map(|alert| alert.alerting_buckets)
        .sum()
}

fn ensure_statuses_no_invalid_operation_alerts(
    statuses: &[FederationLocalStatusResponse],
) -> Result<()> {
    for status in statuses {
        for alert in &status.invalid_operation_alerts {
            if alert.alerting_buckets > 0 {
                bail!(
                    "federation member {} invalid operation alert: source={} alerting_buckets={} max_count={} threshold={} window_secs={}",
                    status.peer_status.member_id,
                    alert.source,
                    alert.alerting_buckets,
                    alert.max_count,
                    alert.threshold,
                    alert.window_secs
                );
            }
        }
    }

    Ok(())
}

fn payment_report_counts(
    status: &FederationLocalStatusResponse,
) -> (usize, usize, usize, usize, usize) {
    let payment_reports = status.payment_observation_reports.len();
    let payment_conflicts = status
        .payment_observation_reports
        .iter()
        .filter(|report| report.status == FederationPaymentObservationReportStatus::Conflicting)
        .count();
    let payment_unresolved = status
        .payment_observation_reports
        .iter()
        .filter(|report| {
            matches!(
                report.status,
                FederationPaymentObservationReportStatus::AwaitingQuorum
                    | FederationPaymentObservationReportStatus::Conflicting
            )
        })
        .count();
    let payment_stale = status
        .payment_observation_reports
        .iter()
        .filter(|report| report.stale)
        .count();
    let payment_quarantine = status
        .payment_observation_reports
        .iter()
        .filter(|report| {
            report.recommended_action == FederationPaymentObservationReportAction::Quarantine
        })
        .count();

    (
        payment_reports,
        payment_conflicts,
        payment_unresolved,
        payment_stale,
        payment_quarantine,
    )
}

fn print_payment_observation_report(report: &FederationPaymentObservationReportSummary) {
    println!("{}", payment_observation_report_line(report));
    for group in &report.groups {
        println!("{}", payment_observation_group_line(group));
    }
}

pub(crate) fn payment_observation_report_line(
    report: &FederationPaymentObservationReportSummary,
) -> String {
    let quote_age_secs = report
        .quote_age_secs
        .map(|age| age.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "  payment_report context={} quote={} status={} observations={} required={} groups={} conflicts={} terminal_conflicts={} quote_age_secs={} stale={} action={} quarantined={} quarantine_ops={} quarantine_ids={} quarantine_indices={}",
        report.context,
        report.quote,
        report.status,
        report.total_observations,
        report.required,
        report.groups.len(),
        report.conflicting_observers.len(),
        report.competing_terminal_state_observers.len(),
        quote_age_secs,
        report.stale,
        report.recommended_action,
        report.quarantined,
        report.quarantine_operation_ids.len(),
        display_list(&report.quarantine_operation_ids),
        display_list(&report.quarantine_operation_indices)
    )
}

pub(crate) fn payment_observation_group_line(
    group: &cdk_common::federation::FederationPaymentObservationEvidenceGroupSummary,
) -> String {
    format!(
        "    payment_group state={} observers={} operation_ids={} operation_indices={} quorum={} conflicts={} terminal={}",
        group.state,
        member_list(&group.observers),
        display_list(&group.operation_ids),
        display_list(&group.operation_indices),
        group.quorum_reached,
        member_list(&group.conflicting_observers),
        group.terminal_state
    )
}

fn member_list(members: &[MemberId]) -> String {
    display_list(members)
}

fn display_list<T>(values: &[T]) -> String
where
    T: ToString,
{
    if values.is_empty() {
        return "none".to_string();
    }

    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn ensure_statuses_ready(statuses: &[FederationLocalStatusResponse]) -> Result<()> {
    for status in statuses {
        if !status.sync_ready {
            bail!(
                "federation member {} is not ready: {}",
                status.peer_status.member_id,
                redact_federation_operator_message(&status.sync_message)
            );
        }
    }

    Ok(())
}

fn ensure_statuses_converged(statuses: &[FederationLocalStatusResponse]) -> Result<()> {
    ensure_statuses_have_materialized_state_digest(statuses)?;
    let Some(first) = statuses.first() else {
        return Ok(());
    };
    for status in statuses.iter().skip(1) {
        if status.peer_status.operation_count != first.peer_status.operation_count
            || status.peer_status.latest_order_digest != first.peer_status.latest_order_digest
            || status.peer_status.materialized_state_digest
                != first.peer_status.materialized_state_digest
        {
            bail!(
                "federation status is not converged: member {} has operations {} order digest {} materialized state digest {}, expected operations {} order digest {} materialized state digest {}",
                status.peer_status.member_id,
                status.peer_status.operation_count,
                status.peer_status.latest_order_digest,
                status
                    .peer_status
                    .materialized_state_digest
                    .map(|digest| digest.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                first.peer_status.operation_count,
                first.peer_status.latest_order_digest,
                first
                    .peer_status
                    .materialized_state_digest
                    .map(|digest| digest.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
        }
    }

    Ok(())
}

fn ensure_statuses_operation_lag(
    statuses: &[FederationLocalStatusResponse],
    max_operation_lag: u64,
) -> Result<()> {
    let Some(max_operation_count) = statuses
        .iter()
        .map(|status| status.peer_status.operation_count)
        .max()
    else {
        return Ok(());
    };

    for status in statuses {
        let operation_count = status.peer_status.operation_count;
        let lag = max_operation_count.saturating_sub(operation_count);
        if lag > max_operation_lag {
            bail!(
                "federation member {} is {} operations behind the highest queried member height {}, allowed lag {}",
                status.peer_status.member_id,
                lag,
                max_operation_count,
                max_operation_lag
            );
        }
    }

    Ok(())
}

fn ensure_statuses_no_digest_alerts(statuses: &[FederationLocalStatusResponse]) -> Result<()> {
    ensure_statuses_have_materialized_state_digest(statuses)?;
    for status in statuses {
        if status.sync_state == "materialized_state_mismatch"
            || (status.sync_state == "halted" && status.sync_message.contains("digest mismatch"))
        {
            bail!(
                "federation member {} reports digest alert: sync={} message={}",
                status.peer_status.member_id,
                status.sync_state,
                redact_federation_operator_message(&status.sync_message)
            );
        }
    }

    for (index, status) in statuses.iter().enumerate() {
        for other in statuses.iter().skip(index + 1) {
            if status.peer_status.operation_count != other.peer_status.operation_count {
                continue;
            }
            if status.peer_status.latest_order_digest != other.peer_status.latest_order_digest {
                bail!(
                    "federation digest alert: members {} and {} disagree on order digest at operation count {}",
                    status.peer_status.member_id,
                    other.peer_status.member_id,
                    status.peer_status.operation_count
                );
            }
            if status.peer_status.materialized_state_digest
                != other.peer_status.materialized_state_digest
            {
                bail!(
                    "federation digest alert: members {} and {} disagree on materialized state digest at operation count {}",
                    status.peer_status.member_id,
                    other.peer_status.member_id,
                    status.peer_status.operation_count
                );
            }
        }
    }

    Ok(())
}

fn ensure_statuses_have_materialized_state_digest(
    statuses: &[FederationLocalStatusResponse],
) -> Result<()> {
    for status in statuses {
        if status.peer_status.materialized_state_digest.is_none() {
            bail!(
                "federation member {} omitted its materialized state digest",
                status.peer_status.member_id
            );
        }
    }
    Ok(())
}

fn ensure_statuses_catch_up_certificate_quorum(
    public_config: &FederationConfig,
    statuses: &[FederationLocalStatusResponse],
) -> Result<()> {
    let required = usize::from(public_config.threshold_params.consensus_threshold);
    for status in statuses {
        let operation_count = status.peer_status.operation_count;
        if operation_count == 0 {
            continue;
        }
        let Some(quorum_operation_count) = status.catch_up_certificate_quorum_operation_count
        else {
            bail!(
                "federation member {} has {} operations but no catch-up certificate quorum evidence",
                status.peer_status.member_id,
                operation_count
            );
        };
        if quorum_operation_count < operation_count {
            bail!(
                "federation member {} has catch-up certificate quorum evidence for {} operations, below current operation count {}",
                status.peer_status.member_id,
                quorum_operation_count,
                operation_count
            );
        }

        let mut distinct_signers = BTreeSet::new();
        for signer in &status.catch_up_certificate_quorum_signers {
            if public_config.member(*signer).is_none() {
                bail!(
                    "federation member {} reports unknown catch-up certificate quorum signer {}",
                    status.peer_status.member_id,
                    signer
                );
            }
            distinct_signers.insert(*signer);
        }
        if distinct_signers.len() < required {
            bail!(
                "federation member {} has {} distinct catch-up certificate quorum signers, required {}",
                status.peer_status.member_id,
                distinct_signers.len(),
                required
            );
        }
    }

    Ok(())
}

fn ensure_statuses_no_payment_observation_alerts(
    statuses: &[FederationLocalStatusResponse],
) -> Result<()> {
    for status in statuses {
        for report in &status.payment_observation_reports {
            if payment_observation_report_is_alert(report) {
                bail!(
                    "federation member {} reports payment observation alert: context={} quote={} status={} action={} stale={} quarantined={} conflicts={} terminal_conflicts={}",
                    status.peer_status.member_id,
                    report.context,
                    report.quote,
                    report.status,
                    report.recommended_action,
                    report.stale,
                    report.quarantined,
                    report.conflicting_observers.len(),
                    report.competing_terminal_state_observers.len()
                );
            }
        }
    }

    Ok(())
}

fn payment_observation_report_is_alert(report: &FederationPaymentObservationReportSummary) -> bool {
    report.recommended_action != FederationPaymentObservationReportAction::None
        || report.status == FederationPaymentObservationReportStatus::Conflicting
        || report.stale
        || report.quarantined
        || !report.conflicting_observers.is_empty()
        || !report.competing_terminal_state_observers.is_empty()
}

fn initial_status_sequence() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
