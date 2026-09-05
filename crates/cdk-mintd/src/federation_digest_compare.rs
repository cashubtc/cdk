use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationMaterializedStateRowFingerprint, MemberConfigPrivate, MemberId,
};

use crate::cli::{CLIArgs, FederationCompareDigestsArgs};
use crate::federation_status::{
    query_local_statuses_with_materialized_rows, FederationLocalStatusResponse,
};

pub(crate) async fn run_compare_digests_command(
    cli_args: &CLIArgs,
    args: &FederationCompareDigestsArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation compare-digests",
    )
    .await?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    // The materialized snapshot and durable frontier are read separately on each member.
    // Run this exact comparison only after quiescing writes so every signed response
    // represents one stable verification point rather than an in-flight transition.
    let statuses = query_local_statuses_with_materialized_rows(
        crate::federation_operator_transport(cli_args).await?,
        &public_config,
        &private_config,
        &args.members,
        true,
    )
    .await
    .context("failed to query exact federation materialized-state digests")?;

    let report = DigestComparisonReport::from_statuses(&statuses);
    print_digest_comparison(&public_config, &private_config, &report);
    if !args.allow_divergence {
        report.ensure_converged()?;
    }

    Ok(())
}

fn print_digest_comparison(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    report: &DigestComparisonReport,
) {
    println!(
        "Federation {} digest comparison signed by member {}",
        public_config.federation_id, private_config.member_id
    );
    println!(
        "digest_summary members={} groups={} converged={}",
        report.queried_members,
        report.groups.len(),
        report.is_converged()
    );
    for group in &report.groups {
        println!("{}", digest_group_line(group));
    }
    for diagnostic in &report.diagnostics {
        println!("{}", digest_diagnostic_line(diagnostic));
    }
    for row_diff in &report.row_diffs {
        println!("{}", materialized_row_diff_line(row_diff));
    }
}

