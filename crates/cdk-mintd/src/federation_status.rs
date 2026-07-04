use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationId, FederationPaymentObservationReportAction,
    FederationPaymentObservationReportStatus, FederationPaymentObservationReportSummary,
    FederationPeerStatus, FederationPeerStatusRequest, FederationRestMessageKind,
    FederationSignedRequest, MemberConfigPrivate, MemberConfigPublic, MemberId,
    FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::{Deserialize, Serialize};

use crate::cli::{CLIArgs, FederationStatusArgs};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FederationLocalStatusResponse {
    pub(crate) version: u16,
    pub(crate) peer_status: FederationPeerStatus,
    pub(crate) sync_state: String,
    pub(crate) sync_ready: bool,
    pub(crate) sync_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_outcome_quorum_signers: Vec<MemberId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
}

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
    if args.require_session_outcome_quorum {
        ensure_statuses_session_outcome_quorum(&public_config, &statuses)?;
    }
    if args.require_no_payment_observation_alerts {
        ensure_statuses_no_payment_observation_alerts(&statuses)?;
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
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;

    let targets = status_targets(public_config, selected_members)?;
    let mut sequence = initial_status_sequence();
    let mut statuses = Vec::with_capacity(targets.len());
    for member in targets {
        sequence = sequence.saturating_add(1);
        let status =
            query_local_status(&transport, public_config, private_config, &member, sequence)
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
) -> Result<FederationLocalStatusResponse>
where
    T: Transport,
{
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        member.member_id,
        sequence,
        FederationRestMessageKind::Status,
        FederationPeerStatusRequest::default(),
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
        for report in &status.payment_observation_reports {
            print_payment_observation_report(report);
        }
    }
}

fn member_status_line(status: &FederationLocalStatusResponse) -> String {
    let peer = &status.peer_status;
    let latest_index = peer
        .latest_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "none".to_string());
    let session_outcome_quorum_operations = status
        .session_outcome_quorum_operation_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "none".to_string());
    let session_outcome_quorum_signers = member_list(&status.session_outcome_quorum_signers);
    let (payment_reports, payment_conflicts, payment_unresolved, payment_stale, payment_quarantine) =
        payment_report_counts(status);
    format!(
        "member {}: sync={} ready={} operations={} latest_index={} order_digest={} state_digest={} session_outcome_quorum_operations={} session_outcome_quorum_signers={} payment_reports={} payment_conflicts={} payment_unresolved={} payment_stale={} payment_quarantine={} message={}",
        peer.member_id,
        status.sync_state,
        status.sync_ready,
        peer.operation_count,
        latest_index,
        peer.latest_order_digest,
        peer.state_digest,
        session_outcome_quorum_operations,
        session_outcome_quorum_signers,
        payment_reports,
        payment_conflicts,
        payment_unresolved,
        payment_stale,
        payment_quarantine,
        status.sync_message
    )
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
                status.sync_message
            );
        }
    }

    Ok(())
}

