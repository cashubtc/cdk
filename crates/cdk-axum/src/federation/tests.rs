use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::cdk_database::MintDatabase;
use cdk::cdk_payment::PaymentIdentifier;
use cdk::federation::{
    generate_trusted_dealer_configs, FederationAlephBftConsensusAdapter,
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationAlephBftUnitPayload, FederationConfigSnapshot, FederationConsensusBackendKind,
    FederationConsensusEngine, FederationConsensusItem, FederationConsensusRuntime,
    FederationDkgSetupMethod, FederationDkgTranscript, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignature, FederationError, FederationJournalAppendOutcome,
    FederationKeysetRotationOperation, FederationMemberStatusKind,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationOperationStatus,
    FederationPaymentObservationPolicy, FederationPaymentObservationQuarantineReason,
    FederationPaymentObservationReportAction, FederationPaymentTrustModel,
    FederationRestoreOperation, InMemoryFederationConsensus, MemberConfigPrivate, MemberId,
    ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, MintQuote, QuoteId};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut01::{evaluate_secret_key_coefficients, BlsG1PublicKey, BlsSecretKey};
use cdk::nuts::nut22::AuthToken;
use cdk::nuts::{
    BlindedMessage, CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteState, MintRequest, PaymentMethod, PublicKey, RestoreRequest,
    SecretKey,
};
use cdk::types::FeeReserve;
use cdk::Amount;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
use cdk_http_client::HttpError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::ServiceExt;

use super::*;

mod aleph_bft_routes;
mod dkg_setup_routes;
mod dkg_transcript_signature;
mod private_operation_route;

#[cfg(feature = "prometheus")]
fn prometheus_metric_value(metric: &str, labels: &[(&str, &str)]) -> f64 {
    let registry = cdk_prometheus::METRICS.registry();
    let scrape = cdk_prometheus::prometheus::TextEncoder::new()
        .encode_to_string(&registry.gather())
        .expect("encode prometheus metrics");
    scrape
        .lines()
        .find_map(|line| {
            if !line.starts_with(metric) {
                return None;
            }
            for (key, value) in labels {
                let fragment = format!("{key}=\"{value}\"");
                if !line.contains(&fragment) {
                    return None;
                }
            }
            line.rsplit_once(' ')
                .and_then(|(_, value)| value.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}

#[test]
fn federation_peer_sync_disable_value_accepts_truthy_values() {
    for value in [
        "1", "true", "TRUE", "True", "yes", "YES", "Yes", "on", "ON", "On",
    ] {
        assert!(federation_peer_sync_disabled_value(value));
    }

    for value in ["", "0", "false", "no", "off"] {
        assert!(!federation_peer_sync_disabled_value(value));
    }
}

#[test]
fn session_outcome_quorum_prefix_stops_when_next_index_would_overflow() {
    let mut signers = BTreeSet::new();
    signers.insert(member_id(1));
    let mut quorums = BTreeMap::new();
    quorums.insert(
        0,
        vec![cdk::federation::FederationSessionOutcomeQuorum {
            required: 1,
            signers,
            from_index: 0,
            latest_index: u64::MAX,
            operation_count: u64::MAX,
            range_order_digest: cdk::federation::FederationJournalDigest::genesis(),
        }],
    );

    let status = contiguous_session_outcome_quorum_prefix(&quorums).expect("quorum prefix status");

    assert_eq!(status.from_index, 0);
    assert_eq!(status.latest_index, u64::MAX);
    assert_eq!(status.operation_count, u64::MAX);
}

#[test]
fn session_outcome_range_rejects_span_larger_than_available_entries() {
    let order_digest = cdk::federation::FederationJournalDigest::genesis();
    let range = SessionOutcomeRangeKey {
        from_index: 0,
        operation_count: u64::MAX,
        latest_index: u64::MAX - 1,
        previous_order_digest: order_digest,
        range_order_digest: order_digest,
        latest_order_digest: order_digest,
        state_digest: cdk::federation::FederationStateDigest::from_order_digest(order_digest),
        operation_ids: Vec::new(),
    };

    assert!(entries_for_session_outcome_range(&BTreeMap::new(), &range).is_none());
}

#[test]
fn federation_peer_sync_config_rejects_invalid_values() {
    let valid = FederationPeerSyncConfig::default();
    let cases = [
        FederationPeerSyncConfig {
            peer_sync_interval: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            peer_request_timeout: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            catch_up_limit: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            digest_recheck_attempts: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            digest_recheck_delay: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            session_outcome_evidence_chunk_size: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            session_outcome_evidence_chunk_size: usize::from(u16::MAX) + 1,
            ..valid
        },
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            reconnect_max_backoff: Duration::ZERO,
            ..valid
        },
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(1),
            ..valid
        },
        FederationPeerSyncConfig {
            private_rest_inbound_concurrency_limit: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            private_rest_outbound_concurrency_limit: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            state_digest_checkpoint_retention: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            pending_mempool_item_limit: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            response_waiter_limit: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            response_cache_limit: 0,
            ..valid
        },
    ];

    for config in cases {
        assert!(
            config.validate().is_err(),
            "invalid peer sync config should fail: {config:?}"
        );
    }
    assert!(valid.validate().is_ok());
}

#[tokio::test(start_paused = true)]
async fn federation_peer_sync_interval_uses_configured_delay() {
    let configured_delay = Duration::from_secs(2);
    let mut interval = federation_peer_sync_interval(FederationPeerSyncConfig {
        peer_sync_interval: configured_delay,
        ..FederationPeerSyncConfig::default()
    });
    interval.tick().await;

    let second_tick = tokio::spawn(async move {
        interval.tick().await;
    });
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(configured_delay - Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(Duration::from_millis(1)).await;
    second_tick.await.expect("second configured tick");
}

#[test]
fn federation_reconnect_backoff_doubles_until_max() {
    let config = FederationPeerSyncConfig {
        reconnect_initial_backoff: Duration::from_secs(2),
        reconnect_max_backoff: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };

    assert_eq!(
        federation_reconnect_backoff_delay(config, 0),
        Duration::from_secs(2)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 1),
        Duration::from_secs(4)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 2),
        Duration::from_secs(5)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, u32::MAX),
        Duration::from_secs(5)
    );
}

#[derive(Debug, Default)]
struct MockFederationTransportState {
    status_response: Option<FederationPeerStatus>,
    submit_response: Option<FederationSubmitOperationResponse>,
    aleph_bft_finalized_unit_response: Option<FederationAlephBftFinalizedUnitResponse>,
    dkg_setup_responses: VecDeque<FederationDkgSetupResponse>,
    catch_up_responses: VecDeque<FederationJournalCatchUpResponse>,
    status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
    dkg_commitment_requests: Vec<FederationSignedRequest<FederationDkgCommitmentRequest>>,
    dkg_reveal_requests: Vec<FederationSignedRequest<FederationDkgRevealRequest>>,
    dkg_secret_share_requests: Vec<FederationSignedRequest<FederationDkgSecretShareRequest>>,
    dkg_transcript_signature_requests:
        Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
    submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
    aleph_bft_finalized_unit_requests:
        Vec<FederationSignedRequest<FederationAlephBftFinalizedUnitRequest>>,
    catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
}

#[derive(Debug, Clone, Default)]
struct MockFederationTransport {
    state: Arc<Mutex<MockFederationTransportState>>,
}

#[derive(Debug, Clone)]
struct DelayTransport {
    delay: Duration,
}

impl Default for DelayTransport {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(60),
        }
    }
}

struct TempMintDbFile {
    path: String,
}

impl TempMintDbFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();

        Self { path }
    }

    fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for TempMintDbFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}-shm", self.path));
        let _ = std::fs::remove_file(format!("{}-wal", self.path));
    }
}

impl MockFederationTransport {
    fn with_status_response(self, response: FederationPeerStatus) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_response = Some(response);
        self
    }

    fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .submit_response = Some(response);
        self
    }

    fn with_aleph_bft_finalized_unit_response(
        self,
        response: FederationAlephBftFinalizedUnitResponse,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_finalized_unit_response = Some(response);
        self
    }

    fn with_dkg_setup_response(self, response: FederationDkgSetupResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_setup_responses
            .push_back(response);
        self
    }

    fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_responses
            .push_back(response);
        self
    }

    fn status_requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_requests
            .clone()
    }

    fn dkg_commitment_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgCommitmentRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_commitment_requests
            .clone()
    }

    fn dkg_reveal_requests(&self) -> Vec<FederationSignedRequest<FederationDkgRevealRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_reveal_requests
            .clone()
    }

    fn dkg_secret_share_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgSecretShareRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_secret_share_requests
            .clone()
    }

    fn dkg_transcript_signature_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_transcript_signature_requests
            .clone()
    }

    fn submitted_operations(
        &self,
    ) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .submitted_operations
            .clone()
    }

    fn aleph_bft_finalized_unit_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftFinalizedUnitRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_finalized_unit_requests
            .clone()
    }

    fn catch_up_requests(&self) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_requests
            .clone()
    }
}

#[async_trait]
impl Transport for DelayTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get<R>(&self, _url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed GET completed".to_string()))
    }

    async fn http_post<P, R>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed POST completed".to_string()))
    }
}

#[async_trait]
impl Transport for MockFederationTransport {
    fn with_proxy(
        &mut self,
        _proxy: url::Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(Vec::new())
    }

    async fn http_get<R>(&self, url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        Err(HttpError::Other(format!(
            "unexpected GET path {}",
            url.path()
        )))
    }

    async fn http_post<P, R>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let mut state = self.state.lock().expect("mock transport lock");
        match url.path() {
            path if path.ends_with("/federation/v1/status") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationPeerStatusRequest>,
                >(serde_json::to_value(payload)?)?;
                state.status_requests.push(request);
                let response = state
                    .status_response
                    .clone()
                    .ok_or_else(|| HttpError::Other("missing status response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/commitments") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgCommitmentRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_commitment_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/reveals") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgRevealRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_reveal_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/secret-shares") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgSecretShareRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_secret_share_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/dkg/transcript-signatures") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationDkgTranscriptSignatureRequest>,
                >(serde_json::to_value(payload)?)?;
                state.dkg_transcript_signature_requests.push(request);
                let response = state
                    .dkg_setup_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing DKG setup response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/operations") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationSubmitOperationRequest>,
                >(serde_json::to_value(payload)?)?;
                state.submitted_operations.push(request);
                let response = state
                    .submit_response
                    .clone()
                    .ok_or_else(|| HttpError::Other("missing submit response".to_string()))?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/aleph-bft/finalized-units") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationAlephBftFinalizedUnitRequest>,
                >(serde_json::to_value(payload)?)?;
                state.aleph_bft_finalized_unit_requests.push(request);
                let response =
                    state
                        .aleph_bft_finalized_unit_response
                        .clone()
                        .ok_or_else(|| {
                            HttpError::Other("missing AlephBFT finalized-unit response".to_string())
                        })?;
                json_response(response)
            }
            path if path.ends_with("/federation/v1/journal") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationJournalCatchUpRequest>,
                >(serde_json::to_value(payload)?)?;
                state.catch_up_requests.push(request);
                let response = state
                    .catch_up_responses
                    .pop_front()
                    .ok_or_else(|| HttpError::Other("missing catch-up response".to_string()))?;
                json_response(response)
            }
            path => Err(HttpError::Other(format!("unexpected POST path {path}"))),
        }
    }
}

fn json_response<T, R>(value: T) -> Result<R, HttpError>
where
    T: Serialize,
    R: DeserializeOwned,
{
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("member id")
}

fn private_member(value: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: None,
        member_id: member_id(value),
        identity_secret_key: SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    }
}

fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    transport_config_with_federation_api_urls(
        MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
        MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
    )
}

fn transport_config_with_federation_api_urls(
    first_federation_api_url: MintUrl,
    second_federation_api_url: MintUrl,
) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
    let members = vec![
        TrustedDealerMember::new(
            member_id(1),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            first_federation_api_url,
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(2),
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            second_federation_api_url,
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::new(2, 2, 2).expect("threshold params"),
        members,
        vec![keyset],
    )
    .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&member_id(1))
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&member_id(2))
        .expect("second private config")
        .clone();
    (bundle.public_config, first_private, second_private)
}

fn transport_config_three() -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    transport_config_three_with_threshold(ThresholdParams::new(3, 2, 2).expect("threshold params"))
}

fn transport_config_three_with_threshold(
    threshold_params: ThresholdParams,
) -> (
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let members = vec![
        TrustedDealerMember::new(
            member_id(1),
            MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(2),
            MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[2; 32]).expect("identity secret"),
        ),
        TrustedDealerMember::new(
            member_id(3),
            MintUrl::from_str("https://mint3.example.com").expect("public mint url"),
            MintUrl::from_str("https://fed3.example.com/api").expect("federation api url"),
            SecretKey::from_slice(&[3; 32]).expect("identity secret"),
        ),
    ];
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[11; 32]),
                BlsSecretKey::from_reduced_bytes(&[12; 32]),
            ],
        )]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let first_private = bundle
        .private_configs
        .get(&member_id(1))
        .expect("first private config")
        .clone();
    let second_private = bundle
        .private_configs
        .get(&member_id(2))
        .expect("second private config")
        .clone();
    let third_private = bundle
        .private_configs
        .get(&member_id(3))
        .expect("third private config")
        .clone();
    (
        bundle.public_config,
        first_private,
        second_private,
        third_private,
    )
}

async fn federation_route_state_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, local_private, peer_private) = transport_config();
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;

    (state, public_config, local_private, peer_private)
}

async fn federation_route_state_three_with_local_private() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPrivate,
) {
    let (public_config, first_private, second_private, third_private) = transport_config_three();
    let state =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;

    (
        state,
        public_config,
        first_private,
        second_private,
        third_private,
    )
}

async fn federation_route_state_for_private(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
) -> MintState {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    federation_route_state_for_private_with_db(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await
}

async fn federation_route_state_for_private_with_db(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        seed,
        FederationConsensusBackendKind::InMemory,
    )
    .await
}

async fn federation_route_state_for_private_with_db_and_backend(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    consensus_backend: FederationConsensusBackendKind,
) -> MintState {
    let runtime_config = cdk::mint::FederationRuntimeConfig::new_with_consensus_backend(
        public_config.clone(),
        local_private.clone(),
        consensus_backend,
    )
    .expect("runtime config");
    let mut builder = MintBuilder::new(db.clone())
        .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
        .with_federation_config(runtime_config.clone());
    let fake = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(fake),
        )
        .await
        .expect("payment processor");
    let mint = builder.build_with_seed(db, seed).await.expect("mint");
    MintState {
        mint: Arc::new(mint),
        cache: Arc::new(crate::cache::HttpCache::default()),
        federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(match consensus_backend {
            FederationConsensusBackendKind::InMemory => FederationConsensusRuntime::with_backend(
                runtime_config.consensus_backend(),
                runtime_config.federation_id(),
                runtime_config.member_id(),
            )
            .expect("consensus runtime"),
            FederationConsensusBackendKind::AlephBft => {
                FederationConsensusRuntime::aleph_bft_from_finalized_items(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                    0,
                    public_config.members.keys().copied(),
                    public_config.threshold_params.consensus_threshold,
                    Vec::new(),
                )
                .expect("AlephBFT consensus runtime")
            }
        }))),
        federation_peer_sync_config: Some(FederationPeerSyncConfig::default()),
        federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
            FederationRestSequenceTracker::new(),
        ))),
        federation_sync: Some(Arc::new(tokio::sync::Mutex::new(
            crate::FederationSyncState::for_consensus_backend(consensus_backend),
        ))),
        federation_notifications: Some(
            Arc::new(crate::FederationOperationNotifications::default()),
        ),
        federation_finalizer: None,
        federation_private_rest_admission: Some(Arc::new(tokio::sync::Semaphore::new(
            FederationPeerSyncConfig::default().private_rest_inbound_concurrency_limit,
        ))),
        federation_invalid_operation_alerts: Some(Arc::new(tokio::sync::Mutex::new(
            crate::FederationInvalidOperationAlertTracker::default(),
        ))),
        federation_aleph_bft_pending_sequences: Some(Arc::new(tokio::sync::Mutex::new(
            HashMap::new(),
        ))),
    }
}

