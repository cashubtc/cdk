use std::str::FromStr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederationConsensusRuntimeStatus,
    FederationInvalidOperationAlertSummary, FederationJournalDigest,
    FederationMaterializedStateDigest, FederationPaymentObservationContext,
    FederationPaymentObservationEvidenceGroupSummary, FederationPeerStatus, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut22::AuthToken;
use cdk_common::{Amount, CurrencyUnit, SecretKey};
use cdk_http_client::{HttpError, RawResponse};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::*;

#[derive(Debug, Clone, Default)]
struct MockStatusTransport {
    inner: Arc<Mutex<MockStatusTransportInner>>,
}

#[derive(Debug, Default)]
struct MockStatusTransportInner {
    requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
    responses: Vec<FederationLocalStatusResponse>,
    urls: Vec<Url>,
}

#[async_trait]
impl Transport for MockStatusTransport {
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get<R>(&self, _url: Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        Err(HttpError::Other("unexpected GET".to_string()))
    }

    async fn http_get_raw(
        &self,
        _url: Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other("unexpected raw GET".to_string()))
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let request_json =
            serde_json::to_value(payload).map_err(|err| HttpError::Other(err.to_string()))?;
        let request: FederationSignedRequest<FederationPeerStatusRequest> =
            serde_json::from_value(request_json)
                .map_err(|err| HttpError::Other(err.to_string()))?;
        let response = {
            let mut inner = self.inner.lock().expect("mock transport lock");
            inner.urls.push(url);
            inner.requests.push(request);
            inner
                .responses
                .pop()
                .ok_or_else(|| HttpError::Other("missing mock response".to_string()))?
        };
        let response_json =
            serde_json::to_value(response).map_err(|err| HttpError::Other(err.to_string()))?;
        serde_json::from_value(response_json).map_err(|err| HttpError::Other(err.to_string()))
    }

    async fn http_post_form_raw<P>(
        &self,
        _url: Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other("unexpected raw form POST".to_string()))
    }
}

impl MockStatusTransport {
    fn new(mut responses: Vec<FederationLocalStatusResponse>) -> Self {
        responses.reverse();
        Self {
            inner: Arc::new(Mutex::new(MockStatusTransportInner {
                requests: Vec::new(),
                responses,
                urls: Vec::new(),
            })),
        }
    }

    fn requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
        self.inner
            .lock()
            .expect("mock transport lock")
            .requests
            .clone()
    }

    fn urls(&self) -> Vec<Url> {
        self.inner.lock().expect("mock transport lock").urls.clone()
    }
}

fn status_fixture() -> (FederationConfig, MemberConfigPrivate) {
    let members = (1..=3)
        .map(|id| {
            let member_id = MemberId::new(id).expect("member id");
            let url =
                MintUrl::from_str(&format!("http://127.0.0.1:808{}", id + 4)).expect("member url");
            TrustedDealerMember::new(member_id, url.clone(), url, SecretKey::generate())
        })
        .collect::<Vec<_>>();
    let unit = CurrencyUnit::Sat;
    let mut amount_coefficients = std::collections::BTreeMap::new();
    amount_coefficients.insert(
        Amount::from(1),
        vec![
            SecretKey::generate_bls().as_bls().expect("bls key").clone(),
            SecretKey::generate_bls().as_bls().expect("bls key").clone(),
        ],
    );
    let keyset_spec = TrustedDealerKeysetSpec::new(unit, true, 0, None, amount_coefficients);
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("threshold"),
        members,
        vec![keyset_spec],
    )
    .expect("trusted dealer bundle");
    let private = bundle
        .private_configs
        .get(&MemberId::new(1).expect("member id"))
        .expect("private config")
        .clone();
    (bundle.public_config, private)
}

