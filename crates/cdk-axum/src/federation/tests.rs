use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use aleph_bft::{Network, NodeIndex, Recipient};
use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::cdk_database::MintDatabase;
use cdk::cdk_payment::PaymentIdentifier;
use cdk::federation::{
    generate_trusted_dealer_configs, FederationAlephBftConsensusAdapter,
    FederationAlephBftFinalityMode, FederationAlephBftFinalizedUnitRequest,
    FederationAlephBftFinalizedUnitResponse, FederationAlephBftNetworkMessageRequest,
    FederationAlephBftNetworkMessageResponse, FederationAlephBftNetworkRetryRecord,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitBodyHash,
    FederationAlephBftUnitPayload, FederationConfigSnapshot, FederationConsensusBackendKind,
    FederationConsensusEngine, FederationConsensusItem, FederationConsensusRuntime,
    FederationDkgSetupMethod, FederationDkgTranscript, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignature, FederationError, FederationJournalAppendOutcome,
    FederationKeysetRotationOperation, FederationMemberStatusKind,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationOperationStatus,
    FederationPaymentObservationPolicy, FederationPaymentObservationQuarantineReason,
    FederationPaymentObservationReportAction, FederationPaymentTrustModel,
    FederationRestMessageKind, FederationRestoreOperation, FederationSignedRequest,
    InMemoryFederationConsensus, MemberConfigPrivate, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, MintQuote, QuoteId};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut01::{evaluate_secret_key_coefficients, BlsG1PublicKey, BlsSecretKey};
use cdk::nuts::nut22::AuthToken;
use cdk::nuts::{
    BlindedMessage, CheckStateRequest, CurrencyUnit, Id, MeltQuoteBolt11Request, MeltQuoteState,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest, PaymentMethod,
    PublicKey, RestoreRequest, SecretKey,
};
use cdk::types::FeeReserve;
use cdk::Amount;
use cdk_common::database::DynMintDatabase;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
use cdk_http_client::HttpError;
use parity_scale_codec::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::ServiceExt;

use crate::auth::AuthHeader;

use super::aleph_bft_network::CdkAlephBftNetworkInboundHandle;
use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick, run_cdk_aleph_bft_network_pump_tick_with_retry_queue,
    CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
    CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
};
use super::aleph_bft_runtime::{CdkAlephBftKeychain, CdkAlephBftNetwork};
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
    aleph_bft_finalized_unit_responses: VecDeque<FederationAlephBftFinalizedUnitResponse>,
    aleph_bft_finalized_unit_delays: VecDeque<Duration>,
    aleph_bft_network_message_response: Option<FederationAlephBftNetworkMessageResponse>,
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
    aleph_bft_network_message_requests:
        Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>,
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

#[derive(Clone)]
struct RouterFederationTransport {
    router: Option<Router>,
}

#[derive(Clone, Default)]
struct RouterFederationTransportRegistry {
    routers: Arc<Mutex<BTreeMap<String, Router>>>,
    missing_router_wait: Duration,
    faults: Arc<Mutex<RouterFederationTransportFaults>>,
    held_aleph_bft_network_message: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    aleph_bft_network_message_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>>>,
}

#[derive(Debug, Default)]
struct RouterFederationTransportFaults {
    aleph_bft_network_message_delays: VecDeque<Duration>,
    aleph_bft_network_message_duplicate_replays: usize,
    aleph_bft_network_message_reorder_once: bool,
    aleph_bft_network_message_reorders_completed: usize,
}

enum AlephBftNetworkMessageReorderDecision {
    None,
    Hold(tokio::sync::oneshot::Receiver<()>),
    ReleaseAfterPost(tokio::sync::oneshot::Sender<()>),
}

impl std::fmt::Debug for RouterFederationTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterFederationTransport")
            .field("configured", &self.router.is_some())
            .finish()
    }
}

impl std::fmt::Debug for RouterFederationTransportRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let router_count = self
            .routers
            .lock()
            .map(|routers| routers.len())
            .unwrap_or_default();
        f.debug_struct("RouterFederationTransportRegistry")
            .field("router_count", &router_count)
            .field("missing_router_wait", &self.missing_router_wait)
            .field(
                "faults",
                &self
                    .faults
                    .lock()
                    .map(|faults| {
                        (
                            faults.aleph_bft_network_message_delays.len(),
                            faults.aleph_bft_network_message_duplicate_replays,
                            faults.aleph_bft_network_message_reorder_once,
                            faults.aleph_bft_network_message_reorders_completed,
                        )
                    })
                    .unwrap_or_default(),
            )
            .field(
                "held_aleph_bft_network_message",
                &self
                    .held_aleph_bft_network_message
                    .lock()
                    .map(|held| held.is_some())
                    .unwrap_or_default(),
            )
            .field(
                "aleph_bft_network_message_requests",
                &self
                    .aleph_bft_network_message_requests
                    .lock()
                    .map(|requests| requests.len())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

impl RouterFederationTransport {
    fn new(router: Router) -> Self {
        Self {
            router: Some(router),
        }
    }
}

impl RouterFederationTransportRegistry {
    fn with_missing_router_wait(self, timeout: Duration) -> Self {
        Self {
            missing_router_wait: timeout,
            ..self
        }
    }

    fn with_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
    where
        I: IntoIterator<Item = Duration>,
    {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_delays
            .extend(delays);
        self
    }

    fn with_aleph_bft_network_message_duplicate_replays(self, count: usize) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_duplicate_replays = count;
        self
    }

    fn with_aleph_bft_network_message_reorder_once(self) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorder_once = true;
        self
    }

    fn aleph_bft_network_message_reorders_completed(&self) -> usize {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed
    }

    fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.aleph_bft_network_message_requests
            .lock()
            .expect("router transport network-message requests lock")
            .clone()
    }

    fn insert(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .insert(endpoint, router);
    }

    fn pop_aleph_bft_network_message_delay(&self) -> Duration {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_delays
            .pop_front()
            .unwrap_or_default()
    }

    fn take_aleph_bft_network_message_duplicate_replay(&self) -> bool {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if faults.aleph_bft_network_message_duplicate_replays == 0 {
            return false;
        }
        faults.aleph_bft_network_message_duplicate_replays -= 1;
        true
    }

    fn aleph_bft_network_message_reorder_decision(&self) -> AlephBftNetworkMessageReorderDecision {
        let mut held = self
            .held_aleph_bft_network_message
            .lock()
            .expect("router transport held-message lock");
        if let Some(release) = held.take() {
            return AlephBftNetworkMessageReorderDecision::ReleaseAfterPost(release);
        }

        let mut faults = self.faults.lock().expect("router transport faults lock");
        if faults.aleph_bft_network_message_reorder_once {
            faults.aleph_bft_network_message_reorder_once = false;
            let (release, wait) = tokio::sync::oneshot::channel();
            *held = Some(release);
            return AlephBftNetworkMessageReorderDecision::Hold(wait);
        }

        AlephBftNetworkMessageReorderDecision::None
    }

    fn mark_aleph_bft_network_message_reorder_completed(&self) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed += 1;
    }
}

impl Default for RouterFederationTransport {
    fn default() -> Self {
        Self { router: None }
    }
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

    fn with_aleph_bft_finalized_unit_responses<I>(self, responses: I) -> Self
    where
        I: IntoIterator<Item = FederationAlephBftFinalizedUnitResponse>,
    {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_finalized_unit_responses
            .extend(responses);
        self
    }

    fn with_aleph_bft_finalized_unit_delays<I>(self, delays: I) -> Self
    where
        I: IntoIterator<Item = Duration>,
    {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_finalized_unit_delays
            .extend(delays);
        self
    }

    fn with_aleph_bft_network_message_response(
        self,
        response: FederationAlephBftNetworkMessageResponse,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_response = Some(response);
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

    fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_requests
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
        if url
            .path()
            .ends_with("/federation/v1/aleph-bft/finalized-units")
        {
            let request = serde_json::from_value::<
                FederationSignedRequest<FederationAlephBftFinalizedUnitRequest>,
            >(serde_json::to_value(payload)?)?;
            let (delay, response) = {
                let mut state = self.state.lock().expect("mock transport lock");
                state.aleph_bft_finalized_unit_requests.push(request);
                let delay = state
                    .aleph_bft_finalized_unit_delays
                    .pop_front()
                    .unwrap_or_default();
                let response = state
                    .aleph_bft_finalized_unit_responses
                    .pop_front()
                    .or_else(|| state.aleph_bft_finalized_unit_response.clone())
                    .ok_or_else(|| {
                        HttpError::Other("missing AlephBFT finalized-unit response".to_string())
                    })?;
                (delay, response)
            };
            if !delay.is_zero() {
                time::sleep(delay).await;
            }
            return json_response(response);
        }

        if url.path().ends_with("/federation/v1/aleph-bft/messages") {
            let request = serde_json::from_value::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(serde_json::to_value(payload)?)?;
            let response = {
                let mut state = self.state.lock().expect("mock transport lock");
                state.aleph_bft_network_message_requests.push(request);
                state
                    .aleph_bft_network_message_response
                    .clone()
                    .ok_or_else(|| {
                        HttpError::Other("missing AlephBFT network-message response".to_string())
                    })?
            };
            return json_response(response);
        }

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

#[async_trait]
impl Transport for RouterFederationTransport {
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
        let router = self
            .router
            .clone()
            .ok_or_else(|| HttpError::Other("router transport not configured".to_string()))?;
        let request = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri(url.path())
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(payload)?))
            .map_err(|err| HttpError::Other(err.to_string()))?;
        let response = router
            .oneshot(request)
            .await
            .map_err(|err| HttpError::Other(err.to_string()))?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map_err(|err| HttpError::Other(err.to_string()))?;
        if !status.is_success() {
            return Err(HttpError::Status {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&body).to_string(),
            });
        }
        Ok(serde_json::from_slice(&body)?)
    }
}

#[async_trait]
impl Transport for RouterFederationTransportRegistry {
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
        let endpoint = url.origin().ascii_serialization();
        let path = url.path().to_string();
        let is_aleph_bft_network_message = path.ends_with("/federation/v1/aleph-bft/messages");
        let deadline = tokio::time::Instant::now() + self.missing_router_wait;
        let router = loop {
            if let Some(router) = self
                .routers
                .lock()
                .expect("router registry lock")
                .get(&endpoint)
                .cloned()
            {
                break router;
            }
            if self.missing_router_wait.is_zero() || tokio::time::Instant::now() >= deadline {
                return Err(HttpError::Other(format!(
                    "router endpoint {endpoint} not configured"
                )));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        let payload_bytes = serde_json::to_vec(payload)?;
        if is_aleph_bft_network_message {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(&payload_bytes)?;
            self.aleph_bft_network_message_requests
                .lock()
                .expect("router transport network-message requests lock")
                .push(signed_request);
            let delay = self.pop_aleph_bft_network_message_delay();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        let reorder_decision = if is_aleph_bft_network_message {
            self.aleph_bft_network_message_reorder_decision()
        } else {
            AlephBftNetworkMessageReorderDecision::None
        };
        let release_after_post = match reorder_decision {
            AlephBftNetworkMessageReorderDecision::Hold(wait_for_overtaking_post) => {
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), wait_for_overtaking_post).await;
                None
            }
            AlephBftNetworkMessageReorderDecision::ReleaseAfterPost(release_held_post) => {
                Some(release_held_post)
            }
            AlephBftNetworkMessageReorderDecision::None => None,
        };

        let response_body_result =
            router_transport_post_once_bytes(&router, &path, payload_bytes.clone()).await;
        if let Some(release_held_post) = release_after_post {
            if release_held_post.send(()).is_ok() {
                self.mark_aleph_bft_network_message_reorder_completed();
            }
        }
        let response_body = response_body_result?;
        if is_aleph_bft_network_message && self.take_aleph_bft_network_message_duplicate_replay() {
            let _ = router_transport_post_once_bytes(&router, &path, payload_bytes).await;
        }

        Ok(serde_json::from_slice(&response_body)?)
    }
}

async fn router_transport_post_once_bytes(
    router: &Router,
    path: &str,
    payload_bytes: Vec<u8>,
) -> Result<Vec<u8>, HttpError> {
    let request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri(path)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(payload_bytes))
        .map_err(|err| HttpError::Other(err.to_string()))?;
    let response = router
        .clone()
        .oneshot(request)
        .await
        .map_err(|err| HttpError::Other(err.to_string()))?;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|err| HttpError::Other(err.to_string()))?;
    if !status.is_success() {
        return Err(HttpError::Status {
            status: status.as_u16(),
            message: String::from_utf8_lossy(&body).to_string(),
        });
    }
    Ok(body.to_vec())
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

async fn response_error_detail(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8_lossy(&bytes).to_string()
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

fn transport_config_with_member_federation_api_urls(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let member_count = threshold_params.member_count;
    assert_eq!(usize::from(member_count), federation_api_urls.len());
    let members = federation_api_urls
        .into_iter()
        .enumerate()
        .map(|(index, federation_api_url)| {
            let value = u16::try_from(index + 1).expect("member index");
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                federation_api_url,
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[31 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=member_count)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
}

fn transport_config_five_with_threshold(
    threshold_params: ThresholdParams,
) -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let members = (1u16..=5)
        .map(|value| {
            TrustedDealerMember::new(
                member_id(value),
                MintUrl::from_str(&format!("https://mint{value}.example.com"))
                    .expect("public mint url"),
                MintUrl::from_str(&format!("https://fed{value}.example.com/api"))
                    .expect("federation api url"),
                SecretKey::from_slice(&[value as u8; 32]).expect("identity secret"),
            )
        })
        .collect::<Vec<_>>();
    let amount_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[21 + index as u8; 32]))
        .collect::<Vec<_>>();
    let keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), amount_coefficients)]),
    );
    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset])
        .expect("trusted dealer bundle");
    let private_configs = (1u16..=5)
        .map(|value| {
            bundle
                .private_configs
                .get(&member_id(value))
                .expect("private config")
                .clone()
        })
        .collect::<Vec<_>>();

    (bundle.public_config, private_configs)
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
        federation_aleph_bft_network_inbound: None,
        federation_aleph_bft_live_session: None,
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

fn signed_catch_up_request_with_kind(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        kind,
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

fn invalid_mint_quote_transcript_envelope(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationOperationEnvelope {
    let mut envelope = mint_quote_envelope(state, quote_id);
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("expected mint quote operation");
    };
    if let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response {
        response.quote = QuoteId::new().to_string();
    }
    envelope
}

fn non_v3_output_for_state(state: &MintState, label: &[u8]) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    BlindedMessage::new(
        Amount::from(1),
        Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        ),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
    )
}