fn ensure_statuses_converged(statuses: &[FederationLocalStatusResponse]) -> Result<()> {
    let Some(first) = statuses.first() else {
        return Ok(());
    };
    for status in statuses.iter().skip(1) {
        if status.peer_status.operation_count != first.peer_status.operation_count
            || status.peer_status.latest_order_digest != first.peer_status.latest_order_digest
            || status.peer_status.state_digest != first.peer_status.state_digest
        {
            bail!(
                "federation status is not converged: member {} has operations {} order digest {} state digest {}, expected operations {} order digest {} state digest {}",
                status.peer_status.member_id,
                status.peer_status.operation_count,
                status.peer_status.latest_order_digest,
                status.peer_status.state_digest,
                first.peer_status.operation_count,
                first.peer_status.latest_order_digest,
                first.peer_status.state_digest
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
    for status in statuses {
        if status.sync_state == "materialized_state_mismatch"
            || (status.sync_state == "halted" && status.sync_message.contains("digest mismatch"))
        {
            bail!(
                "federation member {} reports digest alert: sync={} message={}",
                status.peer_status.member_id,
                status.sync_state,
                status.sync_message
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
            if status.peer_status.state_digest != other.peer_status.state_digest {
                bail!(
                    "federation digest alert: members {} and {} disagree on state digest at operation count {}",
                    status.peer_status.member_id,
                    other.peer_status.member_id,
                    status.peer_status.operation_count
                );
            }
        }
    }

    Ok(())
}

fn ensure_statuses_session_outcome_quorum(
    public_config: &FederationConfig,
    statuses: &[FederationLocalStatusResponse],
) -> Result<()> {
    let required = usize::from(public_config.threshold_params.consensus_threshold);
    for status in statuses {
        let operation_count = status.peer_status.operation_count;
        if operation_count == 0 {
            continue;
        }
        let Some(quorum_operation_count) = status.session_outcome_quorum_operation_count else {
            bail!(
                "federation member {} has {} operations but no session outcome quorum evidence",
                status.peer_status.member_id,
                operation_count
            );
        };
        if quorum_operation_count < operation_count {
            bail!(
                "federation member {} has session outcome quorum evidence for {} operations, below current operation count {}",
                status.peer_status.member_id,
                quorum_operation_count,
                operation_count
            );
        }

        let mut distinct_signers = BTreeSet::new();
        for signer in &status.session_outcome_quorum_signers {
            if public_config.member(*signer).is_none() {
                bail!(
                    "federation member {} reports unknown session outcome quorum signer {}",
                    status.peer_status.member_id,
                    signer
                );
            }
            distinct_signers.insert(*signer);
        }
        if distinct_signers.len() < required {
            bail!(
                "federation member {} has {} distinct session outcome quorum signers, required {}",
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
mod tests {
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationJournalDigest,
        FederationPaymentObservationContext, FederationPaymentObservationEvidenceGroupSummary,
        FederationStateDigest, ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut22::AuthToken;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};
    use cdk_http_client::HttpError;
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

        async fn http_get<R>(&self, _url: Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
        where
            R: DeserializeOwned,
        {
            Err(HttpError::Other("unexpected GET".to_string()))
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
                let url = MintUrl::from_str(&format!("http://127.0.0.1:808{}", id + 4))
                    .expect("member url");
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
            peer_status: FederationPeerStatus::new(
                public_config.federation_id,
                member_id,
                operation_count,
                None,
                FederationJournalDigest::genesis(),
            ),
            sync_state: "healthy".to_string(),
            sync_ready: true,
            sync_message: "federation member is healthy".to_string(),
            session_outcome_quorum_operation_count: None,
            session_outcome_quorum_signers: Vec::new(),
            payment_observation_reports: Vec::new(),
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
        status
    }

    fn payment_report_summary(
        status: FederationPaymentObservationReportStatus,
        recommended_action: FederationPaymentObservationReportAction,
        stale: bool,
        quarantined: bool,
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
            quarantined,
            quarantine_operation_ids: Vec::new(),
            quarantine_operation_indices: Vec::new(),
            quarantine_reasons: Vec::new(),
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
    fn status_lines_include_payment_observation_report_counts() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 2);
        let first_observation_id =
            cdk_common::federation::FederationOperationId::from_bytes([1; 32]);
        let second_observation_id =
            cdk_common::federation::FederationOperationId::from_bytes([2; 32]);
        let third_observation_id =
            cdk_common::federation::FederationOperationId::from_bytes([3; 32]);
        let quarantine_id = cdk_common::federation::FederationOperationId::from_bytes([4; 32]);
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
            recommended_action: FederationPaymentObservationReportAction::Quarantine,
            quarantined: true,
            quarantine_operation_ids: vec![quarantine_id],
            quarantine_operation_indices: vec![4],
            quarantine_reasons: Vec::new(),
        }];

        let member_line = member_status_line(&status);
        assert!(member_line.contains("session_outcome_quorum_operations=none"));
        assert!(member_line.contains("session_outcome_quorum_signers=none"));
        assert!(member_line.contains("payment_reports=1"));
        assert!(member_line.contains("payment_conflicts=1"));
        assert!(member_line.contains("payment_unresolved=1"));
        assert!(member_line.contains("payment_stale=1"));
        assert!(member_line.contains("payment_quarantine=1"));

        let report_line = payment_observation_report_line(&status.payment_observation_reports[0]);
        assert!(report_line.contains("context=mint_quote"));
        assert!(report_line.contains("quote=quote-1"));
        assert!(report_line.contains("status=conflicting"));
        assert!(report_line.contains("observations=3"));
        assert!(report_line.contains("quote_age_secs=7200"));
        assert!(report_line.contains("stale=true"));
        assert!(report_line.contains("action=quarantine"));
        assert!(report_line.contains("quarantined=true"));
        assert!(report_line.contains("quarantine_ops=1"));
        assert!(report_line.contains(
            "quarantine_ids=0404040404040404040404040404040404040404040404040404040404040404"
        ));
        assert!(report_line.contains("quarantine_indices=4"));

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
                false,
            ),
            payment_report_summary(
                FederationPaymentObservationReportStatus::AwaitingQuorum,
                FederationPaymentObservationReportAction::None,
                false,
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
            false,
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
    fn status_payment_observation_alert_guard_rejects_quarantine() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 2);
        status.payment_observation_reports = vec![payment_report_summary(
            FederationPaymentObservationReportStatus::QuorumReached,
            FederationPaymentObservationReportAction::Quarantine,
            false,
            true,
        )];

        let err = ensure_statuses_no_payment_observation_alerts(&[status])
            .expect_err("quarantined report should fail");

        assert!(err.to_string().contains("action=quarantine"));
        assert!(err.to_string().contains("quarantined=true"));
    }

    #[test]
    fn status_line_includes_session_outcome_quorum_evidence() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 3);
        status.session_outcome_quorum_operation_count = Some(3);
        status.session_outcome_quorum_signers = vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two"),
        ];

        let member_line = member_status_line(&status);

        assert!(member_line.contains("session_outcome_quorum_operations=3"));
        assert!(member_line.contains("session_outcome_quorum_signers=1,2"));
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
    fn status_convergence_guard_rejects_state_digest_mismatch() {
        let (public_config, _) = status_fixture();
        let first = local_status(&public_config, 1, 0);
        let mut second = local_status(&public_config, 2, 0);
        second.peer_status.state_digest = FederationStateDigest::genesis();
        let statuses = vec![first, second];

        let err =
            ensure_statuses_converged(&statuses).expect_err("state digest mismatch should fail");

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
    fn status_digest_alert_guard_rejects_state_digest_mismatch() {
        let (public_config, _) = status_fixture();
        let first = local_status(&public_config, 1, 2);
        let mut second = local_status(&public_config, 2, 2);
        second.peer_status.state_digest = FederationStateDigest::genesis();
        let statuses = vec![first, second];

        let err =
            ensure_statuses_no_digest_alerts(&statuses).expect_err("state digest mismatch fails");

        assert!(err.to_string().contains("digest alert"));
        assert!(err.to_string().contains("state digest"));
    }

    #[test]
    fn status_digest_alert_guard_rejects_order_digest_mismatch() {
        let (public_config, _) = status_fixture();
        let first = local_status(&public_config, 1, 2);
        let mut second = local_status(&public_config, 2, 2);
        second.peer_status.latest_order_digest = FederationJournalDigest::from_bytes([42; 32]);
        second.peer_status.state_digest =
            FederationStateDigest::from_order_digest(second.peer_status.latest_order_digest);
        let statuses = vec![first, second];

        let err =
            ensure_statuses_no_digest_alerts(&statuses).expect_err("order digest mismatch fails");

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
    fn status_session_outcome_quorum_guard_accepts_zero_operation_member_without_evidence() {
        let (public_config, _) = status_fixture();
        let statuses = vec![local_status(&public_config, 1, 0)];

        ensure_statuses_session_outcome_quorum(&public_config, &statuses)
            .expect("zero operation member needs no session evidence");
    }

    #[test]
    fn status_session_outcome_quorum_guard_accepts_covered_current_height() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 3);
        status.session_outcome_quorum_operation_count = Some(3);
        status.session_outcome_quorum_signers = vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two"),
            MemberId::new(3).expect("member three"),
        ];

        ensure_statuses_session_outcome_quorum(&public_config, &[status])
            .expect("current height has quorum evidence");
    }

    #[test]
    fn status_session_outcome_quorum_guard_rejects_missing_evidence() {
        let (public_config, _) = status_fixture();
        let status = local_status(&public_config, 1, 3);

        let err = ensure_statuses_session_outcome_quorum(&public_config, &[status])
            .expect_err("non-empty operation history needs quorum evidence");

        assert!(err
            .to_string()
            .contains("no session outcome quorum evidence"));
    }

    #[test]
    fn status_session_outcome_quorum_guard_rejects_short_coverage() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 3);
        status.session_outcome_quorum_operation_count = Some(2);
        status.session_outcome_quorum_signers = vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two"),
            MemberId::new(3).expect("member three"),
        ];

        let err = ensure_statuses_session_outcome_quorum(&public_config, &[status])
            .expect_err("short quorum coverage should fail");

        assert!(err.to_string().contains("below current operation count"));
    }

    #[test]
    fn status_session_outcome_quorum_guard_rejects_insufficient_distinct_signers() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 3);
        status.session_outcome_quorum_operation_count = Some(3);
        status.session_outcome_quorum_signers = vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(1).expect("member one duplicate"),
            MemberId::new(2).expect("member two"),
        ];

        let err = ensure_statuses_session_outcome_quorum(&public_config, &[status])
            .expect_err("duplicate signers should not satisfy quorum");

        assert!(err
            .to_string()
            .contains("distinct session outcome quorum signers"));
    }

    #[test]
    fn status_session_outcome_quorum_guard_rejects_unknown_signer() {
        let (public_config, _) = status_fixture();
        let mut status = local_status(&public_config, 1, 3);
        status.session_outcome_quorum_operation_count = Some(3);
        status.session_outcome_quorum_signers = vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two"),
            MemberId::new(9).expect("unknown member"),
        ];

        let err = ensure_statuses_session_outcome_quorum(&public_config, &[status])
            .expect_err("unknown signer should fail");

        assert!(err
            .to_string()
            .contains("unknown session outcome quorum signer"));
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
}