async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
    let (state, public_config, _, peer_private) = federation_route_state_with_local_private().await;
    (state, public_config, peer_private)
}

fn signed_operation_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    kind: FederationRestMessageKind,
    envelope: FederationOperationEnvelope,
) -> FederationSignedRequest<FederationSubmitOperationRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        kind,
        FederationSubmitOperationRequest::new(envelope),
        private_config,
    )
    .expect("signed operation request")
}

fn signed_status_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    signed_status_request_with_payload(
        public_config,
        private_config,
        receiver,
        sequence,
        FederationPeerStatusRequest::default(),
    )
}

fn signed_status_request_with_payload(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    payload: FederationPeerStatusRequest,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::Status,
        payload,
        private_config,
    )
    .expect("signed status request")
}

fn signed_catch_up_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::CatchUp,
        request,
        private_config,
    )
    .expect("signed catch-up request")
}

fn signed_catch_up_response(
    response: FederationJournalCatchUpResponse,
    private_config: &MemberConfigPrivate,
) -> FederationJournalCatchUpResponse {
    response
        .with_signed_session_outcome(private_config)
        .expect("signed catch-up response")
}

fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> cdk::federation::FederationDkgParticipantCommitmentReveal {
    cdk::federation::FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        member_id,
        coefficient_commitments: BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[seed; 32]).public_key_g2(),
                BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]).public_key_g2(),
            ],
        )]),
    }
}

fn dkg_commitment_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgCommitmentRequest {
    let reveal = dkg_reveal(public_config, member_id, seed);
    let commitment = cdk::federation::FederationDkgParticipantCommitment::from_reveal(&reveal)
        .expect("dkg commitment");
    FederationDkgCommitmentRequest::new(commitment)
}

fn dkg_reveal_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgRevealRequest {
    FederationDkgRevealRequest::new(dkg_reveal(public_config, member_id, seed))
}

fn dkg_secret_share_request(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        sender,
        receiver,
        BTreeMap::from([(
            Amount::from(1),
            evaluate_secret_key_coefficients(
                receiver.to_bls_signer_id(),
                &[
                    BlsSecretKey::from_reduced_bytes(&[seed; 32]),
                    BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]),
                ],
            )
            .expect("dkg secret share"),
        )]),
    )
}

fn signed_dkg_commitment_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgCommitmentRequest,
) -> FederationSignedRequest<FederationDkgCommitmentRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgCommitment,
        request,
        private_config,
    )
    .expect("signed DKG commitment request")
}

fn signed_dkg_reveal_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgRevealRequest,
) -> FederationSignedRequest<FederationDkgRevealRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgReveal,
        request,
        private_config,
    )
    .expect("signed DKG reveal request")
}

fn signed_dkg_secret_share_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgSecretShareRequest,
) -> FederationSignedRequest<FederationDkgSecretShareRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgSecretShare,
        request,
        private_config,
    )
    .expect("signed DKG secret-share request")
}

async fn persist_complete_dkg_round(
    state: &MintState,
    public_config: &FederationConfig,
    first_private: &MemberConfigPrivate,
    second_private: &MemberConfigPrivate,
) {
    for (private_config, seed) in [(first_private, 11), (second_private, 21)] {
        let commitment =
            dkg_commitment_request(public_config, private_config.member_id, seed).commitment;
        state
            .persist_federation_dkg_commitment(&commitment, private_config.member_id, 1)
            .await
            .expect("persist DKG commitment");
        let reveal = dkg_reveal_request(public_config, private_config.member_id, seed).reveal;
        state
            .persist_federation_dkg_reveal(&reveal, private_config.member_id, 2)
            .await
            .expect("persist DKG reveal");
    }
}

fn dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> FederationDkgTranscriptSignatureRequest {
    let transcript = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript");
    let signature = cdk::federation::FederationDkgTranscriptSignature::sign(
        transcript.transcript_hash,
        private_config,
    )
    .expect("transcript signature");
    FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        private_config.member_id,
        transcript.transcript_hash,
        signature,
    )
}

async fn route_derived_dkg_transcript_fixture() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    FederationDkgTranscriptHash,
) {
    let (trusted_public_config, local_private, peer_private) = transport_config();
    let mut setup_public_config = trusted_public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config.validate().expect("setup public config");

    let mut setup_local_private = local_private.clone();
    setup_local_private.keyset_secret_shares.clear();
    setup_local_private.setup_transcript_hash = None;
    setup_local_private
        .validate_against_public(&setup_public_config)
        .expect("setup local private config");
    let mut setup_peer_private = peer_private.clone();
    setup_peer_private.keyset_secret_shares.clear();
    setup_peer_private.setup_transcript_hash = None;
    setup_peer_private
        .validate_against_public(&setup_public_config)
        .expect("setup peer private config");

    let state = federation_route_state_for_private(
        setup_public_config.clone(),
        setup_local_private.clone(),
    )
    .await;
    let local_member = setup_local_private.member_id;
    persist_complete_dkg_round(
        &state,
        &setup_public_config,
        &setup_local_private,
        &setup_peer_private,
    )
    .await;
    let local_contribution =
        dkg_secret_share_request(&setup_public_config, local_member, local_member, 11);
    state
        .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
        .await
        .expect("persist local secret-share contribution");
    let peer_contribution = dkg_secret_share_request(
        &setup_public_config,
        setup_peer_private.member_id,
        local_member,
        21,
    );
    state
        .persist_federation_dkg_secret_share_contribution(
            &peer_contribution,
            setup_peer_private.member_id,
            3,
        )
        .await
        .expect("persist peer secret-share contribution");
    let keyset_share = state
        .mint
        .localstore()
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("route-derived keyset shares")
        .into_iter()
        .next()
        .expect("route-derived keyset share");
    let final_public_config = FederationConfig::new(
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset],
    )
    .expect("final public config");
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
    )
    .expect("route-derived transcript hash");

    (
        state,
        setup_public_config,
        setup_local_private,
        setup_peer_private,
        transcript_hash,
    )
}

fn signed_dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgTranscriptSignatureRequest,
) -> FederationSignedRequest<FederationDkgTranscriptSignatureRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgTranscriptSignature,
        request,
        private_config,
    )
    .expect("signed DKG transcript signature request")
}

async fn consensus_operation_count(state: &MintState) -> u64 {
    state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .peer_status()
        .operation_count
}

fn reset_federation_runtime_journal(state: &mut MintState) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    state.federation_consensus = Some(Arc::new(tokio::sync::Mutex::new(
        FederationConsensusRuntime::with_backend(
            runtime_config.consensus_backend(),
            runtime_config.federation_id(),
            runtime_config.member_id(),
        )
        .expect("empty federation runtime"),
    )));
}

async fn peer_status_with_materialized_digest(
    state: &MintState,
    consensus: &InMemoryFederationConsensus,
) -> FederationPeerStatus {
    let mut status = consensus.peer_status();
    status.state_digest = materialized_state_digest(state, consensus.journal().entries())
        .await
        .expect("materialized peer status digest");
    status
}

async fn catch_up_response_with_materialized_digest(
    state: &MintState,
    consensus: &InMemoryFederationConsensus,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = consensus
        .catch_up_checked(request)
        .expect("peer catch-up response");
    response.state_digest = materialized_state_digest(state, consensus.journal().entries())
        .await
        .expect("materialized catch-up state digest");
    response
}

fn payment_observation_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: "quote-paid".to_string(),
            observer: MemberId::new(1).expect("member id"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(21),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some("fake-wallet-paid".to_string()),
            observed_time: None,
        }),
    )
}

fn oversized_payment_observation_envelope(
    federation_id: FederationId,
) -> FederationOperationEnvelope {
    let mut envelope = payment_observation_envelope(federation_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("mint quote payment observation");
    };
    observation.payment_proof =
        Some("x".repeat(cdk::federation::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
    envelope
}

fn mint_quote_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_operation(
        quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(1),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(Amount::from(1)),
            unit: Some(CurrencyUnit::Sat),
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    )
}

fn paid_observation_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_payment_observation(
        quote_id.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(1),
        Amount::ZERO,
        Some(format!("paid-{quote_id}")),
    )
}

fn mint_payment_observation_envelope_for_member(
    state: &MintState,
    quote_id: &QuoteId,
    observer: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    FederationOperationEnvelope::new(
        config.federation_id(),
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: quote_id.to_string(),
            observer: member_id(observer),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
            observed_time: None,
        }),
    )
}

fn mint_output(state: &MintState, quote_id: &QuoteId) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay output {quote_id}").as_bytes(),
        )),
    )
}

fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
    match amount {
        1 => BlsSecretKey::from_reduced_bytes(&[11; 32]),
        _ => panic!("missing aggregate secret fixture for amount {amount}"),
    }
}

fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
    let signature =
        BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(&aggregate_secret_for_amount(amount));

    cdk::nuts::Proof::new(
        Amount::from(amount),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

fn swap_output(state: &MintState, label: &str) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay swap output {label}").as_bytes(),
        )),
    )
}

async fn melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
    melt_quote_with_description(state, amount_msat, "".to_string()).await
}

async fn melt_quote_with_description(
    state: &MintState,
    amount_msat: u64,
    description: String,
) -> QuoteId {
    let response = state
        .mint
        .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(amount_msat, description),
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .expect("melt quote");

    match response {
        cdk::MeltQuoteCreateResponse::Bolt11(response) => response.quote,
        _ => panic!("expected bolt11 melt quote"),
    }
}

fn melt_payment_observation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    state_observed: MeltQuoteState,
    payment_proof: Option<String>,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.melt_quote_payment_observation(
        quote_id.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        state_observed,
        Amount::from(1),
        Amount::from(1),
        payment_proof,
        None,
    )
}

fn restore_envelope(
    federation_id: FederationId,
    label: &'static str,
) -> FederationOperationEnvelope {
    restore_envelope_with_output(
        federation_id,
        PublicKey::from_hex(label).expect("public key"),
    )
}

fn indexed_restore_envelope(
    federation_id: FederationId,
    index: u64,
) -> FederationOperationEnvelope {
    restore_envelope_with_output(
        federation_id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation evidence restore {index}").as_bytes(),
        )),
    )
}

fn restore_envelope_with_output(
    federation_id: FederationId,
    blinded_secret: PublicKey,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::Restore(cdk::federation::FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![cdk::nuts::BlindedMessage::new(
                    Amount::from(1),
                    cdk::nuts::Id::from_str("00bfa73302d12ffd").expect("keyset id"),
                    blinded_secret,
                )],
            },
        }),
    )
}

#[tokio::test]
async fn rest_peer_client_validates_peer_status() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new(
        federation_id,
        member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status.clone());
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    let response = client.peer_status(member).await.expect("peer status");

    assert_eq!(response, status);
    let requests = transport.status_requests();
    assert_eq!(requests.len(), 1);
    requests[0]
        .verify(&public_config, member.member_id)
        .expect("signed status request");
    assert_eq!(requests[0].sender, member_id(1));
    assert_eq!(requests[0].kind, FederationRestMessageKind::Status);
}

#[tokio::test(start_paused = true)]
async fn rest_peer_client_applies_configured_peer_request_timeout() {
    let (public_config, local_private, _) = transport_config();
    let member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let timeout = Duration::from_secs(2);
    let client = FederationRestPeerClient::new_with_config(
        DelayTransport {
            delay: timeout + Duration::from_secs(60),
        },
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            peer_request_timeout: timeout,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let task = tokio::spawn(async move { client.peer_status(&member).await });
    tokio::task::yield_now().await;
    tokio::time::advance(timeout).await;
    let err = task
        .await
        .expect("peer status task")
        .expect_err("peer status should time out");

    assert!(
        err.to_string().contains("status request timed out"),
        "unexpected timeout error: {err}"
    );
}

#[tokio::test(start_paused = true)]
async fn rest_peer_client_applies_configured_operation_submission_timeout() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let timeout = Duration::from_secs(2);
    let request = FederationSubmitOperationRequest::new(restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    ));
    let client = FederationRestPeerClient::new_with_config(
        DelayTransport {
            delay: timeout + Duration::from_secs(60),
        },
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            operation_submission_timeout: timeout,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let task = tokio::spawn(async move { client.submit_operation(&member, &request).await });
    tokio::task::yield_now().await;
    tokio::time::advance(timeout).await;
    let err = task
        .await
        .expect("operation submission task")
        .expect_err("operation submission should time out");

    assert!(
        err.to_string().contains("operation submission timed out"),
        "unexpected timeout error: {err}"
    );
}

#[tokio::test]
async fn rest_peer_client_rejects_outbound_overflow_before_transport_request() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new(
        federation_id,
        member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            private_rest_outbound_concurrency_limit: 1,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let _permit = client
        .outbound_admission
        .clone()
        .try_acquire_owned()
        .expect("held outbound permit");

    let err = client
        .peer_status(member)
        .await
        .expect_err("outbound overflow should fail");

    assert!(
        err.to_string().contains("outbound REST queue is full"),
        "unexpected overflow error: {err}"
    );
    assert!(
        transport.status_requests().is_empty(),
        "overflow must be rejected before sending a REST request"
    );
}

#[tokio::test]
async fn rest_peer_client_initial_sequence_advances_across_clients() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new(
        federation_id,
        member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let first_client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("first client");
    first_client
        .peer_status(member)
        .await
        .expect("first peer status");

    let second_client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
            .expect("second client");
    second_client
        .peer_status(member)
        .await
        .expect("second peer status");

    let requests = transport.status_requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].sequence > 1);
    assert!(
        requests[1].sequence > requests[0].sequence,
        "fresh clients must not replay stale federation REST sequences"
    );
}

#[tokio::test]
async fn rest_peer_client_older_client_sequence_catches_up_after_newer_client() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new(
        federation_id,
        member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let older_client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("older client");
    older_client
        .peer_status(member)
        .await
        .expect("older first peer status");

    let newer_client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
            .expect("newer client");
    newer_client
        .peer_status(member)
        .await
        .expect("newer peer status");
    older_client
        .peer_status(member)
        .await
        .expect("older second peer status");

    let requests = transport.status_requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1].sequence > requests[0].sequence,
        "newer client should advance the process-wide REST sequence"
    );
    assert!(
        requests[2].sequence > requests[1].sequence,
        "older client must reserve a sequence above newer client traffic"
    );
}