fn digest_group_line(group: &DigestComparisonGroup) -> String {
    format!(
        "digest_group operations={} order_digest={} finalized_next_item_index={} finalized_item_digest={} materialized_state_digest={} members={} ready={}/{} sync_states={}",
        group.key.operation_count,
        group.key.order_digest,
        group.key.finalized_next_item_index,
        group.key.finalized_item_digest,
        group.key.materialized_state_digest,
        display_list(&group.members),
        group.ready_count,
        group.members.len(),
        display_set(&group.sync_states)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestComparisonReport {
    queried_members: usize,
    groups: Vec<DigestComparisonGroup>,
    digest_alert_members: Vec<MemberId>,
    diagnostics: Vec<DigestComparisonDiagnostic>,
    row_diffs: Vec<DigestComparisonRowDiff>,
}

impl DigestComparisonReport {
    fn from_statuses(statuses: &[FederationLocalStatusResponse]) -> Self {
        let mut groups = BTreeMap::<DigestComparisonKey, DigestComparisonGroup>::new();
        let mut digest_alert_members = Vec::new();

        for status in statuses {
            if status_reports_digest_alert(status)
                || status.peer_status.materialized_state_digest.is_none()
            {
                digest_alert_members.push(status.peer_status.member_id);
            }
            let key = DigestComparisonKey {
                operation_count: status.peer_status.operation_count,
                order_digest: status.peer_status.latest_order_digest.to_string(),
                finalized_next_item_index: status.checkpoint.frontier.finalized_next_item_index,
                finalized_item_digest: status.checkpoint.frontier.finalized_item_digest.to_string(),
                materialized_state_digest: status
                    .peer_status
                    .materialized_state_digest
                    .map(|digest| digest.to_string())
                    .unwrap_or_else(|| "none".to_string()),
            };
            let group = groups
                .entry(key.clone())
                .or_insert_with(|| DigestComparisonGroup {
                    key,
                    members: Vec::new(),
                    ready_count: 0,
                    sync_states: BTreeSet::new(),
                });
            group.members.push(status.peer_status.member_id);
            if status.sync_ready {
                group.ready_count = group.ready_count.saturating_add(1);
            }
            group.sync_states.insert(status.sync_state.clone());
        }

        let groups = groups.into_values().collect::<Vec<_>>();
        let diagnostics = digest_comparison_diagnostics(&groups, &digest_alert_members);
        let row_diffs = materialized_row_diffs(statuses);

        Self {
            queried_members: statuses.len(),
            groups,
            digest_alert_members,
            diagnostics,
            row_diffs,
        }
    }

    fn is_converged(&self) -> bool {
        self.groups.len() <= 1 && self.digest_alert_members.is_empty()
    }

    fn ensure_converged(&self) -> Result<()> {
        if !self.digest_alert_members.is_empty() {
            bail!(
                "federation digest comparison saw digest alerts from members {}",
                display_list(&self.digest_alert_members)
            );
        }
        if self.groups.len() > 1 {
            let diagnostics = self
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.kind.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            bail!(
                "federation digest comparison is not converged: {} digest groups across {} queried members ({})",
                self.groups.len(),
                self.queried_members,
                diagnostics
            );
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestComparisonGroup {
    key: DigestComparisonKey,
    members: Vec<MemberId>,
    ready_count: usize,
    sync_states: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestComparisonDiagnostic {
    kind: DigestComparisonDiagnosticKind,
    members: Vec<MemberId>,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestComparisonRowDiff {
    table: String,
    key_digest: String,
    variants: Vec<DigestComparisonRowVariant>,
    missing_members: Vec<MemberId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestComparisonRowVariant {
    row_digest: String,
    members: Vec<MemberId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DigestComparisonDiagnosticKind {
    DigestAlert,
    HeightMismatch,
    OrderDigestMismatch,
    FinalizedFrontierMismatch,
    MaterializedStateMismatch,
}

impl DigestComparisonDiagnosticKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DigestAlert => "digest_alert",
            Self::HeightMismatch => "height_mismatch",
            Self::OrderDigestMismatch => "order_digest_mismatch",
            Self::FinalizedFrontierMismatch => "finalized_frontier_mismatch",
            Self::MaterializedStateMismatch => "materialized_state_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DigestComparisonKey {
    operation_count: u64,
    order_digest: String,
    finalized_next_item_index: u64,
    finalized_item_digest: String,
    materialized_state_digest: String,
}

fn digest_comparison_diagnostics(
    groups: &[DigestComparisonGroup],
    digest_alert_members: &[MemberId],
) -> Vec<DigestComparisonDiagnostic> {
    let mut diagnostics = Vec::new();
    if !digest_alert_members.is_empty() {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::DigestAlert,
            members: sorted_member_list(digest_alert_members.iter().copied()),
            detail: "one or more members reported a local digest alert".to_string(),
        });
    }

    if groups.len() <= 1 {
        return diagnostics;
    }

    let members = grouped_members(groups);
    let operation_counts = groups
        .iter()
        .map(|group| group.key.operation_count)
        .collect::<BTreeSet<_>>();
    if operation_counts.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::HeightMismatch,
            members,
            detail: format!(
                "operation_counts={}",
                operation_counts
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        });
        return diagnostics;
    }

    let operation_count = operation_counts.iter().next().copied().unwrap_or_default();
    let order_digests = groups
        .iter()
        .map(|group| group.key.order_digest.clone())
        .collect::<BTreeSet<_>>();
    if order_digests.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::OrderDigestMismatch,
            members,
            detail: format!(
                "operation_count={} order_digests={}",
                operation_count,
                display_set(&order_digests)
            ),
        });
        return diagnostics;
    }

    let finalized_next_item_indexes = groups
        .iter()
        .map(|group| group.key.finalized_next_item_index)
        .collect::<BTreeSet<_>>();
    let finalized_item_digests = groups
        .iter()
        .map(|group| group.key.finalized_item_digest.clone())
        .collect::<BTreeSet<_>>();
    if finalized_next_item_indexes.len() > 1 || finalized_item_digests.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::FinalizedFrontierMismatch,
            members,
            detail: format!(
                "operation_count={} order_digest={} finalized_next_item_indexes={} finalized_item_digests={}",
                operation_count,
                order_digests
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "none".to_string()),
                finalized_next_item_indexes
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                display_set(&finalized_item_digests)
            ),
        });
        return diagnostics;
    }

    let materialized_state_digests = groups
        .iter()
        .map(|group| group.key.materialized_state_digest.clone())
        .collect::<BTreeSet<_>>();
    if materialized_state_digests.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::MaterializedStateMismatch,
            members,
            detail: format!(
                "operation_count={} order_digest={} materialized_materialized_state_digests={}",
                operation_count,
                order_digests
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "none".to_string()),
                display_set(&materialized_state_digests)
            ),
        });
    }

    diagnostics
}