fn invalid_mint_operation_keyset_envelope(state: &MintState) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![non_v3_output_for_state(state, b"invalid route mint output")],
                signature: None,
            }),
        },
    ))
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
        cdk::nuts::Id::from_str("00bfa73302d12ffd").expect("keyset id"),
        PublicKey::from_hex(label).expect("public key"),
    )
}

fn restore_envelope_for_state(
    state: &MintState,
    label: &'static str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    restore_envelope_for_config(config.public_config(), label)
}

fn restore_envelope_for_config(
    config: &FederationConfig,
    label: &'static str,
) -> FederationOperationEnvelope {
    restore_envelope_with_output(
        config.federation_id,
        config.keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
    )
}

fn indexed_restore_envelope_for_config(
    config: &FederationConfig,
    index: u64,
) -> FederationOperationEnvelope {
    restore_envelope_with_output(
        config.federation_id,
        config.keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation evidence restore {index}").as_bytes(),
        )),
    )
}

fn restore_envelope_with_output(
    federation_id: FederationId,
    keyset_id: Id,
    blinded_secret: PublicKey,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::Restore(cdk::federation::FederationRestoreOperation {
            request: RestoreRequest {
                outputs: vec![cdk::nuts::BlindedMessage::new(
                    Amount::from(1),
                    keyset_id,
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

#[tokio::test(start_paused = true)]
async fn rest_peer_client_applies_peer_request_timeout_to_aleph_bft_finalized_units() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let timeout = Duration::from_secs(2);
    let request = FederationAlephBftFinalizedUnitRequest::new(
        FederationAlephBftUnitPayload::new(
            federation_id,
            local_private.member_id,
            0,
            0,
            vec![FederationConsensusItem::operation(restore_envelope(
                federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f42",
            ))],
        )
        .expect("AlephBFT unit"),
    );
    let client = FederationRestPeerClient::new_with_config(
        DelayTransport {
            delay: timeout + Duration::from_secs(60),
        },
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            peer_request_timeout: timeout,
            operation_submission_timeout: Duration::from_secs(3600),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");

    let task = tokio::spawn(async move {
        client
            .submit_aleph_bft_finalized_unit(&member, &request)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(timeout).await;
    let err = task
        .await
        .expect("AlephBFT finalized-unit task")
        .expect_err("AlephBFT finalized-unit submission should time out");

    assert!(
        err.to_string()
            .contains("AlephBFT finalized-unit submission timed out"),
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
    let sender_federation_api_url =
        MintUrl::from_str("http://127.0.0.1:9").expect("sender federation api url");
    let receiver_federation_api_url =
        MintUrl::from_str("http://127.0.0.1:10").expect("receiver federation api url");
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

    let client = FederationRestPeerClient::new(
        RouterFederationTransport::new(router),
        &public_config,
        sender_private,
    )
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
    let (public_config, primary_private, local_private, witness_private) = transport_config_three();
    let federation_id = public_config.federation_id;
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let primary_member = public_config.member(member_id(1)).expect("primary member");
    let blinded_secret = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"non primary forwarded restore",
    ));
    let envelope =
        restore_envelope_with_output(federation_id, public_config.keysets[0].id, blinded_secret);
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
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
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
        .with_catch_up_response(catch_up_response)
        .with_catch_up_response(witness_response);

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
    let mut state = federation_route_state_for_private(public_config.clone(), local_private).await;
    let timeout = Duration::from_secs(2);
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: timeout,
        ..FederationPeerSyncConfig::default()
    });
    let envelope = restore_envelope_for_config(
        &public_config,
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
async fn non_primary_submit_rejects_invalid_quote_admission_before_forwarding() {
    let (public_config, _, local_private) = transport_config();
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let quote_id = QuoteId::new();
    let envelope = invalid_mint_quote_transcript_envelope(&state, &quote_id);
    let transport = MockFederationTransport::default();

    let err = submit_operation_to_primary_if_needed_with_transport(
        &state,
        envelope,
        None,
        transport.clone(),
    )
    .await
    .expect_err("invalid quote transcript should fail before forwarding");

    assert!(matches!(err, cdk::Error::UnknownQuote));
    assert!(transport.submitted_operations().is_empty());
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert!(
        entries.is_empty(),
        "invalid forwarded admission must not append locally"
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
    let first = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope_for_config(
        &public_config,
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
    let first = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope_for_config(
        &public_config,
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
    let first = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope_for_config(
        &public_config,
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
    let envelope = restore_envelope_for_state(
        &state,
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
async fn lagging_member_refuses_public_restore_and_checkstate_reads_until_healthy() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 1)
        .await;
    let y = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"lagging member checkstate read",
    ));
    let restore_request = match restore_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    )
    .operation
    {
        FederationOperation::Restore(operation) => operation.request,
        _ => unreachable!("helper returns restore operation"),
    };

    let check_error = crate::router_handlers::post_check(
        AuthHeader::None,
        State(state.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect_err("lagging member must not serve authoritative check-state");
    let check_body = response_error_detail(check_error).await;
    assert!(
        check_body.contains("lagging behind peer"),
        "check-state should fail closed on federation lag, got: {check_body}"
    );

    let restore_error = crate::router_handlers::post_restore(
        AuthHeader::None,
        State(state.clone()),
        Json(restore_request.clone()),
    )
    .await
    .expect_err("lagging member must not serve authoritative restore");
    let restore_body = response_error_detail(restore_error).await;
    assert!(
        restore_body.contains("lagging behind peer"),
        "restore should fail closed on federation lag, got: {restore_body}"
    );

    state.mark_federation_healthy().await;
    let _ = crate::router_handlers::post_check(
        AuthHeader::None,
        State(state.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect("healthy member can serve check-state");
    let _ =
        crate::router_handlers::post_restore(AuthHeader::None, State(state), Json(restore_request))
            .await
            .expect("healthy member can serve restore");
}

#[tokio::test]
async fn divergent_non_primary_refuses_public_submit_before_forwarding() {
    let (public_config, _, secondary_private) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), secondary_private).await;
    let primary_member = public_config.member(member_id(1)).expect("primary member");
    state
        .mark_federation_materialized_state_mismatch(primary_member.member_id, 1)
        .await;
    let envelope = restore_envelope_for_state(
        &state,
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
    let peer_entry = restore_envelope_for_state(
        &state,
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

    let next = restore_envelope_for_state(
        &state,
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
    let peer_entry = restore_envelope_for_state(
        &state,
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

    let next = restore_envelope_for_state(
        &state,
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
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = restore_envelope_for_config(
        &public_config,
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
        let entry = indexed_restore_envelope_for_config(&public_config, index);
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
    let entry = restore_envelope_for_config(
        &public_config,
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
        let entry = indexed_restore_envelope_for_config(&public_config, index);
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
        let entry = indexed_restore_envelope_for_config(&public_config, index);
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
    let peer_entry = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = restore_envelope_for_config(
        &public_config,
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
    status.aleph_bft_next_item_index = Some(0);
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
}

#[tokio::test]
async fn rest_peer_client_rejects_aleph_bft_status_without_finalized_cursor() {
    let (public_config, local_private, _) = transport_config();
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
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_state_digest(
        public_config.federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationStateDigest::genesis(),
    )
    .with_consensus_backend(FederationConsensusBackendKind::AlephBft);
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    let err = sync.expect_err("missing AlephBFT cursor should fail");
    assert!(
        err.to_string().contains("missing finalized item cursor"),
        "unexpected error: {err}"
    );
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
    let peer_entry = restore_envelope_for_config(
        &public_config,
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
async fn catch_up_marks_conflicting_swap_rejected_and_continues_replay() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first_input = proof_for_amount(&state, 1, "catch-up-conflict-first");
    let first_input_y = first_input.y().expect("first input y");
    let first_output = swap_output(&state, "catch-up-conflict-first");
    let first_request =
        cdk::nuts::SwapRequest::new(vec![first_input.clone()], vec![first_output.clone()]);
    let first_envelope = config
        .swap_operation(first_request.clone())
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "catch-up-conflict-loser");
    let conflicting_request =
        cdk::nuts::SwapRequest::new(vec![first_input], vec![conflicting_output.clone()]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");

    let later_input = proof_for_amount(&state, 1, "catch-up-conflict-later");
    let later_input_y = later_input.y().expect("later input y");
    let later_output = swap_output(&state, "catch-up-conflict-later");
    let later_request = cdk::nuts::SwapRequest::new(vec![later_input], vec![later_output.clone()]);
    let later_envelope = config
        .swap_operation(later_request.clone())
        .expect("later swap envelope");
    let later_operation_id = later_envelope.operation_id().expect("later operation id");

    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    for envelope in [first_envelope, conflicting_envelope, later_envelope] {
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted swap operation");
    }
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    let catch_up_response = peer_consensus
        .catch_up_checked(&local_consensus.lock().await.catch_up_request())
        .expect("catch-up response");
    assert_eq!(catch_up_response.entries.len(), 3);

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("catch-up should continue past deterministic rejected swap");

    assert_eq!(applied.len(), 3);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_proofs_states(&[first_input_y, later_input_y])
            .await
            .expect("proof state lookup"),
        vec![Some(cdk::nuts::State::Spent), Some(cdk::nuts::State::Spent)]
    );

    let first_response = config
        .swap_share_response(first_operation_id, &first_request)
        .expect("first expected share response");
    let later_response = config
        .swap_share_response(later_operation_id, &later_request)
        .expect("later expected share response");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_output.blinded_secret,
                conflicting_output.blinded_secret,
                later_output.blinded_secret,
            ])
            .await
            .expect("signature lookup"),
        vec![
            Some(first_response.signatures[0].clone()),
            None,
            Some(later_response.signatures[0].clone()),
        ]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(persisted[1].operation_id, conflicting_operation_id);
    assert_eq!(persisted[1].status, FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status, FederationOperationStatus::Applied);

    let runtime_consensus = local_consensus.lock().await;
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(first_operation_id)
            .expect("first runtime entry")
            .status,
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(conflicting_operation_id)
            .expect("conflicting runtime entry")
            .status,
        FederationOperationStatus::Rejected
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(later_operation_id)
            .expect("later runtime entry")
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
    let (mut state, public_config, _, peer_private) =
        federation_route_state_with_local_private().await;
    let peer_state =
        federation_route_state_for_private(public_config.clone(), peer_private.clone()).await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);

    let mint_quote_id = QuoteId::new();
    let minted_output = mint_output(&state, &mint_quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![minted_output.clone()],
        signature: None,
    });
    let mint_envelope = config
        .mint_operation(mint_input.clone())
        .expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");

    let swap_input = proof_for_amount(&state, 1, "offline-catch-up-swap-input");
    let swap_input_y = swap_input.y().expect("swap input y");
    let swapped_output = swap_output(&state, "offline-catch-up-swap-output");
    let swap_request = cdk::nuts::SwapRequest::new(vec![swap_input], vec![swapped_output.clone()]);
    let swap_envelope = config
        .swap_operation(swap_request.clone())
        .expect("swap envelope");
    let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");

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
    let melt_envelope = config
        .melt_operation(melt_request.clone())
        .expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    let melt_payment_envelope = melt_payment_observation_envelope(
        &state,
        &melt_quote_id,
        MeltQuoteState::Paid,
        Some("offline-catch-up-preimage".to_string()),
    );

    for envelope in [
        mint_quote_envelope(&state, &mint_quote_id),
        paid_observation_envelope(&state, &mint_quote_id),
        mint_envelope.clone(),
        swap_envelope.clone(),
        melt_quote_envelope,
        melt_envelope.clone(),
    ] {
        peer_state
            .submit_federation_envelope_with_sequence(envelope.clone(), None)
            .await
            .expect("peer accepted operation");
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted operation");
    }

    let local_consensus = state.federation_consensus.as_ref().expect("consensus");
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        0
    );
    let catch_up_response = catch_up_response_with_materialized_digest(
        &peer_state,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .await;

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply offline catch-up");

    assert_eq!(applied.len(), 6);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .peer_status()
            .latest_order_digest,
        peer_consensus.peer_status().latest_order_digest
    );

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("restart federation runtime from durable catch-up state");
    state.federation_consensus = replayed_consensus;
    state.federation_sequences = replayed_sequences;
    let local_consensus = state.federation_consensus.as_ref().expect("consensus");

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 6);
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

    let duplicate_mint = state
        .submit_federation_envelope(mint_envelope.clone())
        .await
        .expect("duplicate mint after catch-up");
    assert!(matches!(
        duplicate_mint.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let duplicate_mint_response = state
        .mint
        .persisted_mint_share_response(mint_operation_id, &mint_input)
        .await
        .expect("persisted duplicate mint share response");
    assert_eq!(duplicate_mint_response.signatures.len(), 1);

    let duplicate_swap = state
        .submit_federation_envelope(swap_envelope.clone())
        .await
        .expect("duplicate swap after catch-up");
    assert!(matches!(
        duplicate_swap.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let duplicate_swap_response = state
        .mint
        .persisted_swap_share_response(swap_operation_id, &swap_request)
        .await
        .expect("persisted duplicate swap share response");
    assert_eq!(duplicate_swap_response.signatures.len(), 1);

    let duplicate_pending_melt = state
        .submit_federation_envelope(melt_envelope.clone())
        .await
        .expect("duplicate pending melt after catch-up");
    assert!(matches!(
        duplicate_pending_melt.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let melt_quote = state
        .mint
        .localstore()
        .get_melt_quote(&melt_quote_id)
        .await
        .expect("melt quote lookup")
        .expect("melt quote persisted");
    assert_eq!(melt_quote.state, MeltQuoteState::Pending);
    assert!(melt_quote.payment_proof.is_none());
    let melt_states = state
        .mint
        .localstore()
        .get_proofs_states(&melt_input_ys)
        .await
        .expect("pending melt state lookup");
    assert_eq!(
        melt_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );
    let pending_change_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[melt_change_output.blinded_secret])
        .await
        .expect("pending melt change signature lookup");
    assert!(pending_change_signatures[0].is_none());

    peer_state
        .submit_federation_envelope_with_sequence(melt_payment_envelope.clone(), None)
        .await
        .expect("peer accepted delayed paid melt observation");
    peer_consensus
        .submit_envelope(melt_payment_envelope)
        .expect("peer accepted delayed paid melt observation");
    let delayed_paid_response = catch_up_response_with_materialized_digest(
        &peer_state,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .await;
    let delayed_applied = state
        .apply_federation_catch_up_response(&delayed_paid_response)
        .await
        .expect("apply delayed paid melt catch-up");
    assert_eq!(delayed_applied.len(), 1);

    let melt_quote = state
        .mint
        .localstore()
        .get_melt_quote(&melt_quote_id)
        .await
        .expect("melt quote lookup after paid observation")
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
    let duplicate_melt_response = state
        .mint
        .melt_federation(&melt_request, melt_operation_id)
        .await
        .expect("duplicate final melt response after delayed catch-up");
    assert_eq!(duplicate_melt_response.state(), MeltQuoteState::Paid);
    let duplicate_change = duplicate_melt_response
        .change()
        .expect("duplicate final melt change signatures");
    assert_eq!(duplicate_change.len(), 1);

    let local_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("local journal lookup after delayed catch-up");
    let peer_entries = peer_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal lookup after delayed catch-up");
    assert_eq!(local_entries.len(), 7);
    assert_eq!(peer_entries.len(), 7);
    let local_digest = materialized_state_digest(&state, &local_entries)
        .await
        .expect("local materialized digest");
    let peer_digest = materialized_state_digest(&peer_state, &peer_entries)
        .await
        .expect("peer materialized digest");
    assert_eq!(local_digest, peer_digest);

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
    let peer_entry = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = restore_envelope_for_config(
        &public_config,
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
    let envelope = restore_envelope_for_config(
        &public_config,
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
async fn router_startup_replays_aleph_bft_finalized_items_before_serving_local_status() {
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

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT router startup transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist matching journal entry");
    tx.commit()
        .await
        .expect("commit AlephBFT router startup state");
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before router startup")
        .is_none());

    let router =
        crate::create_mint_router(state.mint.clone(), vec![KnownMethod::Bolt11.to_string()])
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
    assert!(!local_status.sync_ready);
    assert_eq!(local_status.sync_state, "verifying_session_outcome");
    assert!(
        local_status
            .sync_message
            .contains("verifying quorum session outcome evidence"),
        "unexpected sync message: {}",
        local_status.sync_message
    );
    assert!(local_status.aleph_bft_live_session_active);
    assert!(local_status.aleph_bft_network_inbound_active);
    let aleph_bft_status = local_status
        .aleph_bft_status
        .expect("AlephBFT local status");
    assert_eq!(
        aleph_bft_status.finality,
        FederationAlephBftFinalityMode::NetworkFinalityAvailable
    );
    assert_eq!(aleph_bft_status.accepted_operation_count, 1);
    assert!(
        local_status
            .materialized_state_rows
            .iter()
            .any(|row| row.table == "mint_quotes"),
        "router startup should materialize AlephBFT finalized mint quote before serving status"
    );

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after router startup")
        .expect("mint quote replayed from finalized items");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(replayed_entries.len(), 1);
    assert_eq!(replayed_entries[0].operation_id, operation_id);
    assert_eq!(
        replayed_entries[0].status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_replays_finalized_items_and_applies_entries() {
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
    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_outcome = signed_catch_up_response(base_response, &local_private)
        .session_outcome
        .expect("witness session outcome");

    let applied = state
        .apply_federation_catch_up_response_with_session_outcome_quorum(
            &response,
            &[witness_outcome],
        )
        .await
        .expect("apply AlephBFT catch-up");

    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].operation_id, operation_id);
    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after catch-up")
        .expect("mint quote replayed from catch-up evidence");
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
    let stored_outcomes = state
        .mint
        .localstore()
        .get_federation_session_outcomes()
        .await
        .expect("stored session outcomes");
    assert_eq!(stored_outcomes.len(), 2);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .aleph_bft_status()
            .expect("status")
            .next_item_index,
        1
    );
    assert_eq!(
        consensus.journal().entries()[0].status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_checkpoints_rejected_trailing_entry() {
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
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let input = proof_for_amount(&state, 1, "aleph-catch-up-trailing-rejected");
    let first_output = swap_output(&state, "aleph-catch-up-trailing-rejected-winner");
    let first_request = cdk::nuts::SwapRequest::new(vec![input.clone()], vec![first_output]);
    let first_envelope = config
        .swap_operation(first_request)
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let conflicting_output = swap_output(&state, "aleph-catch-up-trailing-rejected-loser");
    let conflicting_request = cdk::nuts::SwapRequest::new(vec![input], vec![conflicting_output]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");

    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_envelope),
            FederationConsensusItem::operation(conflicting_envelope),
        ],
    )
    .expect("AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_outcome = signed_catch_up_response(base_response, &local_private)
        .session_outcome
        .expect("witness session outcome");

    let applied = state
        .apply_federation_catch_up_response_with_session_outcome_quorum(
            &response,
            &[witness_outcome],
        )
        .await
        .expect("apply AlephBFT catch-up");
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0].operation_id, first_operation_id);
    assert_eq!(applied[1].operation_id, conflicting_operation_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status, FederationOperationStatus::Rejected);
    let expected_state_digest = materialized_state_digest(&state, &persisted)
        .await
        .expect("materialized state digest");

    let checkpoints = state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].operation_index, persisted[1].index);
    assert_eq!(checkpoints[0].operation_id, conflicting_operation_id);
    assert_eq!(checkpoints[0].order_digest, persisted[1].order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);
}

#[tokio::test]
async fn aleph_bft_peer_sync_catches_up_finalized_cursor_without_new_operations() {
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
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first AlephBFT unit");

    state
        .apply_federation_aleph_bft_finalized_unit(first_unit.clone())
        .await
        .expect("local first finalized unit");
    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    peer_adapter
        .finalize_unit(first_unit)
        .expect("peer first finalized unit");
    let duplicate_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("duplicate AlephBFT unit");
    peer_adapter
        .finalize_unit(duplicate_unit)
        .expect("peer duplicate finalized unit");
    assert_eq!(peer_adapter.journal().entries().len(), 1);
    assert_eq!(peer_adapter.finalized_items().len(), 2);

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    assert_eq!(request.next_index, 1);
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        50,
        100,
    );
    response.state_digest = materialized_state_digest(&state, peer_adapter.journal().entries())
        .await
        .expect("materialized state digest");
    let response =
        response.with_aleph_bft_finalized_items(peer_adapter.finalized_items()[1..].to_vec(), true);
    let transport = MockFederationTransport::default()
        .with_status_response(
            peer_adapter.peer_status(public_config.federation_id, peer_private.member_id),
        )
        .with_catch_up_response(response);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("client");

    client
        .sync_once_from_member(&state, peer_member)
        .await
        .expect("sync finalized cursor");

    let requests = transport.catch_up_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].payload.next_index, 1);
    assert_eq!(requests[0].payload.aleph_bft_next_item_index, 1);
    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 2);
    assert_eq!(stored_finalized_items[1].operation_id, Some(operation_id));
    let stored_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("stored journal entries");
    assert_eq!(stored_entries.len(), 1);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .aleph_bft_status()
            .expect("AlephBFT status")
            .next_item_index,
        2
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_rejects_journal_entries_without_finalized_item_evidence() {
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
    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        50,
        100,
    );
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_outcome = signed_catch_up_response(base_response, &local_private)
        .session_outcome
        .expect("witness session outcome");

    let err = state
        .apply_federation_catch_up_response_with_session_outcome_quorum(
            &response,
            &[witness_outcome],
        )
        .await
        .expect_err("journal-only AlephBFT catch-up should fail");
    assert!(
        err.to_string().contains("missing finalized-item evidence"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected catch-up")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after rejected catch-up")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after rejected catch-up")
        .is_empty());
}

#[tokio::test]
async fn aleph_bft_catch_up_rejects_incomplete_finalized_unit_page_without_mutation() {
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
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote_id)),
            FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote_id)),
        ],
    )
    .expect("two-item AlephBFT unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request()
        .with_limit(1);
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        1,
        100,
    );
    response.state_digest = materialized_state_digest(&state, peer_adapter.journal().entries())
        .await
        .expect("materialized state digest");
    response = response.with_aleph_bft_finalized_items(Vec::new(), false);
    let response = signed_catch_up_response(response.clone(), &peer_private);
    let witness_outcome = signed_catch_up_response(response.clone(), &local_private)
        .session_outcome
        .expect("witness session outcome");

    let err = state
        .apply_federation_catch_up_response_with_session_outcome_quorum(
            &response,
            &[witness_outcome],
        )
        .await
        .expect_err("incomplete finalized unit page should fail");
    assert!(
        err.to_string()
            .contains("AlephBFT catch-up made no finalized-item progress"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("first quote lookup after rejected catch-up")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&second_quote_id)
        .await
        .expect("second quote lookup after rejected catch-up")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after rejected catch-up")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after rejected catch-up")
        .is_empty());
}

#[tokio::test]
async fn aleph_bft_catch_up_rejects_reordered_finalized_items_without_mutation() {
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
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let mut peer_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote_id)),
            FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote_id)),
        ],
    )
    .expect("two-item AlephBFT unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");
    let mut reordered_finalized_items = peer_adapter.finalized_items().to_vec();
    reordered_finalized_items.swap(0, 1);

    let request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        &request,
        50,
        100,
    );
    response.state_digest = materialized_state_digest(&state, peer_adapter.journal().entries())
        .await
        .expect("materialized state digest");
    response = response.with_aleph_bft_finalized_items(reordered_finalized_items, true);
    let response = signed_catch_up_response(response.clone(), &peer_private);
    let witness_outcome = signed_catch_up_response(response.clone(), &local_private)
        .session_outcome
        .expect("witness session outcome");

    let err = state
        .apply_federation_catch_up_response_with_session_outcome_quorum(
            &response,
            &[witness_outcome],
        )
        .await
        .expect_err("reordered finalized items should fail");
    assert!(
        err.to_string()
            .contains("non-contiguous accepted item index"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("first quote lookup after reordered catch-up")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&second_quote_id)
        .await
        .expect("second quote lookup after reordered catch-up")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after reordered catch-up")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after reordered catch-up")
        .is_empty());
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
async fn federation_aleph_bft_finalized_unit_reports_state_rejected_swap() {
    let (public_config, local_private, _) = transport_config();
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
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let first_input = proof_for_amount(&state, 1, "aleph-conflict-first");
    let first_output = swap_output(&state, "aleph-conflict-first");
    let first_request =
        cdk::nuts::SwapRequest::new(vec![first_input.clone()], vec![first_output.clone()]);
    let first_envelope = config
        .swap_operation(first_request.clone())
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "aleph-conflict-loser");
    let conflicting_request =
        cdk::nuts::SwapRequest::new(vec![first_input], vec![conflicting_output.clone()]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");

    let later_input = proof_for_amount(&state, 1, "aleph-conflict-later");
    let later_output = swap_output(&state, "aleph-conflict-later");
    let later_request = cdk::nuts::SwapRequest::new(vec![later_input], vec![later_output.clone()]);
    let later_envelope = config
        .swap_operation(later_request.clone())
        .expect("later swap envelope");
    let later_operation_id = later_envelope.operation_id().expect("later operation id");

    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_envelope),
            FederationConsensusItem::operation(conflicting_envelope),
            FederationConsensusItem::operation(later_envelope),
        ],
    )
    .expect("AlephBFT finalized unit");
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(conflicting_operation_id)
            .await
            .expect("notified conflicting operation")
    });
    tokio::task::yield_now().await;

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].operation_id, first_operation_id);
    assert_eq!(
        responses[0].outcome.entry().status,
        FederationOperationStatus::Applied
    );
    assert_eq!(responses[1].operation_id, conflicting_operation_id);
    assert_eq!(
        responses[1].outcome.entry().status,
        FederationOperationStatus::Rejected
    );
    assert_eq!(responses[2].operation_id, later_operation_id);
    assert_eq!(
        responses[2].outcome.entry().status,
        FederationOperationStatus::Applied
    );
    let notified = waiter.await.expect("waiter task");
    assert_eq!(notified.operation_id, conflicting_operation_id);
    assert_eq!(
        notified.outcome.entry().status,
        FederationOperationStatus::Rejected
    );

    let first_response = config
        .swap_share_response(first_operation_id, &first_request)
        .expect("first expected share response");
    let later_response = config
        .swap_share_response(later_operation_id, &later_request)
        .expect("later expected share response");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_output.blinded_secret,
                conflicting_output.blinded_secret,
                later_output.blinded_secret,
            ])
            .await
            .expect("signature lookup"),
        vec![
            Some(first_response.signatures[0].clone()),
            None,
            Some(later_response.signatures[0].clone()),
        ]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status, FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status, FederationOperationStatus::Applied);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_checkpoints_rejected_trailing_entry() {
    let (public_config, local_private, _) = transport_config();
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
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let input = proof_for_amount(&state, 1, "aleph-trailing-rejected");
    let first_output = swap_output(&state, "aleph-trailing-rejected-winner");
    let first_request = cdk::nuts::SwapRequest::new(vec![input.clone()], vec![first_output]);
    let first_envelope = config
        .swap_operation(first_request)
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "aleph-trailing-rejected-loser");
    let conflicting_request = cdk::nuts::SwapRequest::new(vec![input], vec![conflicting_output]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_envelope),
            FederationConsensusItem::operation(conflicting_envelope),
        ],
    )
    .expect("AlephBFT finalized unit");

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    assert_eq!(
        responses
            .iter()
            .map(|response| response.outcome.entry().status)
            .collect::<Vec<_>>(),
        vec![
            FederationOperationStatus::Applied,
            FederationOperationStatus::Rejected
        ]
    );
    assert_eq!(responses[0].operation_id, first_operation_id);
    assert_eq!(responses[1].operation_id, conflicting_operation_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status, FederationOperationStatus::Rejected);
    let expected_state_digest = materialized_state_digest(&state, &persisted)
        .await
        .expect("materialized state digest");

    let checkpoints = state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].operation_index, persisted[1].index);
    assert_eq!(checkpoints[0].operation_id, conflicting_operation_id);
    assert_eq!(checkpoints[0].order_digest, persisted[1].order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_units_serialize_cross_member_value_conflicts() {
    let (public_config, first_private, second_private) = transport_config();
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let config = first_state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();

    let mint_quote_id = QuoteId::new();
    let first_mint_output = mint_output(&first_state, &mint_quote_id);
    let conflicting_mint_output = swap_output(&first_state, "aleph-cross-member-mint-loser");
    let first_mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![first_mint_output.clone()],
        signature: None,
    });
    let conflicting_mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![conflicting_mint_output.clone()],
        signature: None,
    });
    let first_mint_envelope = config
        .mint_operation(first_mint_input)
        .expect("first mint envelope");
    let first_mint_operation_id = first_mint_envelope
        .operation_id()
        .expect("first mint operation id");
    let conflicting_mint_envelope = config
        .mint_operation(conflicting_mint_input)
        .expect("conflicting mint envelope");
    let conflicting_mint_operation_id = conflicting_mint_envelope
        .operation_id()
        .expect("conflicting mint operation id");

    let swap_input = proof_for_amount(&first_state, 1, "aleph-cross-member-swap-input");
    let swap_input_y = swap_input.y().expect("swap input y");
    let first_swap_output = swap_output(&first_state, "aleph-cross-member-swap-winner");
    let conflicting_swap_output = swap_output(&first_state, "aleph-cross-member-swap-loser");
    let first_swap_request =
        cdk::nuts::SwapRequest::new(vec![swap_input.clone()], vec![first_swap_output.clone()]);
    let conflicting_swap_request =
        cdk::nuts::SwapRequest::new(vec![swap_input], vec![conflicting_swap_output.clone()]);
    let first_swap_envelope = config
        .swap_operation(first_swap_request)
        .expect("first swap envelope");
    let first_swap_operation_id = first_swap_envelope
        .operation_id()
        .expect("first swap operation id");
    let conflicting_swap_envelope = config
        .swap_operation(conflicting_swap_request)
        .expect("conflicting swap envelope");
    let conflicting_swap_operation_id = conflicting_swap_envelope
        .operation_id()
        .expect("conflicting swap operation id");

    let melt_quote_id = QuoteId::new();
    let melt_quote_envelope = config.melt_quote_operation(
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "aleph cross-member melt".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_inputs = vec![
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-a"),
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-b"),
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-c"),
    ];
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("melt input y"))
        .collect::<Vec<_>>();
    let mut first_melt_change_output = swap_output(&first_state, "aleph-cross-member-melt-winner");
    first_melt_change_output.amount = Amount::ZERO;
    let mut conflicting_melt_change_output =
        swap_output(&first_state, "aleph-cross-member-melt-loser");
    conflicting_melt_change_output.amount = Amount::ZERO;
    let first_melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs.clone(),
        Some(vec![first_melt_change_output.clone()]),
    );
    let conflicting_melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs,
        Some(vec![conflicting_melt_change_output.clone()]),
    );
    let first_melt_envelope = config
        .melt_operation(first_melt_request)
        .expect("first melt envelope");
    let first_melt_operation_id = first_melt_envelope
        .operation_id()
        .expect("first melt operation id");
    let conflicting_melt_envelope = config
        .melt_operation(conflicting_melt_request)
        .expect("conflicting melt envelope");
    let conflicting_melt_operation_id = conflicting_melt_envelope
        .operation_id()
        .expect("conflicting melt operation id");

    let first_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        first_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&first_state, &mint_quote_id)),
            FederationConsensusItem::operation(paid_observation_envelope(
                &first_state,
                &mint_quote_id,
            )),
            FederationConsensusItem::operation(first_mint_envelope),
            FederationConsensusItem::operation(first_swap_envelope),
            FederationConsensusItem::operation(melt_quote_envelope),
            FederationConsensusItem::operation(first_melt_envelope),
        ],
    )
    .expect("first AlephBFT finalized unit");
    let second_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        second_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(conflicting_mint_envelope),
            FederationConsensusItem::operation(conflicting_swap_envelope),
            FederationConsensusItem::operation(conflicting_melt_envelope),
        ],
    )
    .expect("second AlephBFT finalized unit");

    for state in [&first_state, &second_state] {
        let first_responses = state
            .apply_federation_aleph_bft_finalized_unit(first_unit.clone())
            .await
            .expect("first finalized unit");
        assert_eq!(
            first_responses
                .iter()
                .map(|response| response.outcome.entry().status)
                .collect::<Vec<_>>(),
            vec![FederationOperationStatus::Applied; 6]
        );
        assert_eq!(first_responses[2].operation_id, first_mint_operation_id);
        assert_eq!(first_responses[3].operation_id, first_swap_operation_id);
        assert_eq!(first_responses[5].operation_id, first_melt_operation_id);

        let second_responses = state
            .apply_federation_aleph_bft_finalized_unit(second_unit.clone())
            .await
            .expect("second finalized unit");
        assert_eq!(
            second_responses
                .iter()
                .map(|response| response.outcome.entry().status)
                .collect::<Vec<_>>(),
            vec![FederationOperationStatus::Rejected; 3]
        );
        assert_eq!(
            second_responses[0].operation_id,
            conflicting_mint_operation_id
        );
        assert_eq!(
            second_responses[1].operation_id,
            conflicting_swap_operation_id
        );
        assert_eq!(
            second_responses[2].operation_id,
            conflicting_melt_operation_id
        );

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&mint_quote_id)
            .await
            .expect("mint quote lookup")
            .expect("mint quote");
        assert_eq!(quote.state(), MintQuoteState::Issued);
        assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));

        let melt_quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(melt_quote.state, MeltQuoteState::Paid);

        let swap_states = state
            .mint
            .localstore()
            .get_proofs_states(&[swap_input_y])
            .await
            .expect("swap proof state lookup");
        assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
        let melt_states = state
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("melt proof state lookup");
        assert_eq!(
            melt_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_mint_output.blinded_secret,
                conflicting_mint_output.blinded_secret,
                first_swap_output.blinded_secret,
                conflicting_swap_output.blinded_secret,
                first_melt_change_output.blinded_secret,
                conflicting_melt_change_output.blinded_secret,
            ])
            .await
            .expect("signature lookup");
        assert!(signatures[0].is_some());
        assert!(signatures[1].is_none());
        assert!(signatures[2].is_some());
        assert!(signatures[3].is_none());
        assert!(signatures[4].is_some());
        assert!(signatures[5].is_none());

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 9);
        assert_eq!(
            persisted
                .iter()
                .map(|entry| entry.status)
                .collect::<Vec<_>>(),
            vec![
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Rejected,
                FederationOperationStatus::Rejected,
                FederationOperationStatus::Rejected,
            ]
        );
    }

    let first_entries = first_state
        .federation_consensus
        .as_ref()
        .expect("first consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation_consensus
        .as_ref()
        .expect("second consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first materialized digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second materialized digest");
    assert_eq!(first_digest, second_digest);
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
async fn aleph_bft_live_session_startup_decision_spawns_only_for_aleph_bft_backend() {
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
        federation_aleph_bft_live_session_startup_decision(&in_memory_state)
            .await
            .expect("in-memory decision"),
        FederationAlephBftLiveSessionStartupDecision::NonAlephBftBackend {
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
        federation_aleph_bft_live_session_startup_decision(&aleph_bft_state)
            .await
            .expect("AlephBFT decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 0,
        }
    );
}

#[tokio::test]
async fn aleph_bft_live_session_start_installs_inbound_queue_and_marks_network_finality() {
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

    let handle = start_federation_aleph_bft_live_session_with_config(
        &mut state,
        FederationPeerSyncConfig::default(),
        CdkAlephBftNetworkPumpConfig::default(),
    )
    .await
    .expect("start live session")
    .expect("live session handle");

    assert!(state.federation_aleph_bft_network_inbound.is_some());
    assert!(state.federation_aleph_bft_live_session.is_some());
    assert!(state.federation_aleph_bft_network_inbound_active());
    assert!(state.federation_aleph_bft_live_session_active());
    assert!(Arc::ptr_eq(
        &handle,
        state
            .federation_aleph_bft_live_session
            .as_ref()
            .expect("stored live session handle")
    ));
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("AlephBFT status")
            .finality,
        FederationAlephBftFinalityMode::NetworkFinalityAvailable
    );
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("already-started decision"),
        FederationAlephBftLiveSessionStartupDecision::AlreadyStarted
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal")
        .is_empty());
}