#[tokio::test]
async fn rest_peer_client_submits_dkg_setup_messages() {
    let (public_config, local_private, _) = transport_config();
    let member = public_config.member(member_id(2)).expect("peer member");
    let commitment_request = dkg_commitment_request(&public_config, local_private.member_id, 11);
    let reveal_request = dkg_reveal_request(&public_config, local_private.member_id, 11);
    let secret_share_request = dkg_secret_share_request(
        &public_config,
        local_private.member_id,
        member.member_id,
        11,
    );
    let transcript_signature_request =
        dkg_transcript_signature_request(&public_config, &local_private);
    let transport = MockFederationTransport::default()
        .with_dkg_setup_response(FederationDkgSetupResponse::new(
            public_config.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgCommitment,
            local_private.member_id,
        ))
        .with_dkg_setup_response(FederationDkgSetupResponse::new(
            public_config.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgReveal,
            local_private.member_id,
        ))
        .with_dkg_setup_response(FederationDkgSetupResponse::new(
            public_config.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgSecretShare,
            local_private.member_id,
        ))
        .with_dkg_setup_response(FederationDkgSetupResponse::new(
            public_config.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgTranscriptSignature,
            local_private.member_id,
        ));
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("client");

    client
        .submit_dkg_commitment(member, &commitment_request)
        .await
        .expect("submit DKG commitment");
    client
        .submit_dkg_reveal(member, &reveal_request)
        .await
        .expect("submit DKG reveal");
    client
        .submit_dkg_secret_share(member, &secret_share_request)
        .await
        .expect("submit DKG secret share");
    client
        .submit_dkg_transcript_signature(member, &transcript_signature_request)
        .await
        .expect("submit DKG transcript signature");

    let commitments = transport.dkg_commitment_requests();
    let reveals = transport.dkg_reveal_requests();
    let secret_shares = transport.dkg_secret_share_requests();
    let transcript_signatures = transport.dkg_transcript_signature_requests();
    assert_eq!(commitments.len(), 1);
    assert_eq!(reveals.len(), 1);
    assert_eq!(secret_shares.len(), 1);
    assert_eq!(transcript_signatures.len(), 1);
    commitments[0]
        .verify(&public_config, member.member_id)
        .expect("commitment signature");
    commitments[0]
        .payload
        .validate_sender(&public_config, commitments[0].sender)
        .expect("commitment payload owner");
    reveals[0]
        .verify(&public_config, member.member_id)
        .expect("reveal signature");
    reveals[0]
        .payload
        .validate_sender(&public_config, reveals[0].sender)
        .expect("reveal payload owner");
    secret_shares[0]
        .verify(&public_config, member.member_id)
        .expect("secret-share request");
    secret_shares[0]
        .payload
        .validate_sender_receiver(&public_config, secret_shares[0].sender, member.member_id)
        .expect("secret-share payload owner");
    transcript_signatures[0]
        .verify(&public_config, member.member_id)
        .expect("transcript signature request");
    transcript_signatures[0]
        .payload
        .validate_sender(&public_config, transcript_signatures[0].sender)
        .expect("transcript signature payload owner");
    assert_eq!(commitments[0].sender, local_private.member_id);
    assert_eq!(reveals[0].sender, local_private.member_id);
    assert_eq!(secret_shares[0].sender, local_private.member_id);
    assert_eq!(transcript_signatures[0].sender, local_private.member_id);
    assert!(reveals[0].sequence > commitments[0].sequence);
    assert!(secret_shares[0].sequence > reveals[0].sequence);
    assert!(transcript_signatures[0].sequence > secret_shares[0].sequence);
}

#[tokio::test]
async fn rest_peer_client_submits_dkg_setup_messages_over_http() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind receiver federation route");
    let address = listener.local_addr().expect("receiver route address");
    let sender_federation_api_url =
        MintUrl::from_str("http://127.0.0.1:9").expect("sender federation api url");
    let receiver_federation_api_url =
        MintUrl::from_str(&format!("http://127.0.0.1:{}", address.port()))
            .expect("receiver federation api url");
    let (public_config, sender_private, receiver_private) =
        transport_config_with_federation_api_urls(
            sender_federation_api_url,
            receiver_federation_api_url,
        );
    let receiver_member = public_config
        .member(receiver_private.member_id)
        .expect("receiver member");
    let commitment_request = dkg_commitment_request(&public_config, sender_private.member_id, 11);
    let expected_commitment = commitment_request.commitment;
    let reveal_request = dkg_reveal_request(&public_config, sender_private.member_id, 11);
    let expected_reveal = reveal_request.reveal.clone();
    let secret_share_request = dkg_secret_share_request(
        &public_config,
        sender_private.member_id,
        receiver_private.member_id,
        11,
    );
    let expected_secret_share = secret_share_request.clone();
    let receiver_state =
        federation_route_state_for_private(public_config.clone(), receiver_private).await;
    let router = Router::new()
        .nest("/federation/v1", create_federation_router())
        .with_state(receiver_state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("receiver federation route server");
    });
    tokio::task::yield_now().await;

    let client = FederationRestPeerClient::new(Async::default(), &public_config, sender_private)
        .expect("client");
    client
        .submit_dkg_commitment(receiver_member, &commitment_request)
        .await
        .expect("submit DKG commitment");
    client
        .submit_dkg_reveal(receiver_member, &reveal_request)
        .await
        .expect("submit DKG reveal");
    client
        .submit_dkg_secret_share(receiver_member, &secret_share_request)
        .await
        .expect("submit DKG secret share");

    let sequences = receiver_state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    let sender_sequence = sequences
        .get(&member_id(1))
        .copied()
        .expect("persisted sender sequence");
    assert!(sender_sequence >= 3);
    let commitments = receiver_state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("persisted commitments");
    let reveals = receiver_state
        .mint
        .localstore()
        .get_federation_dkg_reveals()
        .await
        .expect("persisted reveals");
    let secret_shares = receiver_state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("persisted secret-share contributions");
    assert_eq!(commitments, vec![expected_commitment]);
    assert_eq!(reveals, vec![expected_reveal]);
    assert_eq!(secret_shares, vec![expected_secret_share]);

    server.abort();
}

#[tokio::test]
async fn rest_peer_client_rejects_status_wrong_federation() {
    let (public_config, local_private, _) = transport_config();
    let member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new(
        super::FederationId::from_bytes([32; 32]),
        member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    assert!(client.peer_status(member).await.is_err());
}

#[tokio::test]
async fn rest_peer_client_submits_payment_observation_operations() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let envelope = payment_observation_envelope(federation_id);
    let request = FederationSubmitOperationRequest::new(envelope.clone());
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
    let submit_response = peer_consensus
        .submit_operation(FederationSubmitOperationRequest::new(envelope))
        .expect("peer submit response");
    let transport =
        MockFederationTransport::default().with_submit_response(submit_response.clone());
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    let response = client
        .submit_operation(member, &request)
        .await
        .expect("submit operation");

    assert_eq!(response.operation_id, submit_response.operation_id);
    let submitted = transport.submitted_operations();
    assert_eq!(submitted.len(), 1);
    submitted[0]
        .verify(&public_config, member.member_id)
        .expect("signed submit request");
    assert_eq!(
        submitted[0].payload.operation_id().expect("operation id"),
        response.operation_id
    );
    assert_eq!(
        submitted[0].payload.envelope.operation.kind(),
        FederationOperationKind::MintQuotePayment
    );
}

#[tokio::test]
async fn rest_peer_client_rejects_submit_response_with_mismatched_outcome_operation_id() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config.member(member_id(2)).expect("peer member");
    let envelope = payment_observation_envelope(federation_id);
    let request = FederationSubmitOperationRequest::new(envelope);
    let expected_operation_id = request.operation_id().expect("expected operation id");
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
    let other_envelope = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut mismatched_response = peer_consensus
        .submit_operation(FederationSubmitOperationRequest::new(other_envelope))
        .expect("peer submit response");
    let found_operation_id = mismatched_response.outcome.entry().operation_id;
    assert_ne!(expected_operation_id, found_operation_id);
    mismatched_response.operation_id = expected_operation_id;
    let transport =
        MockFederationTransport::default().with_submit_response(mismatched_response.clone());
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    let err = client
        .submit_operation(member, &request)
        .await
        .expect_err("mismatched embedded outcome should be rejected");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::SubmitOperationResponseMismatch { expected, found })
            if *expected == expected_operation_id && *found == found_operation_id
    ));
    let submitted = transport.submitted_operations();
    assert_eq!(submitted.len(), 1);
    assert_eq!(
        submitted[0].payload.operation_id().expect("operation id"),
        expected_operation_id
    );
}

#[tokio::test]
async fn non_primary_submit_forwards_to_primary_and_catches_up() {
    let (public_config, primary_private, local_private) = transport_config();
    let federation_id = public_config.federation_id;
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let primary_member = public_config.member(member_id(1)).expect("primary member");
    let blinded_secret = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"non primary forwarded restore",
    ));
    let envelope = restore_envelope_with_output(federation_id, blinded_secret);
    let operation_id = envelope.operation_id().expect("operation id");

    let mut primary_consensus =
        InMemoryFederationConsensus::new(federation_id, primary_member.member_id);
    let submit_response = primary_consensus
        .submit_operation(FederationSubmitOperationRequest::new(envelope.clone()))
        .expect("primary submit response");
    let local_catch_up_request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let mut catch_up_response = primary_consensus
        .catch_up_checked(&local_catch_up_request)
        .expect("primary catch-up response");
    let state_digest =
        FederationMaterializedStateSnapshot::from_rows([FederationMaterializedStateRow::new(
            "blind_signatures",
            blinded_secret.to_hex(),
        )
        .with_field("missing", "true")])
        .digest()
        .expect("state digest");
    catch_up_response.state_digest = state_digest;
    let catch_up_response = signed_catch_up_response(catch_up_response, &primary_private);
    let primary_status = FederationPeerStatus::new_with_state_digest(
        federation_id,
        primary_member.member_id,
        primary_consensus.peer_status().operation_count,
        primary_consensus.peer_status().latest_index,
        primary_consensus.peer_status().latest_order_digest,
        state_digest,
    );
    let transport = MockFederationTransport::default()
        .with_submit_response(submit_response.clone())
        .with_status_response(primary_status)
        .with_catch_up_response(catch_up_response);

    let response = submit_operation_to_primary_if_needed_with_transport(
        &state,
        envelope,
        None,
        transport.clone(),
    )
    .await
    .expect("forward to primary")
    .expect("non-primary should forward");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(response.operation_id, submit_response.operation_id);
    let submitted = transport.submitted_operations();
    assert_eq!(submitted.len(), 1);
    submitted[0]
        .verify(&public_config, primary_member.member_id)
        .expect("signed primary submit");
    assert_eq!(submitted[0].sender, local_private.member_id);
    assert_eq!(submitted[0].receiver, primary_member.member_id);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("persisted journal");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].operation_id, operation_id);
}

#[tokio::test(start_paused = true)]
async fn non_primary_submit_uses_configured_operation_submission_timeout() {
    let (public_config, _, local_private) = transport_config();
    let federation_id = public_config.federation_id;
    let mut state = federation_route_state_for_private(public_config.clone(), local_private).await;
    let timeout = Duration::from_secs(2);
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: timeout,
        ..FederationPeerSyncConfig::default()
    });
    let envelope = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let task = tokio::spawn(async move {
        submit_operation_to_primary_if_needed_with_transport(
            &state,
            envelope,
            None,
            DelayTransport {
                delay: timeout + Duration::from_secs(60),
            },
        )
        .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(timeout).await;
    let err = task
        .await
        .expect("forwarding task")
        .expect_err("forwarding should use configured timeout");

    assert!(
        err.to_string().contains("operation submission timed out"),
        "unexpected forwarding timeout error: {err}"
    );
}

#[tokio::test]
async fn rest_peer_client_rejects_oversized_operation_before_broadcast() {
    let (public_config, local_private, _) = transport_config();
    let member = public_config.member(member_id(2)).expect("peer member");
    let request = FederationSubmitOperationRequest::new(oversized_payment_observation_envelope(
        public_config.federation_id,
    ));
    let transport = MockFederationTransport::default();
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    let err = client
        .submit_operation(member, &request)
        .await
        .expect_err("oversized operation should not be broadcast");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::OperationEnvelopeTooLarge { .. })
    ));
    assert!(transport.submitted_operations().is_empty());
}

#[tokio::test]
async fn rest_peer_client_catches_up_local_consensus_from_member() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let witness_member = public_config.member(member_id(3)).expect("witness member");
    let first = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    peer_consensus
        .submit_envelope(first)
        .expect("peer first operation");
    peer_consensus
        .submit_envelope(second)
        .expect("peer second operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("peer catch-up response");
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private))
        .with_catch_up_response(witness_response);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            catch_up_limit: 7,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let applied = client
        .catch_up_state_from_member(&state, peer_member)
        .await
        .expect("catch up");

    assert_eq!(applied.1.len(), 1);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .peer_status()
            .latest_order_digest,
        peer_consensus.peer_status().latest_order_digest
    );
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("persisted journal");
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[1].operation_id, applied.1[0].operation_id);
    let persisted_outcomes = state
        .mint
        .localstore()
        .get_federation_session_outcomes()
        .await
        .expect("persisted session outcomes");
    assert_eq!(persisted_outcomes.len(), 2);
    assert_eq!(persisted_outcomes[0].from_index, 1);
    assert_eq!(persisted_outcomes[1].from_index, 1);
    let persisted_signers = persisted_outcomes
        .iter()
        .map(|outcome| outcome.signer)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        persisted_signers,
        BTreeSet::from([peer_member.member_id, witness_member.member_id])
    );
    let requests = transport.catch_up_requests();
    assert_eq!(requests.len(), 2);
    requests[0]
        .verify(&public_config, peer_member.member_id)
        .expect("signed catch-up request");
    assert_eq!(requests[0].payload.next_index, 1);
    assert_eq!(requests[0].payload.limit, Some(7));
    requests[1]
        .verify(&public_config, witness_member.member_id)
        .expect("signed witness catch-up request");
    assert_eq!(requests[1].payload.next_index, 1);
    assert_eq!(requests[1].payload.limit, Some(1));
}

#[tokio::test]
async fn rest_peer_client_tolerates_already_applied_catch_up_overlap() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    peer_consensus
        .submit_envelope(first)
        .expect("peer first operation");
    peer_consensus
        .submit_envelope(second.clone())
        .expect("peer second operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("peer catch-up response");

    state
        .submit_federation_envelope(second)
        .await
        .expect("background sync applied second operation first");
    let transport = MockFederationTransport::default()
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    let applied = client
        .catch_up_state_from_member(&state, peer_member)
        .await
        .expect("stale catch-up response is idempotent");

    assert!(applied.1.is_empty());
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        2
    );
    let persisted_outcomes = state
        .mint
        .localstore()
        .get_federation_session_outcomes()
        .await
        .expect("persisted session outcomes");
    assert_eq!(persisted_outcomes.len(), 1);
    assert_eq!(persisted_outcomes[0].from_index, 1);
    assert_eq!(persisted_outcomes[0].signer, peer_member.member_id);
    assert_eq!(transport.catch_up_requests()[0].payload.next_index, 2);
}

#[tokio::test]
async fn rest_peer_client_rejects_unsigned_nonempty_catch_up_response() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    peer_consensus
        .submit_envelope(first)
        .expect("peer first operation");
    peer_consensus
        .submit_envelope(second)
        .expect("peer second operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("peer catch-up response");
    assert!(catch_up_response.session_outcome.is_none());
    let transport = MockFederationTransport::default().with_catch_up_response(catch_up_response);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let err = client
        .catch_up_state_from_member(&state, peer_member)
        .await
        .expect_err("unsigned response must fail");

    let err = err.to_string();
    assert!(
        err.contains(
            "Federation session outcome mismatch: non-empty catch-up response missing outcome"
        ),
        "unexpected unsigned catch-up error: {err}"
    );
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        1
    );
}

#[tokio::test]
async fn lagging_member_refuses_public_federation_submit_until_healthy() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 1)
        .await;
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let blocked = state.submit_federation_envelope(envelope.clone()).await;

    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Lagging { .. })
    ));
    let status_request = signed_status_request(&public_config, &peer_private, member_id(1), 1);
    let status = post_local_status(State(state.clone()), Json(status_request))
        .await
        .expect("lagging local status")
        .0;
    assert_eq!(status.sync_state, "lagging");
    assert_eq!(status.sync_peer, Some(peer_member.member_id));
    assert_eq!(status.sync_local_operation_count, Some(0));
    assert_eq!(status.sync_peer_operation_count, Some(1));
    assert_eq!(status.sync_operation_count, None);

    state.mark_federation_healthy().await;
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("healthy member can submit");
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test]
async fn divergent_non_primary_refuses_public_submit_before_forwarding() {
    let (public_config, _, secondary_private) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), secondary_private).await;
    let primary_member = public_config.member(member_id(1)).expect("primary member");
    state
        .mark_federation_materialized_state_mismatch(primary_member.member_id, 1)
        .await;
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("lagging non-primary member must not forward public submits");

    assert!(
        err.to_string()
            .contains("materialized state digest convergence"),
        "expected materialized-state readiness error before primary forwarding, got: {err}"
    );
}