fn local_status(
    public_config: &FederationConfig,
    member_id: u16,
    operation_count: u64,
) -> FederationLocalStatusResponse {
    let member_id = MemberId::new(member_id).expect("member id");
    FederationLocalStatusResponse {
        version: FEDERATION_REST_API_VERSION,
        peer_status: FederationPeerStatus::new_with_materialized_state_digest(
            public_config.federation_id,
            member_id,
            operation_count,
            None,
            FederationJournalDigest::genesis(),
            FederationMaterializedStateDigest::from_bytes([1; 32]),
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
        catch_up_certificate_quorum_operation_count: None,
        catch_up_certificate_quorum_from_index: None,
        catch_up_certificate_quorum_latest_index: None,
        catch_up_certificate_quorum_signers: Vec::new(),
        payment_observation_reports: Vec::new(),
        aleph_bft_status: None,
        aleph_bft_live_session_active: true,
        aleph_bft_network_inbound_active: true,
        materialized_state_rows: Vec::new(),
        invalid_operation_alerts: Vec::new(),
    }
}

fn not_ready_status(
    public_config: &FederationConfig,
    member_id: u16,
) -> FederationLocalStatusResponse {
    let mut status = local_status(public_config, member_id, 0);
    status.sync_state = "lagging".to_string();
    status.sync_ready = false;
    status.sync_message = "federation member is lagging behind peer 2".to_string();
    status.sync_peer = Some(MemberId::new(2).expect("peer member"));
    status.sync_local_operation_count = Some(0);
    status.sync_peer_operation_count = Some(1);
    status
}

fn payment_report_summary(
    status: FederationPaymentObservationReportStatus,
    recommended_action: FederationPaymentObservationReportAction,
    stale: bool,
) -> FederationPaymentObservationReportSummary {
    FederationPaymentObservationReportSummary {
        context: FederationPaymentObservationContext::MintQuote,
        quote: "quote-1".to_string(),
        required: 2,
        total_observations: 1,
        groups: Vec::new(),
        conflicting_observers: Vec::new(),
        competing_terminal_state_observers: Vec::new(),
        status,
        quote_age_secs: Some(60),
        stale,
        recommended_action,
    }
}

#[tokio::test]
async fn status_query_signs_local_status_requests_for_selected_members() {
    let (public_config, private_config) = status_fixture();
    let transport = MockStatusTransport::new(vec![
        local_status(&public_config, 1, 0),
        local_status(&public_config, 3, 2),
    ]);

    let statuses =
        query_local_statuses(transport.clone(), &public_config, &private_config, &[1, 3])
            .await
            .expect("status query");

    assert_eq!(statuses.len(), 2);
    assert_eq!(
        statuses[0].peer_status.member_id,
        MemberId::new(1).expect("member id")
    );
    assert_eq!(
        statuses[1].peer_status.member_id,
        MemberId::new(3).expect("member id")
    );

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].sender, private_config.member_id);
    assert_eq!(requests[0].receiver, MemberId::new(1).expect("member id"));
    assert_eq!(requests[1].receiver, MemberId::new(3).expect("member id"));
    assert!(!requests[0].payload.include_materialized_rows);
    assert!(!requests[1].payload.include_materialized_rows);
    assert!(requests[1].sequence > requests[0].sequence);
    requests[0]
        .verify(&public_config, MemberId::new(1).expect("member id"))
        .expect("first request verifies");
    requests[1]
        .verify(&public_config, MemberId::new(3).expect("member id"))
        .expect("second request verifies");

    let urls = transport.urls();
    assert_eq!(
        urls[0].as_str(),
        "http://127.0.0.1:8085/federation/v1/local-status"
    );
    assert_eq!(
        urls[1].as_str(),
        "http://127.0.0.1:8087/federation/v1/local-status"
    );
}

#[tokio::test]
async fn status_query_materialized_rows_are_opt_in() {
    let (public_config, private_config) = status_fixture();
    let transport = MockStatusTransport::new(vec![local_status(&public_config, 1, 0)]);

    let statuses = query_local_statuses_with_materialized_rows(
        transport.clone(),
        &public_config,
        &private_config,
        &[1],
        true,
    )
    .await
    .expect("status query");

    assert_eq!(statuses.len(), 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].payload.include_materialized_rows);
    requests[0]
        .verify(&public_config, MemberId::new(1).expect("member id"))
        .expect("request verifies");
}