#[tokio::test]
async fn aleph_bft_live_session_persists_and_recovers_empty_active_session() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .begin_aleph_bft_session(1)
        .expect("begin empty session one");

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
    );
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let stale_pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        stale_pump_config,
        transport,
    )
    .await
    .expect("start empty session")
    .expect("live session handle");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_active_session()
            .await
            .expect("persisted active session"),
        Some(1)
    );
    drop(handle);

    let mut restarted_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let (consensus, sequences) = crate::create_federation_runtime_state(&restarted_state.mint)
        .await
        .expect("recover active empty session");
    restarted_state.federation_consensus = consensus;
    restarted_state.federation_sequences = sequences;
    let status = restarted_state
        .federation_aleph_bft_status()
        .await
        .expect("recovered AlephBFT status");
    assert_eq!(status.session, 1);
    assert_eq!(status.finalized_item_count, 0);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&restarted_state)
            .await
            .expect("startup decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 1,
        }
    );
}

#[tokio::test]
async fn aleph_bft_live_session_uses_recovered_session_for_network_pump_messages() {
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

    let initial_quote_id = QuoteId::new();
    let initial_envelope = mint_quote_envelope(&state, &initial_quote_id);
    let mut adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        local_private.member_id,
        1,
        public_config.members.keys().copied(),
    )
    .expect("session one AlephBFT adapter");
    let recovered_session_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        1,
        0,
        vec![FederationConsensusItem::operation(initial_envelope)],
    )
    .expect("session one finalized unit");
    adapter
        .finalize_unit(recovered_session_unit)
        .expect("finalize recovered session unit");
    let entry = adapter.journal().entries()[0].clone();
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin session recovery transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist session one finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist session one journal entry");
    tx.commit().await.expect("commit session recovery state");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("recover session one runtime");
    state.federation_consensus = consensus;
    state.federation_sequences = sequences;
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("recovered AlephBFT status")
            .session,
        1
    );
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("session one startup decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 1,
        }
    );

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
    );
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let stale_pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        stale_pump_config,
        transport.clone(),
    )
    .await
    .expect("start recovered session live session")
    .expect("live session handle");

    let next_envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(next_envelope)
        .expect("queue session one operation");
    let requests = time::timeout(Duration::from_secs(5), async {
        loop {
            let requests = transport.aleph_bft_network_message_requests();
            if !requests.is_empty() {
                return requests;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("session one network-message request");

    assert!(
        requests
            .iter()
            .all(|request| request.payload.session_id == 1),
        "live network pump must derive session from recovered consensus status"
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("session one network-message request verifies");
        assert_eq!(request.sender, local_private.member_id);
    }
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_operation_over_private_rest_network_messages() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    assert_eq!(
        first_state
            .federation_aleph_bft_status()
            .await
            .expect("first AlephBFT status")
            .finalized_item_count,
        1
    );
    assert_eq!(
        second_state
            .federation_aleph_bft_status()
            .await
            .expect("second AlephBFT status")
            .finalized_item_count,
        1
    );
    assert!(first_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("first quote lookup")
        .is_some());
    assert!(second_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("second quote lookup")
        .is_some());
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_with_duplicate_and_delayed_private_rest_messages() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default()
        .with_aleph_bft_network_message_duplicate_replays(16)
        .with_aleph_bft_network_message_delays([
            Duration::from_millis(45),
            Duration::ZERO,
            Duration::from_millis(20),
            Duration::ZERO,
            Duration::from_millis(35),
            Duration::ZERO,
        ]);
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("live AlephBFT operation response under transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_rejects_malformed_signed_network_message_without_consuming_sequence(
) {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let malformed_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        second_private.member_id,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, vec![1, 2, 3]),
        &first_private,
    )
    .expect("signed malformed AlephBFT network-message request");
    let malformed_url = url::Url::parse(&format!("{second_url}/federation/v1/aleph-bft/messages"))
        .expect("malformed AlephBFT network-message URL");
    let malformed_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(malformed_url, None, &malformed_request)
        .await;
    let malformed_error = malformed_result.expect_err("malformed network message must fail");
    match malformed_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("Invalid federation AlephBFT network message"),
                "unexpected malformed-message error: {message}"
            );
        }
        err => panic!("unexpected malformed network-message error: {err:?}"),
    }
    assert_eq!(
        second_state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("sequences after malformed network message")
            .get(&first_private.member_id),
        None,
        "a rejected malformed network message must not consume the sender sequence"
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("live AlephBFT operation response after malformed message");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
    }
    assert!(
        second_state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("sequences after valid finality")
            .get(&first_private.member_id)
            .is_some(),
        "valid post-rejection network traffic should still advance the sender sequence"
    );
}