#[tokio::test]
async fn rest_peer_client_sync_requires_quorum_evidence_before_public_submit() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(peer_entry)
        .expect("peer accepted operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
            .await;
    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private))
        .with_catch_up_response(witness_response);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let synced_status = client
        .sync_once_from_member(&state, peer_member)
        .await
        .expect("peer sync");

    assert_eq!(synced_status.operation_count, 1);
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::VerifyingSessionOutcome {
            operation_count: 1,
            ..
        })
    ));

    let next = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let blocked = state.submit_federation_envelope(next).await;
    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
}

#[tokio::test]
async fn peer_sync_round_catches_up_from_configured_peer() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(peer_entry)
        .expect("peer accepted operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
            .await;
    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private))
        .with_catch_up_response(witness_response);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        1
    );
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));

    let next = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    state
        .submit_federation_envelope(next)
        .await
        .expect("quorum-verified member can submit");
}

#[tokio::test(start_paused = true)]
async fn peer_sync_round_backs_off_failed_peer_until_retry_delay() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let transport = MockFederationTransport::default();
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(8),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let mut backoff = FederationPeerReconnectBackoff::default();

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 1);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "retry before reconnect backoff expires must be skipped"
    );

    tokio::time::advance(Duration::from_secs(2) - Duration::from_millis(1)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "retry just before reconnect backoff expiry must still be skipped"
    );

    tokio::time::advance(Duration::from_millis(1)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 2);

    let status = FederationPeerStatus::new_with_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationStateDigest::genesis(),
    );
    let _transport = transport.clone().with_status_response(status);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        2,
        "second failed attempt should double the reconnect backoff"
    );

    tokio::time::advance(Duration::from_secs(4)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 1);
    assert_eq!(transport.status_requests().len(), 3);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 1);
    assert_eq!(
        transport.status_requests().len(),
        4,
        "successful peer sync must clear reconnect backoff"
    );
}

#[tokio::test(start_paused = true)]
async fn peer_sync_round_skips_evidence_for_failed_or_backing_off_peer() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    state
        .submit_federation_envelope(entry)
        .await
        .expect("local accepted operation");
    let transport = MockFederationTransport::default();
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(8),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let mut backoff = FederationPeerReconnectBackoff::default();

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 1);
    assert_eq!(
        transport.catch_up_requests().len(),
        0,
        "failed status sync must not immediately retry the peer through evidence collection"
    );

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "backing-off peer should not receive another status request"
    );
    assert_eq!(
        transport.catch_up_requests().len(),
        0,
        "backing-off peer should not receive evidence catch-up requests"
    );
}

#[tokio::test(start_paused = true)]
async fn peer_sync_round_stops_evidence_after_first_peer_failure() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    let operation_count = 3;
    for index in 0..operation_count {
        let entry = indexed_restore_envelope(federation_id, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        peer_consensus
            .submit_envelope(entry)
            .expect("peer accepted operation");
    }
    state
        .mark_federation_verifying_session_outcome(peer_member.member_id, operation_count)
        .await;

    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default().with_status_response(peer_status);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig {
            session_outcome_evidence_chunk_size: 1,
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(8),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let mut backoff = FederationPeerReconnectBackoff::default();

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 1);
    assert_eq!(transport.status_requests().len(), 1);
    assert_eq!(
        transport.catch_up_requests().len(),
        1,
        "evidence collection must stop after the first failed peer request"
    );

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "evidence failure should put the peer into reconnect backoff"
    );
    assert_eq!(
        transport.catch_up_requests().len(),
        1,
        "backing-off peer should not receive more evidence requests"
    );
}

#[tokio::test]
async fn peer_sync_round_collects_session_outcome_evidence_from_equal_peer() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    state
        .submit_federation_envelope(entry.clone())
        .await
        .expect("local accepted operation");
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(entry)
        .expect("peer accepted operation");

    assert!(session_outcome_quorum_status(&state)
        .await
        .expect("quorum status")
        .is_none());

    let catch_up_request = FederationJournalCatchUpRequest::new(0)
        .with_limit(1)
        .with_known_order_digest(FederationJournalDigest::genesis());
    let catch_up_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
            .await;
    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    assert_eq!(transport.catch_up_requests().len(), 1);
    let quorum = session_outcome_quorum_status(&state)
        .await
        .expect("quorum status")
        .expect("quorum evidence");
    assert_eq!(quorum.operation_count, 1);
    assert_eq!(quorum.signers.len(), 2);
    assert!(quorum.signers.contains(&local_private.member_id));
    assert!(quorum.signers.contains(&peer_private.member_id));
}

#[tokio::test]
async fn peer_sync_round_collects_configured_chunked_session_outcome_evidence() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    let chunk_size = 2;
    let operation_count = chunk_size + 1;
    for index in 0..operation_count as u64 {
        let entry = indexed_restore_envelope(federation_id, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        peer_consensus
            .submit_envelope(entry)
            .expect("peer accepted operation");
    }
    state
        .mark_federation_verifying_session_outcome(peer_member.member_id, operation_count as u64)
        .await;

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), operation_count);
    let first_request = FederationJournalCatchUpRequest::new(0)
        .with_limit(chunk_size as u16)
        .with_known_order_digest(FederationJournalDigest::genesis());
    let second_request = FederationJournalCatchUpRequest::new(chunk_size as u64)
        .with_limit(1)
        .with_known_order_digest(entries[chunk_size].previous_order_digest);
    let first_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &first_request).await;
    let second_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &second_request).await;
    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(signed_catch_up_response(first_response, &peer_private))
        .with_catch_up_response(signed_catch_up_response(second_response, &peer_private));
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig {
            session_outcome_evidence_chunk_size: chunk_size,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    let requests = transport.catch_up_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].payload.next_index, 0);
    assert_eq!(requests[0].payload.limit, Some(chunk_size as u16));
    assert_eq!(requests[1].payload.next_index, chunk_size as u64);
    assert_eq!(requests[1].payload.limit, Some(1));
    let quorum = session_outcome_quorum_status(&state)
        .await
        .expect("quorum status")
        .expect("chunked quorum evidence");
    assert_eq!(quorum.from_index, 0);
    assert_eq!(quorum.operation_count, operation_count as u64);
    assert_eq!(quorum.latest_index, operation_count as u64 - 1);
    assert!(quorum.signers.contains(&local_private.member_id));
    assert!(quorum.signers.contains(&peer_private.member_id));
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test]
async fn peer_sync_round_collects_only_unverified_session_outcome_suffix() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    let chunk_size = 2;
    let operation_count = chunk_size + 1;
    for index in 0..operation_count as u64 {
        let entry = indexed_restore_envelope(federation_id, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        peer_consensus
            .submit_envelope(entry)
            .expect("peer accepted operation");
    }

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let local_status = local_peer_status(&state).await.expect("local status");
    let runtime_config = state.mint.federation_config().expect("federation config");
    let local_chunks = persist_local_session_outcome_evidence_chunks(
        &state,
        runtime_config,
        &entries[..chunk_size],
        local_status.latest_order_digest,
        local_status.state_digest,
        chunk_size,
    )
    .await;
    assert_eq!(local_chunks, 1);

    let first_request = FederationJournalCatchUpRequest::new(0)
        .with_limit(chunk_size as u16)
        .with_known_order_digest(FederationJournalDigest::genesis());
    let first_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &first_request).await;
    let first_response = signed_catch_up_response(first_response, &peer_private);
    state
        .persist_federation_session_outcome(
            first_response
                .session_outcome
                .as_ref()
                .expect("first peer session outcome"),
        )
        .await
        .expect("persist first peer session outcome");

    let prefix = session_outcome_quorum_status(&state)
        .await
        .expect("prefix quorum status")
        .expect("prefix quorum evidence");
    assert_eq!(prefix.from_index, 0);
    assert_eq!(prefix.operation_count, chunk_size as u64);

    let second_request = FederationJournalCatchUpRequest::new(chunk_size as u64)
        .with_limit(1)
        .with_known_order_digest(entries[chunk_size].previous_order_digest);
    let second_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &second_request).await;
    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(signed_catch_up_response(second_response, &peer_private));
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            session_outcome_evidence_chunk_size: chunk_size,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    let requests = transport.catch_up_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].payload.next_index, chunk_size as u64);
    assert_eq!(requests[0].payload.limit, Some(1));
    let quorum = session_outcome_quorum_status(&state)
        .await
        .expect("quorum status")
        .expect("complete quorum evidence");
    assert_eq!(quorum.from_index, 0);
    assert_eq!(quorum.operation_count, operation_count as u64);
}

#[tokio::test]
async fn peer_sync_round_does_not_clear_higher_known_lag_with_lower_peer() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 2)
        .await;
    let status = FederationPeerStatus::new_with_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationStateDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Lagging {
            peer_operation_count: 2,
            ..
        })
    ));
}

#[tokio::test]
async fn rest_peer_client_halts_on_equal_height_digest_mismatch() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = restore_envelope(
        federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(peer_entry)
        .expect("peer accepted operation");
    state
        .submit_federation_envelope(local_entry)
        .await
        .expect("local accepted operation");
    let transport =
        MockFederationTransport::default().with_status_response(peer_consensus.peer_status());
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    assert!(sync.is_err());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { .. })
    ));

    let blocked = state
        .submit_federation_envelope(restore_envelope(
            federation_id,
            "03b45d181a961a91a284e8c64eb72839d43300afaa4e6b3076f5183bb14f50c2cc",
        ))
        .await;
    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
}

#[tokio::test]
async fn rest_peer_client_halts_on_consensus_backend_mismatch() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut status = FederationPeerStatus::new_with_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationStateDigest::genesis(),
    );
    status.consensus_backend = cdk::federation::FederationConsensusBackendKind::AlephBft;
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    assert!(sync
        .expect_err("backend mismatch should fail")
        .to_string()
        .contains("consensus backend mismatch"));
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { .. })
    ));

    let blocked = state
        .submit_federation_envelope(restore_envelope(
            federation_id,
            "03b45d181a961a91a284e8c64eb72839d43300afaa4e6b3076f5183bb14f50c2cc",
        ))
        .await;
    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
}

#[tokio::test]
async fn rest_peer_client_uses_status_as_cheap_order_status() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationStateDigest::from_bytes([9; 32]),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    assert!(sync.is_ok());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test(start_paused = true)]
async fn rest_peer_client_detects_catch_up_materialized_state_mismatch() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = restore_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(peer_entry)
        .expect("peer accepted operation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let mut catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("peer catch-up response");
    catch_up_response.state_digest = cdk::federation::FederationStateDigest::from_bytes([9; 32]);
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_status_response(peer_consensus.peer_status())
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private))
        .with_catch_up_response(witness_response);
    let digest_recheck_delay = Duration::from_secs(2);
    let client = FederationRestPeerClient::new_with_config(
        transport,
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            digest_recheck_attempts: 2,
            digest_recheck_delay,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let state_for_sync = state.clone();
    let peer_member_for_sync = peer_member.clone();
    let task = tokio::spawn(async move {
        client
            .sync_once_from_member(&state_for_sync, &peer_member_for_sync)
            .await
    });
    tokio::task::yield_now().await;

    assert!(
        !task.is_finished(),
        "digest mismatch should wait for the configured recheck delay before halting"
    );
    tokio::time::advance(digest_recheck_delay).await;
    let sync = task.await.expect("digest recheck task");

    assert!(sync.is_err());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::MaterializedStateMismatch { .. })
    ));
}

#[tokio::test]
async fn catch_up_applies_mint_quote_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted quote");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_payment_quarantine_and_local_status_reports_it() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let local_member = config.member_id();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let first_observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let first_operation_id = first_observation
        .operation_id()
        .expect("first operation id");
    let conflicting_observation = mint_payment_observation_envelope_for_member(
        &state,
        &quote_id,
        2,
        2,
        "different-settlement",
    );
    let conflicting_operation_id = conflicting_observation
        .operation_id()
        .expect("conflicting operation id");
    let quarantine = config.payment_observation_quarantine(
        cdk::federation::FederationPaymentObservationContext::MintQuote,
        quote_id.clone(),
        FederationPaymentObservationQuarantineReason::ConflictingEvidence,
        vec![first_operation_id, conflicting_operation_id],
    );
    let quarantine_operation_id = quarantine.operation_id().expect("quarantine operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    for envelope in [
        mint_quote_envelope(&state, &quote_id),
        first_observation,
        conflicting_observation,
        quarantine,
    ] {
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted operation");
    }
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    assert_eq!(applied.len(), 4);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 4);
    assert!(persisted
        .iter()
        .all(|entry| entry.status == FederationOperationStatus::Applied));

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.payment_observation_reports.len(), 1);
    let report = &response.payment_observation_reports[0];
    assert_eq!(
        report.context,
        cdk::federation::FederationPaymentObservationContext::MintQuote
    );
    assert_eq!(report.quote, quote_id.to_string());
    assert_eq!(report.total_observations, 2);
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::Conflicting
    );
    assert!(report.quarantined);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Quarantine
    );
    assert_eq!(
        report.quarantine_operation_ids,
        vec![quarantine_operation_id]
    );
    assert_eq!(report.quarantine_operation_indices, vec![3]);
    assert_eq!(
        report.quarantine_reasons,
        vec![FederationPaymentObservationQuarantineReason::ConflictingEvidence]
    );

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
    assert!(!status_json.contains("different-settlement"));
}