#[tokio::test]
async fn status_query_rejects_mismatched_member_response() {
    let (public_config, private_config) = status_fixture();
    let transport = MockStatusTransport::new(vec![local_status(&public_config, 2, 0)]);

    let err = query_local_statuses(transport, &public_config, &private_config, &[1])
        .await
        .expect_err("mismatched status member should fail");

    assert!(err
        .to_string()
        .contains("federation member status mismatch"));
}

#[tokio::test]
async fn prevalidated_status_query_skips_private_share_validation() {
    let (public_config, mut private_config) = status_fixture();
    private_config.keyset_secret_shares.clear();

    let err = query_local_statuses(
        MockStatusTransport::new(vec![local_status(&public_config, 1, 0)]),
        &public_config,
        &private_config,
        &[1],
    )
    .await
    .expect_err("normal status query should validate private shares");
    assert!(err
        .to_string()
        .contains("private federation config does not match public federation config"));

    let statuses = query_local_statuses_prevalidated(
        MockStatusTransport::new(vec![local_status(&public_config, 1, 0)]),
        &public_config,
        &private_config,
        &[1],
        false,
    )
    .await
    .expect("prevalidated status query");

    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].peer_status.member_id,
        MemberId::new(1).expect("member id")
    );
}

#[test]
fn status_targets_default_to_all_members() {
    let (public_config, _) = status_fixture();
    let targets = status_targets(&public_config, &[]).expect("all targets");
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].member_id, MemberId::new(1).expect("member id"));
    assert_eq!(targets[1].member_id, MemberId::new(2).expect("member id"));
    assert_eq!(targets[2].member_id, MemberId::new(3).expect("member id"));
}

#[test]
fn status_readiness_guard_rejects_lagging_member() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 0),
        not_ready_status(&public_config, 2),
    ];

    let err = ensure_statuses_ready(&statuses).expect_err("lagging member should fail");
    assert!(err.to_string().contains("is not ready"));
}

#[test]
fn status_readiness_guard_rejects_inactive_consensus_transport() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 0);
    status.aleph_bft_network_inbound_active = false;

    let err = ensure_statuses_ready(&[status])
        .expect_err("inactive consensus transport should fail readiness");

    assert!(err.to_string().contains("network inbound active=false"));
}

#[test]
fn status_line_redacts_sensitive_sync_message() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.sync_state = "halted".to_string();
    status.sync_ready = false;
    status.sync_message = concat!(
        "peer 2 digest mismatch at operation count 2 ",
        "password=hunter2 token=sync-token ",
        "private_config_path=/home/example/fed/private.json ",
        "database_url=postgresql://status:db-secret@db.example.invalid/cdk"
    )
    .to_string();

    let line = member_status_line(&status);

    assert!(line.contains("digest mismatch"));
    assert!(line.contains("password=[REDACTED]"));
    assert!(line.contains("token=[REDACTED]"));
    assert!(line.contains("private_config_path=[REDACTED]"));
    assert!(!line.contains("hunter2"));
    assert!(!line.contains("sync-token"));
    assert!(!line.contains("/home/example"));
    assert!(!line.contains("db-secret"));
}

#[test]
fn status_line_prints_federation_runtime_counters() {
    let (public_config, _) = status_fixture();
    let status = local_status(&public_config, 1, 2);

    let line = member_status_line(&status);

    assert!(line.contains("sync_peer=none"));
    assert!(line.contains("sync_local_operations=none"));
    assert!(line.contains("sync_peer_operations=none"));
    assert!(line.contains("sync_operations=none"));
    assert!(line.contains("pending_mempool_items=0"));
    assert!(line.contains("pending_response_waiters=0"));
    assert!(line.contains("response_cache_items=0"));
    assert!(line.contains("response_cache_hits=0/0"));
    assert!(line.contains("response_cache_hit_rate_bps=0"));
    assert!(line.contains("duplicate_submissions=0"));
    assert!(line.contains("duplicate_pending_waiter_joins=0"));
    assert!(line.contains("aleph_bft=none"));
}