#[tokio::test]
async fn aleph_bft_live_session_rejects_same_sequence_equivocated_network_message() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("live AlephBFT operation response before equivocation");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    let accepted_request = wait_for_aleph_bft_network_message_request(
        &transport,
        first_private.member_id,
        second_private.member_id,
    )
    .await;
    accepted_request
        .verify(&public_config, accepted_request.receiver)
        .expect("captured network-message request verifies");
    let sequence_before = second_state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences before equivocated network message")
        .get(&first_private.member_id)
        .copied()
        .expect("sender sequence before equivocated network message");
    assert!(
        sequence_before >= accepted_request.sequence,
        "receiver must have accepted the captured request before equivocation"
    );

    let mut equivocated_message = accepted_request
        .payload
        .message_bytes()
        .expect("captured AlephBFT message bytes");
    equivocated_message.push(0);
    let equivocated_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        second_private.member_id,
        accepted_request.sequence,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, equivocated_message),
        &first_private,
    )
    .expect("signed equivocated AlephBFT network-message request");
    assert_ne!(
        equivocated_request.payload.encoded_message,
        accepted_request.payload.encoded_message
    );
    let equivocated_url =
        url::Url::parse(&format!("{second_url}/federation/v1/aleph-bft/messages"))
            .expect("equivocated AlephBFT network-message URL");
    let equivocated_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(equivocated_url, None, &equivocated_request)
        .await;
    let equivocated_error =
        equivocated_result.expect_err("same-sequence equivocated network message must fail");
    match equivocated_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("stale sequence"),
                "unexpected equivocation error: {message}"
            );
        }
        err => panic!("unexpected equivocated network-message error: {err:?}"),
    }
    assert_eq!(
        second_state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("sequences after equivocated network message")
            .get(&first_private.member_id)
            .copied(),
        Some(sequence_before)
    );
    for state in [&first_state, &second_state] {
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries after equivocated network message")
                .len(),
            1
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after equivocated network message")
                .finalized_item_count,
            1
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_with_reordered_private_rest_messages() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport =
        RouterFederationTransportRegistry::default().with_aleph_bft_network_message_reorder_once();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("live AlephBFT operation response under reordered transport");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    assert_eq!(transport.aleph_bft_network_message_reorders_completed(), 1);
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
    }
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_finalize_with_reorder_duplicate_and_delay_faults() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1))
        .with_aleph_bft_network_message_reorder_once()
        .with_aleph_bft_network_message_duplicate_replays(32)
        .with_aleph_bft_network_message_delays([
            Duration::from_millis(45),
            Duration::ZERO,
            Duration::from_millis(25),
            Duration::from_millis(10),
            Duration::ZERO,
            Duration::from_millis(35),
            Duration::ZERO,
            Duration::from_millis(15),
        ]);
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 5);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = states[0]
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("five-member AlephBFT operation response under combined transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    assert_eq!(transport.aleph_bft_network_message_reorders_completed(), 1);
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after combined faults")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_reject_same_sequence_equivocation_and_continue() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 5);

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = states[0]
        .submit_federation_envelope_with_sequence(first_envelope, None)
        .await
        .expect("first five-member AlephBFT operation response");

    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, first_operation_id).await;
    }

    let accepted_request = wait_for_aleph_bft_network_message_request(
        &transport,
        private_configs[0].member_id,
        private_configs[1].member_id,
    )
    .await;
    accepted_request
        .verify(&public_config, accepted_request.receiver)
        .expect("captured five-member network-message request verifies");
    let receiver_sequence_before = states[1]
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("receiver sequences before equivocated network message")
        .get(&private_configs[0].member_id)
        .copied()
        .expect("sender sequence before equivocated network message");
    assert!(
        receiver_sequence_before >= accepted_request.sequence,
        "receiver must have accepted the captured request before equivocation"
    );

    let mut equivocated_message = accepted_request
        .payload
        .message_bytes()
        .expect("captured AlephBFT message bytes");
    equivocated_message.push(0);
    let equivocated_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        private_configs[1].member_id,
        accepted_request.sequence,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, equivocated_message),
        &private_configs[0],
    )
    .expect("signed five-member equivocated AlephBFT network-message request");
    assert_ne!(
        equivocated_request.payload.encoded_message,
        accepted_request.payload.encoded_message
    );
    let equivocated_url = url::Url::parse(&format!(
        "{}/federation/v1/aleph-bft/messages",
        federation_api_urls[1]
    ))
    .expect("equivocated AlephBFT network-message URL");
    let equivocated_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(equivocated_url, None, &equivocated_request)
        .await;
    let equivocated_error = equivocated_result
        .expect_err("five-member same-sequence equivocated network message must fail");
    match equivocated_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("stale sequence"),
                "unexpected five-member equivocation error: {message}"
            );
        }
        err => panic!("unexpected five-member equivocated network-message error: {err:?}"),
    }
    assert_eq!(
        states[1]
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("receiver sequences after equivocated network message")
            .get(&private_configs[0].member_id)
            .copied(),
        Some(receiver_sequence_before)
    );
    for state in &states {
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries after equivocated network message")
                .len(),
            1
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after equivocated network message")
                .finalized_item_count,
            1
        );
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&states[2], &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = states[2]
        .submit_federation_envelope_with_sequence(second_envelope, None)
        .await
        .expect("second five-member AlephBFT operation response after equivocation");

    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal entries after second operation");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after second operation")
                .finalized_item_count,
            2
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_with_unavailable_member_stall_safely_without_durable_mutation() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs.into_iter().take(4) {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 4);

    states[0].federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect_err("unavailable live member should leave operation pending");

    assert_pending_finalization_error(&err, operation_id, 250);
    for state in &states {
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            0
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_after_unavailable_member_returns() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }

    states[0].federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect_err("unavailable member should leave operation pending first");
    assert_pending_finalization_error(&err, operation_id, 250);

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut states[4],
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    states[0].federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    });
    let retry_envelope = mint_quote_envelope(&states[0], &quote_id);
    let response = states[0]
        .submit_federation_envelope_with_sequence(retry_envelope, None)
        .await
        .expect("returned member should allow live finality");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after finality")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_multiple_pending_operations_after_long_unavailable_window(
) {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }

    states[0].federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let mut pending_operations = Vec::new();
    for _ in 0..3 {
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&states[0], &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let err = states[0]
            .submit_federation_envelope_with_sequence(envelope, None)
            .await
            .expect_err("long unavailable window should leave operation pending");
        assert_pending_finalization_error(&err, operation_id, 250);
        pending_operations.push((quote_id, operation_id));
    }

    let pending_ids = wait_for_aleph_bft_pending_operations(&states[0], 0)
        .await
        .into_iter()
        .map(|pending| pending.operation_id)
        .collect::<Vec<_>>();
    for (_, operation_id) in &pending_operations {
        assert!(
            pending_ids.contains(operation_id),
            "pending operation {operation_id} was not durably recorded"
        );
    }
    for state in states.iter().take(4) {
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            0
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup before returned member")
            .is_empty());
        for (quote_id, _) in &pending_operations {
            assert!(state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await
                .expect("quote lookup before returned member")
                .is_none());
        }
    }

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut states[4],
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    let expected_operation_ids = pending_operations
        .iter()
        .map(|(_, operation_id)| *operation_id)
        .collect::<Vec<_>>();
    for state in &states {
        for (_, operation_id) in &pending_operations {
            wait_for_applied_operation(state, *operation_id).await;
        }
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after returned member");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            expected_operation_ids.len() as u64
        );
        for (quote_id, _) in &pending_operations {
            assert!(state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await
                .expect("quote lookup after returned member")
                .is_some());
        }
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after returned member")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_replays_pending_operation_when_restart_precedes_backup() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let seed = Mnemonic::generate(12)
            .expect("mnemonic")
            .to_seed_normalized("");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed);
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }

    states[0].federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect_err("unavailable member should leave operation pending first");
    assert_pending_finalization_error(&err, operation_id, 250);

    assert_eq!(
        wait_for_aleph_bft_pending_operations(&states[0], 0)
            .await
            .iter()
            .map(|pending| pending.operation_id)
            .collect::<Vec<_>>(),
        vec![operation_id]
    );
    assert!(!wait_for_aleph_bft_network_retries(&states[0], 0)
        .await
        .is_empty());
    let backup_bytes = states[0]
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_bytes(0)
        .await
        .expect("AlephBFT backup lookup before restart")
        .unwrap_or_default();
    assert!(
        backup_bytes.is_empty(),
        "this regression covers the restart window before upstream AlephBFT backup bytes exist"
    );

    states[0].federation_aleph_bft_network_inbound = None;
    states[0].federation_aleph_bft_live_session = None;
    live_session_handles.remove(0);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation_consensus = replayed_consensus;
    restarted_first_state.federation_sequences = replayed_sequences;
    let restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );
    states[0] = restarted_first_state;
    live_session_handles.push(restarted_first_handle);

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut states[4],
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    for (index, state) in states.iter().enumerate() {
        if !operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
            let status = state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status");
            let entries = state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal lookup");
            let pending = state
                .mint
                .localstore()
                .get_federation_aleph_bft_pending_operations(0)
                .await
                .expect("pending operation lookup");
            let retries = state
                .mint
                .localstore()
                .get_federation_aleph_bft_network_retries(0)
                .await
                .expect("network retry lookup");
            panic!(
                "member {index} did not apply restarted AlephBFT operation; status={status:?}; journal_entries={}; pending_operations={}; network_retries={}",
                entries.len(),
                pending.len(),
                retries.len()
            );
        }
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after restarted finality")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_restarts_from_persisted_backup_and_continues_finality() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let second_seed = Mnemonic::generate(12)
        .expect("second mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        Arc::clone(&first_db),
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(first_envelope, None)
        .await
        .expect("first operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    assert!(!wait_for_aleph_bft_backup_bytes(&first_state, 0)
        .await
        .is_empty());

    first_state.federation_aleph_bft_network_inbound = None;
    first_state.federation_aleph_bft_live_session = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        first_private,
        Arc::clone(&first_db),
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation_consensus = replayed_consensus;
    restarted_first_state.federation_sequences = replayed_sequences;
    let _restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&restarted_first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = restarted_first_state
        .submit_federation_envelope_with_sequence(second_envelope, None)
        .await
        .expect("second operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );

    for state in [&restarted_first_state, &second_state] {
        wait_for_applied_operation(state, first_operation_id).await;
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after post-backup restart");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].operation_id, first_operation_id);
        assert_eq!(entries[1].operation_id, second_operation_id);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&first_quote_id)
            .await
            .expect("first quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&second_quote_id)
            .await
            .expect("second quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after post-backup restart")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_replays_finalized_operation_from_backup_without_local_apply() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let second_seed = Mnemonic::generate(12)
        .expect("second mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(first_envelope, None)
        .await
        .expect("first operation finality before crash-window recovery");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    let backup_bytes = wait_for_aleph_bft_backup_bytes(&first_state, 0).await;

    first_state.federation_aleph_bft_network_inbound = None;
    first_state.federation_aleph_bft_live_session = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let crash_window_db = Arc::new(
        cdk_sqlite::mint::memory::empty()
            .await
            .expect("crash-window first db"),
    );
    let mut recovered_first_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        first_private,
        Arc::clone(&crash_window_db),
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut tx = recovered_first_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin crash-window backup transaction");
    tx.append_federation_aleph_bft_backup_bytes(0, &backup_bytes)
        .await
        .expect("copy upstream AlephBFT backup bytes");
    tx.set_federation_aleph_bft_active_session(0)
        .await
        .expect("persist crash-window active session");
    tx.commit()
        .await
        .expect("commit crash-window backup transaction");

    assert!(recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal before backup-only recovery")
        .is_empty());
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items before backup-only recovery")
        .is_empty());
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("quote before backup-only recovery")
        .is_none());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&recovered_first_state.mint)
            .await
            .expect("replay backup-only AlephBFT runtime");
    recovered_first_state.federation_consensus = replayed_consensus;
    recovered_first_state.federation_sequences = replayed_sequences;
    let recovered_status = recovered_first_state
        .federation_aleph_bft_status()
        .await
        .expect("backup-only AlephBFT status before live start");
    assert_eq!(recovered_status.session, 0);
    assert_eq!(recovered_status.finalized_item_count, 0);

    let _recovered_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut recovered_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart backup-only first live session")
    .expect("backup-only first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(recovered_first_state.clone()),
    );

    wait_for_applied_operation(&recovered_first_state, first_operation_id).await;
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("quote after backup-only recovery")
        .is_some());
    let recovered_entries = recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after backup-only recovery");
    assert_eq!(recovered_entries.len(), 1);
    assert_eq!(recovered_entries[0].operation_id, first_operation_id);

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&recovered_first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = recovered_first_state
        .submit_federation_envelope_with_sequence(second_envelope, None)
        .await
        .expect("second operation finality after backup-only recovery");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );

    for state in [&recovered_first_state, &second_state] {
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal after backup-only continuation");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after backup-only continuation")
                .finalized_item_count,
            2
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_continue_after_post_backup_restart_with_multiple_operations(
) {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let seed = Mnemonic::generate(12)
            .expect("mnemonic")
            .to_seed_normalized("");
        states.push(
            federation_route_state_for_private_with_db_and_backend(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
                FederationConsensusBackendKind::AlephBft,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed);
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 5);

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = states[0]
        .submit_federation_envelope_with_sequence(first_envelope, None)
        .await
        .expect("first five-member operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, first_operation_id).await;
    }
    let backup_before_restart = wait_for_aleph_bft_backup_bytes(&states[0], 0).await;

    states[0].federation_aleph_bft_network_inbound = None;
    states[0].federation_aleph_bft_live_session = None;
    let first_handle = live_session_handles.remove(0);
    drop(first_handle);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation_consensus = replayed_consensus;
    restarted_first_state.federation_sequences = replayed_sequences;
    let restarted_status = restarted_first_state
        .federation_aleph_bft_status()
        .await
        .expect("restarted AlephBFT status before live start");
    assert_eq!(restarted_status.finalized_item_count, 1);
    assert_eq!(restarted_status.next_unit_index, 1);
    let restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );
    states[0] = restarted_first_state;
    live_session_handles.push(restarted_first_handle);
    assert_eq!(live_session_handles.len(), 5);

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&states[0], &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = states[0]
        .submit_federation_envelope_with_sequence(second_envelope, None)
        .await
        .expect("second five-member operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );
    let backup_after_restart =
        wait_for_aleph_bft_backup_bytes_longer_than(&states[0], 0, backup_before_restart.len())
            .await;
    for state in &states {
        wait_for_applied_operation(state, second_operation_id).await;
    }

    let third_quote_id = QuoteId::new();
    let third_envelope = mint_quote_envelope(&states[1], &third_quote_id);
    let third_operation_id = third_envelope.operation_id().expect("third operation id");
    let response = states[1]
        .submit_federation_envelope_with_sequence(third_envelope, None)
        .await
        .expect("third five-member operation finality after restart");
    assert_eq!(response.operation_id, third_operation_id);
    assert_eq!(
        response.outcome.entry().status,
        FederationOperationStatus::Applied
    );

    for state in &states {
        wait_for_applied_operation(state, third_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after five-member post-backup restart");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id, third_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            3
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after five-member post-backup restart")
            .is_empty());
    }
    assert!(
        backup_after_restart.len() > backup_before_restart.len(),
        "restarted member should append new upstream AlephBFT backup bytes after finalizing another local unit"
    );
}