#[tokio::test]
async fn catch_up_applies_melt_quote_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let invoice = create_fake_invoice(1000, "".to_string());
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice.clone(),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let envelope = config.melt_quote_operation(quote_id.clone(), request);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted melt quote");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.id, quote_id);
    assert_eq!(
        quote.payment_method,
        PaymentMethod::Known(KnownMethod::Bolt11)
    );
    assert_eq!(quote.request.to_string(), invoice.to_string());
    assert_eq!(quote.state, MeltQuoteState::Unpaid);
    assert_eq!(quote.amount(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.fee_reserve(), Amount::new(1, CurrencyUnit::Sat));

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_swap_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let input = proof_for_amount(&state, 1, "catch-up");
    let input_y = input.y().expect("input y");
    let output = swap_output(&state, "catch-up");
    let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
    let envelope = config
        .swap_operation(request.clone())
        .expect("swap envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted swap");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&[input_y])
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    let expected_response = config
        .swap_share_response(operation_id, &request)
        .expect("expected share response");
    assert_eq!(
        signatures,
        vec![Some(expected_response.signatures[0].clone())]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_melt_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = melt_quote(&state, 1000).await;
    let inputs = vec![
        proof_for_amount(&state, 1, "melt-catch-up-a"),
        proof_for_amount(&state, 1, "melt-catch-up-b"),
        proof_for_amount(&state, 1, "melt-catch-up-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "melt-catch-up-change");
    change_output.amount = Amount::ZERO;
    let request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let envelope = config.melt_operation(request).expect("melt envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted melt");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    let signature = signatures[0].clone().expect("change share persisted");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, change_output.keyset_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_paid_melt_payment_observation_to_pending_melt() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "paid-observation-a"),
        proof_for_amount(&state, 1, "paid-observation-b"),
        proof_for_amount(&state, 1, "paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("federation-paid-preimage".to_string()),
    );
    let paid_operation_id = paid_envelope.operation_id().expect("paid operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");
    peer_consensus
        .submit_envelope(paid_envelope)
        .expect("peer accepted paid observation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);
    assert_eq!(
        quote.payment_proof.as_deref(),
        Some("federation-paid-preimage")
    );

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    let signature = signatures[0].clone().expect("change share persisted");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, change_output.keyset_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 2);
    assert!(persisted
        .iter()
        .all(|entry| entry.status == FederationOperationStatus::Applied));
    let runtime = local_consensus.lock().await;
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(melt_operation_id)
            .expect("melt runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(paid_operation_id)
            .expect("paid runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_resolves_pending_melt_after_later_paid_observation() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "late-paid-observation-a"),
        proof_for_amount(&state, 1, "late-paid-observation-b"),
        proof_for_amount(&state, 1, "late-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "late-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("late-paid-observation-preimage".to_string()),
    );
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let pending_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    let pending_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("pending proof states");
    assert_eq!(
        pending_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("pending change signature lookup")[0]
        .is_none());

    peer_consensus
        .submit_envelope(paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("paid catch-up response");
    state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up");

    let paid_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote status");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote.change().expect("paid quote change");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);

    let spent_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("spent proof states");
    assert_eq!(
        spent_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "paid observation catch-up should clear pending melt saga metadata"
    );
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        2
    );
}

#[tokio::test]
async fn catch_up_rolls_back_pending_melt_after_later_failed_observation() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "late-failed-observation-a"),
        proof_for_amount(&state, 1, "late-failed-observation-b"),
        proof_for_amount(&state, 1, "late-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "late-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let pending_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    let pending_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("pending proof states");
    assert_eq!(
        pending_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );

    peer_consensus
        .submit_envelope(failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("failed catch-up response");
    state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up");

    let failed_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("failed quote status");
    assert_eq!(failed_quote.state(), MeltQuoteState::Unpaid);
    assert!(failed_quote.payment_proof().is_none());
    assert!(failed_quote.change().is_none());
    let released_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("released proof states");
    assert_eq!(released_states, vec![None, None, None]);
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup")[0]
        .is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "failed observation catch-up should clear pending melt saga metadata"
    );
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        2
    );
}

#[tokio::test]
async fn restart_replay_preserves_pending_melt_then_catches_up_paid_observation() {
    let (mut state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "restart-paid-observation-a"),
        proof_for_amount(&state, 1, "restart-paid-observation-b"),
        proof_for_amount(&state, 1, "restart-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "restart-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("restart-paid-observation-preimage".to_string()),
    );
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let pending_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status before replay");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before replay")
        .is_some());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state");
    state.federation_consensus = replayed_consensus;
    state.federation_sequences = replayed_sequences;
    assert_eq!(
        state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        1
    );
    let replayed_pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status after replay");
    assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
    assert!(replayed_pending_quote.change().is_none());

    peer_consensus
        .submit_envelope(paid_envelope)
        .expect("peer accepted paid observation");
    let replayed_consensus = state
        .federation_consensus
        .as_ref()
        .expect("replayed consensus");
    let paid_response = peer_consensus
        .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
        .expect("paid catch-up response after replay");
    state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up after replay");

    let paid_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote status after replay");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote.change().expect("paid quote change after replay");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after replay");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after paid catch-up")
            .is_none(),
        "paid observation after replay should clear pending melt saga metadata"
    );
    assert_eq!(
        state
            .federation_consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
}

#[tokio::test]
async fn restart_replay_preserves_pending_melt_then_catches_up_failed_observation() {
    let (mut state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "restart-failed-observation-a"),
        proof_for_amount(&state, 1, "restart-failed-observation-b"),
        proof_for_amount(&state, 1, "restart-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "restart-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let pending_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status before replay");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before replay")
        .is_some());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state");
    state.federation_consensus = replayed_consensus;
    state.federation_sequences = replayed_sequences;
    assert_eq!(
        state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        1
    );
    let replayed_pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status after replay");
    assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
    assert!(replayed_pending_quote.change().is_none());

    peer_consensus
        .submit_envelope(failed_envelope)
        .expect("peer accepted failed observation");
    let replayed_consensus = state
        .federation_consensus
        .as_ref()
        .expect("replayed consensus");
    let failed_response = peer_consensus
        .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
        .expect("failed catch-up response after replay");
    state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up after replay");

    let rolled_back_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("rolled back quote status after replay");
    assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
    assert!(rolled_back_quote.payment_proof().is_none());
    assert!(rolled_back_quote.change().is_none());
    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after rollback");
    assert_eq!(proof_states, vec![None, None, None]);
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup after replay")[0]
        .is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after failed catch-up")
            .is_none(),
        "failed observation after replay should clear pending melt saga metadata"
    );
    assert_eq!(
        state
            .federation_consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
}

#[tokio::test]
async fn sqlite_restart_replays_pending_melt_then_catches_up_paid_observation() {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-restart");
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        db,
        &seed,
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-a"),
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-b"),
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "sqlite-restart-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("sqlite-restart-paid-observation-preimage".to_string()),
    );
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response")
    };
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");
    assert_eq!(
        state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote before restart")
            .state(),
        MeltQuoteState::Pending
    );
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before restart")
        .is_some());
    drop(state);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
    restarted_state.federation_consensus = replayed_consensus;
    restarted_state.federation_sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        1
    );

    let restarted_pending_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote after sqlite restart");
    assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
    assert!(restarted_pending_quote.change().is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup after sqlite restart")
        .is_some());

    peer_consensus
        .submit_envelope(paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = {
        let replayed_consensus = restarted_state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus");
        peer_consensus
            .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
            .expect("paid catch-up response after sqlite restart")
    };
    restarted_state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up after sqlite restart");

    let paid_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote after sqlite restart");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote
        .change()
        .expect("paid quote change after restart");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);
    let proof_states = restarted_state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after sqlite restart");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after paid catch-up")
            .is_none(),
        "paid observation after sqlite restart should clear pending melt saga metadata"
    );
    assert_eq!(
        restarted_state
            .federation_consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
}

#[tokio::test]
async fn sqlite_restart_replays_pending_melt_then_catches_up_failed_observation() {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-failed-restart");
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        db,
        &seed,
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-a"),
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-b"),
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "sqlite-restart-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response")
    };
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");
    assert_eq!(
        state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote before restart")
            .state(),
        MeltQuoteState::Pending
    );
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before restart")
        .is_some());
    drop(state);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
    restarted_state.federation_consensus = replayed_consensus;
    restarted_state.federation_sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        1
    );

    let restarted_pending_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote after sqlite restart");
    assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
    assert!(restarted_pending_quote.change().is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup after sqlite restart")
        .is_some());

    peer_consensus
        .submit_envelope(failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = {
        let replayed_consensus = restarted_state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus");
        peer_consensus
            .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
            .expect("failed catch-up response after sqlite restart")
    };
    restarted_state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up after sqlite restart");

    let rolled_back_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("rolled back quote after sqlite restart");
    assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
    assert!(rolled_back_quote.payment_proof().is_none());
    assert!(rolled_back_quote.change().is_none());
    let proof_states = restarted_state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after sqlite restart");
    assert_eq!(proof_states, vec![None, None, None]);
    assert!(restarted_state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup after sqlite restart")[0]
        .is_none());
    assert!(
        restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after failed catch-up")
            .is_none(),
        "failed observation after sqlite restart should clear pending melt saga metadata"
    );
    assert_eq!(
        restarted_state
            .federation_consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
}

#[tokio::test]
async fn catch_up_applies_unpaid_melt_payment_observation_to_pending_melt() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "unpaid-observation-a"),
        proof_for_amount(&state, 1, "unpaid-observation-b"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let melt_request = cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, None);
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let unpaid_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Unpaid, None);
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(melt_envelope)
        .expect("peer accepted pending melt");
    peer_consensus
        .submit_envelope(unpaid_envelope)
        .expect("peer accepted unpaid observation");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Unpaid);
    assert!(quote.payment_proof.is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "rollback should remove pending melt saga metadata"
    );

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![None, None]);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 2);
    assert!(persisted
        .iter()
        .all(|entry| entry.status == FederationOperationStatus::Applied));
}

#[tokio::test]
async fn offline_member_catches_up_multiple_operations_and_converges() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);

    let mint_quote_id = QuoteId::new();
    let minted_output = mint_output(&state, &mint_quote_id);
    let mint_request = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![minted_output.clone()],
        signature: None,
    });

    let swap_input = proof_for_amount(&state, 1, "offline-catch-up-swap-input");
    let swap_input_y = swap_input.y().expect("swap input y");
    let swapped_output = swap_output(&state, "offline-catch-up-swap-output");
    let swap_request = cdk::nuts::SwapRequest::new(vec![swap_input], vec![swapped_output.clone()]);

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    );
    let melt_quote_envelope = config.melt_quote_operation(
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: melt_invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_inputs = vec![
        proof_for_amount(&state, 1, "offline-catch-up-melt-a"),
        proof_for_amount(&state, 1, "offline-catch-up-melt-b"),
        proof_for_amount(&state, 1, "offline-catch-up-melt-c"),
    ];
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("melt input y"))
        .collect::<Vec<_>>();
    let mut melt_change_output = swap_output(&state, "offline-catch-up-melt-change");
    melt_change_output.amount = Amount::ZERO;
    let melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs,
        Some(vec![melt_change_output.clone()]),
    );
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let melt_payment_envelope = melt_payment_observation_envelope(
        &state,
        &melt_quote_id,
        MeltQuoteState::Paid,
        Some("offline-catch-up-preimage".to_string()),
    );

    for envelope in [
        mint_quote_envelope(&state, &mint_quote_id),
        paid_observation_envelope(&state, &mint_quote_id),
        config
            .mint_operation(mint_request.clone())
            .expect("mint envelope"),
        config
            .swap_operation(swap_request.clone())
            .expect("swap envelope"),
        melt_quote_envelope,
        melt_envelope,
        melt_payment_envelope,
    ] {
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted operation");
    }

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        0
    );
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("peer catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply offline catch-up");

    assert_eq!(applied.len(), 7);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .peer_status()
            .latest_order_digest,
        peer_consensus.peer_status().latest_order_digest
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 7);
    assert!(persisted
        .iter()
        .all(|entry| entry.status == FederationOperationStatus::Applied));

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&mint_quote_id)
        .await
        .expect("mint quote lookup")
        .expect("mint quote persisted");
    assert_eq!(quote.state(), MintQuoteState::Issued);

    let mint_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[minted_output.blinded_secret])
        .await
        .expect("mint signature lookup");
    assert!(mint_signatures[0].is_some());

    let swap_states = state
        .mint
        .localstore()
        .get_proofs_states(&[swap_input_y])
        .await
        .expect("swap state lookup");
    assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
    let swap_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[swapped_output.blinded_secret])
        .await
        .expect("swap signature lookup");
    assert!(swap_signatures[0].is_some());

    let melt_quote = state
        .mint
        .localstore()
        .get_melt_quote(&melt_quote_id)
        .await
        .expect("melt quote lookup")
        .expect("melt quote persisted");
    assert_eq!(melt_quote.state, MeltQuoteState::Paid);
    assert_eq!(
        melt_quote.payment_proof.as_deref(),
        Some("offline-catch-up-preimage")
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&melt_quote_id)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "paid observation replay should complete pending melt saga metadata"
    );

    let melt_states = state
        .mint
        .localstore()
        .get_proofs_states(&melt_input_ys)
        .await
        .expect("melt state lookup");
    assert_eq!(
        melt_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    let melt_change_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[melt_change_output.blinded_secret])
        .await
        .expect("melt change signature lookup");
    assert!(melt_change_signatures[0].is_some());

    let conflicting_output = swap_output(&state, "offline-catch-up-conflicting-mint");
    let conflicting_mint = config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id,
            outputs: vec![conflicting_output],
            signature: None,
        }))
        .expect("conflicting mint envelope");
    let conflict = state.submit_federation_envelope(conflicting_mint).await;
    assert!(matches!(
        conflict,
        Err(cdk::Error::Federation(
            FederationError::ConflictingOperation { .. }
        ))
    ));
}

#[tokio::test]
async fn diverged_member_rejects_peer_catch_up_digest_mismatch() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = restore_envelope(
        public_config.federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(peer_entry)
        .expect("peer accepted operation");
    state
        .submit_federation_envelope(local_entry)
        .await
        .expect("local accepted divergent operation");

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let request = local_consensus.lock().await.catch_up_request();
    assert_eq!(request.next_index, 1);
    assert!(matches!(
        peer_consensus.catch_up_checked(&request),
        Err(FederationError::CatchUpDigestMismatch { .. })
    ));

    let unchecked_response = peer_consensus.catch_up(&request);
    let apply = state
        .apply_federation_catch_up_response(&unchecked_response)
        .await;
    assert!(matches!(
        apply,
        Err(cdk::Error::Federation(
            FederationError::CatchUpDigestMismatch { .. }
        ))
    ));
}

#[tokio::test]
async fn federation_runtime_startup_persists_public_config_snapshot() {
    let (state, public_config, _) = federation_route_state().await;

    crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("runtime state");

    let snapshot = state
        .mint
        .localstore()
        .get_federation_config(public_config.federation_id)
        .await
        .expect("load federation config")
        .expect("stored federation config");
    assert_eq!(snapshot.config, public_config);
    assert_eq!(
        snapshot.config_digest,
        snapshot.config.config_digest().expect("config digest")
    );
}

#[tokio::test]
async fn federation_runtime_startup_rejects_conflicting_public_config_snapshot() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));

    let mut conflicting_config = public_config.clone();
    conflicting_config
        .payment_observation_policy
        .max_pending_quotes += 1;
    conflicting_config
        .validate()
        .expect("conflicting config validates");
    let snapshot =
        FederationConfigSnapshot::new(conflicting_config).expect("conflicting config snapshot");
    assert_eq!(snapshot.federation_id, public_config.federation_id);
    assert_ne!(
        snapshot.config_digest,
        public_config.config_digest().expect("public config digest")
    );

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin conflicting config transaction");
    tx.set_federation_config(&snapshot)
        .await
        .expect("persist conflicting config");
    tx.commit().await.expect("commit conflicting config");

    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let err = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect_err("conflicting config rejected");
    assert!(
        err.to_string().contains("Conflicting federation config"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn federation_runtime_state_replays_persisted_journal_and_sequences() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let response = state
        .submit_federation_envelope(envelope)
        .await
        .expect("persist operation");
    state
        .persist_federation_sequence(peer_member, 42)
        .await
        .expect("persist sequence");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("runtime state");
    let consensus = consensus.expect("consensus");
    let sequences = sequences.expect("sequences");
    let consensus = consensus.lock().await;
    let sequences = sequences.lock().await;

    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .first()
            .expect("journal entry")
            .operation_id,
        response.operation_id
    );
    assert_eq!(sequences.sequences().get(&peer_member), Some(&42));
}