#[test]
fn status_line_includes_structured_lagging_sync_fields() {
    let (public_config, _) = status_fixture();
    let status = not_ready_status(&public_config, 1);

    let line = member_status_line(&status);

    assert!(line.contains("sync=lagging"));
    assert!(line.contains("sync_peer=2"));
    assert!(line.contains("sync_local_operations=0"));
    assert!(line.contains("sync_peer_operations=1"));
    assert!(line.contains("sync_operations=none"));
}

#[test]
fn status_line_includes_response_cache_and_duplicate_counters() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.response_cache_lookups = 7;
    status.response_cache_hits = 3;
    status.response_cache_hit_rate_bps = 4285;
    status.duplicate_submissions = 5;
    status.duplicate_pending_waiter_joins = 2;

    let line = member_status_line(&status);

    assert!(line.contains("response_cache_hits=3/7"));
    assert!(line.contains("response_cache_hit_rate_bps=4285"));
    assert!(line.contains("duplicate_submissions=5"));
    assert!(line.contains("duplicate_pending_waiter_joins=2"));
}

#[test]
fn status_line_includes_consensus_runtime_counters_when_present() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.aleph_bft_live_session_active = true;
    status.aleph_bft_network_inbound_active = true;
    status.aleph_bft_status = Some(FederationConsensusRuntimeStatus {
        federation_id: public_config.federation_id,
        local_member: MemberId::new(1).expect("member one"),
        session: 4,
        next_unit_index: 7,
        next_unit_finalization_index: 8,
        next_item_index: 9,
        queued_item_count: 2,
        pending_finalization_item_count: 4,
        finalized_item_count: 3,
        accepted_operation_count: 1,
        latest_accepted_operation_index: Some(0),
        allowed_member_count: Some(2),
    });

    let line = member_status_line(&status);

    assert!(line.contains("aleph_bft_live_session=true"));
    assert!(line.contains("aleph_bft_network_inbound=true"));
    assert!(line.contains(
        "aleph_bft=local_member:1 session:4 queued:2 pending_finalization:4 finalized:3 accepted:1 next_unit:7 next_finalization_unit:8 next_item:9 latest_accepted:0 members:2"
    ));
}

#[test]
fn status_readiness_guard_redacts_sensitive_sync_message() {
    let (public_config, _) = status_fixture();
    let mut status = not_ready_status(&public_config, 1);
    status.sync_message =
        "lagging while reading secret=sync-secret from /home/example/fed/private.json".to_string();

    let err = ensure_statuses_ready(&[status]).expect_err("not ready should fail");
    let message = err.to_string();

    assert!(message.contains("secret=[REDACTED]"));
    assert!(message.contains("[REDACTED_PATH]"));
    assert!(!message.contains("sync-secret"));
    assert!(!message.contains("/home/example"));
}