#[tokio::test]
async fn aleph_bft_live_session_startup_clears_orphan_network_retries_without_backup() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url, second_url);
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        first_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let orphan_retry = FederationAlephBftNetworkRetryRecord {
        retry_id: "orphan-retry".to_string(),
        session_id: 0,
        request: FederationAlephBftNetworkMessageRequest::new(0, vec![1, 2, 3]),
        peer_member_ids: vec![second_private.member_id],
    };
    state
        .mint
        .localstore()
        .add_federation_aleph_bft_network_retry(&orphan_retry)
        .await
        .expect("persist orphan retry");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_network_retries(0)
            .await
            .expect("orphan retry before startup"),
        vec![orphan_retry]
    );

    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_secs(60),
        ..FederationPeerSyncConfig::default()
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start live session")
    .expect("live session handle");

    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_network_retries(0)
        .await
        .expect("orphan retry after startup")
        .is_empty());
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_session_startup_clears_recovered_session_orphan_network_retries_without_backup(
) {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url, second_url);
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        first_private,
        Arc::clone(&db),
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin active-session transaction");
    tx.set_federation_aleph_bft_active_session(1)
        .await
        .expect("persist recovered active session");
    tx.commit()
        .await
        .expect("commit recovered active-session marker");
    let recovered_retry = FederationAlephBftNetworkRetryRecord {
        retry_id: "recovered-session-orphan-retry".to_string(),
        session_id: 1,
        request: FederationAlephBftNetworkMessageRequest::new(1, vec![4, 5, 6]),
        peer_member_ids: vec![second_private.member_id],
    };
    state
        .mint
        .localstore()
        .add_federation_aleph_bft_network_retry(&recovered_retry)
        .await
        .expect("persist recovered-session orphan retry");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_network_retries(1)
            .await
            .expect("recovered-session orphan retry before startup"),
        vec![recovered_retry]
    );
    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("recover active empty session runtime");
    state.federation_consensus = consensus;
    state.federation_sequences = sequences;
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("recovered AlephBFT status")
            .session,
        1
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_bytes(1)
        .await
        .expect("recovered session backup lookup")
        .unwrap_or_default()
        .is_empty());

    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_secs(60),
        ..FederationPeerSyncConfig::default()
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start recovered live session")
    .expect("recovered live session handle");

    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_active_session()
            .await
            .expect("active session after startup"),
        Some(1)
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_network_retries(1)
        .await
        .expect("recovered-session orphan retry after startup")
        .is_empty());
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_session_startup_reloads_network_retries_when_backup_exists() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let second_seed = Mnemonic::generate(12)
        .expect("second mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        first_private.clone(),
        Arc::clone(&first_db),
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let transport = RouterFederationTransportRegistry::default();
    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("first operation finality before retry reload test");
    assert_eq!(response.operation_id, operation_id);
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    let captured_request = transport
        .aleph_bft_network_message_requests()
        .into_iter()
        .find(|request| {
            request.sender == first_private.member_id
                && request.receiver == second_private.member_id
        })
        .expect("captured first-to-second AlephBFT network-message request");
    captured_request
        .verify(&public_config, captured_request.receiver)
        .expect("captured network-message request verifies");
    assert_eq!(captured_request.sender, first_private.member_id);
    assert_eq!(captured_request.payload.session_id, 0);
    let backup_before_restart = wait_for_aleph_bft_backup_bytes(&first_state, 0).await;
    assert!(
        !backup_before_restart.is_empty(),
        "retry reload requires durable upstream backup bytes"
    );
    first_state.federation_aleph_bft_network_inbound = None;
    first_state.federation_aleph_bft_live_session = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let retry = FederationAlephBftNetworkRetryRecord {
        retry_id: "backup-backed-retry".to_string(),
        session_id: 0,
        request: captured_request.payload,
        peer_member_ids: vec![second_private.member_id],
    };
    first_state
        .mint
        .localstore()
        .add_federation_aleph_bft_network_retry(&retry)
        .await
        .expect("persist backup-backed retry");
    assert_eq!(
        first_state
            .mint
            .localstore()
            .get_federation_aleph_bft_network_retries(0)
            .await
            .expect("backup-backed retry before restart"),
        vec![retry]
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        first_private,
        Arc::clone(&first_db),
        &first_seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay backup-backed retry runtime");
    restarted_first_state.federation_consensus = replayed_consensus;
    restarted_first_state.federation_sequences = replayed_sequences;
    let restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start restarted first live session")
    .expect("restarted first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );

    time::timeout(Duration::from_secs(5), async {
        loop {
            let retries = restarted_first_state
                .mint
                .localstore()
                .get_federation_aleph_bft_network_retries(0)
                .await
                .expect("backup-backed retry lookup after restart");
            if retries.is_empty() {
                return;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("backup-backed retry redelivery cleared storage");

    drop(restarted_first_handle);
    drop(second_handle);
}

async fn wait_for_applied_operation(state: &MintState, operation_id: FederationOperationId) {
    assert!(
        operation_applied_within(state, operation_id, Duration::from_secs(15)).await,
        "operation applied before timeout"
    );
}

async fn operation_applied_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> bool {
    time::timeout(timeout, async {
        loop {
            let entries = state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal lookup");
            if entries.iter().any(|entry| {
                entry.operation_id == operation_id
                    && entry.status == FederationOperationStatus::Applied
            }) {
                return;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

async fn wait_for_aleph_bft_pending_operations(
    state: &MintState,
    session_id: u64,
) -> Vec<FederationAlephBftPendingOperationRecord> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let pending = state
                .mint
                .localstore()
                .get_federation_aleph_bft_pending_operations(session_id)
                .await
                .expect("AlephBFT pending operation lookup");
            if !pending.is_empty() {
                return pending;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT pending operations were not persisted")
}

async fn wait_for_aleph_bft_network_retries(
    state: &MintState,
    session_id: u64,
) -> Vec<FederationAlephBftNetworkRetryRecord> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let retries = state
                .mint
                .localstore()
                .get_federation_aleph_bft_network_retries(session_id)
                .await
                .expect("AlephBFT network retry lookup");
            if !retries.is_empty() {
                return retries;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT network retries were not persisted")
}

async fn wait_for_aleph_bft_network_message_request(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    receiver: MemberId,
) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(request) = transport
                .aleph_bft_network_message_requests()
                .into_iter()
                .find(|request| request.sender == sender && request.receiver == receiver)
            {
                return request;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT network message request was not captured")
}

async fn wait_for_aleph_bft_backup_bytes(state: &MintState, session_id: u64) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if !backup_bytes.is_empty() {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes were not persisted")
}

async fn wait_for_aleph_bft_backup_bytes_longer_than(
    state: &MintState,
    session_id: u64,
    previous_len: usize,
) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if backup_bytes.len() > previous_len {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes did not grow")
}

#[tokio::test]
async fn aleph_bft_public_submit_is_not_ready_until_network_finality_is_installed() {
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

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("AlephBFT public submit should be gated until network finality is installed");

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
async fn aleph_bft_public_submit_stays_gated_when_sync_is_healthy_without_network_finality() {
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
    state.mark_federation_healthy().await;
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("aleph-bft status")
            .finality,
        FederationAlephBftFinalityMode::ExternalFinalizationRequired
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("healthy sync must not reopen AlephBFT submit without network finality");

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
async fn aleph_bft_network_finality_activation_rejects_non_aleph_bft_runtime() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::InMemory,
    )
    .await;

    let err = state
        .mark_aleph_bft_network_finality_available()
        .await
        .expect_err("in-memory runtime must not activate AlephBFT finality");

    assert!(
        err.to_string()
            .contains("requires aleph_bft consensus backend"),
        "unexpected error: {err}"
    );
    assert!(state.federation_aleph_bft_status().await.is_none());
}

#[tokio::test]
async fn aleph_bft_network_finality_activation_opens_queueing_but_still_waits_for_finalized_unit() {
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
    state.mark_federation_healthy().await;
    state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("aleph-bft status")
            .finality,
        FederationAlephBftFinalityMode::ExternalFinalizationRequired
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect_err("healthy sync without live AlephBFT finality must stay gated");
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
    {
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    state
        .mark_aleph_bft_network_finality_available()
        .await
        .expect("AlephBFT runtime can enable network finality");
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("aleph-bft status")
            .finality,
        FederationAlephBftFinalityMode::NetworkFinalityAvailable
    );

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("network finality mode should still wait for a finalized unit");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after unfinalized submit")
        .is_none());
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_private_rest_broadcast_stand_in_idles_after_network_finality_activation() {
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

    state
        .mark_aleph_bft_network_finality_available()
        .await
        .expect("AlephBFT runtime can enable network finality");
    let outcome =
        broadcast_next_aleph_bft_finalized_unit_round(&state, &client, std::slice::from_ref(peer))
            .await
            .expect("stand-in broadcast should idle under network finality ownership");

    assert!(outcome.is_none());
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after idle stand-in")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup after idle stand-in")
        .is_empty());
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_network_pump_broadcasts_everyone_to_remote_members() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(42, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert!(!outcome.reached_idle);
    let requests = transport.aleph_bft_network_message_requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.receiver)
            .collect::<Vec<_>>(),
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("signed request verifies");
        assert_eq!(request.sender, local_private.member_id);
        assert_eq!(request.payload.session_id, 7);
        let bytes = request.payload.message_bytes().expect("message bytes");
        let decoded = u64::decode(&mut &bytes[..]).expect("scale u64");
        assert_eq!(decoded, 42);
    }
}

#[tokio::test]
async fn aleph_bft_network_pump_sends_node_recipient_to_one_remote_member() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 8);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(99, Recipient::Node(NodeIndex(2)));
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 8,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(outcome.remote_accepted_members, vec![member_id(3)]);
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    let requests = transport.aleph_bft_network_message_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].receiver, member_id(3));
    let bytes = requests[0].payload.message_bytes().expect("message bytes");
    let decoded = u64::decode(&mut &bytes[..]).expect("scale u64");
    assert_eq!(decoded, 99);
}

#[tokio::test]
async fn aleph_bft_network_pump_does_not_rest_send_local_node_recipient() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 9);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(100, Recipient::Node(NodeIndex(0)));
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 9,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert!(outcome.remote_accepted_members.is_empty());
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert!(transport.aleph_bft_network_message_requests().is_empty());
}

#[tokio::test]
async fn aleph_bft_network_pump_records_delivery_failures_without_failing_tick() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(123, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 10,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick records peer failures");

    assert_eq!(outcome.messages_drained, 1);
    assert!(outcome.remote_accepted_members.is_empty());
    assert_eq!(
        outcome.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_redelivers_failed_members_after_recovery() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(456, Recipient::Everyone);
    let first = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 11,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        None,
    )
    .await
    .expect("network pump first tick");

    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert!(first.remote_accepted_members.is_empty());
    assert_eq!(
        first.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert_eq!(first.remote_requeued_members, first.remote_failed_members);
    assert_eq!(first.retry_queue_len, 1);
    assert_eq!(retry_queue.len(), 1);
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);

    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 11);
    transport
        .clone()
        .with_aleph_bft_network_message_response(response);
    let second = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 11,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        None,
    )
    .await
    .expect("network pump retry tick");

    assert_eq!(second.messages_drained, 0);
    assert_eq!(second.retry_messages_drained, 1);
    assert_eq!(
        second.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(second.remote_failed_members.is_empty());
    assert!(second.remote_requeued_members.is_empty());
    assert_eq!(second.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 8);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_eviction_prunes_persisted_retry_rows() {
    let (public_config, local_private, _) = transport_config();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let db: DynMintDatabase = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();
    let message_count = CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT + 1;

    for payload in 0..message_count as u64 {
        network.send(payload, Recipient::Everyone);
    }

    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 13,
            max_messages_per_tick: message_count,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        Some(&db),
    )
    .await
    .expect("overflowing network pump tick");

    assert_eq!(outcome.messages_drained, message_count);
    assert_eq!(outcome.retry_messages_drained, 0);
    assert_eq!(outcome.remote_failed_members.len(), message_count);
    assert!(outcome
        .remote_failed_members
        .iter()
        .all(|failed_member_id| *failed_member_id == member_id(2)));
    assert_eq!(outcome.remote_requeued_members.len(), message_count);
    assert_eq!(
        outcome.retry_queue_len,
        CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT
    );
    assert_eq!(retry_queue.len(), CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT);

    let persisted_retries = db
        .get_federation_aleph_bft_network_retries(13)
        .await
        .expect("persisted retries after queue overflow");
    assert_eq!(
        persisted_retries.len(),
        CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT
    );
    let mut persisted_payloads = persisted_retries
        .iter()
        .map(|retry| {
            let bytes = retry.request.message_bytes().expect("retry payload bytes");
            let mut input = bytes.as_slice();
            u64::decode(&mut input).expect("retry payload")
        })
        .collect::<Vec<_>>();
    persisted_payloads.sort_unstable();
    assert_eq!(
        persisted_payloads,
        (1..message_count as u64).collect::<Vec<_>>()
    );
    assert_eq!(
        transport.aleph_bft_network_message_requests().len(),
        message_count
    );
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_reloads_only_current_session_known_members_from_db() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let db: DynMintDatabase = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let valid_request = FederationAlephBftNetworkMessageRequest::new(12, 700u64.encode());
    let mixed_known_request = FederationAlephBftNetworkMessageRequest::new(12, 701u64.encode());
    let stale_retry_session_request =
        FederationAlephBftNetworkMessageRequest::new(12, 702u64.encode());
    let stale_request_session = FederationAlephBftNetworkMessageRequest::new(99, 703u64.encode());
    let unknown_member_request = FederationAlephBftNetworkMessageRequest::new(12, 704u64.encode());
    for retry in [
        FederationAlephBftNetworkRetryRecord {
            retry_id: "valid".to_string(),
            session_id: 12,
            request: valid_request,
            peer_member_ids: vec![member_id(2), member_id(3)],
        },
        FederationAlephBftNetworkRetryRecord {
            retry_id: "mixed-known".to_string(),
            session_id: 12,
            request: mixed_known_request,
            peer_member_ids: vec![member_id(4), member_id(99)],
        },
        FederationAlephBftNetworkRetryRecord {
            retry_id: "stale-retry-session".to_string(),
            session_id: 13,
            request: stale_retry_session_request,
            peer_member_ids: vec![member_id(2)],
        },
        FederationAlephBftNetworkRetryRecord {
            retry_id: "stale-request-session".to_string(),
            session_id: 12,
            request: stale_request_session,
            peer_member_ids: vec![member_id(2)],
        },
        FederationAlephBftNetworkRetryRecord {
            retry_id: "unknown-member".to_string(),
            session_id: 12,
            request: unknown_member_request,
            peer_member_ids: vec![member_id(99)],
        },
    ] {
        db.add_federation_aleph_bft_network_retry(&retry)
            .await
            .expect("persist retry");
    }

    let mut retry_queue = CdkAlephBftNetworkRetryQueue::from_db(&db, &keychain, 12)
        .await
        .expect("reload retry queue");
    assert_eq!(retry_queue.len(), 2);
    let mut persisted_retry_ids = db
        .get_federation_aleph_bft_network_retries(12)
        .await
        .expect("session 12 retries after reload pruning")
        .into_iter()
        .map(|retry| retry.retry_id)
        .collect::<Vec<_>>();
    persisted_retry_ids.sort();
    assert_eq!(
        persisted_retry_ids,
        vec!["mixed-known".to_string(), "valid".to_string()]
    );
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 12),
    );
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (_network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 12,
            max_messages_per_tick: 8,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        Some(&db),
    )
    .await
    .expect("drain reloaded retries");

    assert_eq!(outcome.retry_messages_drained, 2);
    let mut accepted_members = outcome.remote_accepted_members.clone();
    accepted_members.sort();
    assert_eq!(
        accepted_members,
        vec![member_id(2), member_id(3), member_id(4)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_requeued_members.is_empty());
    assert_eq!(outcome.retry_queue_len, 0);
    assert!(db
        .get_federation_aleph_bft_network_retries(12)
        .await
        .expect("session 12 retries after successful redelivery")
        .is_empty());
    let stale_session_queue = CdkAlephBftNetworkRetryQueue::from_db(&db, &keychain, 13)
        .await
        .expect("reload stale retry session");
    assert!(stale_session_queue.is_empty());
    assert!(db
        .get_federation_aleph_bft_network_retries(13)
        .await
        .expect("session 13 retries after pruning")
        .is_empty());
    let mut receivers = transport
        .aleph_bft_network_message_requests()
        .iter()
        .map(|request| request.receiver)
        .collect::<Vec<_>>();
    receivers.sort();
    assert_eq!(receivers, vec![member_id(2), member_id(3), member_id(4)]);
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
async fn aleph_bft_startup_restore_retires_startup_halt_after_live_finality_for_empty_history() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut restored_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    assert!(matches!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { reason })
            if reason.contains("public federation submissions are disabled")
    ));

    restored_state
        .mark_aleph_bft_network_finality_available()
        .await
        .expect("AlephBFT runtime can enable network finality");
    restored_state.restore_federation_sync_status().await;
    restored_state.restore_federation_startup_readiness().await;
    assert_eq!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );

    restored_state.federation_peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&restored_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = restored_state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("live finality without finalized unit should time out pending");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert!(restored_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after pending finality")
        .is_none());
    let consensus = restored_state
        .federation_consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_restart_after_unfinalized_local_unit_requires_retry_without_state_mutation() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope.clone()).await;
    assert_eq!(unit.operation_ids, vec![operation_id]);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before restart")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal before restart")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items before restart")
        .is_empty());
    {
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 1);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }
    drop(state);

    let restarted_state = federation_route_state_for_private_with_db_and_backend(
        public_config,
        local_private,
        db,
        &seed,
        FederationConsensusBackendKind::AlephBft,
    )
    .await;
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restart")
        .is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after restart")
        .is_empty());
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after restart")
        .is_empty());
    {
        let consensus = restarted_state
            .federation_consensus
            .as_ref()
            .expect("restarted consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    let retried_unit = queue_local_aleph_bft_unit(&restarted_state, envelope).await;
    assert_eq!(retried_unit.operation_ids, vec![operation_id]);
    assert_eq!(
        retried_unit.canonical_bytes().expect("retried unit bytes"),
        unit.canonical_bytes().expect("original unit bytes")
    );
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after retry queue")
        .is_none());
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
async fn aleph_bft_duplicate_after_lost_waiters_returns_journal_duplicate() {
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
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_with_sequence(envelope, None)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");
    let accepted_response = submit.await.expect("submit task").expect("submit response");
    assert_eq!(accepted_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    // Simulate a restart after accepted apply where in-memory waiters/cache are gone.
    state.federation_notifications =
        Some(Arc::new(crate::FederationOperationNotifications::default()));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);

    let duplicate_response = state
        .submit_federation_envelope_with_sequence(mint_quote_envelope(&state, &quote_id), None)
        .await
        .expect("duplicate submit after lost waiters");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("operation remains durable")
            .operation_id,
        operation_id
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.response_cache_lookups, 0);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_admission_race_returns_journal_duplicate() {
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
    let peer = public_config
        .members
        .get(&peer_private.member_id)
        .expect("peer member");

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .validate_federation_envelope_admission(&envelope)
        .expect("operation admitted before competing acceptance");

    let competing_state = state.clone();
    let competing_envelope = envelope.clone();
    let competing_submit = tokio::spawn(async move {
        competing_state
            .submit_federation_envelope_with_sequence(competing_envelope, None)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    run_federation_aleph_bft_driver_tick(
        &state,
        &client,
        std::slice::from_ref(peer),
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
    .expect("driver tick");
    let accepted = competing_submit
        .await
        .expect("competing submit task")
        .expect("competing submit response");
    assert_eq!(accepted.operation_id, operation_id);

    let duplicate = state
        .submit_federation_envelope_with_sequence(envelope, None)
        .await
        .expect("post-admission race submit should return duplicate");
    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("accepted operation remains durable")
            .operation_id,
        operation_id
    );
    drop(consensus);

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
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
async fn aleph_bft_driver_tick_applies_batched_unit_with_multiple_responses() {
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
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let second_envelope = mint_quote_envelope(&state, &second_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let unit = {
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(first_envelope)
            .expect("queue first AlephBFT operation");
        consensus
            .submit_aleph_bft_envelope(second_envelope)
            .expect("queue second AlephBFT operation");
        consensus
            .preview_next_aleph_bft_unit_payload()
            .expect("preview batched AlephBFT unit")
            .expect("batched unit")
    };
    assert_eq!(
        unit.operation_ids,
        vec![first_operation_id, second_operation_id]
    );
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    assert_eq!(response.responses.len(), 2);
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
    assert_eq!(outcome.local_responses, 2);
    assert_eq!(outcome.remote_accepted_members, 1);
    assert_eq!(outcome.remote_failed_members, 0);
    assert_eq!(outcome.remote_skipped_members, 0);
    assert!(outcome.reached_idle);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);

    let first_quote = state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("first quote lookup after driver tick")
        .expect("first mint quote applied");
    assert_eq!(first_quote.request, format!("invoice-{first_quote_id}"));
    let second_quote = state
        .mint
        .localstore()
        .get_mint_quote(&second_quote_id)
        .await
        .expect("second quote lookup after driver tick")
        .expect("second mint quote applied");
    assert_eq!(second_quote.request, format!("invoice-{second_quote_id}"));

    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(finalized_items.len(), 2);
    assert_eq!(finalized_items[0].operation_id, Some(first_operation_id));
    assert_eq!(finalized_items[1].operation_id, Some(second_operation_id));
    assert!(finalized_items
        .iter()
        .all(|item| item.unit_index == unit.unit_index));

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 2);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>(),
        vec![first_operation_id, second_operation_id]
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
async fn aleph_bft_broadcast_round_reports_distinct_skipped_members() {
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
    let peers = vec![second_peer, third_peer.clone(), third_peer];

    let outcome = broadcast_next_aleph_bft_finalized_unit_round_with_config(
        &state,
        &client,
        &peers,
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 3,
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
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
}

#[tokio::test]
async fn aleph_bft_broadcast_round_counts_distinct_remote_members_for_quorum() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_finalized_unit_responses(vec![response.clone(), response]);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let second_peer = public_config
        .member(second_private.member_id)
        .expect("second peer")
        .clone();
    let third_peer = public_config
        .member(third_private.member_id)
        .expect("third peer")
        .clone();
    let peers = vec![second_peer.clone(), second_peer, third_peer];

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
        vec![second_private.member_id, third_private.member_id]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(outcome.local_responses[0].operation_id, operation_id);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 2);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after distinct quorum")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test(start_paused = true)]
async fn aleph_bft_broadcast_round_times_out_slow_peer_and_uses_next_for_quorum() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let timeout = Duration::from_secs(2);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_finalized_unit_responses(vec![response.clone(), response])
        .with_aleph_bft_finalized_unit_delays(vec![
            timeout + Duration::from_secs(60),
            Duration::ZERO,
        ]);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig {
            peer_request_timeout: timeout,
            operation_submission_timeout: Duration::from_secs(3600),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("peer client");
    let second_peer = public_config
        .member(second_private.member_id)
        .expect("second peer")
        .clone();
    let third_peer = public_config
        .member(third_private.member_id)
        .expect("third peer")
        .clone();
    let peers = vec![second_peer, third_peer];
    let state_for_broadcast = state.clone();

    let task = tokio::spawn(async move {
        broadcast_next_aleph_bft_finalized_unit_round_with_config(
            &state_for_broadcast,
            &client,
            &peers,
            FederationAlephBftDriverTickConfig {
                max_remote_peer_deliveries_per_unit: 2,
                ..FederationAlephBftDriverTickConfig::default()
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    tokio::time::advance(timeout - Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(
        !task.is_finished(),
        "broadcast should still wait before the peer timeout elapses"
    );
    tokio::time::advance(Duration::from_millis(1)).await;

    let outcome = task
        .await
        .expect("broadcast task")
        .expect("broadcast round")
        .expect("unit broadcast");

    assert_eq!(
        outcome.remote_failed_members,
        vec![second_private.member_id]
    );
    assert_eq!(
        outcome.remote_accepted_members,
        vec![third_private.member_id]
    );
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(outcome.local_responses[0].operation_id, operation_id);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 2);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after timeout recovery")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn aleph_bft_broadcast_round_five_members_tolerates_bad_ack_before_quorum() {
    let (public_config, private_configs) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 4).expect("params"));
    let local_private = private_configs[0].clone();
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
    let mut bad_response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    bad_response.unit_body_hash = FederationAlephBftUnitBodyHash::from_bytes([7; 32]);
    let good_response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_responses(vec![
            bad_response,
            good_response.clone(),
            good_response.clone(),
            good_response,
        ]);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let peers = private_configs
        .iter()
        .skip(1)
        .map(|private| {
            public_config
                .member(private.member_id)
                .expect("remote member")
                .clone()
        })
        .collect::<Vec<_>>();

    let outcome = broadcast_next_aleph_bft_finalized_unit_round_with_config(
        &state,
        &client,
        &peers,
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 4,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect("broadcast round")
    .expect("unit broadcast");

    assert_eq!(outcome.unit.body_hash, unit.body_hash);
    assert_eq!(
        outcome.remote_failed_members,
        vec![private_configs[1].member_id]
    );
    assert_eq!(
        outcome.remote_accepted_members,
        vec![
            private_configs[2].member_id,
            private_configs[3].member_id,
            private_configs[4].member_id,
        ]
    );
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(outcome.local_responses[0].operation_id, operation_id);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 4);

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
}

#[tokio::test]
async fn aleph_bft_broadcast_round_retries_after_partial_delivery_reaches_quorum() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope).await;
    let response = finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_finalized_unit_responses(vec![response.clone()]);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let second_peer = public_config
        .member(second_private.member_id)
        .expect("second peer")
        .clone();
    let third_peer = public_config
        .member(third_private.member_id)
        .expect("third peer")
        .clone();
    let peers = vec![second_peer, third_peer];

    let err = broadcast_next_aleph_bft_finalized_unit_round_with_config(
        &state,
        &client,
        &peers,
        FederationAlephBftDriverTickConfig {
            max_remote_peer_deliveries_per_unit: 2,
            ..FederationAlephBftDriverTickConfig::default()
        },
    )
    .await
    .expect_err("partial delivery should not reach quorum");

    assert!(
        err.to_string().contains("reached 2 of 3 required"),
        "unexpected error: {err}"
    );
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 2);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after partial delivery")
        .is_none());
    {
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 1);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    transport
        .clone()
        .with_aleph_bft_finalized_unit_responses(vec![response.clone(), response]);
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
    .expect("retry broadcast")
    .expect("unit broadcast after retry");

    assert_eq!(
        outcome.remote_accepted_members,
        vec![second_private.member_id, third_private.member_id]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert_eq!(outcome.local_responses.len(), 1);
    assert_eq!(outcome.local_responses[0].operation_id, operation_id);
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 4);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after retry")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

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
async fn aleph_bft_broadcast_round_rejects_invalid_remote_ack_without_draining() {
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
    let mut response =
        finalized_unit_response_for_peer(&public_config, local_private.member_id, &unit);
    response.unit_body_hash = FederationAlephBftUnitBodyHash::from_bytes([9; 32]);
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
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
            .expect_err("invalid remote acknowledgement must not count toward quorum");

    assert!(
        err.to_string().contains("reached 1 of 2 required"),
        "unexpected error: {err}"
    );
    assert!(
        err.to_string().contains("remote failures 1"),
        "unexpected error: {err}"
    );
    assert_eq!(transport.aleph_bft_finalized_unit_requests().len(), 1);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after invalid ack")
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
async fn submit_mint_quote_rejects_invalid_admission_before_journal_append() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = invalid_mint_quote_transcript_envelope(&state, &quote_id);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("invalid quote transcript should fail before consensus");

    assert!(matches!(err, cdk::Error::UnknownQuote));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .is_none());
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
}

#[tokio::test]
async fn submit_mint_operation_rejects_invalid_admission_before_journal_append() {
    let (state, _, _) = federation_route_state().await;
    let envelope = invalid_mint_operation_keyset_envelope(&state);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("invalid mint operation should fail before consensus");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::InvalidFederatedKeysetVersion { .. })
    ));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
}