#[tokio::test]
async fn federation_runtime_startup_reloads_aleph_bft_finalized_items_fail_closed() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    adapter.finalize_unit(unit).expect("finalize unit");
    let entry = adapter.journal().entries()[0].clone();
    let finalized_items = adapter.finalized_items().to_vec();

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT startup transaction");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist matching journal entry");
    tx.commit().await.expect("commit AlephBFT startup state");

    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before startup replay")
        .is_none());

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("AlephBFT runtime state");
    let consensus = consensus.expect("consensus");
    assert!(sequences.is_some());

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after startup replay")
        .expect("mint quote replayed from finalized items");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let mut consensus = consensus.lock().await;
    assert_eq!(
        consensus.backend_kind(),
        FederationConsensusBackendKind::AlephBft
    );
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].operation_id,
        entry.operation_id
    );
    assert_eq!(
        consensus.journal().entries()[0].status,
        FederationOperationStatus::Applied
    );
    let err = consensus
        .submit_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .expect_err("AlephBFT runtime should reject synchronous submit");
    assert!(
        err.to_string().contains("requires external finalization"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn federation_runtime_startup_rejects_aleph_bft_journal_mismatch() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let finalized_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mismatched_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mut adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(finalized_envelope)],
    )
    .expect("AlephBFT finalized unit");
    adapter.finalize_unit(unit).expect("finalize unit");
    let mut mismatched_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, local_private.member_id);
    let mismatched_entry = mismatched_consensus
        .submit_envelope(mismatched_envelope)
        .expect("mismatched journal entry")
        .outcome
        .entry()
        .clone();

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT mismatch transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&mismatched_entry)
        .await
        .expect("persist mismatched journal entry");
    tx.commit().await.expect("commit AlephBFT mismatch state");

    let err = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect_err("AlephBFT journal mismatch rejected");
    assert!(
        err.to_string().contains("journal entry mismatch"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_persists_applies_and_notifies() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = {
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(envelope)
            .expect("queue AlephBFT envelope");
        consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit")
    };

    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified operation")
    });
    tokio::task::yield_now().await;

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized AlephBFT unit");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].operation_id, operation_id);
    let notified = waiter.await.expect("waiter task");
    assert_eq!(notified.operation_id, operation_id);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after finalization")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 1);
    assert_eq!(stored_finalized_items[0].operation_id, Some(operation_id));
    let stored_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("stored journal entries");
    assert_eq!(stored_entries.len(), 1);
    assert_eq!(stored_entries[0].operation_id, operation_id);
    assert_eq!(stored_entries[0].status, FederationOperationStatus::Applied);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_conflict_does_not_mutate_runtime() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state, &quote_id,
        ))],
    )
    .expect("AlephBFT finalized unit");

    let mut conflicting_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("conflicting AlephBFT adapter");
    let conflicting_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state,
            &QuoteId::new(),
        ))],
    )
    .expect("conflicting finalized unit");
    conflicting_adapter
        .finalize_unit(conflicting_unit)
        .expect("conflicting finalized item");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflicting finalized item transaction");
    tx.add_federation_aleph_bft_finalized_items(conflicting_adapter.finalized_items())
        .await
        .expect("persist conflicting finalized item");
    tx.commit()
        .await
        .expect("commit conflicting finalized item");

    let err = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect_err("conflicting finalized item should reject");
    assert!(
        err.to_string()
            .contains("Conflicting federation AlephBFT finalized item"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected finalization")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after rejected finalization")
        .is_empty());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 0);
}

async fn queue_local_aleph_bft_unit(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationAlephBftUnitPayload {
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let mut consensus = consensus.lock().await;
    consensus
        .submit_aleph_bft_envelope(envelope)
        .expect("queue AlephBFT envelope");
    consensus
        .preview_next_aleph_bft_unit_payload()
        .expect("preview AlephBFT unit")
        .expect("queued unit")
}

async fn wait_for_local_aleph_bft_unit(state: &MintState) -> FederationAlephBftUnitPayload {
    for _ in 0..50 {
        let maybe_unit = {
            let consensus = state.federation_consensus.as_ref().expect("consensus");
            let consensus = consensus.lock().await;
            consensus
                .preview_next_aleph_bft_unit_payload()
                .expect("preview AlephBFT unit")
        };
        if let Some(unit) = maybe_unit {
            return unit;
        }
        tokio::task::yield_now().await;
    }

    panic!("AlephBFT unit was not queued");
}

fn finalized_unit_response_for_peer(
    public_config: &FederationConfig,
    local_member: MemberId,
    unit: &FederationAlephBftUnitPayload,
) -> FederationAlephBftFinalizedUnitResponse {
    let mut runtime = FederationConsensusRuntime::aleph_bft_from_finalized_items(
        public_config.federation_id,
        local_member,
        0,
        public_config.members.keys().copied(),
        public_config.threshold_params.consensus_threshold,
        Vec::new(),
    )
    .expect("peer AlephBFT runtime");
    let responses = runtime
        .finalize_aleph_bft_unit(unit.clone())
        .expect("peer finalizes unit");
    FederationAlephBftFinalizedUnitResponse::new(public_config.federation_id, unit, responses)
}

fn assert_pending_finalization_error(
    err: &cdk::Error,
    operation_id: FederationOperationId,
    timeout_ms: u64,
) {
    assert!(
        !err.is_definitive_failure(),
        "pending finalization must remain retryable: {err}"
    );
    match err {
        cdk::Error::Federation(FederationError::OperationPendingFinalization {
            operation_id: found_operation_id,
            timeout_ms: found_timeout_ms,
        }) => {
            assert_eq!(found_operation_id, &operation_id);
            assert_eq!(*found_timeout_ms, timeout_ms);
        }
        _ => panic!("unexpected error: {err}"),
    }
}

#[tokio::test]
async fn aleph_bft_driver_startup_decision_spawns_only_for_aleph_bft_backend() {
    let (public_config, local_private, _) = transport_config();
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let in_memory_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db")),
        &seed,
        FederationConsensusBackendKind::InMemory,
    )
    .await;
    assert_eq!(
        federation_aleph_bft_driver_startup_decision(&in_memory_state),
        FederationAlephBftDriverStartupDecision::NonAlephBftBackend {
            backend: FederationConsensusBackendKind::InMemory,
        }
    );

    let aleph_bft_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db")),
        &seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    assert_eq!(
        federation_aleph_bft_driver_startup_decision(&aleph_bft_state),
        FederationAlephBftDriverStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
        }
    );
}

#[tokio::test]
async fn aleph_bft_public_submit_is_not_ready_until_real_finality_backend_exists() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);

    let err = state.submit_federation_envelope(envelope).await.expect_err(
        "AlephBFT public submit should be gated while production finality is unavailable",
    );

    assert!(
        err.to_string()
            .contains("public federation submissions are disabled"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_startup_restore_keeps_public_submit_halted_over_stale_healthy_status() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    first_state.mark_federation_healthy().await;
    assert!(matches!(
        first_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));

    let restored_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    restored_state.restore_federation_sync_status().await;

    assert!(matches!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { reason })
            if reason.contains("public federation submissions are disabled")
    ));

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&restored_state, &quote_id);
    let err = restored_state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("stale persisted healthy status must not reopen AlephBFT public submit");

    assert!(
        err.to_string()
            .contains("public federation submissions are disabled"),
        "unexpected error: {err}"
    );
    assert!(restored_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restored gated submit")
        .is_none());

    let consensus = restored_state
        .federation_consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_broadcast_round_drains_and_applies_after_threshold_delivery() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified operation")
    });
    tokio::task::yield_now().await;

    let outcome =
        broadcast_next_aleph_bft_finalized_unit_round(&state, &client, std::slice::from_ref(peer))
            .await
            .expect("broadcast round")
            .expect("unit broadcast");

    assert_eq!(outcome.unit.body_hash, unit.body_hash);
    assert_eq!(
        outcome.remote_accepted_members,
        vec![peer_private.member_id]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert_eq!(outcome.remote_responses.len(), 1);
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(outcome.local_responses[0].operation_id, operation_id);
    assert_eq!(
        waiter.await.expect("waiter task").operation_id,
        operation_id
    );

    let requests = transport.aleph_bft_finalized_unit_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].payload.unit.body_hash, unit.body_hash);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after finalization")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn aleph_bft_submit_envelope_waits_for_driver_finalization() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_with_sequence(envelope, None)
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    let submit_response = submit.await.expect("submit task");
    assert_eq!(submit_response.operation_id, operation_id);
    assert!(matches!(
        submit_response.outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after submit")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
}

#[tokio::test]
async fn aleph_bft_submit_envelope_persists_sequence_after_driver_finalization() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let sender_sequence = 42;
    let submit_state = state.clone();
    let sender = peer_private.member_id;
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_with_sequence(envelope, Some((sender, sender_sequence)))
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let sequences_before = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences before finalization");
    assert!(sequences_before.is_empty());

    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    submit.await.expect("submit task");
    let sequences_after = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences after finalization");
    assert_eq!(
        sequences_after.get(&peer_private.member_id).copied(),
        Some(sender_sequence)
    );
}

#[tokio::test]
async fn aleph_bft_submit_envelope_times_out_without_finalization() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect_err("unfinalized submit should time out");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after timeout")
        .is_none());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
}

#[tokio::test]
async fn aleph_bft_timed_out_submit_retry_receives_finalized_response_without_requeueing() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_with_sequence(envelope.clone(), None)
        .await
        .expect_err("unfinalized submit should time out");
    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let retry_state = state.clone();
    let retry_submit = tokio::spawn(async move {
        retry_state
            .submit_federation_envelope_with_sequence(envelope, None)
            .await
    });
    for _ in 0..50 {
        if state.federation_pending_response_waiter_count() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    let retry_response = retry_submit
        .await
        .expect("retry submit task")
        .expect("retry submit response");
    assert_eq!(retry_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after finalization")
        .is_some());
}

#[tokio::test]
async fn aleph_bft_submit_envelope_releases_response_waiter_after_timeout() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let first_err = state
        .submit_federation_envelope_with_sequence(first_envelope, None)
        .await
        .expect_err("first unfinalized submit should time out");
    assert_pending_finalization_error(&first_err, first_operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let second_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let second_operation_id = second_envelope.operation_id().expect("second operation id");

    let second_err = state
        .submit_federation_envelope_with_sequence(second_envelope, None)
        .await
        .expect_err("second unfinalized submit should time out after waiter cleanup");
    assert_pending_finalization_error(&second_err, second_operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
}

#[tokio::test]
async fn aleph_bft_submit_envelope_rejects_when_response_waiters_are_full() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_with_sequence(
                mint_quote_envelope(&first_state, &QuoteId::new()),
                None,
            )
            .await
    });
    for _ in 0..50 {
        if state.federation_pending_response_waiter_count() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let err = state
        .submit_federation_envelope_with_sequence(
            mint_quote_envelope(&state, &QuoteId::new()),
            None,
        )
        .await
        .expect_err("full response waiter set should reject");
    assert!(
        err.to_string().contains("response waiter limit exceeded"),
        "unexpected error: {err}"
    );
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    first_submit.abort();
    let _ = first_submit.await;
    for _ in 0..50 {
        if state.federation_pending_response_waiter_count() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
}

#[tokio::test]
async fn aleph_bft_duplicate_pending_submit_joins_existing_waiter_without_requeueing() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 3,
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let first_state = state.clone();
    let first_envelope = envelope.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_with_sequence(first_envelope, None)
            .await
    });
    for _ in 0..50 {
        if state.federation_pending_response_waiter_count() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let second_state = state.clone();
    let second_submit = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_with_sequence(envelope, None)
            .await
    });
    for _ in 0..50 {
        if state.federation_pending_response_waiter_count() == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 2);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.response_cache_lookups, 3);
    assert_eq!(stats.response_cache_hits, 0);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    let first_response = first_submit
        .await
        .expect("first submit task")
        .expect("first submit response");
    let second_response = second_submit
        .await
        .expect("second submit task")
        .expect("second submit response");
    assert_eq!(first_response.operation_id, operation_id);
    assert_eq!(second_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_response_cache_eviction_returns_journal_duplicate() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        response_cache_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_with_sequence(first_envelope.clone(), None)
            .await
    });
    let first_unit = wait_for_local_aleph_bft_unit(&state).await;
    let first_response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &first_unit);
    let first_transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(first_response);
    let first_client = FederationRestPeerClient::new(
        first_transport.clone(),
        &public_config,
        local_private.clone(),
    )
    .expect("first peer client");
    run_federation_aleph_bft_driver_tick(
        &state,
        &first_client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("first driver tick");
    let first_submit_response = first_submit
        .await
        .expect("first submit task")
        .expect("first submit response");
    assert_eq!(first_submit_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let second_state = state.clone();
    let second_submit = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_with_sequence(second_envelope, None)
            .await
    });
    let second_unit = wait_for_local_aleph_bft_unit(&state).await;
    let second_response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &second_unit);
    let second_transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(second_response);
    let second_client = FederationRestPeerClient::new(
        second_transport.clone(),
        &public_config,
        local_private.clone(),
    )
    .expect("second peer client");
    run_federation_aleph_bft_driver_tick(
        &state,
        &second_client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("second driver tick");
    let second_submit_response = second_submit
        .await
        .expect("second submit task")
        .expect("second submit response");
    assert_eq!(second_submit_response.operation_id, second_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let duplicate_response = state
        .submit_federation_envelope_with_sequence(
            mint_quote_envelope(&state, &first_quote_id),
            None,
        )
        .await
        .expect("duplicate submit after cache eviction");
    assert_eq!(duplicate_response.operation_id, first_operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    assert_eq!(first_transport.aleph_bft_finalized_unit_requests().len(), 1);
    assert_eq!(
        second_transport.aleph_bft_finalized_unit_requests().len(),
        1
    );

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 2);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(first_operation_id)
            .expect("first operation remains durable")
            .operation_id,
        first_operation_id
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
}

#[tokio::test]
async fn aleph_bft_submit_envelope_rejects_when_pending_mempool_is_full() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        pending_mempool_item_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let queued_quote_id = QuoteId::new();
    let queued_envelope = mint_quote_envelope(&state, &queued_quote_id);
    let queued_operation_id = queued_envelope.operation_id().expect("queued operation id");
    {
        let consensus = state
            .federation_consensus
            .as_ref()
            .expect("consensus handle");
        consensus
            .lock()
            .await
            .submit_aleph_bft_envelope(queued_envelope.clone())
            .expect("prequeued AlephBFT operation");
    }
    let rejected_quote_id = QuoteId::new();

    let err = state
        .submit_federation_envelope_with_sequence(
            mint_quote_envelope(&state, &rejected_quote_id),
            None,
        )
        .await
        .expect_err("full pending mempool should reject new operation");

    assert!(
        err.to_string().contains("pending consensus mempool full"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&rejected_quote_id)
        .await
        .expect("quote lookup after mempool rejection")
        .is_none());
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
    drop(consensus);

    let duplicate_err = state
        .submit_federation_envelope_with_sequence(queued_envelope, None)
        .await
        .expect_err("duplicate pending operation should wait on existing queue entry");
    assert_pending_finalization_error(&duplicate_err, queued_operation_id, 10);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let retained_unit = consensus
        .preview_next_aleph_bft_unit_payload()
        .expect("preview retained AlephBFT unit")
        .expect("retained AlephBFT unit");
    assert_eq!(
        retained_unit
            .items
            .first()
            .expect("retained consensus item")
            .operation_id()
            .expect("retained operation id"),
        Some(queued_operation_id)
    );
    drop(consensus);

    let response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &retained_unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let outcome = run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    assert_eq!(outcome.finalized_units, 1);
    assert_eq!(outcome.local_responses, 1);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&queued_quote_id)
        .await
        .expect("quote lookup after retained operation finalizes")
        .expect("queued mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{queued_quote_id}"));
    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 1);
    assert_eq!(
        stored_finalized_items[0].operation_id,
        Some(queued_operation_id)
    );
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .first()
            .expect("journal entry")
            .operation_id,
        queued_operation_id
    );
}

#[tokio::test]
async fn aleph_bft_submit_envelope_returns_finalized_duplicate_without_requeueing() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope.clone()).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    let duplicate_sequence = 24;
    let duplicate = state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("duplicate response");
    let duplicate_with_sequence = state
        .submit_federation_envelope_with_sequence(
            mint_quote_envelope(&state, &quote_id),
            Some((peer_private.member_id, duplicate_sequence)),
        )
        .await
        .expect("duplicate response with sequence");

    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(duplicate_with_sequence.operation_id, operation_id);
    assert!(matches!(
        duplicate_with_sequence.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted duplicate sequence");
    assert_eq!(
        sequences.get(&peer_private.member_id).copied(),
        Some(duplicate_sequence)
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 2);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn aleph_bft_driver_tick_returns_idle_without_peer_request() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let outcome = run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");

    assert_eq!(outcome.finalized_units, 0);
    assert_eq!(outcome.local_responses, 0);
    assert_eq!(outcome.remote_accepted_members, 0);
    assert_eq!(outcome.remote_failed_members, 0);
    assert!(outcome.reached_idle);
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());
}