#[test]
fn status_lines_include_payment_observation_report_counts() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    let first_observation_id = cdk_common::federation::FederationOperationId::from_bytes([1; 32]);
    let second_observation_id = cdk_common::federation::FederationOperationId::from_bytes([2; 32]);
    let third_observation_id = cdk_common::federation::FederationOperationId::from_bytes([3; 32]);
    status.payment_observation_reports = vec![FederationPaymentObservationReportSummary {
        context: FederationPaymentObservationContext::MintQuote,
        quote: "quote-1".to_string(),
        required: 2,
        total_observations: 3,
        groups: vec![
            FederationPaymentObservationEvidenceGroupSummary {
                state: "PAID".to_string(),
                observers: vec![
                    MemberId::new(1).expect("member one"),
                    MemberId::new(2).expect("member two"),
                ],
                observer_count: 2,
                operation_ids: vec![first_observation_id, second_observation_id],
                operation_indices: vec![1, 2],
                conflicting_observers: vec![MemberId::new(3).expect("member three")],
                quorum_reached: true,
                terminal_state: true,
            },
            FederationPaymentObservationEvidenceGroupSummary {
                state: "PAID".to_string(),
                observers: vec![MemberId::new(3).expect("member three")],
                observer_count: 1,
                operation_ids: vec![third_observation_id],
                operation_indices: vec![3],
                conflicting_observers: vec![
                    MemberId::new(1).expect("member one"),
                    MemberId::new(2).expect("member two"),
                ],
                quorum_reached: false,
                terminal_state: true,
            },
        ],
        conflicting_observers: vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two"),
            MemberId::new(3).expect("member three"),
        ],
        competing_terminal_state_observers: Vec::new(),
        status: FederationPaymentObservationReportStatus::Conflicting,
        quote_age_secs: Some(7_200),
        stale: true,
        recommended_action: FederationPaymentObservationReportAction::Inspect,
    }];

    let member_line = member_status_line(&status);
    assert!(member_line.contains("materialized_rows=0"));
    assert!(member_line.contains("catch_up_certificate_quorum_operations=none"));
    assert!(member_line.contains("catch_up_certificate_quorum_from=none"));
    assert!(member_line.contains("catch_up_certificate_quorum_latest=none"));
    assert!(member_line.contains("catch_up_certificate_quorum_signers=none"));
    assert!(member_line.contains("payment_reports=1"));
    assert!(member_line.contains("payment_conflicts=1"));
    assert!(member_line.contains("payment_unresolved=0"));
    assert!(member_line.contains("payment_stale=1"));

    let report_line = payment_observation_report_line(&status.payment_observation_reports[0]);
    assert!(report_line.contains("context=mint_quote"));
    assert!(report_line.contains("quote=quote-1"));
    assert!(report_line.contains("status=conflicting"));
    assert!(report_line.contains("observations=3"));
    assert!(report_line.contains("quote_age_secs=7200"));
    assert!(report_line.contains("stale=true"));
    assert!(report_line.contains("action=inspect"));

    let group_line =
        payment_observation_group_line(&status.payment_observation_reports[0].groups[0]);
    assert!(group_line.contains("state=PAID"));
    assert!(group_line.contains("observers=1,2"));
    assert!(group_line.contains("operation_ids=0101010101010101010101010101010101010101010101010101010101010101,0202020202020202020202020202020202020202020202020202020202020202"));
    assert!(group_line.contains("operation_indices=1,2"));
    assert!(group_line.contains("conflicts=3"));
    assert!(group_line.contains("terminal=true"));
}

#[test]
fn status_payment_observation_alert_guard_accepts_normal_reports() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.payment_observation_reports = vec![
        payment_report_summary(
            FederationPaymentObservationReportStatus::QuorumReached,
            FederationPaymentObservationReportAction::None,
            false,
        ),
        payment_report_summary(
            FederationPaymentObservationReportStatus::AwaitingQuorum,
            FederationPaymentObservationReportAction::None,
            false,
        ),
    ];

    ensure_statuses_no_payment_observation_alerts(&[status])
        .expect("normal payment observation progress has no alert");
}

#[test]
fn status_payment_observation_alert_guard_rejects_inspect_action() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.payment_observation_reports = vec![payment_report_summary(
        FederationPaymentObservationReportStatus::AwaitingQuorum,
        FederationPaymentObservationReportAction::Inspect,
        true,
    )];

    let err = ensure_statuses_no_payment_observation_alerts(&[status])
        .expect_err("stale inspect report should fail");

    assert!(err.to_string().contains("payment observation alert"));
    assert!(err.to_string().contains("action=inspect"));
}

#[test]
fn status_payment_observation_alert_guard_rejects_conflict_status() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    let mut report = payment_report_summary(
        FederationPaymentObservationReportStatus::Conflicting,
        FederationPaymentObservationReportAction::None,
        false,
    );
    report.conflicting_observers = vec![MemberId::new(2).expect("member two")];
    status.payment_observation_reports = vec![report];

    let err = ensure_statuses_no_payment_observation_alerts(&[status])
        .expect_err("conflicting report should fail");

    assert!(err.to_string().contains("status=conflicting"));
    assert!(err.to_string().contains("conflicts=1"));
}