#[tokio::test]
async fn submit_payment_observation_rejects_invalid_private_admission_before_sequence_persistence()
{
    let (state, _, peer_private) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let mut envelope = paid_observation_envelope(&state, &quote_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("expected mint quote payment observation");
    };
    observation.method = PaymentMethod::Custom("bad/method".to_string());
    let sender_sequence = 33;

    let err = state
        .submit_federation_envelope_with_sequence(
            envelope,
            Some((peer_private.member_id, sender_sequence)),
        )
        .await
        .expect_err("invalid payment observation should fail before consensus");

    assert!(matches!(err, cdk::Error::InvalidPaymentMethod));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequence lookup");
    assert!(
        sequences.is_empty(),
        "invalid private admission must not persist the sender sequence"
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
async fn submit_federation_envelope_returns_duplicate_when_acceptance_races_submission() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .validate_federation_envelope_admission(&envelope)
        .expect("operation admitted before race");

    let accepted = state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("operation accepted by competing path");
    assert_eq!(accepted.operation_id, operation_id);
    assert!(matches!(
        accepted.outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));

    let duplicate = state
        .submit_federation_envelope(envelope)
        .await
        .expect("post-race submit should return duplicate response");
    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status, FederationOperationStatus::Applied);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("mint quote lookup")
        .is_some());

    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    let consensus = state.federation_consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
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
async fn catch_up_replay_records_state_digest_checkpoint() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(envelope)
        .expect("peer accepted mint quote");
    let catch_up_request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    assert!(state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoint lookup before catch-up")
        .is_empty());

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 1);

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
    let entry = applied.first().expect("applied entry");

    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].operation_index, entry.index);
    assert_eq!(checkpoints[0].operation_id, entry.operation_id);
    assert_eq!(checkpoints[0].order_digest, entry.order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("mint quote lookup")
        .is_some());
}

