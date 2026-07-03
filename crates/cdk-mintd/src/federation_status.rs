use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use cdk_common::federation::{
    FederationConfig, FederationId, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate, MemberConfigPublic,
    MemberId, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::{Deserialize, Serialize};

use crate::cli::{CLIArgs, FederationStatusArgs};
use crate::config;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FederationLocalStatusResponse {
    pub(crate) version: u16,
    pub(crate) peer_status: FederationPeerStatus,
    pub(crate) sync_state: String,
    pub(crate) sync_ready: bool,
    pub(crate) sync_message: String,
}

pub(crate) async fn run_status_command(
    cli_args: &CLIArgs,
    args: &FederationStatusArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = status_config_paths(cli_args, args)?;
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
    if args.require_converged {
        ensure_statuses_converged(&statuses)?;
    }

    Ok(())
}

fn status_config_paths(
    cli_args: &CLIArgs,
    args: &FederationStatusArgs,
) -> Result<(PathBuf, PathBuf)> {
    match (&args.public_config, &args.private_config) {
        (Some(public_config), Some(private_config)) => {
            Ok((public_config.clone(), private_config.clone()))
        }
        (None, None) => {
            let config_path = cli_args.config.as_deref().ok_or_else(|| {
                anyhow!(
                    "federation status needs --public-config and --private-config, or a mintd --config with [federation] paths"
                )
            })?;
            let settings = config::Settings::try_new(Some(config_path))
                .with_context(|| format!("failed to load mint config {}", config_path.display()))?;
            let federation = settings
                .federation
                .ok_or_else(|| anyhow!("mint config has no [federation] section"))?;
            let public_config = federation.public_config_path.ok_or_else(|| {
                anyhow!("mint config [federation] has no public_config_path")
            })?;
            let private_config = federation.private_config_path.ok_or_else(|| {
                anyhow!("mint config [federation] has no private_config_path")
            })?;
            Ok((public_config, private_config))
        }
        _ => bail!(
            "federation status requires --public-config and --private-config to be provided together"
        ),
    }
}

async fn query_local_statuses<T>(
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
            public_config
                .member(member_id)
                .cloned()
                .ok_or_else(|| anyhow!("federation public config has no member {}", member_id))
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
        let peer = &status.peer_status;
        let latest_index = peer
            .latest_index
            .map(|index| index.to_string())
            .unwrap_or_else(|| "none".to_string());
        println!(
            "member {}: sync={} ready={} operations={} latest_index={} order_digest={} state_digest={} message={}",
            peer.member_id,
            status.sync_state,
            status.sync_ready,
            peer.operation_count,
            latest_index,
            peer.latest_order_digest,
            peer.state_digest,
            status.sync_message
        );
    }
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
        generate_trusted_dealer_configs, FederationJournalDigest, FederationStateDigest,
        ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
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