#[test]
fn status_line_includes_catch_up_certificate_quorum_evidence() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.catch_up_certificate_quorum_operation_count = Some(3);
    status.catch_up_certificate_quorum_from_index = Some(0);
    status.catch_up_certificate_quorum_latest_index = Some(2);
    status.catch_up_certificate_quorum_signers = vec![
        MemberId::new(1).expect("member one"),
        MemberId::new(2).expect("member two"),
    ];

    let member_line = member_status_line(&status);

    assert!(member_line.contains("catch_up_certificate_quorum_operations=3"));
    assert!(member_line.contains("catch_up_certificate_quorum_from=0"));
    assert!(member_line.contains("catch_up_certificate_quorum_latest=2"));
    assert!(member_line.contains("catch_up_certificate_quorum_signers=1,2"));
}

#[test]
fn status_line_includes_invalid_operation_alert_count() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.invalid_operation_alerts = vec![
        FederationInvalidOperationAlertSummary {
            source: "auth_token".to_string(),
            alerting_buckets: 2,
            max_count: 25,
            threshold: 20,
            window_secs: 60,
        },
        FederationInvalidOperationAlertSummary {
            source: "client_ip".to_string(),
            alerting_buckets: 1,
            max_count: 20,
            threshold: 20,
            window_secs: 60,
        },
    ];

    let member_line = member_status_line(&status);

    assert!(member_line.contains("invalid_operation_alerts=3"));
    assert!(!member_line.contains("token-a"));
}

#[test]
fn status_invalid_operation_alert_guard_accepts_empty_alerts() {
    let (public_config, _) = status_fixture();
    let status = local_status(&public_config, 1, 3);

    ensure_statuses_no_invalid_operation_alerts(&[status])
        .expect("empty invalid-operation alerts are accepted");
}

#[test]
fn status_invalid_operation_alert_guard_rejects_alerting_bucket() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.invalid_operation_alerts = vec![FederationInvalidOperationAlertSummary {
        source: "auth_token".to_string(),
        alerting_buckets: 1,
        max_count: 21,
        threshold: 20,
        window_secs: 60,
    }];

    let err = ensure_statuses_no_invalid_operation_alerts(&[status])
        .expect_err("invalid-operation alert should fail");

    assert!(err.to_string().contains("invalid operation alert"));
    assert!(err.to_string().contains("source=auth_token"));
    assert!(err.to_string().contains("max_count=21"));
}

#[test]
fn status_convergence_guard_rejects_height_mismatch() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 0),
        local_status(&public_config, 2, 1),
    ];

    let err = ensure_statuses_converged(&statuses).expect_err("divergent member should fail");
    assert!(err.to_string().contains("not converged"));
}

#[test]
fn status_convergence_guard_rejects_session_mismatch_at_equal_height() {
    let (public_config, _) = status_fixture();
    let first = local_status(&public_config, 1, 7);
    let mut second = local_status(&public_config, 2, 7);
    second.peer_status.aleph_bft_session = 1;

    let err = ensure_statuses_converged(&[first, second])
        .expect_err("equal-height session split should fail");

    assert!(err.to_string().contains("not converged"));
    assert!(err.to_string().contains("AlephBFT session"));
}

#[test]
fn status_convergence_guard_rejects_missing_materialized_state_digest() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.peer_status.materialized_state_digest = None;

    let err = ensure_statuses_converged(&[status])
        .expect_err("missing materialized state digest should fail");

    assert!(err
        .to_string()
        .contains("omitted its materialized state digest"));
}

#[test]
fn status_convergence_guard_rejects_materialized_state_digest_mismatch() {
    let (public_config, _) = status_fixture();
    let first = local_status(&public_config, 1, 0);
    let mut second = local_status(&public_config, 2, 0);
    second.peer_status.materialized_state_digest =
        Some(FederationMaterializedStateDigest::genesis());
    let statuses = vec![first, second];

    let err = ensure_statuses_converged(&statuses).expect_err("state digest mismatch should fail");

    assert!(err.to_string().contains("state digest"));
}