#[tokio::test]
async fn catch_up_replay_checkpoints_completed_operation_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let output = mint_output(&state, &quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_envelope = config.mint_operation(mint_input).expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    peer_consensus
        .submit_envelope(paid_observation_envelope(&state, &quote_id))
        .expect("peer accepted paid observation");
    peer_consensus
        .submit_envelope(mint_envelope)
        .expect("peer accepted mint");
    let catch_up_request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let before_snapshot = materialized_state_snapshot(&state, &catch_up_response.entries)
        .await
        .expect("before snapshot");
    assert!(
        completed_operation_snapshot_row(&before_snapshot, mint_operation_id)
            .fields
            .contains_key("missing"),
        "lagging member should expose a missing completed-operation row before replay"
    );

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 3);

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let snapshot = materialized_state_snapshot(&state, &entries)
        .await
        .expect("materialized snapshot");
    let expected_state_digest = snapshot.digest().expect("materialized digest");
    let completed_row = completed_operation_snapshot_row(&snapshot, mint_operation_id);
    assert_eq!(
        completed_row.fields.get("kind").map(String::as_str),
        Some("mint")
    );
    assert!(!completed_row.fields.contains_key("missing"));

    let checkpoints = state
        .mint
        .localstore()
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    let mint_entry = entries.last().expect("mint journal entry");

    assert_eq!(checkpoints.len(), 1);
    assert_eq!(mint_entry.operation_id, mint_operation_id);
    assert_eq!(checkpoints[0].operation_index, mint_entry.index);
    assert_eq!(checkpoints[0].operation_id, mint_entry.operation_id);
    assert_eq!(checkpoints[0].order_digest, mint_entry.order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);
}