#[tokio::test]
async fn aleph_bft_driver_tick_rejects_zero_work_bound_before_peer_request() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let err = run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig {
            max_finalized_units_per_tick: 0,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect_err("zero work bound must reject");

    assert!(
        err.to_string()
            .contains("max finalized units per tick must be greater than zero"),
        "unexpected error: {err}"
    );
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());

    let err = run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 0,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect_err("zero remote delivery bound must reject");

    assert!(
        err.to_string()
            .contains("max remote peer deliveries per unit must be greater than zero"),
        "unexpected error: {err}"
    );
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());
}

#[tokio::test]
async fn aleph_bft_driver_tick_applies_bounded_unit_then_stops_at_idle() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let outcome = run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig {
            max_finalized_units_per_tick: 2,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect("driver tick");

    assert_eq!(outcome.finalized_units, 1);
    assert_eq!(outcome.local_responses, 1);
    assert_eq!(outcome.remote_accepted_members, 1);
    assert_eq!(outcome.remote_failed_members, 0);
    assert_eq!(outcome.remote_skipped_members, 0);
    assert!(outcome.reached_idle);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after driver tick")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .first()
            .expect("journal entry")
            .operation_id,
        operation_id
    );
}

#[tokio::test]
async fn aleph_bft_broadcast_round_stops_remote_delivery_after_quorum() {
    let (public_config, local_private, second_private, third_private) = transport_config_three();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = queue_local_aleph_bft_unit(&state, mint_quote_envelope(&state, &quote_id)).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let second_peer = public_config
        .members
        .get(&second_private.member_id)
        .expect("second peer")
        .clone();
    let third_peer = public_config
        .members
        .get(&third_private.member_id)
        .expect("third peer")
        .clone();
    let peers = vec![second_peer, third_peer];

    let outcome = broadcast_next_aleph_bft_finalized_unit_round_with_config(
        &state,
        &client,
        &peers,
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 2,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect("broadcast round")
    .expect("unit broadcast");

    assert_eq!(
        outcome.remote_accepted_members,
        vec![second_private.member_id]
    );
    assert_eq!(
        outcome.remote_skipped_members,
        vec![third_private.member_id]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn aleph_bft_broadcast_round_rejects_delivery_limit_below_quorum_before_peer_request() {
    let (public_config, local_private, second_private, third_private) =
        transport_config_three_with_threshold(
            ThresholdParams::new(3, 2, 3).expect("threshold params"),
        );
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = queue_local_aleph_bft_unit(&state, mint_quote_envelope(&state, &quote_id)).await;
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let second_peer = public_config
        .members
        .get(&second_private.member_id)
        .expect("second peer")
        .clone();
    let third_peer = public_config
        .members
        .get(&third_private.member_id)
        .expect("third peer")
        .clone();
    let peers = vec![second_peer, third_peer];

    let err = broadcast_next_aleph_bft_finalized_unit_round_with_config(
        &state,
        &client,
        &peers,
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 1,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect_err("delivery limit below quorum should fail");

    assert!(
        err.to_string()
            .contains("peer delivery limit 1 is below 2 remote members required for quorum"),
        "unexpected error: {err}"
    );
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let still_queued = consensus
        .preview_next_aleph_bft_unit_payload()
        .expect("preview retained unit")
        .expect("unit retained");
    assert_eq!(
        still_queued.canonical_bytes().expect("retained bytes"),
        unit.canonical_bytes().expect("original bytes")
    );
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_broadcast_round_keeps_queue_when_delivery_lacks_quorum() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = queue_local_aleph_bft_unit(&state, mint_quote_envelope(&state, &quote_id)).await;
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let err =
        broadcast_next_aleph_bft_finalized_unit_round(&state, &client, std::slice::from_ref(peer))
            .await
            .expect_err("delivery without quorum should fail");

    assert!(
        err.to_string().contains("reached 1 of 2 required"),
        "unexpected error: {err}"
    );
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected broadcast")
        .is_none());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let still_queued = consensus
        .preview_next_aleph_bft_unit_payload()
        .expect("preview retained unit")
        .expect("unit retained");
    assert_eq!(
        still_queued.canonical_bytes().expect("retained bytes"),
        unit.canonical_bytes().expect("original bytes")
    );
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_local_exact_drain_mismatch_keeps_queue_and_state() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = queue_local_aleph_bft_unit(&state, mint_quote_envelope(&state, &quote_id)).await;
    let mut mismatched = unit.clone();
    mismatched.unit_index = mismatched.unit_index.saturating_add(7);

    let err = state
        .drain_and_apply_local_federation_aleph_bft_finalized_unit(&mismatched)
        .await
        .expect_err("mismatched preview should fail");

    assert!(
        err.to_string().contains("proposal mismatch"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected drain")
        .is_none());

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let still_queued = consensus
        .preview_next_aleph_bft_unit_payload()
        .expect("preview retained unit")
        .expect("unit retained");
    assert_eq!(
        still_queued.canonical_bytes().expect("retained bytes"),
        unit.canonical_bytes().expect("original bytes")
    );
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn startup_replays_accepted_but_unapplied_journal_entry_to_same_state_digest() {
    let (public_config, local_private, _) = transport_config();
    let control_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let mut restarted_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&control_state, &quote_id);
    control_state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("control submit");
    let control_status = local_peer_status(&control_state)
        .await
        .expect("control status");

    let mut crashed_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, local_private.member_id);
    let response = crashed_consensus
        .submit_envelope(envelope)
        .expect("accepted before crash");
    let entry = response.outcome.entry().clone();
    let mut tx = restarted_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin crash window transaction");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist accepted journal entry");
    tx.commit().await.expect("commit crash window journal");

    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before replay")
        .is_none());
    let missing_status = local_peer_status(&restarted_state)
        .await
        .expect("missing materialized status");
    assert_ne!(missing_status.state_digest, control_status.state_digest);

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("runtime state replay");
    restarted_state.federation_consensus = replayed_consensus;
    restarted_state.federation_sequences = replayed_sequences;

    let replayed_quote = restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after replay")
        .expect("mint quote replayed");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(replayed_entries.len(), 1);
    assert_eq!(
        replayed_entries[0].status,
        FederationOperationStatus::Applied
    );

    let replayed_status = local_peer_status(&restarted_state)
        .await
        .expect("replayed status");
    assert_eq!(replayed_status.operation_count, 1);
    assert_eq!(replayed_status.state_digest, control_status.state_digest);
    let replayed_consensus = restarted_state
        .federation_consensus
        .as_ref()
        .expect("replayed consensus")
        .lock()
        .await;
    assert_eq!(replayed_consensus.peer_status().operation_count, 1);
    assert_eq!(
        replayed_consensus.journal().entries()[0].status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn router_startup_replays_accepted_journal_before_serving_local_status() {
    let (public_config, local_private, peer_private) = transport_config();
    let control_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let restarted_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&control_state, &quote_id);
    control_state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("control submit");
    let control_status = local_peer_status(&control_state)
        .await
        .expect("control status");

    let mut crashed_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, local_private.member_id);
    let response = crashed_consensus
        .submit_envelope(envelope)
        .expect("accepted before crash");
    let entry = response.outcome.entry().clone();
    let mut tx = restarted_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin crash window transaction");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist accepted journal entry");
    tx.commit().await.expect("commit crash window journal");
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before router startup")
        .is_none());

    let router = crate::create_mint_router(
        restarted_state.mint.clone(),
        vec![KnownMethod::Bolt11.to_string()],
    )
    .await
    .expect("mint router");
    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_private.member_id,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::POST)
                .uri("/federation/v1/local-status")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&request).expect("request json"),
                ))
                .expect("local status request"),
        )
        .await
        .expect("local status response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("local status body");
    let local_status: FederationLocalStatusResponse =
        serde_json::from_slice(&body).expect("local status json");

    assert_eq!(local_status.peer_status.operation_count, 1);
    assert_eq!(
        local_status.peer_status.state_digest,
        control_status.state_digest
    );
    assert!(
        local_status
            .materialized_state_rows
            .iter()
            .any(|row| row.table == "mint_quotes"),
        "router startup should materialize the accepted mint quote before serving status"
    );

    let replayed_quote = restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after router startup")
        .expect("mint quote replayed");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(
        replayed_entries[0].status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn submit_mint_quote_operation_applies_quote_to_local_store() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint quote");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
    assert_eq!(quote.amount, Some(Amount::new(1, CurrencyUnit::Sat)));
    assert_eq!(quote.state(), MintQuoteState::Unpaid);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(
        state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn local_status_uses_materialized_database_state_digest() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let status = local_peer_status(&state).await.expect("local status");
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let journal_only_digest = consensus.lock().await.state_digest();

    assert_eq!(status.operation_count, 1);
    assert_ne!(
        status.state_digest, journal_only_digest,
        "status digest should include applied mint database rows, not only the accepted journal"
    );
    assert_ne!(status.state_digest, FederationStateDigest::genesis());
}

#[tokio::test]
async fn materialized_state_digest_tracks_keyset_rotation_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("before digest");
    let keyset = public_config.keysets[0].clone();
    let envelope = FederationOperationEnvelope::new(
        public_config.federation_id,
        FederationOperation::KeysetRotation(FederationKeysetRotationOperation { keyset }),
    );
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit keyset rotation");
    let after_entries = consensus.lock().await.journal().entries().to_vec();
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationStateDigest::genesis());

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|entry| entry.operation_id == operation_id)
            .expect("rotation entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn materialized_state_digest_ignores_local_mint_payment_metadata() {
    let (first_state, _, _) = federation_route_state().await;
    let (second_state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
    let paid_envelope = paid_observation_envelope(&first_state, &quote_id);

    for state in [&first_state, &second_state] {
        state
            .submit_federation_envelope(quote_envelope.clone())
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_envelope.clone())
            .await
            .expect("submit paid observation");
    }

    let mut tx = second_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin tx");
    let mut quote = tx
        .get_mint_quote(&quote_id)
        .await
        .expect("load quote")
        .expect("mint quote");
    quote
        .add_payment(
            Amount::from(0).with_unit(CurrencyUnit::Sat),
            "local-backend-zero-payment".to_string(),
            Some(42),
        )
        .expect("add local payment metadata");
    tx.update_mint_quote(&mut quote)
        .await
        .expect("persist local payment metadata");
    tx.commit().await.expect("commit tx");

    let first_entries = first_state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();

    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second digest");

    assert_eq!(first_digest, second_digest);
}

#[tokio::test]
async fn materialized_state_digest_tracks_unfinalized_payment_observation_rows() {
    let (mut public_config, local_private, _) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let state = federation_route_state_for_private(public_config, local_private).await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before observation");

    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit unfinalized observation");
    let after_entries = consensus.lock().await.journal().entries().to_vec();
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after observation");

    assert_ne!(
            before_digest, after_digest,
            "accepted payment-observation evidence must affect the materialized digest before quote finality"
        );

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state(), MintQuoteState::Unpaid);
    assert_eq!(quote.amount_paid(), Amount::new(0, CurrencyUnit::Sat));

    let observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            &quote_id.to_string(),
        )
        .await
        .expect("payment observations");
    assert_eq!(observations.len(), 1);
}

#[tokio::test]
async fn materialized_state_digest_tracks_payment_observation_quarantine_rows() {
    let (state, _, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let first_response = state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first observation");
    let conflicting_response = state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            2,
            2,
            "different-settlement",
        ))
        .await
        .expect("submit conflicting observation");

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before quarantine");

    state
        .submit_federation_envelope(config.payment_observation_quarantine(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            quote_id.clone(),
            FederationPaymentObservationQuarantineReason::ConflictingEvidence,
            vec![
                first_response.operation_id,
                conflicting_response.operation_id,
            ],
        ))
        .await
        .expect("submit quarantine");
    let after_entries = consensus.lock().await.journal().entries().to_vec();
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after quarantine");

    assert_ne!(
        before_digest, after_digest,
        "accepted payment-observation quarantine rows must affect the materialized digest"
    );
    assert_ne!(after_digest, FederationStateDigest::genesis());
}

#[tokio::test]
async fn materialized_state_digest_tracks_swap_spend_and_output_share_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let input = proof_for_amount(&state, 1, "digest-swap");
    let input_y = input.y().expect("input y");
    let output = swap_output(&state, "digest-swap");
    let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
    let envelope = config.swap_operation(request).expect("swap envelope");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted swap");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationStateDigest::genesis());

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&[input_y])
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_some());
}

#[tokio::test]
async fn submit_federation_envelope_notifies_operation_acceptance() {
    let (mut state, _, _) = federation_route_state().await;
    state.start_federation_finalization_loop();
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let waiter_state = state.clone();
    let waiter = tokio::spawn(async move {
        waiter_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified response")
    });
    tokio::task::yield_now().await;

    let response = state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint quote");
    let notified = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("notification waiter")
        .expect("notification task");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(notified.operation_id, operation_id);
    assert_eq!(notified.outcome.entry().operation_id, operation_id);
}

#[tokio::test]
async fn submit_federation_envelope_bounds_response_cache() {
    let (mut state, _, _) = federation_route_state().await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        response_cache_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_response = state
        .submit_federation_envelope(first_envelope.clone())
        .await
        .expect("submit first operation");
    assert_eq!(first_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let second_quote_id = QuoteId::new();
    let second_response = state
        .submit_federation_envelope(mint_quote_envelope(&state, &second_quote_id))
        .await
        .expect("submit second operation");
    assert_ne!(second_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let duplicate_response = state
        .submit_federation_envelope(first_envelope)
        .await
        .expect("resubmit first operation after cache eviction");
    assert_eq!(duplicate_response.operation_id, first_operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.response_cache_items, 1);
    assert_eq!(stats.response_cache_lookups, 3);
    assert_eq!(stats.response_cache_hits, 3);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);
}

#[tokio::test]
async fn submit_federation_envelope_records_state_digest_checkpoint() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let response = state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let entry = response.outcome.entry();
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let expected_state_digest = materialized_state_digest(&state, &entries)
        .await
        .expect("materialized state digest");

    let checkpoints = state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].operation_index, entry.index);
    assert_eq!(checkpoints[0].operation_id, response.operation_id);
    assert_eq!(checkpoints[0].order_digest, entry.order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);
}

#[tokio::test]
async fn submit_federation_envelope_prunes_state_digest_checkpoints_to_retention() {
    let (mut state, _, _) = federation_route_state().await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        state_digest_checkpoint_retention: 2,
        ..FederationPeerSyncConfig::default()
    });

    for _ in 0..3 {
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
    }

    let checkpoints = state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[0].operation_index, 1);
    assert_eq!(checkpoints[1].operation_index, 2);
}

#[cfg(feature = "prometheus")]
#[tokio::test]
async fn submit_federation_envelope_records_prometheus_operation_metrics() {
    let (state, _, _) = federation_route_state().await;
    let member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id()
        .to_string();
    let accepted_labels = [("member", member.as_str()), ("operation", "mint_quote")];
    let rejected_labels = [
        ("member", member.as_str()),
        ("operation", "mint_quote"),
        ("reason", "consensus"),
    ];

    let accepted_before =
        prometheus_metric_value("cdk_federation_accepted_operations_total", &accepted_labels);
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let accepted_after =
        prometheus_metric_value("cdk_federation_accepted_operations_total", &accepted_labels);
    assert_eq!(accepted_after, accepted_before + 1.0);

    let rejected_before =
        prometheus_metric_value("cdk_federation_rejected_operations_total", &rejected_labels);
    let mut conflicting = mint_quote_envelope(&state, &quote_id);
    let FederationOperation::MintQuote(operation) = &mut conflicting.operation else {
        panic!("mint quote operation");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("bolt11 mint quote response");
    };
    response.request.push_str("-conflict");

    state
        .submit_federation_envelope(conflicting)
        .await
        .expect_err("conflicting mint quote must be rejected");
    let rejected_after =
        prometheus_metric_value("cdk_federation_rejected_operations_total", &rejected_labels);
    assert_eq!(rejected_after, rejected_before + 1.0);
}