#[test]
fn status_digest_alert_guard_accepts_height_mismatch() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 0),
        local_status(&public_config, 2, 1),
    ];

    ensure_statuses_no_digest_alerts(&statuses)
        .expect("ordinary lag is handled by lag and convergence guards");
}

#[test]
fn status_digest_alert_guard_rejects_materialized_state_digest_mismatch() {
    let (public_config, _) = status_fixture();
    let first = local_status(&public_config, 1, 2);
    let mut second = local_status(&public_config, 2, 2);
    second.peer_status.materialized_state_digest =
        Some(FederationMaterializedStateDigest::genesis());
    let statuses = vec![first, second];

    let err = ensure_statuses_no_digest_alerts(&statuses).expect_err("state digest mismatch fails");

    assert!(err.to_string().contains("digest alert"));
    assert!(err.to_string().contains("state digest"));
}

#[test]
fn status_digest_alert_guard_rejects_order_digest_mismatch() {
    let (public_config, _) = status_fixture();
    let first = local_status(&public_config, 1, 2);
    let mut second = local_status(&public_config, 2, 2);
    second.peer_status.latest_order_digest = FederationJournalDigest::from_bytes([42; 32]);
    let statuses = vec![first, second];

    let err = ensure_statuses_no_digest_alerts(&statuses).expect_err("order digest mismatch fails");

    assert!(err.to_string().contains("digest alert"));
    assert!(err.to_string().contains("order digest"));
}

#[test]
fn status_digest_alert_guard_rejects_reported_materialized_state_mismatch() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.sync_state = "materialized_state_mismatch".to_string();
    status.sync_ready = false;
    status.sync_message =
        "waiting for materialized state digest convergence with peer 2".to_string();

    let err = ensure_statuses_no_digest_alerts(&[status]).expect_err("reported mismatch fails");

    assert!(err.to_string().contains("reports digest alert"));
    assert!(err.to_string().contains("materialized_state_mismatch"));
}

#[test]
fn status_digest_alert_guard_rejects_halted_digest_mismatch() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.sync_state = "halted".to_string();
    status.sync_ready = false;
    status.sync_message = "peer 2 digest mismatch at operation count 2".to_string();

    let err =
        ensure_statuses_no_digest_alerts(&[status]).expect_err("halted digest mismatch fails");

    assert!(err.to_string().contains("reports digest alert"));
    assert!(err.to_string().contains("halted"));
}

#[test]
fn status_digest_alert_guard_redacts_sensitive_halted_message() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 2);
    status.sync_state = "halted".to_string();
    status.sync_ready = false;
    status.sync_message = concat!(
        "peer 2 digest mismatch at operation count 2 ",
        "password=hunter2 private_config_path=/home/example/fed/private.json"
    )
    .to_string();

    let err =
        ensure_statuses_no_digest_alerts(&[status]).expect_err("halted digest mismatch fails");
    let message = err.to_string();

    assert!(message.contains("digest mismatch"));
    assert!(message.contains("password=[REDACTED]"));
    assert!(message.contains("private_config_path=[REDACTED]"));
    assert!(!message.contains("hunter2"));
    assert!(!message.contains("/home/example"));
}

#[test]
fn status_operation_lag_guard_accepts_members_within_limit() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 7),
        local_status(&public_config, 2, 5),
        local_status(&public_config, 3, 6),
    ];

    ensure_statuses_operation_lag(&statuses, 2).expect("members are within lag limit");
}

#[test]
fn status_operation_lag_guard_accepts_empty_status_set() {
    ensure_statuses_operation_lag(&[], 0).expect("empty status set has no lag");
}

#[test]
fn status_operation_lag_guard_rejects_member_beyond_limit() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 7),
        local_status(&public_config, 2, 4),
        local_status(&public_config, 3, 6),
    ];

    let err =
        ensure_statuses_operation_lag(&statuses, 2).expect_err("member two is too far behind");

    assert!(err.to_string().contains("member 2"));
    assert!(err.to_string().contains("3 operations behind"));
    assert!(err.to_string().contains("allowed lag 2"));
}