#[tokio::test]
async fn catch_up_replay_checkpoints_unresolved_payment_observation_rows() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let observation_id = observation
        .operation_id()
        .expect("observation operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    peer_consensus
        .submit_envelope(observation)
        .expect("peer accepted unresolved payment observation");
    let catch_up_request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 2);

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
    let observation_entry = entries.last().expect("observation journal entry");

    assert_eq!(checkpoints.len(), 1);
    assert_eq!(observation_entry.operation_id, observation_id);
    assert_eq!(checkpoints[0].operation_index, observation_entry.index);
    assert_eq!(checkpoints[0].operation_id, observation_entry.operation_id);
    assert_eq!(checkpoints[0].order_digest, observation_entry.order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);

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

    let status_request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let status = post_local_status(State(state), Json(status_request))
        .await
        .expect("local status response")
        .0;
    assert_eq!(status.payment_observation_reports.len(), 1);
    let report = status
        .payment_observation_reports
        .first()
        .expect("payment observation report");
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum
    );
    assert_eq!(report.required, 2);
    assert_eq!(report.total_observations, 1);
    assert!(!report.quarantined);
    assert!(!report.groups[0].quorum_reached);
    assert!(report.stale);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );
    let status_json = serde_json::to_string(&status).expect("status json");
    assert!(!status_json.contains("settled"));
}

#[tokio::test]
async fn catch_up_replay_checkpoints_payment_observation_quarantine_rows() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let first_observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let second_observation = mint_payment_observation_envelope_for_member(
        &state,
        &quote_id,
        2,
        2,
        "different-settlement",
    );
    let first_observation_id = first_observation
        .operation_id()
        .expect("first observation operation id");
    let second_observation_id = second_observation
        .operation_id()
        .expect("second observation operation id");
    let quarantine = config.payment_observation_quarantine(
        cdk::federation::FederationPaymentObservationContext::MintQuote,
        quote_id.clone(),
        FederationPaymentObservationQuarantineReason::ConflictingEvidence,
        vec![first_observation_id, second_observation_id],
    );
    let quarantine_id = quarantine.operation_id().expect("quarantine operation id");
    let mut peer_consensus =
        InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
    peer_consensus
        .submit_envelope(mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    peer_consensus
        .submit_envelope(first_observation)
        .expect("peer accepted first payment observation");
    peer_consensus
        .submit_envelope(second_observation)
        .expect("peer accepted conflicting payment observation");
    peer_consensus
        .submit_envelope(quarantine)
        .expect("peer accepted quarantine");
    let catch_up_request = state
        .federation_consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = peer_consensus
        .catch_up_checked(&catch_up_request)
        .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 4);

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
    let quarantine_entry = entries.last().expect("quarantine journal entry");

    assert_eq!(checkpoints.len(), 1);
    assert_eq!(quarantine_entry.operation_id, quarantine_id);
    assert_eq!(checkpoints[0].operation_index, quarantine_entry.index);
    assert_eq!(checkpoints[0].operation_id, quarantine_entry.operation_id);
    assert_eq!(checkpoints[0].order_digest, quarantine_entry.order_digest);
    assert_eq!(checkpoints[0].state_digest, expected_state_digest);

    let status_request = signed_status_request(
        &public_config,
        &peer_private,
        state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id(),
        1,
    );
    let status = post_local_status(State(state), Json(status_request))
        .await
        .expect("local status response")
        .0;
    assert_eq!(status.payment_observation_reports.len(), 1);
    let report = status
        .payment_observation_reports
        .first()
        .expect("payment observation report");
    assert!(report.quarantined);
    assert_eq!(report.quarantine_operation_ids, vec![quarantine_id]);
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

fn completed_operation_snapshot_row(
    snapshot: &FederationMaterializedStateSnapshot,
    operation_id: FederationOperationId,
) -> &FederationMaterializedStateRow {
    let operation_uuid = federation_completed_operation_uuid(operation_id);
    snapshot
        .rows()
        .iter()
        .find(|row| row.table == "completed_operations" && row.key == operation_uuid.to_string())
        .expect("completed operation materialized row")
}

#[tokio::test]
async fn materialized_state_digest_tracks_completed_operation_rows_for_value_operations() {
    let (state, _, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();

    let mint_quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &mint_quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &mint_quote_id))
        .await
        .expect("submit paid observation");
    let mint_output = mint_output(&state, &mint_quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id,
        outputs: vec![mint_output],
        signature: None,
    });
    let mint_envelope = config.mint_operation(mint_input).expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    state
        .submit_federation_envelope(mint_envelope)
        .await
        .expect("submit mint operation");

    let swap_input = proof_for_amount(&state, 1, "completed-operation-swap");
    let swap_change = swap_output(&state, "completed-operation-swap");
    let swap_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![swap_change],
        ))
        .expect("swap envelope");
    let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");
    state
        .submit_federation_envelope(swap_envelope)
        .await
        .expect("submit swap operation");

    let paid_melt_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_id = melt_quote_with_description(
        &state,
        1_000,
        serde_json::to_string(&paid_melt_description).expect("paid melt description"),
    )
    .await;
    let melt_inputs = vec![
        proof_for_amount(&state, 1, "completed-operation-melt-a"),
        proof_for_amount(&state, 1, "completed-operation-melt-b"),
        proof_for_amount(&state, 1, "completed-operation-melt-c"),
    ];
    let mut melt_change = swap_output(&state, "completed-operation-melt-change");
    melt_change.amount = Amount::ZERO;
    let melt_envelope = config
        .melt_operation(cdk::nuts::MeltRequest::new(
            melt_quote_id,
            melt_inputs,
            Some(vec![melt_change]),
        ))
        .expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    state
        .submit_federation_envelope(melt_envelope)
        .await
        .expect("submit melt operation");

    let entries = state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let snapshot = materialized_state_snapshot(&state, &entries)
        .await
        .expect("materialized snapshot");
    let digest = snapshot.digest().expect("materialized digest");
    assert_ne!(digest, FederationStateDigest::genesis());

    let mint_row = completed_operation_snapshot_row(&snapshot, mint_operation_id);
    assert_eq!(
        mint_row.fields.get("kind").map(String::as_str),
        Some("mint")
    );
    assert_eq!(
        mint_row.fields.get("total_issued").map(String::as_str),
        Some("1")
    );
    assert!(!mint_row.fields.contains_key("missing"));

    let swap_row = completed_operation_snapshot_row(&snapshot, swap_operation_id);
    assert_eq!(
        swap_row.fields.get("kind").map(String::as_str),
        Some("swap")
    );
    assert_eq!(
        swap_row.fields.get("total_issued").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        swap_row.fields.get("total_redeemed").map(String::as_str),
        Some("1")
    );
    assert!(!swap_row.fields.contains_key("missing"));

    let melt_row = completed_operation_snapshot_row(&snapshot, melt_operation_id);
    assert_eq!(
        melt_row.fields.get("kind").map(String::as_str),
        Some("melt")
    );
    assert_eq!(
        melt_row.fields.get("total_redeemed").map(String::as_str),
        Some("3")
    );
    assert!(!melt_row.fields.contains_key("missing"));

    let fingerprints = snapshot
        .row_fingerprints()
        .expect("materialized row fingerprints");
    assert!(
        fingerprints
            .iter()
            .filter(|row| row.table == "completed_operations")
            .count()
            >= 3
    );
    let fingerprints_json =
        serde_json::to_string(&fingerprints).expect("materialized fingerprint json");
    assert!(!fingerprints_json.contains(&mint_operation_id.to_string()));
    assert!(!fingerprints_json.contains(&swap_operation_id.to_string()));
    assert!(!fingerprints_json.contains(&melt_operation_id.to_string()));
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
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("submit mint quote");

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
async fn private_local_status_route_includes_unresolved_payment_observation_fingerprints() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            1,
            1,
            "row-fingerprint-payment-proof",
        ))
        .await
        .expect("submit unresolved payment observation");

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert!(response
        .materialized_state_rows
        .iter()
        .any(|row| row.table == "payment_observations"));
    assert!(response
        .payment_observation_reports
        .iter()
        .any(|report| report.quote == quote_id.to_string()
            && report.status
                == cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum));
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("row-fingerprint-payment-proof"));
}

#[tokio::test]
async fn private_local_status_route_includes_completed_operation_fingerprints() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let local_member = config.member_id();
    let quote_id = QuoteId::new();
    let output = mint_output(&state, &quote_id);
    let mint_envelope = config
        .mint_operation(MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        }))
        .expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit paid observation");
    state
        .submit_federation_envelope(mint_envelope)
        .await
        .expect("submit mint issuance");

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert!(response
        .materialized_state_rows
        .iter()
        .any(|row| row.table == "completed_operations"));
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains(&mint_operation_id.to_string()));
    assert!(
        !status_json.contains(&federation_completed_operation_uuid(mint_operation_id).to_string())
    );
    assert!(!status_json.contains(&output.blinded_secret.to_hex()));
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
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("submit mint quote");
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
    let envelope = restore_envelope_for_config(
        &public_config,
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
        .submit_federation_envelope(restore_envelope_for_config(
            &public_config,
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
        .submit_federation_envelope(restore_envelope_for_config(
            &public_config,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .await;
    assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
}

#[tokio::test]
async fn startup_verifying_session_outcome_blocks_restore_and_checkstate_reads() {
    let (state, public_config, _, peer_private, _) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(restore_envelope_for_config(
            &public_config,
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

    let y = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"startup verifying checkstate read",
    ));
    let check_error = crate::router_handlers::post_check(
        AuthHeader::None,
        State(restarted.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect_err("startup verification must not serve authoritative check-state");
    let check_body = response_error_detail(check_error).await;
    assert!(
        check_body.contains("verifying quorum session outcome evidence"),
        "check-state should fail closed while startup verifies session outcome: {check_body}"
    );

    let restore_request = match restore_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    )
    .operation
    {
        FederationOperation::Restore(operation) => operation.request,
        _ => unreachable!("helper returns restore operation"),
    };
    let restore_error = crate::router_handlers::post_restore(
        AuthHeader::None,
        State(restarted),
        Json(restore_request),
    )
    .await
    .expect_err("startup verification must not serve authoritative restore");
    let restore_body = response_error_detail(restore_error).await;
    assert!(
        restore_body.contains("verifying quorum session outcome evidence"),
        "restore should fail closed while startup verifies session outcome: {restore_body}"
    );
}

#[tokio::test]
async fn startup_returns_healthy_when_replayed_journal_has_session_outcome_quorum() {
    let (state, public_config, _, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    let restore_envelope = restore_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let restore_request = match &restore_envelope.operation {
        FederationOperation::Restore(operation) => operation.request.clone(),
        _ => unreachable!("helper returns restore operation"),
    };
    state
        .submit_federation_envelope(restore_envelope)
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

    let y = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"startup verified checkstate read",
    ));
    let _ = crate::router_handlers::post_check(
        AuthHeader::None,
        State(restarted.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect("verified startup member can serve check-state");

    let _ = crate::router_handlers::post_restore(
        AuthHeader::None,
        State(restarted),
        Json(restore_request.clone()),
    )
    .await
    .expect("verified startup member can serve restore");
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
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("submit mint quote");
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

#[tokio::test]
async fn private_journal_route_returns_whole_aleph_bft_finalized_item_units() {
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
    let local_member = local_private.member_id;
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote_id)),
            FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote_id)),
        ],
    )
    .expect("two-item AlephBFT unit");
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply AlephBFT unit");

    let partial_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationJournalCatchUpRequest::new(0)
            .with_limit(1)
            .with_aleph_bft_next_item_index(0),
    );
    let partial_response = post_journal(State(state.clone()), Json(partial_request))
        .await
        .expect("partial-limit journal response")
        .0;
    assert_eq!(partial_response.entries.len(), 1);
    assert!(partial_response.aleph_bft_finalized_items.is_empty());
    assert!(!partial_response.aleph_bft_finalized_items_complete);

    let complete_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationJournalCatchUpRequest::new(0)
            .with_limit(2)
            .with_aleph_bft_next_item_index(0),
    );
    let complete_response = post_journal(State(state), Json(complete_request))
        .await
        .expect("complete journal response")
        .0;
    assert_eq!(complete_response.entries.len(), 2);
    assert_eq!(complete_response.aleph_bft_finalized_items.len(), 2);
    assert!(complete_response.aleph_bft_finalized_items_complete);
    assert!(complete_response
        .aleph_bft_finalized_items
        .iter()
        .all(|item| item.unit_index == 0));
}

#[tokio::test]
async fn private_journal_route_rejects_wrong_message_kind_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = signed_catch_up_request_with_kind(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationRestMessageKind::Status,
        FederationJournalCatchUpRequest::new(0),
    );

    let response = post_journal(State(state.clone()), Json(request))
        .await
        .expect_err("wrong message kind must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "wrong-kind catch-up requests must not persist the sender sequence"
    );
}