fn digest_diagnostic_line(diagnostic: &DigestComparisonDiagnostic) -> String {
    format!(
        "digest_diagnostic kind={} members={} detail={}",
        diagnostic.kind.as_str(),
        display_list(&diagnostic.members),
        diagnostic.detail
    )
}

fn materialized_row_diffs(
    statuses: &[FederationLocalStatusResponse],
) -> Vec<DigestComparisonRowDiff> {
    let mut groups = BTreeMap::<(u64, String), Vec<&FederationLocalStatusResponse>>::new();
    for status in statuses {
        groups
            .entry((
                status.peer_status.operation_count,
                status.peer_status.latest_order_digest.to_string(),
            ))
            .or_default()
            .push(status);
    }

    let mut diffs = Vec::new();
    for group in groups.into_values() {
        if group.len() <= 1 {
            continue;
        }

        let materialized_state_digests = group
            .iter()
            .map(|status| {
                status
                    .peer_status
                    .materialized_state_digest
                    .map(|digest| digest.to_string())
                    .unwrap_or_else(|| "none".to_string())
            })
            .collect::<BTreeSet<_>>();
        if materialized_state_digests.len() <= 1 {
            continue;
        }

        let identities = group
            .iter()
            .flat_map(|status| status.materialized_state_rows.iter())
            .map(row_identity)
            .collect::<BTreeSet<_>>();
        for identity in identities {
            let mut variants = BTreeMap::<String, Vec<MemberId>>::new();
            let mut missing_members = Vec::new();
            for status in &group {
                match member_row_digest_value(status, &identity) {
                    Some(row_digest) => variants
                        .entry(row_digest)
                        .or_default()
                        .push(status.peer_status.member_id),
                    None => missing_members.push(status.peer_status.member_id),
                }
            }

            if variants.len() <= 1 && missing_members.is_empty() {
                continue;
            }

            diffs.push(DigestComparisonRowDiff {
                table: identity.table,
                key_digest: identity.key_digest,
                variants: variants
                    .into_iter()
                    .map(|(row_digest, members)| DigestComparisonRowVariant {
                        row_digest,
                        members: sorted_member_list(members),
                    })
                    .collect(),
                missing_members: sorted_member_list(missing_members),
            });
        }
    }

    diffs
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MaterializedRowIdentity {
    table: String,
    key_digest: String,
}

fn row_identity(row: &FederationMaterializedStateRowFingerprint) -> MaterializedRowIdentity {
    MaterializedRowIdentity {
        table: row.table.clone(),
        key_digest: row.key_digest.to_string(),
    }
}

fn member_row_digest_value(
    status: &FederationLocalStatusResponse,
    identity: &MaterializedRowIdentity,
) -> Option<String> {
    let digests = status
        .materialized_state_rows
        .iter()
        .filter(|row| {
            row.table == identity.table && row.key_digest.to_string() == identity.key_digest
        })
        .map(|row| row.row_digest.to_string())
        .collect::<Vec<_>>();
    if digests.is_empty() {
        return None;
    }

    Some(digests.join("+"))
}

fn materialized_row_diff_line(diff: &DigestComparisonRowDiff) -> String {
    let variants = if diff.variants.is_empty() {
        "none".to_string()
    } else {
        diff.variants
            .iter()
            .map(|variant| format!("{}:{}", variant.row_digest, display_list(&variant.members)))
            .collect::<Vec<_>>()
            .join("|")
    };
    format!(
        "materialized_row_diff table={} key_digest={} variants={} missing_members={}",
        diff.table,
        diff.key_digest,
        variants,
        display_list(&diff.missing_members)
    )
}

fn grouped_members(groups: &[DigestComparisonGroup]) -> Vec<MemberId> {
    sorted_member_list(
        groups
            .iter()
            .flat_map(|group| group.members.iter().copied()),
    )
}

fn sorted_member_list<I>(members: I) -> Vec<MemberId>
where
    I: IntoIterator<Item = MemberId>,
{
    members
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn status_reports_digest_alert(status: &FederationLocalStatusResponse) -> bool {
    status.sync_state == "materialized_state_mismatch"
        || (status.sync_state == "halted" && status.sync_message.contains("digest mismatch"))
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

fn display_set(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    values.iter().cloned().collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests;
