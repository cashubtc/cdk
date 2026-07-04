use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{FederationConfig, MemberConfigPrivate, MemberId};
use cdk_http_client::Async;

use crate::cli::{CLIArgs, FederationCompareDigestsArgs};
use crate::federation_status::{query_local_statuses, FederationLocalStatusResponse};

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
    let statuses = query_local_statuses(
        Async::default(),
        &public_config,
        &private_config,
        &args.members,
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
}

fn digest_group_line(group: &DigestComparisonGroup) -> String {
    format!(
        "digest_group operations={} order_digest={} state_digest={} members={} ready={}/{} sync_states={}",
        group.key.operation_count,
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

        Self {
            queried_members: statuses.len(),
            groups: groups.into_values().collect(),
            digest_alert_members,
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
            bail!(
                "federation digest comparison is not converged: {} digest groups across {} queried members",
                self.groups.len(),
                self.queried_members
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DigestComparisonKey {
    operation_count: u64,
    order_digest: String,
    state_digest: String,
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
        FederationId, FederationJournalDigest, FederationPeerStatus, FederationStateDigest,
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
            session_outcome_quorum_operation_count: None,
            session_outcome_quorum_signers: Vec::new(),
            payment_observation_reports: Vec::new(),
        }
        .with_state_digest(state_digest)
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
    }
}