#[tokio::test]
async fn submit_mint_operation_applies_issuance_and_persists_share() {
    let (state, _, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit paid observation");

    let output = mint_output(&state, &quote_id);
    let input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let envelope = config.mint_operation(input.clone()).expect("mint envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.amount_paid(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.state(), MintQuoteState::Issued);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    let persisted_signature = signatures[0].clone().expect("signature persisted");
    let expected_response = config
        .mint_share_response(operation_id, &input)
        .expect("expected share response");
    assert_eq!(persisted_signature, expected_response.signatures[0]);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|entry| entry.operation_id == operation_id)
            .expect("mint entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn materialized_state_digest_ignores_member_local_signature_share_bytes() {
    let (public_config, first_private, second_private) = transport_config();
    let first_state =
        federation_route_state_for_private(public_config.clone(), first_private).await;
    let second_state =
        federation_route_state_for_private(public_config.clone(), second_private).await;
    let quote_id = QuoteId::new();
    let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
    let paid_envelope = paid_observation_envelope(&first_state, &quote_id);
    let output = mint_output(&first_state, &quote_id);
    let input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_envelope = first_state
        .mint
        .federation_config()
        .expect("federation config")
        .mint_operation(input)
        .expect("mint envelope");

    for state in [&first_state, &second_state] {
        state
            .submit_federation_envelope_with_sequence(quote_envelope.clone(), None)
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope_with_sequence(paid_envelope.clone(), None)
            .await
            .expect("submit paid observation");
        state
            .submit_federation_envelope_with_sequence(mint_envelope.clone(), None)
            .await
            .expect("submit mint");
    }

    let first_signature = first_state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("first signature lookup")
        .remove(0)
        .expect("first signature");
    let second_signature = second_state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("second signature lookup")
        .remove(0)
        .expect("second signature");
    assert_ne!(first_signature.c, second_signature.c);

    let first_entries = first_state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second digest");

    assert_eq!(first_digest, second_digest);
}

#[tokio::test]
async fn materialized_state_digest_tracks_melt_quote_spend_and_change_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = melt_quote(&state, 1000).await;
    let inputs = vec![
        proof_for_amount(&state, 1, "digest-melt-a"),
        proof_for_amount(&state, 1, "digest-melt-b"),
        proof_for_amount(&state, 1, "digest-melt-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "digest-melt-change");
    change_output.amount = Amount::ZERO;
    let request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let envelope = config.melt_operation(request).expect("melt envelope");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted melt");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationStateDigest::genesis());

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_some());
}

#[tokio::test]
async fn materialized_state_digest_tracks_restore_output_share_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let output = mint_output(&state, &quote_id);
    let mut restore_output = output.clone();
    restore_output.amount = Amount::ZERO;
    let restore_envelope = FederationOperationEnvelope::new(
        public_config.federation_id,
        FederationOperation::Restore(FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![restore_output],
            },
        }),
    );
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(restore_envelope)
        .expect("peer accepted restore");
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let missing_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("missing digest");
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit paid observation");
    let input = MintInput::Single(MintRequest {
        quote: quote_id,
        outputs: vec![output.clone()],
        signature: None,
    });
    state
        .submit_federation_envelope(config.mint_operation(input).expect("mint envelope"))
        .await
        .expect("submit mint");
    let materialized_digest = materialized_state_digest(&state, &catch_up_response.entries)
        .await
        .expect("materialized digest");

    assert_ne!(missing_digest, materialized_digest);
    assert_ne!(materialized_digest, FederationStateDigest::genesis());

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_some());
}

#[tokio::test]
async fn private_local_status_route_returns_consensus_and_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("submit restore");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state.clone()), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.version, FEDERATION_REST_API_VERSION);
    assert_eq!(response.peer_status.member_id, local_member);
    assert_eq!(response.peer_status.operation_count, 1);
    assert_eq!(response.sync_state, "healthy");
    assert!(response.sync_ready);
    assert_eq!(response.sync_message, "federation member is healthy");
    assert_eq!(response.session_outcome_quorum_operation_count, None);
    assert_eq!(response.session_outcome_quorum_from_index, None);
    assert_eq!(response.session_outcome_quorum_latest_index, None);
    assert!(response.session_outcome_quorum_signers.is_empty());
    assert!(response.payment_observation_reports.is_empty());
    assert!(response.materialized_state_rows.is_empty());
    assert!(response.invalid_operation_alerts.is_empty());

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response with rows")
        .0;
    assert!(!response.materialized_state_rows.is_empty());
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(
        !status_json.contains("0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41")
    );
}

#[tokio::test]
async fn private_local_status_route_reports_invalid_operation_alerts() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    for _ in 0..crate::FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD {
        state
            .record_federation_invalid_operation_alert(
                crate::FederationInvalidOperationSource::auth_token("clear", b"secret-token"),
            )
            .await;
    }

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.invalid_operation_alerts.len(), 1);
    assert_eq!(response.invalid_operation_alerts[0].source, "auth_token");
    assert_eq!(response.invalid_operation_alerts[0].alerting_buckets, 1);
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("secret-token"));
}

#[tokio::test]
async fn private_local_status_route_reports_payment_observation_conflicts() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let first_response = state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first payment observation");
    let conflicting_response = state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            2,
            2,
            "different-settlement",
        ))
        .await
        .expect("submit conflicting second payment observation");
    let quarantine_response = state
        .submit_federation_envelope(config.payment_observation_quarantine(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            quote_id.clone(),
            FederationPaymentObservationQuarantineReason::ConflictingEvidence,
            vec![
                first_response.operation_id,
                conflicting_response.operation_id,
            ],
        ))
        .await
        .expect("submit quarantine");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.payment_observation_reports.len(), 1);
    let report = &response.payment_observation_reports[0];
    assert_eq!(
        report.context,
        cdk::federation::FederationPaymentObservationContext::MintQuote
    );
    assert_eq!(report.quote, quote_id.to_string());
    assert_eq!(report.required, 1);
    assert_eq!(report.total_observations, 2);
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::Conflicting
    );
    assert_eq!(report.groups.len(), 2);
    assert_eq!(report.groups[0].state, "PAID");
    assert_eq!(report.groups[0].observers, vec![member_id(1)]);
    assert!(report.groups[0].quorum_reached);
    assert_eq!(report.groups[0].conflicting_observers, vec![member_id(2)]);
    assert_eq!(
        report.conflicting_observers,
        vec![member_id(1), member_id(2)]
    );
    assert!(report.quarantined);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Quarantine
    );
    assert_eq!(
        report.quarantine_operation_ids,
        vec![quarantine_response.operation_id]
    );
    assert_eq!(report.quarantine_operation_indices, vec![3]);
    assert_eq!(
        report.quarantine_reasons,
        vec![FederationPaymentObservationQuarantineReason::ConflictingEvidence]
    );

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
    assert!(!status_json.contains("different-settlement"));
}

#[tokio::test]
async fn private_local_status_route_reports_stale_payment_observations() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    public_config
        .payment_observation_policy
        .stale_observation_secs = 1;
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let quote_id = QuoteId::new();
    let old_quote = MintQuote::new(
        Some(quote_id.clone()),
        format!("invoice-{quote_id}"),
        CurrencyUnit::Sat,
        Some(Amount::from(1).with_unit(CurrencyUnit::Sat)),
        10_000,
        PaymentIdentifier::QuoteId(quote_id.clone()),
        None,
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        PaymentMethod::Known(KnownMethod::Bolt11),
        1,
        Vec::new(),
        Vec::new(),
        None,
    );
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin transaction");
    tx.add_mint_quote(old_quote).await.expect("add old quote");
    tx.commit().await.expect("commit old quote");

    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first payment observation");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.payment_observation_reports.len(), 1);
    let report = &response.payment_observation_reports[0];
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum
    );
    assert_eq!(report.required, 2);
    assert_eq!(report.total_observations, 1);
    assert!(report.stale);
    assert!(report.quote_age_secs.expect("quote age") >= 1);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );
    assert!(!report.groups[0].quorum_reached);

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
}

#[tokio::test]
async fn private_local_status_route_reports_session_outcome_quorum_evidence() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    state
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("submit restore");
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let peer_status = local_peer_status(&state).await.expect("local status");
    let local_outcome = FederationSessionOutcome::new_signed(
        public_config.federation_id,
        &entries,
        peer_status.latest_order_digest,
        peer_status.state_digest,
        &local_private,
    )
    .expect("local session outcome");
    let peer_outcome = FederationSessionOutcome::new_signed(
        public_config.federation_id,
        &entries,
        peer_status.latest_order_digest,
        peer_status.state_digest,
        &peer_private,
    )
    .expect("peer session outcome");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin transaction");
    tx.add_federation_session_outcome(&local_outcome)
        .await
        .expect("persist local outcome");
    tx.add_federation_session_outcome(&peer_outcome)
        .await
        .expect("persist peer outcome");
    tx.commit().await.expect("commit outcomes");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 1);
    assert_eq!(response.session_outcome_quorum_operation_count, Some(1));
    assert_eq!(response.session_outcome_quorum_from_index, Some(0));
    assert_eq!(response.session_outcome_quorum_latest_index, Some(0));
    assert_eq!(response.session_outcome_quorum_signers.len(), 2);
    assert!(response
        .session_outcome_quorum_signers
        .contains(&local_private.member_id));
    assert!(response
        .session_outcome_quorum_signers
        .contains(&peer_private.member_id));
}

#[tokio::test]
async fn private_status_routes_read_persisted_journal_after_runtime_reset() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit restore");
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);

    reset_federation_runtime_journal(&mut state);
    assert_eq!(consensus_operation_count(&state).await, 0);

    let status_request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let status = post_status(State(state.clone()), Json(status_request))
        .await
        .expect("status response")
        .0;
    assert_eq!(status.operation_count, 1);
    assert_eq!(status.latest_index, Some(0));
    assert_eq!(status.latest_order_digest, entries[0].order_digest);

    let local_status_request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let local_status = post_local_status(State(state), Json(local_status_request))
        .await
        .expect("local status response")
        .0;
    assert_eq!(local_status.peer_status.operation_count, 1);
    assert_eq!(local_status.peer_status.latest_index, Some(0));
    assert_eq!(
        local_status.peer_status.latest_order_digest,
        entries[0].order_digest
    );
    assert!(!local_status.materialized_state_rows.is_empty());
}

#[tokio::test]
async fn private_status_route_rejects_inbound_overflow_before_sequence_persistence() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state.federation_private_rest_admission = Some(Arc::new(tokio::sync::Semaphore::new(1)));
    let held_permit = state
        .federation_private_rest_admission
        .as_ref()
        .expect("admission")
        .clone()
        .try_acquire_owned()
        .expect("held inbound permit");
    let request = signed_status_request(&public_config, &peer_private, local_member, 1);

    let response = post_status(State(state.clone()), Json(request.clone()))
        .await
        .expect_err("inbound overflow should fail");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "overflow must not persist the sender sequence"
    );

    drop(held_permit);
    let status = post_status(State(state), Json(request))
        .await
        .expect("status response after capacity is available")
        .0;
    assert_eq!(status.member_id, local_member);
}

#[tokio::test]
async fn private_local_status_route_reports_lagging_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state.mark_federation_lagging(peer_member, 0, 3).await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 0);
    assert_eq!(response.sync_state, "lagging");
    assert!(!response.sync_ready);
    assert!(response
        .sync_message
        .contains("federation member is lagging behind peer"));
}

#[tokio::test]
async fn sync_status_transitions_persist_member_status_snapshot() {
    let (state, _, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;

    state.mark_federation_lagging(peer_member, 1, 4).await;
    let lagging = state
        .mint
        .localstore()
        .get_federation_member_status(local_member)
        .await
        .expect("persisted lagging status")
        .expect("lagging status");
    assert_eq!(lagging.status, FederationMemberStatusKind::Lagging);
    assert_eq!(lagging.peer, Some(peer_member));
    assert_eq!(lagging.local_operation_count, Some(1));
    assert_eq!(lagging.peer_operation_count, Some(4));
    assert!(lagging
        .message
        .expect("lagging message")
        .contains("lagging behind peer"));

    state
        .mark_federation_halted(
            "halted while reading secret=sync-secret from /home/example/fed/private.json",
        )
        .await;
    let halted = state
        .mint
        .localstore()
        .get_federation_member_status(local_member)
        .await
        .expect("persisted halted status")
        .expect("halted status");
    let halted_message = halted.message.expect("halted message");
    assert_eq!(halted.status, FederationMemberStatusKind::Halted);
    assert!(halted_message.contains("secret=[REDACTED]"));
    assert!(halted_message.contains("[REDACTED_PATH]"));
    assert!(!halted_message.contains("sync-secret"));
    assert!(!halted_message.contains("/home/example"));
}

#[tokio::test]
async fn startup_restores_persisted_federation_sync_status() {
    let (state, _, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    state.mark_federation_lagging(peer_member, 2, 5).await;

    let mut restarted = state.clone();
    restarted.federation_sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );

    restarted.restore_federation_sync_status().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Lagging {
            peer: peer_member,
            local_operation_count: 2,
            peer_operation_count: 5,
        })
    );
}

#[tokio::test]
async fn startup_requires_session_outcome_quorum_for_replayed_journal() {
    let (state, public_config, _, peer_private, _) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("persist accepted operation");

    let mut restarted = state.clone();
    restarted.federation_sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::VerifyingSessionOutcome {
            peer: peer_member,
            operation_count: 1,
        })
    );

    let blocked = restarted
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .await;
    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
}

#[tokio::test]
async fn startup_returns_healthy_when_replayed_journal_has_session_outcome_quorum() {
    let (state, public_config, _, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("persist accepted operation");
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let journal = InMemoryFederationJournal::from_entries(entries).expect("startup journal");
    let mut response = FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        &journal,
        &FederationJournalCatchUpRequest::new(0),
        100,
        100,
    );
    response.state_digest = local_peer_status(&state)
        .await
        .expect("local status")
        .state_digest;
    let peer_outcome = signed_catch_up_response(response.clone(), &peer_private)
        .session_outcome
        .expect("peer session outcome");
    let witness_outcome = signed_catch_up_response(response, &witness_private)
        .session_outcome
        .expect("witness session outcome");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin evidence transaction");
    tx.add_federation_session_outcome(&peer_outcome)
        .await
        .expect("persist peer outcome");
    tx.add_federation_session_outcome(&witness_outcome)
        .await
        .expect("persist witness outcome");
    tx.commit().await.expect("commit evidence");

    let mut restarted = state.clone();
    restarted.federation_sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    restarted
        .mark_federation_verifying_session_outcome(peer_member, 1)
        .await;
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );
}

#[tokio::test]
async fn private_local_status_route_reports_verifying_session_outcome_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state
        .mark_federation_verifying_session_outcome(peer_member, 3)
        .await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 0);
    assert_eq!(response.sync_state, "verifying_session_outcome");
    assert!(!response.sync_ready);
    assert!(response
        .sync_message
        .contains("verifying quorum session outcome evidence"));
}

#[tokio::test]
async fn private_journal_route_returns_signed_session_outcome() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state
        .submit_federation_envelope(restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("submit restore");
    reset_federation_runtime_journal(&mut state);
    assert_eq!(consensus_operation_count(&state).await, 0);

    let request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationJournalCatchUpRequest::new(0),
    );

    let response = post_journal(State(state), Json(request))
        .await
        .expect("journal response")
        .0;

    assert_eq!(response.entries.len(), 1);
    assert!(response.session_outcome.is_some());
    response
        .verify_session_outcome(public_config.member(local_member).expect("local member"))
        .expect("session outcome verifies");
}