#[test]
fn status_catch_up_certificate_quorum_guard_accepts_zero_operation_member_without_evidence() {
    let (public_config, _) = status_fixture();
    let statuses = vec![local_status(&public_config, 1, 0)];

    ensure_statuses_catch_up_certificate_quorum(&public_config, &statuses)
        .expect("zero operation member needs no session evidence");
}

#[test]
fn status_catch_up_certificate_quorum_guard_accepts_covered_current_height() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.catch_up_certificate_quorum_operation_count = Some(3);
    status.catch_up_certificate_quorum_signers = vec![
        MemberId::new(1).expect("member one"),
        MemberId::new(2).expect("member two"),
        MemberId::new(3).expect("member three"),
    ];

    ensure_statuses_catch_up_certificate_quorum(&public_config, &[status])
        .expect("current height has quorum evidence");
}

#[test]
fn status_json_rejects_missing_current_fields() {
    let (public_config, _) = status_fixture();
    let status = local_status(&public_config, 1, 0);

    for field in [
        "pending_mempool_items",
        "catch_up_certificate_quorum_signers",
        "materialized_state_rows",
    ] {
        let mut value = serde_json::to_value(&status).expect("status json");
        value.as_object_mut().expect("status object").remove(field);
        assert!(serde_json::from_value::<FederationLocalStatusResponse>(value).is_err());
    }
}

#[test]
fn status_catch_up_certificate_quorum_guard_rejects_missing_evidence() {
    let (public_config, _) = status_fixture();
    let status = local_status(&public_config, 1, 3);

    let err = ensure_statuses_catch_up_certificate_quorum(&public_config, &[status])
        .expect_err("non-empty operation history needs quorum evidence");

    assert!(err
        .to_string()
        .contains("no catch-up certificate quorum evidence"));
}

#[test]
fn status_catch_up_certificate_quorum_guard_rejects_short_coverage() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.catch_up_certificate_quorum_operation_count = Some(2);
    status.catch_up_certificate_quorum_signers = vec![
        MemberId::new(1).expect("member one"),
        MemberId::new(2).expect("member two"),
        MemberId::new(3).expect("member three"),
    ];

    let err = ensure_statuses_catch_up_certificate_quorum(&public_config, &[status])
        .expect_err("short quorum coverage should fail");

    assert!(err.to_string().contains("below current operation count"));
}

#[test]
fn status_catch_up_certificate_quorum_guard_rejects_insufficient_distinct_signers() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.catch_up_certificate_quorum_operation_count = Some(3);
    status.catch_up_certificate_quorum_signers = vec![
        MemberId::new(1).expect("member one"),
        MemberId::new(1).expect("member one duplicate"),
        MemberId::new(2).expect("member two"),
    ];

    let err = ensure_statuses_catch_up_certificate_quorum(&public_config, &[status])
        .expect_err("duplicate signers should not satisfy quorum");

    assert!(err
        .to_string()
        .contains("distinct catch-up certificate quorum signers"));
}

#[test]
fn status_catch_up_certificate_quorum_guard_rejects_unknown_signer() {
    let (public_config, _) = status_fixture();
    let mut status = local_status(&public_config, 1, 3);
    status.catch_up_certificate_quorum_operation_count = Some(3);
    status.catch_up_certificate_quorum_signers = vec![
        MemberId::new(1).expect("member one"),
        MemberId::new(2).expect("member two"),
        MemberId::new(9).expect("unknown member"),
    ];

    let err = ensure_statuses_catch_up_certificate_quorum(&public_config, &[status])
        .expect_err("unknown signer should fail");

    assert!(err
        .to_string()
        .contains("unknown catch-up certificate quorum signer"));
}

#[test]
fn status_guards_accept_ready_converged_members() {
    let (public_config, _) = status_fixture();
    let statuses = vec![
        local_status(&public_config, 1, 0),
        local_status(&public_config, 2, 0),
        local_status(&public_config, 3, 0),
    ];

    ensure_statuses_ready(&statuses).expect("members ready");
    ensure_statuses_converged(&statuses).expect("members converged");
}
