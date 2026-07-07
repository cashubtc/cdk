use cdk_common::federation::{
    FederationConsensusBackendKind, FederationId, FederationJournalDigest,
    FederationMaterializedStateRow, FederationMaterializedStateRowFingerprint,
    FederationPeerStatus, FederationStateDigest, FEDERATION_REST_API_VERSION,
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
        sync_peer: None,
        sync_local_operation_count: None,
        sync_peer_operation_count: None,
        sync_operation_count: None,
        pending_mempool_items: 0,
        pending_response_waiters: 0,
        response_cache_items: 0,
        response_cache_lookups: 0,
        response_cache_hits: 0,
        response_cache_hit_rate_bps: 0,
        duplicate_submissions: 0,
        duplicate_pending_waiter_joins: 0,
        session_outcome_quorum_operation_count: None,
        session_outcome_quorum_from_index: None,
        session_outcome_quorum_latest_index: None,
        session_outcome_quorum_signers: Vec::new(),
        payment_observation_reports: Vec::new(),
        aleph_bft_status: None,
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

fn payment_observation_row(
    key: &str,
    state: &str,
    evidence_hash: &str,
) -> FederationMaterializedStateRowFingerprint {
    FederationMaterializedStateRow::new("payment_observations", key)
        .with_field("context", "mint_quote")
        .with_field("quote", "secret-quote-id")
        .with_field("operation_index", "7")
        .with_field("operation_id", "secret-operation-id")
        .with_field("observer", "1")
        .with_field("state", state)
        .with_field("evidence_hash", evidence_hash)
        .fingerprint()
        .expect("payment observation row fingerprint")
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
    assert!(digest_diagnostic_line(&report.diagnostics[0]).contains("kind=order_digest_mismatch"));
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
    assert!(
        digest_diagnostic_line(&report.diagnostics[0]).contains("kind=consensus_backend_mismatch")
    );
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
    assert!(
        digest_diagnostic_line(&report.diagnostics[0]).contains("kind=materialized_state_mismatch")
    );
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

#[test]
fn digest_comparison_diagnoses_missing_completed_operation_row() {
    let order_digest = FederationJournalDigest::from_bytes([1; 32]);
    let first_state_digest = FederationStateDigest::from_bytes([2; 32]);
    let second_state_digest = FederationStateDigest::from_bytes([3; 32]);
    let completed_operation_row =
        FederationMaterializedStateRow::new("completed_operations", "completed-operation-row-uuid")
            .with_field("federation_operation_id", "accepted-operation-id")
            .with_field("kind", "mint")
            .fingerprint()
            .expect("completed operation row fingerprint");
    let mut first = local_status(1, 4, order_digest, first_state_digest);
    first.materialized_state_rows = vec![completed_operation_row];
    let second = local_status(2, 4, order_digest, second_state_digest);
    let statuses = vec![first, second];

    let report = DigestComparisonReport::from_statuses(&statuses);

    assert_eq!(report.row_diffs.len(), 1);
    let diff = &report.row_diffs[0];
    assert_eq!(diff.table, "completed_operations");
    assert_eq!(
        diff.missing_members,
        vec![MemberId::new(2).expect("member two")]
    );
    assert_eq!(diff.variants.len(), 1);
    assert_eq!(
        diff.variants[0].members,
        vec![MemberId::new(1).expect("member one")]
    );
    let line = materialized_row_diff_line(diff);
    assert!(line.contains("table=completed_operations"));
    assert!(line.contains("missing_members=2"));
    assert!(!line.contains("completed-operation-row-uuid"));
    assert!(!line.contains("accepted-operation-id"));
}

#[test]
fn digest_comparison_diagnoses_payment_observation_row_variant_without_leaking_payment_material() {
    let order_digest = FederationJournalDigest::from_bytes([1; 32]);
    let matching_state_digest = FederationStateDigest::from_bytes([2; 32]);
    let divergent_state_digest = FederationStateDigest::from_bytes([3; 32]);
    let payment_observation_key = "mint_quote:secret-quote-id:7:secret-operation-id";

    let mut third = local_status(3, 7, order_digest, matching_state_digest);
    third.materialized_state_rows = vec![payment_observation_row(
        payment_observation_key,
        "paid",
        "payment-proof-secret-a",
    )];
    let mut second = local_status(2, 7, order_digest, divergent_state_digest);
    second.materialized_state_rows = vec![payment_observation_row(
        payment_observation_key,
        "paid",
        "payment-proof-secret-b",
    )];
    let mut first = local_status(1, 7, order_digest, matching_state_digest);
    first.materialized_state_rows = vec![payment_observation_row(
        payment_observation_key,
        "paid",
        "payment-proof-secret-a",
    )];
    let statuses = vec![third, second, first];

    let report = DigestComparisonReport::from_statuses(&statuses);

    assert_eq!(report.row_diffs.len(), 1);
    let diff = &report.row_diffs[0];
    assert_eq!(diff.table, "payment_observations");
    assert!(diff.missing_members.is_empty());
    assert_eq!(diff.variants.len(), 2);
    let matching_variant = diff
        .variants
        .iter()
        .find(|variant| variant.members.len() == 2)
        .expect("matching payment observation variant");
    assert_eq!(
        matching_variant.members,
        vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(3).expect("member three"),
        ]
    );
    let line = materialized_row_diff_line(diff);
    assert!(line.contains("table=payment_observations"));
    assert!(line.contains("missing_members=none"));
    assert!(!line.contains(payment_observation_key));
    assert!(!line.contains("secret-quote-id"));
    assert!(!line.contains("secret-operation-id"));
    assert!(!line.contains("payment-proof-secret-a"));
    assert!(!line.contains("payment-proof-secret-b"));
}
