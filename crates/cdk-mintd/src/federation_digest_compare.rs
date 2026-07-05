use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationMaterializedStateRowFingerprint, MemberConfigPrivate, MemberId,
};
use cdk_http_client::Async;

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
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    let statuses = query_local_statuses_with_materialized_rows(
        Async::default(),
        &public_config,
        &private_config,
        &args.members,
        true,
    )
    .await
    .context("failed to query signed federation digest status")?;

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
        "digest_group operations={} consensus_backend={} order_digest={} state_digest={} members={} ready={}/{} sync_states={}",
        group.key.operation_count,
        group.key.consensus_backend,
        group.key.order_digest,
        group.key.state_digest,
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
            if status_reports_digest_alert(status) {
                digest_alert_members.push(status.peer_status.member_id);
            }
            let key = DigestComparisonKey {
                operation_count: status.peer_status.operation_count,
                consensus_backend: status.peer_status.consensus_backend.to_string(),
                order_digest: status.peer_status.latest_order_digest.to_string(),
                state_digest: status.peer_status.state_digest.to_string(),
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
    ConsensusBackendMismatch,
    HeightMismatch,
    OrderDigestMismatch,
    MaterializedStateMismatch,
}

impl DigestComparisonDiagnosticKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DigestAlert => "digest_alert",
            Self::ConsensusBackendMismatch => "consensus_backend_mismatch",
            Self::HeightMismatch => "height_mismatch",
            Self::OrderDigestMismatch => "order_digest_mismatch",
            Self::MaterializedStateMismatch => "materialized_state_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DigestComparisonKey {
    operation_count: u64,
    consensus_backend: String,
    order_digest: String,
    state_digest: String,
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
    let consensus_backends = groups
        .iter()
        .map(|group| group.key.consensus_backend.clone())
        .collect::<BTreeSet<_>>();
    if consensus_backends.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::ConsensusBackendMismatch,
            members,
            detail: format!(
                "operation_count={} consensus_backends={}",
                operation_count,
                display_set(&consensus_backends)
            ),
        });
        return diagnostics;
    }

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

    let state_digests = groups
        .iter()
        .map(|group| group.key.state_digest.clone())
        .collect::<BTreeSet<_>>();
    if state_digests.len() > 1 {
        diagnostics.push(DigestComparisonDiagnostic {
            kind: DigestComparisonDiagnosticKind::MaterializedStateMismatch,
            members,
            detail: format!(
                "operation_count={} order_digest={} state_digests={}",
                operation_count,
                order_digests
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "none".to_string()),
                display_set(&state_digests)
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

        let state_digests = group
            .iter()
            .map(|status| status.peer_status.state_digest.to_string())
            .collect::<BTreeSet<_>>();
        if state_digests.len() <= 1 {
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
                        members,
                    })
                    .collect(),
                missing_members,
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
mod tests {
    use cdk_common::federation::{
        FederationConsensusBackendKind, FederationId, FederationJournalDigest,
        FederationMaterializedStateRow, FederationPeerStatus, FederationStateDigest,
        FEDERATION_REST_API_VERSION,
    };

    use super::*;

    fn local_status(
        member_id: u16,
        operation_count: u64,
        order_digest: FederationJournalDigest,
        state_digest: FederationStateDigest,
    ) -> FederationLocalStatusResponse {
        let member_id = MemberId::new(member_id).expect("member id");
        FederationLocalStatusResponse {
            version: FEDERATION_REST_API_VERSION,
            peer_status: FederationPeerStatus::new(
                FederationId::from_bytes([7; 32]),
                member_id,
                operation_count,
                None,
                order_digest,
            ),
            sync_state: "healthy".to_string(),
            sync_ready: true,
            sync_message: "federation member is healthy".to_string(),
            pending_mempool_items: 0,
            session_outcome_quorum_operation_count: None,
            session_outcome_quorum_signers: Vec::new(),
            payment_observation_reports: Vec::new(),
            materialized_state_rows: Vec::new(),
            invalid_operation_alerts: Vec::new(),
        }
        .with_state_digest(state_digest)
    }

    fn materialized_row(
        table: &str,
        key: &str,
        field_value: &str,
    ) -> FederationMaterializedStateRowFingerprint {
        FederationMaterializedStateRow::new(table, key)
            .with_field("state", field_value)
            .fingerprint()
            .expect("row fingerprint")
    }

    trait StatusFixtureExt {
        fn with_state_digest(self, state_digest: FederationStateDigest) -> Self;
    }

    impl StatusFixtureExt for FederationLocalStatusResponse {
        fn with_state_digest(mut self, state_digest: FederationStateDigest) -> Self {
            self.peer_status.state_digest = state_digest;
            self
        }
    }

    #[test]
    fn digest_comparison_groups_matching_members() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let state_digest = FederationStateDigest::from_order_digest(order_digest);
        let statuses = vec![
            local_status(1, 4, order_digest, state_digest),
            local_status(2, 4, order_digest, state_digest),
        ];

        let report = DigestComparisonReport::from_statuses(&statuses);

        assert!(report.is_converged());
        assert_eq!(report.groups.len(), 1);
        assert_eq!(
            report.groups[0].members,
            vec![
                MemberId::new(1).expect("member one"),
                MemberId::new(2).expect("member two"),
            ]
        );
        assert_eq!(report.groups[0].ready_count, 2);
        assert!(digest_group_line(&report.groups[0]).contains("members=1,2"));
        report
            .ensure_converged()
            .expect("matching digests converge");
    }

    #[test]
    fn digest_comparison_rejects_divergent_groups() {
        let first_order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let second_order_digest = FederationJournalDigest::from_bytes([2; 32]);
        let statuses = vec![
            local_status(
                1,
                4,
                first_order_digest,
                FederationStateDigest::from_order_digest(first_order_digest),
            ),
            local_status(
                2,
                4,
                second_order_digest,
                FederationStateDigest::from_order_digest(second_order_digest),
            ),
        ];

        let report = DigestComparisonReport::from_statuses(&statuses);
        let err = report
            .ensure_converged()
            .expect_err("different digest groups fail");

        assert!(!report.is_converged());
        assert_eq!(report.groups.len(), 2);
        assert!(err.to_string().contains("not converged"));
        assert!(err.to_string().contains("order_digest_mismatch"));
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(
            report.diagnostics[0].kind,
            DigestComparisonDiagnosticKind::OrderDigestMismatch
        );
        assert!(
            digest_diagnostic_line(&report.diagnostics[0]).contains("kind=order_digest_mismatch")
        );
    }

    #[test]
    fn digest_comparison_rejects_reported_digest_alert() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let state_digest = FederationStateDigest::from_order_digest(order_digest);
        let mut status = local_status(1, 4, order_digest, state_digest);
        status.sync_state = "halted".to_string();
        status.sync_message = "peer 2 digest mismatch at operation count 4".to_string();

        let report = DigestComparisonReport::from_statuses(&[status]);
        let err = report
            .ensure_converged()
            .expect_err("reported digest alert fails");

        assert!(!report.is_converged());
        assert!(err.to_string().contains("digest alerts"));
        assert!(err.to_string().contains("members 1"));
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(
            report.diagnostics[0].kind,
            DigestComparisonDiagnosticKind::DigestAlert
        );
    }

    #[test]
    fn digest_comparison_rejects_consensus_backend_mismatch() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let state_digest = FederationStateDigest::from_order_digest(order_digest);
        let first = local_status(1, 4, order_digest, state_digest);
        let mut second = local_status(2, 4, order_digest, state_digest);
        second.peer_status.consensus_backend = FederationConsensusBackendKind::AlephBft;
        let statuses = vec![first, second];

        let report = DigestComparisonReport::from_statuses(&statuses);
        let err = report
            .ensure_converged()
            .expect_err("backend mismatch fails");

        assert!(!report.is_converged());
        assert_eq!(report.groups.len(), 2);
        assert!(err.to_string().contains("consensus_backend_mismatch"));
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(
            report.diagnostics[0].kind,
            DigestComparisonDiagnosticKind::ConsensusBackendMismatch
        );
        assert!(digest_group_line(&report.groups[0]).contains("consensus_backend="));
        assert!(digest_diagnostic_line(&report.diagnostics[0])
            .contains("kind=consensus_backend_mismatch"));
    }

    #[test]
    fn digest_comparison_diagnoses_height_mismatch() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let state_digest = FederationStateDigest::from_order_digest(order_digest);
        let statuses = vec![
            local_status(1, 4, order_digest, state_digest),
            local_status(2, 6, order_digest, state_digest),
        ];

        let report = DigestComparisonReport::from_statuses(&statuses);
        let err = report
            .ensure_converged()
            .expect_err("height mismatch fails");

        assert!(err.to_string().contains("height_mismatch"));
        assert_eq!(
            report.diagnostics[0].kind,
            DigestComparisonDiagnosticKind::HeightMismatch
        );
        assert!(report.diagnostics[0]
            .detail
            .contains("operation_counts=4,6"));
        assert!(digest_diagnostic_line(&report.diagnostics[0]).contains("members=1,2"));
    }

    #[test]
    fn digest_comparison_diagnoses_materialized_state_mismatch() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let first_state_digest = FederationStateDigest::from_bytes([2; 32]);
        let second_state_digest = FederationStateDigest::from_bytes([3; 32]);
        let statuses = vec![
            local_status(1, 4, order_digest, first_state_digest),
            local_status(2, 4, order_digest, second_state_digest),
        ];

        let report = DigestComparisonReport::from_statuses(&statuses);
        let err = report
            .ensure_converged()
            .expect_err("materialized state mismatch fails");

        assert!(err.to_string().contains("materialized_state_mismatch"));
        assert_eq!(
            report.diagnostics[0].kind,
            DigestComparisonDiagnosticKind::MaterializedStateMismatch
        );
        assert!(report.diagnostics[0].detail.contains("operation_count=4"));
        assert!(digest_diagnostic_line(&report.diagnostics[0])
            .contains("kind=materialized_state_mismatch"));
    }

    #[test]
    fn digest_comparison_prints_materialized_row_diffs() {
        let order_digest = FederationJournalDigest::from_bytes([1; 32]);
        let first_state_digest = FederationStateDigest::from_bytes([2; 32]);
        let second_state_digest = FederationStateDigest::from_bytes([3; 32]);
        let mut first = local_status(1, 4, order_digest, first_state_digest);
        first.materialized_state_rows = vec![materialized_row(
            "proof_states",
            "secret-proof-y",
            "PENDING",
        )];
        let mut second = local_status(2, 4, order_digest, second_state_digest);
        second.materialized_state_rows =
            vec![materialized_row("proof_states", "secret-proof-y", "SPENT")];
        let statuses = vec![first, second];

        let report = DigestComparisonReport::from_statuses(&statuses);

        assert_eq!(report.row_diffs.len(), 1);
        let line = materialized_row_diff_line(&report.row_diffs[0]);
        assert!(line.contains("table=proof_states"));
        assert!(line.contains("missing_members=none"));
        assert!(!line.contains("secret-proof-y"));
    }
}
