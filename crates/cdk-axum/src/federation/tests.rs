use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aleph_bft::{default_delay_config, Network, NodeIndex, Recipient};
use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::cdk_database::{
    MintDatabase, MintProofsDatabase, MintQuotesDatabase, MintSignaturesDatabase,
};
use cdk::cdk_payment::{
    self, CreateIncomingPaymentResponse, DynMintPayment, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk::federation::{
    generate_trusted_dealer_configs, FederationAlephBftActivityRequest,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitPayload,
    FederationApplicationState, FederationConfigSnapshot, FederationConsensusItem,
    FederationConsensusRuntime, FederationDkgActivationRequest, FederationDkgCeremonyId,
    FederationDkgCeremonyRecord, FederationDkgCeremonyState, FederationDkgCommitmentRound,
    FederationDkgMemberKeysetShare, FederationDkgSetupMethod, FederationDkgTranscript,
    FederationDkgTranscriptHash, FederationDkgTranscriptSignature, FederationError,
    FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltQuoteMetadata,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationKind, FederationOperationStatus,
    FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationPolicy, FederationPaymentObservationRecord,
    FederationPaymentObservationReportAction, FederationPaymentVerificationPolicy,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate, MemberConfigPublic,
    MemberId, ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, MintQuote, QuoteId};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut01::{
    aggregate_blind_signature_shares, evaluate_secret_key_coefficients, BlsG1PublicKey,
    BlsSecretKey,
};
use cdk::nuts::nut22::AuthToken;
use cdk::nuts::{
    BlindedMessage, CheckStateRequest, CurrencyUnit, Id, MeltQuoteBolt11Request,
    MeltQuoteBolt11Response, MeltQuoteState, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteState, MintRequest, PaymentMethod, PublicKey, RestoreRequest, SecretKey,
};
use cdk::types::FeeReserve;
use cdk::Amount;
use cdk_common::database::mint::{CompletedOperationsDatabase, FederationApplicationDatabase};
use cdk_common::database::MintFederationJournalDatabase;
use cdk_common::mint::Operation;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
use cdk_http_client::{HttpError, RawResponse};
use futures::Stream;
use parity_scale_codec::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::ServiceExt;

use crate::auth::AuthHeader;

use super::aleph_bft_latency::{
    CdkAlephBftLatencyEvent, CdkAlephBftLatencyRecorder, CdkAlephBftLatencyStage,
};
use super::aleph_bft_network::CdkAlephBftNetworkInboundHandle;
use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick, run_cdk_aleph_bft_network_pump_tick_with_retry_queue,
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound,
    CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
    CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
};
use super::aleph_bft_runtime::CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE;
use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftKeychain, CdkAlephBftNetwork, CdkAlephBftNetworkData,
};
use super::aleph_bft_session::AlephBftSessionSupervisorState;
use super::peer_client::FederationRestPeerClientError;
use super::*;

mod aleph_bft_routes;
mod dkg_driver;
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
fn catch_up_certificate_quorum_prefix_stops_when_next_index_would_overflow() {
    let mut signers = BTreeSet::new();
    signers.insert(member_id(1));
    let mut quorums = BTreeMap::new();
    quorums.insert(
        0,
        vec![cdk::federation::FederationCatchUpCertificateQuorum {
            required: 1,
            signers,
            from_index: 0,
            latest_index: u64::MAX,
            operation_count: u64::MAX,
            range_order_digest: cdk::federation::FederationJournalDigest::genesis(),
            materialized_state_digest: cdk::federation::FederationMaterializedStateDigest::genesis(
            ),
        }],
    );

    let status =
        contiguous_catch_up_certificate_quorum_prefix(&quorums).expect("quorum prefix status");

    assert_eq!(status.from_index, 0);
    assert_eq!(status.latest_index, u64::MAX);
    assert_eq!(status.operation_count, u64::MAX);
}

#[test]
fn catch_up_certificate_quorum_prefix_retains_latest_certified_state_digest() {
    let signers = BTreeSet::from([member_id(1)]);
    let first_digest = cdk::federation::FederationMaterializedStateDigest::from_bytes([1; 32]);
    let latest_digest = cdk::federation::FederationMaterializedStateDigest::from_bytes([2; 32]);
    let mut quorums = BTreeMap::new();
    quorums.insert(
        0,
        vec![cdk::federation::FederationCatchUpCertificateQuorum {
            required: 1,
            signers: signers.clone(),
            from_index: 0,
            latest_index: 0,
            operation_count: 1,
            range_order_digest: cdk::federation::FederationJournalDigest::from_bytes([1; 32]),
            materialized_state_digest: first_digest,
        }],
    );
    quorums.insert(
        1,
        vec![cdk::federation::FederationCatchUpCertificateQuorum {
            required: 1,
            signers,
            from_index: 1,
            latest_index: 1,
            operation_count: 1,
            range_order_digest: cdk::federation::FederationJournalDigest::from_bytes([2; 32]),
            materialized_state_digest: latest_digest,
        }],
    );

    let status =
        contiguous_catch_up_certificate_quorum_prefix(&quorums).expect("quorum prefix status");

    assert_eq!(status.from_index, 0);
    assert_eq!(status.latest_index, 1);
    assert_eq!(status.operation_count, 2);
    assert_eq!(status.materialized_state_digest, latest_digest);
}

#[test]
fn catch_up_certificate_range_rejects_span_larger_than_available_entries() {
    let order_digest = cdk::federation::FederationJournalDigest::genesis();
    let range = CatchUpCertificateRangeKey {
        from_index: 0,
        operation_count: u64::MAX,
        latest_index: u64::MAX - 1,
        previous_order_digest: order_digest,
        range_order_digest: order_digest,
        latest_order_digest: order_digest,
        materialized_state_digest: cdk::federation::FederationMaterializedStateDigest::genesis(),
        operation_ids: Vec::new(),
    };

    assert!(entries_for_catch_up_certificate_range(&BTreeMap::new(), &range).is_none());
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
            catch_up_certificate_chunk_size: 0,
            ..valid
        },
        FederationPeerSyncConfig {
            catch_up_certificate_chunk_size: usize::from(u16::MAX) + 1,
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
        FederationPeerSyncConfig {
            aleph_bft_live_session_max_round: Some(0),
            ..valid
        },
        FederationPeerSyncConfig {
            aleph_bft_live_session_max_round: Some(9),
            aleph_bft_live_session_rotation_limit: Some(0),
            ..valid
        },
        FederationPeerSyncConfig {
            aleph_bft_live_session_rotation_limit: Some(2),
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

#[test]
fn aleph_bft_live_session_options_use_configured_sync_round_cap() {
    let pump_config = CdkAlephBftNetworkPumpConfig::default();
    let default_options =
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, Default::default());
    assert_eq!(default_options.max_round(), 3000);
    assert_eq!(default_options.automatic_rotation_limit(), None);

    let configured_options = FederationAlephBftLiveSessionOptions::from_sync_config(
        pump_config,
        FederationPeerSyncConfig {
            aleph_bft_live_session_max_round: Some(9),
            aleph_bft_live_session_rotation_limit: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(configured_options.max_round(), 9);
    assert_eq!(configured_options.automatic_rotation_limit(), Some(2));

    let first_rotated_options = configured_options.after_automatic_rotation();
    assert_eq!(first_rotated_options.max_round(), 9);
    assert_eq!(first_rotated_options.automatic_rotation_limit(), Some(1));

    let second_rotated_options = first_rotated_options.after_automatic_rotation();
    assert_eq!(second_rotated_options.max_round(), 3000);
    assert_eq!(second_rotated_options.automatic_rotation_limit(), None);
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
fn federation_peer_sync_initial_delay_evenly_phases_members() {
    let interval = Duration::from_secs(5);
    let phases = (0..5)
        .map(|rank| federation_peer_sync_initial_delay(interval, rank, 5))
        .collect::<Vec<_>>();

    assert_eq!(
        phases,
        vec![
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(3),
            Duration::from_secs(4),
        ]
    );
    assert!(phases.iter().all(|phase| *phase < interval));
    assert_eq!(
        federation_peer_sync_initial_delay(interval, 0, 1),
        Duration::ZERO
    );
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
    aleph_bft_network_message_response: Option<FederationAlephBftNetworkMessageResponse>,
    aleph_bft_network_message_delays: VecDeque<Duration>,
    aleph_bft_network_message_status_error: Option<(u16, String)>,
    dkg_setup_responses: VecDeque<FederationDkgSetupResponse>,
    catch_up_responses: VecDeque<FederationJournalCatchUpResponse>,
    status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
    dkg_commitment_requests: Vec<FederationSignedRequest<FederationDkgCommitmentRequest>>,
    dkg_reveal_requests: Vec<FederationSignedRequest<FederationDkgRevealRequest>>,
    dkg_secret_share_requests: Vec<FederationSignedRequest<FederationDkgSecretShareRequest>>,
    dkg_transcript_signature_requests:
        Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
    submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
    aleph_bft_network_message_requests:
        Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>,
    catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
}

#[derive(Debug, Clone, Default)]
struct MockFederationTransport {
    state: Arc<Mutex<MockFederationTransportState>>,
    aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
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
    shadow_routers: Arc<Mutex<BTreeMap<String, Router>>>,
    shadow_delivery_lock: Arc<tokio::sync::Mutex<()>>,
    missing_router_wait: Duration,
    faults: Arc<Mutex<RouterFederationTransportFaults>>,
    held_aleph_bft_network_message: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    aleph_bft_activity_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftActivityRequest>>>>,
    dkg_readiness_requests: Arc<Mutex<Vec<FederationSignedRequest<FederationDkgReadinessRequest>>>>,
    aleph_bft_network_message_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>>>,
    aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Default)]
struct RouterFederationTransportFaults {
    post_response_failures_after_delivery: BTreeMap<String, usize>,
    post_delays_before_delivery: BTreeMap<(String, MemberId), VecDeque<Duration>>,
    aleph_bft_network_message_delays: VecDeque<Duration>,
    aleph_bft_network_message_duplicate_replays: usize,
    aleph_bft_network_message_reorder_once: bool,
    aleph_bft_network_message_reorders_completed: usize,
    held_aleph_bft_data_senders: BTreeSet<MemberId>,
    blocked_federation_links: BTreeSet<(MemberId, MemberId)>,
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
        let shadow_router_count = self
            .shadow_routers
            .lock()
            .map(|routers| routers.len())
            .unwrap_or_default();
        f.debug_struct("RouterFederationTransportRegistry")
            .field("router_count", &router_count)
            .field("shadow_router_count", &shadow_router_count)
            .field("missing_router_wait", &self.missing_router_wait)
            .field(
                "faults",
                &self
                    .faults
                    .lock()
                    .map(|faults| {
                        (
                            faults.post_response_failures_after_delivery.len(),
                            faults.post_delays_before_delivery.len(),
                            faults.aleph_bft_network_message_delays.len(),
                            faults.aleph_bft_network_message_duplicate_replays,
                            faults.aleph_bft_network_message_reorder_once,
                            faults.aleph_bft_network_message_reorders_completed,
                            faults.held_aleph_bft_data_senders.len(),
                            faults.blocked_federation_links.len(),
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
                "aleph_bft_activity_requests",
                &self
                    .aleph_bft_activity_requests
                    .lock()
                    .map(|requests| requests.len())
                    .unwrap_or_default(),
            )
            .field(
                "dkg_readiness_requests",
                &self
                    .dkg_readiness_requests
                    .lock()
                    .map(|requests| requests.len())
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
    fn fail_post_responses_after_delivery(&self, path_suffix: &str, count: usize) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_response_failures_after_delivery
            .insert(path_suffix.to_string(), count);
    }

    fn take_post_response_failure_after_delivery(&self, path: &str) -> bool {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        let Some((_, remaining)) = faults
            .post_response_failures_after_delivery
            .iter_mut()
            .find(|(suffix, remaining)| path.ends_with(suffix.as_str()) && **remaining > 0)
        else {
            return false;
        };
        *remaining = remaining.saturating_sub(1);
        true
    }

    fn delay_next_posts_before_delivery<I>(&self, path_suffix: &str, receiver: MemberId, delays: I)
    where
        I: IntoIterator<Item = Duration>,
    {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_delays_before_delivery
            .entry((path_suffix.to_string(), receiver))
            .or_default()
            .extend(delays);
    }

    fn take_post_delay_before_delivery(&self, path: &str, receiver: MemberId) -> Duration {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_delays_before_delivery
            .iter_mut()
            .find(|((suffix, configured_receiver), delays)| {
                *configured_receiver == receiver && path.ends_with(suffix) && !delays.is_empty()
            })
            .and_then(|(_, delays)| delays.pop_front())
            .unwrap_or_default()
    }

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

    async fn wait_for_aleph_bft_network_message_request_matching<F>(
        &self,
        matches: F,
    ) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest>
    where
        F: Fn(&FederationSignedRequest<FederationAlephBftNetworkMessageRequest>) -> bool,
    {
        loop {
            let changed = self
                .aleph_bft_network_message_changed
                .clone()
                .notified_owned();
            if let Some(request) = self
                .aleph_bft_network_message_requests()
                .into_iter()
                .find(|request| matches(request))
            {
                return request;
            }
            changed.await;
        }
    }

    fn aleph_bft_activity_request_count(&self) -> usize {
        self.aleph_bft_activity_requests
            .lock()
            .expect("router transport activity requests lock")
            .len()
    }

    fn dkg_readiness_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgReadinessRequest>> {
        self.dkg_readiness_requests
            .lock()
            .expect("router transport DKG readiness requests lock")
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

    fn remove(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .remove(&endpoint);
    }

    fn insert_shadow(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .insert(endpoint, router);
    }

    fn remove_shadow(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .remove(&endpoint);
    }

    fn set_member_outbound_partition(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        partitioned: bool,
    ) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        for receiver in public_config
            .members
            .keys()
            .copied()
            .filter(|receiver| *receiver != sender)
        {
            if partitioned {
                faults.blocked_federation_links.insert((sender, receiver));
            } else {
                faults.blocked_federation_links.remove(&(sender, receiver));
            }
        }
    }

    fn set_member_aleph_bft_data_hold(&self, sender: MemberId, held: bool) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if held {
            faults.held_aleph_bft_data_senders.insert(sender);
        } else {
            faults.held_aleph_bft_data_senders.remove(&sender);
        }
    }

    fn aleph_bft_data_is_held(&self, sender: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .held_aleph_bft_data_senders
            .contains(&sender)
    }

    fn federation_link_is_blocked(&self, sender: MemberId, receiver: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .blocked_federation_links
            .contains(&(sender, receiver))
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

    fn with_mock_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
    where
        I: IntoIterator<Item = Duration>,
    {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_delays
            .extend(delays);
        self
    }

    fn with_aleph_bft_network_message_status_error(
        self,
        status: u16,
        message: impl Into<String>,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_status_error = Some((status, message.into()));
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

    fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_requests
            .clone()
    }

    async fn wait_for_aleph_bft_network_message_request_matching<F>(
        &self,
        matches: F,
    ) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest>
    where
        F: Fn(&FederationSignedRequest<FederationAlephBftNetworkMessageRequest>) -> bool,
    {
        loop {
            let changed = self
                .aleph_bft_network_message_changed
                .clone()
                .notified_owned();
            if let Some(request) = self
                .aleph_bft_network_message_requests()
                .into_iter()
                .find(|request| matches(request))
            {
                return request;
            }
            changed.await;
        }
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

    async fn http_get_raw(
        &self,
        _url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed raw GET completed".to_string()))
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

    async fn http_post_form_raw<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        time::sleep(self.delay).await;
        Err(HttpError::Other(
            "delayed raw form POST completed".to_string(),
        ))
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

    async fn http_get_raw(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
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
        if url.path().ends_with("/federation/v1/aleph-bft/messages") {
            let request = serde_json::from_value::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(serde_json::to_value(payload)?)?;
            let (response, status_error, delay) = {
                let mut state = self.state.lock().expect("mock transport lock");
                state.aleph_bft_network_message_requests.push(request);
                (
                    state.aleph_bft_network_message_response.clone(),
                    state.aleph_bft_network_message_status_error.clone(),
                    state
                        .aleph_bft_network_message_delays
                        .pop_front()
                        .unwrap_or_default(),
                )
            };
            self.aleph_bft_network_message_changed.notify_waiters();
            if !delay.is_zero() {
                time::sleep(delay).await;
            }
            if let Some((status, message)) = status_error {
                return Err(HttpError::Status { status, message });
            }
            let response = response.ok_or_else(|| {
                HttpError::Other("missing AlephBFT network-message response".to_string())
            })?;
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

    async fn http_post_form_raw<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
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

    async fn http_get_raw(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
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

    async fn http_post_form_raw<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
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

    async fn http_get_raw(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
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
        let is_aleph_bft_activity = path.ends_with("/federation/v1/aleph-bft/activity");
        let is_aleph_bft_network_message = path.ends_with("/federation/v1/aleph-bft/messages");
        let is_dkg_readiness = path.ends_with("/federation/v1/dkg/readiness");
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
        let signed_request =
            serde_json::from_slice::<FederationSignedRequest<serde_json::Value>>(&payload_bytes)?;
        if is_aleph_bft_activity {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationAlephBftActivityRequest>,
            >(&payload_bytes)?;
            self.aleph_bft_activity_requests
                .lock()
                .expect("router transport activity requests lock")
                .push(signed_request);
        }
        if is_dkg_readiness {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationDkgReadinessRequest>,
            >(&payload_bytes)?;
            self.dkg_readiness_requests
                .lock()
                .expect("router transport DKG readiness requests lock")
                .push(signed_request);
        }
        let mut aleph_bft_message_contains_data = false;
        if is_aleph_bft_network_message {
            let signed_request = serde_json::from_slice::<
                FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
            >(&payload_bytes)?;
            aleph_bft_message_contains_data = signed_request
                .payload
                .message_bytes()
                .ok()
                .and_then(|bytes| {
                    let mut input = &bytes[..];
                    CdkAlephBftNetworkData::decode(&mut input)
                        .ok()
                        .filter(|_| input.is_empty())
                })
                .is_some_and(|message| !message.included_data().is_empty());
            self.aleph_bft_network_message_requests
                .lock()
                .expect("router transport network-message requests lock")
                .push(signed_request);
            self.aleph_bft_network_message_changed.notify_waiters();
        }
        if self.federation_link_is_blocked(signed_request.sender, signed_request.receiver) {
            return Err(HttpError::Other(format!(
                "federation link {} -> {} is partitioned",
                signed_request.sender, signed_request.receiver
            )));
        }
        let post_delay = self.take_post_delay_before_delivery(&path, signed_request.receiver);
        if !post_delay.is_zero() {
            tokio::time::sleep(post_delay).await;
        }
        if aleph_bft_message_contains_data && self.aleph_bft_data_is_held(signed_request.sender) {
            return Err(HttpError::Other(format!(
                "AlephBFT data messages from member {} are held",
                signed_request.sender
            )));
        }
        if is_aleph_bft_network_message {
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

        let shadow_router = if is_aleph_bft_activity || is_aleph_bft_network_message {
            self.shadow_routers
                .lock()
                .expect("shadow router registry lock")
                .get(&endpoint)
                .cloned()
        } else {
            None
        };
        let _shadow_delivery_guard = match shadow_router.as_ref() {
            Some(_) => Some(self.shadow_delivery_lock.lock().await),
            None => None,
        };

        let response_body_result = match shadow_router {
            Some(shadow_router) => {
                let (primary_result, shadow_result) = tokio::join!(
                    router_transport_post_once_bytes(&router, &path, payload_bytes.clone()),
                    router_transport_post_once_bytes(&shadow_router, &path, payload_bytes.clone()),
                );
                shadow_result?;
                primary_result
            }
            None => router_transport_post_once_bytes(&router, &path, payload_bytes.clone()).await,
        };
        if let Some(release_held_post) = release_after_post {
            if release_held_post.send(()).is_ok() {
                self.mark_aleph_bft_network_message_reorder_completed();
            }
        }
        let response_body = response_body_result?;
        if is_aleph_bft_network_message && self.take_aleph_bft_network_message_duplicate_replay() {
            let _ = router_transport_post_once_bytes(&router, &path, payload_bytes).await;
        }
        if self.take_post_response_failure_after_delivery(&path) {
            return Err(HttpError::Other(format!(
                "simulated response loss after durable POST delivery for {path}"
            )));
        }

        Ok(serde_json::from_slice(&response_body)?)
    }

    async fn http_post_form_raw<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(format!(
            "unexpected raw form POST path {}",
            url.path()
        )))
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

#[test]
fn private_federation_error_statuses_preserve_failure_class() {
    assert_eq!(
        federation_error_status(&FederationError::RuntimeUnavailable {
            component: "consensus",
        }),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        federation_error_status(&FederationError::PendingConsensusMempoolFull {
            pending: 4,
            max: 4,
        }),
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        federation_error_status(&FederationError::ConflictingOperation {
            conflict_key: "quote:test".to_string(),
            existing_operation_id: "existing".to_string(),
            attempted_operation_id: "attempted".to_string(),
        }),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        federation_error_status(&FederationError::AlephBftFinalizedItemRecoveryMismatch {
            reason: "stored state mismatch".to_string(),
        }),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        federation_error_status(&FederationError::InvalidMemberId(0)),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        federation_error_status(&FederationError::JournalConsensusEvidenceMissing { index: 1 }),
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn private_internal_federation_error_hides_diagnostic_detail() {
    let response =
        federation_error_response(FederationError::AlephBftFinalizedItemRecoveryMismatch {
            reason: "secret=internal-value".to_string(),
        });

    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        response_error_detail(response).await,
        "federation internal error"
    );
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

fn transport_config_with_member_federation_api_urls_and_keysets(
    threshold_params: ThresholdParams,
    federation_api_urls: Vec<MintUrl>,
    keysets: Vec<TrustedDealerKeysetSpec>,
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
    let bundle = generate_trusted_dealer_configs(threshold_params, members, keysets)
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

#[derive(Debug)]
struct CountingFakeWallet {
    inner: FakeWallet,
    make_payment_calls: Arc<AtomicUsize>,
}

impl CountingFakeWallet {
    fn with_call_counter() -> (DynMintPayment, Arc<AtomicUsize>) {
        let make_payment_calls = Arc::new(AtomicUsize::new(0));
        let wallet = Self {
            inner: FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 0.0,
                },
                HashMap::default(),
                HashSet::default(),
                0,
                CurrencyUnit::Sat,
            ),
            make_payment_calls: Arc::clone(&make_payment_calls),
        };

        (Arc::new(wallet), make_payment_calls)
    }
}

#[async_trait]
impl MintPayment for CountingFakeWallet {
    type Err = cdk_payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.make_payment_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.make_payment(unit, options).await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.check_outgoing_payment(payment_identifier).await
    }
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
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        fake,
        true,
    )
    .await
}

async fn federation_route_state_for_private_with_db_and_aleph_bft(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
) -> MintState {
    let fake: DynMintPayment = Arc::new(FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    ));
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        fake,
        false,
    )
    .await
}

async fn federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
) -> MintState {
    federation_route_state_for_private_with_db_payment_and_finality(
        public_config,
        local_private,
        db,
        seed,
        payment_processor,
        false,
    )
    .await
}

async fn federation_route_state_for_private_with_db_payment_and_finality(
    public_config: FederationConfig,
    local_private: MemberConfigPrivate,
    db: Arc<cdk_sqlite::MintSqliteDatabase>,
    seed: &[u8],
    payment_processor: DynMintPayment,
    local_finality_test: bool,
) -> MintState {
    let runtime_config =
        cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
            .expect("runtime config");
    let mut builder = MintBuilder::new(db.clone())
        .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
        .with_federation_config(runtime_config.clone());
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            payment_processor,
        )
        .await
        .expect("payment processor");
    let mint = builder.build_with_seed(db, seed).await.expect("mint");
    let consensus = FederationConsensusRuntime::authenticated_aleph_bft_from_finalized_items(
        runtime_config.public_config(),
        runtime_config.member_id(),
        0,
        Vec::new(),
    )
    .expect("AlephBFT consensus runtime");
    let mint = Arc::new(mint);
    MintState {
        mint: Arc::clone(&mint),
        cache: Arc::new(crate::cache::HttpCache::default()),
        federation: crate::federation::FederationNodeRuntime {
            mint: Some(mint),
            consensus: Some(Arc::new(tokio::sync::Mutex::new(consensus))),
            peer_sync_config: Some(FederationPeerSyncConfig::default()),
            dkg_auto_start_config: Some(Arc::new(FederationDkgAutoStartConfig {
                ceremony_id: FederationDkgCeremonyId::from_bytes([55; 32]),
                keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    [Amount::from(1)].into_iter().collect(),
                )
                .expect("test DKG keyset spec"),
                driver: FederationDkgDriverRunConfig::default(),
            })),
            sequences: Some(Arc::new(tokio::sync::Mutex::new(
                FederationRestSequenceTracker::new(),
            ))),
            sync: Some(Arc::new(tokio::sync::Mutex::new(if local_finality_test {
                crate::FederationSyncState::default()
            } else {
                crate::FederationSyncState::for_aleph_bft()
            }))),
            notifications: Some(Arc::new(crate::FederationOperationNotifications::default())),
            private_rest_admission: Some(Arc::new(tokio::sync::Semaphore::new(
                FederationPeerSyncConfig::default().private_rest_inbound_concurrency_limit,
            ))),
            dkg_member_admission: Some(crate::federation_dkg_member_admission(
                &public_config,
                FederationDkgDriverRunConfig::default().member_request_burst,
                FederationDkgDriverRunConfig::default().member_request_refill_interval,
            )),
            invalid_operation_alerts: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationInvalidOperationAlertTracker::default(),
            ))),
            payment_observer: crate::federation::FederationPaymentObserver::default(),
            fail_next_pending_operation_persistence: Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            suppress_next_terminal_response_publication: Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            local_finality_test,
            manual_finality_test: false,
        },
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
        .with_signed_catch_up_certificate(private_config)
        .expect("signed catch-up response")
}

fn finalized_aleph_bft_history<I>(
    public_config: &FederationConfig,
    submitting_member: MemberId,
    envelopes: I,
) -> FederationConsensusRuntime
where
    I: IntoIterator<Item = FederationOperationEnvelope>,
{
    let mut adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        submitting_member,
        0,
        public_config.members.keys().copied(),
    )
    .expect("test AlephBFT adapter");
    for (unit_index, envelope) in (0_u64..).zip(envelopes) {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            submitting_member,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("test AlephBFT finalized unit");
        adapter
            .finalize_unit(unit)
            .expect("finalize test AlephBFT history");
    }
    adapter
}

fn test_aleph_bft_peer(
    federation_id: cdk::federation::FederationId,
    member_id: MemberId,
) -> FederationConsensusRuntime {
    FederationConsensusRuntime::new(federation_id, member_id, 0)
}

fn aleph_bft_catch_up_response(
    public_config: &FederationConfig,
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

fn test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let status = adapter.status();
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        status.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

fn checked_test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> Result<FederationJournalCatchUpResponse, FederationError> {
    if let Some(known_order_digest) = request.known_order_digest {
        if let Some(expected_digest) = adapter.journal().order_digest_before(request.next_index) {
            if expected_digest != known_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: request.next_index,
                    expected: expected_digest,
                    found: known_order_digest,
                });
            }
        }
    }

    Ok(test_catch_up_response(adapter, request))
}

fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> cdk::federation::FederationDkgParticipantCommitmentReveal {
    cdk::federation::FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        ceremony_id: cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
        member_id,
        keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec"),
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
        cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
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

async fn derive_and_persist_dkg_member_keyset_share(
    state: &MintState,
    public_config: &FederationConfig,
) -> FederationDkgMemberKeysetShare {
    let db = state.mint.localstore();
    let commitments = db
        .get_federation_dkg_commitments()
        .await
        .expect("DKG commitments")
        .into_iter()
        .map(|commitment| (commitment.member_id, commitment))
        .collect();
    let round = FederationDkgCommitmentRound::new(public_config, commitments)
        .expect("complete DKG commitment round");
    let reveals = db
        .get_federation_dkg_reveals()
        .await
        .expect("DKG reveals")
        .into_iter()
        .map(|reveal| (reveal.member_id, reveal))
        .collect::<BTreeMap<_, _>>();
    round
        .validate_reveals(public_config, &reveals)
        .expect("complete DKG reveals");
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("DKG secret-share contributions")
        .into_iter()
        .filter(|contribution| contribution.receiver == local_member)
        .map(|contribution| (contribution.sender, contribution))
        .collect::<BTreeMap<_, _>>();
    let keyset_share = round
        .derive_member_keyset_share(public_config, &reveals, local_member, &contributions)
        .expect("driver-derived member keyset share");
    let mut tx = db.begin_transaction().await.expect("begin keyset share");
    tx.add_federation_dkg_member_keyset_share(&keyset_share)
        .await
        .expect("persist driver-derived keyset share");
    tx.commit()
        .await
        .expect("commit driver-derived keyset share");
    keyset_share
}

async fn durable_dkg_transcript_fixture() -> (
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
    let keyset_share =
        derive_and_persist_dkg_member_keyset_share(&state, &setup_public_config).await;
    let mut final_public_config = FederationConfig::new(
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset.clone()],
    )
    .expect("final public config");
    final_public_config.payment_observation_policy =
        setup_public_config.payment_observation_policy.clone();
    let public_ceremonies = vec![keyset_share.public_ceremony];
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
    )
    .expect("durable transcript hash");

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
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .peer_status()
        .operation_count
}

fn reset_federation_runtime_journal(state: &mut MintState) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    state.federation.consensus = Some(Arc::new(tokio::sync::Mutex::new(
        FederationConsensusRuntime::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            0,
        ),
    )));
}

fn activate_local_test_finality(state: &mut MintState) {
    state.federation.local_finality_test = true;
}

async fn peer_status_with_materialized_digest(
    state: &MintState,
    consensus: &FederationConsensusRuntime,
) -> FederationPeerStatus {
    let adapter_status = consensus.status();
    let mut status =
        consensus.peer_status_for(adapter_status.federation_id, adapter_status.local_member);
    status.materialized_state_digest = Some(
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized peer status digest"),
    );
    status
}

async fn catch_up_response_with_materialized_digest(
    state: &MintState,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = test_catch_up_response(consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized catch-up state digest");
    response
}

async fn aleph_bft_peer_status_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    member_id: MemberId,
    consensus: &FederationConsensusRuntime,
) -> FederationPeerStatus {
    let mut status = consensus.peer_status_for(public_config.federation_id, member_id);
    status.materialized_state_digest = Some(
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT peer status digest"),
    );
    status
}

async fn aleph_bft_catch_up_response_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = aleph_bft_catch_up_response(public_config, consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT catch-up state digest");
    response
}

fn finalize_test_operation(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> Result<FederationSubmitOperationResponse, FederationError> {
    let status = consensus.status();
    let unit_index = consensus
        .finalized_items()
        .last()
        .map_or(0, |item| item.unit_index.saturating_add(1));
    let unit = FederationAlephBftUnitPayload::new(
        status.federation_id,
        status.local_member,
        status.session,
        unit_index,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("test AlephBFT finalized unit");
    let mut responses = consensus.finalize_unit(unit)?;
    Ok(responses
        .pop()
        .expect("one finalized response for one test operation"))
}

fn finalize_test_entry(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    finalize_test_operation(consensus, envelope)
        .expect("finalize test journal entry")
        .outcome
        .entry()
        .clone()
}

async fn persist_test_journal_entry(state: &MintState, entry: &FederationJournalEntry) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    let db = state.mint.localstore();
    let retained_items = db
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("load test AlephBFT finalized items");
    let mut adapter = FederationConsensusRuntime::recover_for_members_from_finalized_items(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        0,
        runtime_config.public_config().members.keys().copied(),
        retained_items.clone(),
    )
    .expect("recover test AlephBFT history");
    let unit = FederationAlephBftUnitPayload::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        0,
        retained_items.len() as u64,
        vec![FederationConsensusItem::operation(entry.envelope.clone())],
    )
    .expect("test AlephBFT finalized unit");
    adapter
        .finalize_unit(unit)
        .expect("finalize test journal entry");
    let finalized_entry = adapter
        .journal()
        .entries()
        .last()
        .expect("finalized test journal entry");
    assert_eq!(finalized_entry.operation_id, entry.operation_id);
    let new_items = &adapter.finalized_items()[retained_items.len()..];
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin journal transaction");
    tx.add_federation_aleph_bft_finalized_items(new_items)
        .await
        .expect("persist test AlephBFT finalized item");
    tx.add_federation_journal_entry(finalized_entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");
}

async fn persist_finalized_aleph_bft_history(
    state: &MintState,
    adapter: &FederationConsensusRuntime,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin finalized AlephBFT history transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized AlephBFT history");
    for entry in adapter.journal().entries() {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist finalized AlephBFT journal entry");
    }
    tx.commit()
        .await
        .expect("commit finalized AlephBFT history");
}

async fn add_blinded_message_conflict(
    state: &MintState,
    quote_id: Option<&QuoteId>,
    output: &BlindedMessage,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict transaction");
    tx.add_blinded_messages(
        quote_id,
        std::slice::from_ref(output),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .expect("persist conflicting blinded message");
    tx.commit().await.expect("commit conflict transaction");
}

async fn remove_blinded_message_conflict(state: &MintState, output: &BlindedMessage) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict cleanup transaction");
    tx.delete_blinded_messages(&[output.blinded_secret])
        .await
        .expect("remove conflicting blinded message");
    tx.commit().await.expect("commit conflict cleanup");
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
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize payment observation")
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
        .authorize(&private_member(1))
        .expect("reauthorize oversized payment observation")
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
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
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

fn mint_operation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    output_label: &[u8],
) -> (FederationOperationEnvelope, BlindedMessage) {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let output = BlindedMessage::new(
        Amount::from(1),
        keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(output_label)),
    );
    let envelope = config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: quote_id.to_string(),
                outputs: vec![output.clone()],
                signature: None,
            }),
        },
    ));

    (envelope, output)
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
        }),
    )
    .authorize(&private_member(observer))
    .expect("authorize member payment observation")
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
    proof_for_amount_with_aggregate_secret(
        state,
        amount,
        label,
        &aggregate_secret_for_amount(amount),
    )
}

fn proof_for_amount_with_aggregate_secret(
    state: &MintState,
    amount: u64,
    label: &str,
    aggregate_secret: &BlsSecretKey,
) -> cdk::nuts::Proof {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
    let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(aggregate_secret);

    cdk::nuts::Proof::new(
        Amount::from(amount),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

async fn proof_from_accepted_output(
    states: &[MintState],
    signature_threshold: usize,
    operation_id: FederationOperationId,
    output_index: u32,
    output: &BlindedMessage,
    secret: cdk::secret::Secret,
) -> cdk::nuts::Proof {
    let mut shares = Vec::with_capacity(signature_threshold);
    for state in states.iter().take(signature_threshold) {
        let records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, output_index)
            .await
            .expect("accepted output signature share lookup");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].blinded_secret, output.blinded_secret);
        assert!(records[0].accepted_operation_index.is_some());
        shares.push(records[0].share);
    }
    let signature = aggregate_blind_signature_shares(signature_threshold, &shares)
        .expect("accepted output aggregate signature");

    cdk::nuts::Proof::new(
        output.amount,
        output.keyset_id,
        secret,
        PublicKey::from(signature),
    )
}

fn elected_melt_payment_member(
    public_config: &FederationConfig,
    operation_id: FederationOperationId,
) -> MemberId {
    let members = public_config.members.keys().copied().collect::<Vec<_>>();
    let operation_bytes = operation_id.to_bytes();
    let mut selector_bytes = [0_u8; 8];
    selector_bytes.copy_from_slice(&operation_bytes[..8]);
    let selector = u64::from_be_bytes(selector_bytes);
    members[(selector % members.len() as u64) as usize]
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

fn test_melt_quote_operation(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
) -> FederationOperationEnvelope {
    let cdk::MeltQuoteRequest::Bolt11(bolt11) = &request else {
        panic!("federation route tests only construct bolt11 melt quotes");
    };
    let response = cdk::MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: quote.clone(),
        amount: Amount::from(
            bolt11
                .request
                .amount_milli_satoshis()
                .unwrap_or(1_000)
                .div_ceil(1_000),
        ),
        fee_reserve: Amount::from(1),
        state: MeltQuoteState::Unpaid,
        expiry: 4_000_000_000,
        payment_preimage: None,
        change: None,
        request: Some(bolt11.request.to_string()),
        unit: Some(bolt11.unit.clone()),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    });
    config.melt_quote_operation(
        quote,
        request,
        response,
        FederationMeltQuoteMetadata::default(),
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
    )
}

async fn submit_melt_payment_observation_from_backend(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationPaymentObservationRecord {
    let observer = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let observed = state
                .federation
                .payment_observer
                .observe_melt_quote(state, quote_id)
                .await
                .expect("probe and submit melt payment observation");
            if observed {
                return state
                    .mint
                    .localstore()
                    .get_federation_payment_observations(
                        FederationPaymentObservationContext::MeltQuote,
                        &quote_id.to_string(),
                    )
                    .await
                    .expect("melt payment observation lookup")
                    .into_iter()
                    .find(|record| record.observation.observer() == observer)
                    .expect("observer payment record");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("melt payment observation timeout")
}

fn matching_melt_payment_observation_envelope(
    state: &MintState,
    evidence: &FederationPaymentObservation,
) -> FederationOperationEnvelope {
    let FederationPaymentObservation::MeltQuote(evidence) = evidence else {
        panic!("expected melt payment evidence");
    };
    state
        .mint
        .federation_config()
        .expect("federation config")
        .melt_quote_payment_observation_with_metadata(
            evidence.quote.clone(),
            evidence.method.clone(),
            evidence.state,
            evidence.amount.clone(),
            evidence.fee_reserve.clone(),
            evidence.payment_proof.clone(),
            evidence.payment_id.clone(),
        )
}

fn test_operation_envelope(
    federation_id: FederationId,
    label: &str,
) -> FederationOperationEnvelope {
    let hash = BlsG1PublicKey::hash_to_curve(label.as_bytes()).to_bytes();
    let mut quote_bytes = [0u8; 16];
    quote_bytes.copy_from_slice(&hash[..16]);
    let quote = QuoteId::from(uuid::Uuid::from_bytes(quote_bytes)).to_string();
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote,
            observer: member_id(1),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(1),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(label.to_string()),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize test operation")
}

fn test_operation_envelope_for_state(
    state: &MintState,
    label: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    test_operation_envelope_for_config(config.public_config(), label)
}

fn test_operation_envelope_for_config(
    config: &FederationConfig,
    label: &str,
) -> FederationOperationEnvelope {
    test_operation_envelope(config.federation_id, label)
}

fn indexed_test_operation_envelope_for_config(
    config: &FederationConfig,
    index: u64,
) -> FederationOperationEnvelope {
    test_operation_envelope(
        config.federation_id,
        &format!("federation evidence operation {index}"),
    )
}

fn restore_request_for_config(config: &FederationConfig, label: &str) -> RestoreRequest {
    RestoreRequest {
        outputs: vec![cdk::nuts::BlindedMessage::new(
            Amount::ZERO,
            config.keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
        )],
    }
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
    let expected_member_id = member.member_id;
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

    assert!(matches!(
        err.downcast_ref::<FederationRestPeerClientError>(),
        Some(FederationRestPeerClientError::RequestTimeout {
            member_id,
            operation,
            timeout: found_timeout,
        }) if *member_id == expected_member_id
            && operation == "status request"
            && *found_timeout == timeout
    ));
}

#[tokio::test(start_paused = true)]
async fn rest_peer_client_applies_configured_operation_submission_timeout() {
    let (public_config, local_private, _) = transport_config();
    let federation_id = public_config.federation_id;
    let member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let expected_member_id = member.member_id;
    let timeout = Duration::from_secs(2);
    let request = FederationSubmitOperationRequest::new(test_operation_envelope(
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

    assert!(matches!(
        err.downcast_ref::<FederationRestPeerClientError>(),
        Some(FederationRestPeerClientError::RequestTimeout {
            member_id,
            operation,
            timeout: found_timeout,
        }) if *member_id == expected_member_id
            && operation == "operation submission"
            && *found_timeout == timeout
    ));
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

    assert!(matches!(
        err.downcast_ref::<FederationRestPeerClientError>(),
        Some(FederationRestPeerClientError::OutboundAdmissionFull {
            member_id,
            operation,
        }) if *member_id == member.member_id && operation == "status request"
    ));
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
        .get_federation_dkg_member_sequences()
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
async fn public_federation_config_route_returns_finalized_config_without_authentication() {
    let (state, public_config, _, _) = federation_route_state_with_local_private().await;
    let router = Router::new()
        .nest("/federation/v1", create_federation_router())
        .with_state(state);

    let old_response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/v1/config")
                .body(axum::body::Body::empty())
                .expect("old public config request"),
        )
        .await
        .expect("old public config response");
    assert_eq!(old_response.status(), axum::http::StatusCode::NOT_FOUND);

    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/federation/v1/config")
                .body(axum::body::Body::empty())
                .expect("public config request"),
        )
        .await
        .expect("public config response");

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("public config body");
    let decoded: FederationConfig =
        serde_json::from_slice(&body).expect("public federation config json");

    assert_eq!(decoded, public_config);
    let json = String::from_utf8(body.to_vec()).expect("public config utf8");
    assert!(!json.contains("identity_secret_key"));
    assert!(!json.contains("keyset_secret_shares"));
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
    let mut peer_consensus = test_aleph_bft_peer(federation_id, member.member_id);
    let submit_response =
        finalize_test_operation(&mut peer_consensus, envelope).expect("peer submit response");
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
    let mut peer_consensus = test_aleph_bft_peer(federation_id, member.member_id);
    let other_envelope = test_operation_envelope(
        federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut mismatched_response =
        finalize_test_operation(&mut peer_consensus, other_envelope).expect("peer submit response");
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
async fn admission_validation_uses_effective_config_after_keyset_rotation() {
    let threshold_params = ThresholdParams::new(2, 2, 2).expect("threshold params");
    let inactive_rotation = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        false,
        0,
        None,
        BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[51; 32]),
                BlsSecretKey::from_reduced_bytes(&[52; 32]),
            ],
        )]),
    );
    let (public_config, private_configs) =
        transport_config_with_member_federation_api_urls_and_keysets(
            threshold_params,
            vec![
                MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
                MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
            ],
            vec![
                TrustedDealerKeysetSpec::new(
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
                ),
                inactive_rotation,
            ],
        );
    let state = federation_route_state_for_private(
        public_config.clone(),
        private_configs
            .first()
            .expect("local private config")
            .clone(),
    )
    .await;
    let mut rotated_keyset = public_config
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    rotated_keyset.active = true;
    let runtime_config = state.mint.federation_config().expect("federation config");
    let rotation = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: rotated_keyset.clone(),
        },
    ));
    state
        .submit_federation_envelope(rotation)
        .await
        .expect("submit keyset rotation");

    let output = BlindedMessage::new(
        Amount::from(1),
        rotated_keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"rotated admission output")),
    );
    let swap = runtime_config.operation_envelope(FederationOperation::Swap(
        cdk::federation::FederationSwapOperation {
            request: cdk::nuts::SwapRequest::new(
                vec![proof_for_amount(&state, 1, "rotated-admission-input")],
                vec![output],
            ),
        },
    ));

    let static_err = state
        .mint
        .federation_config()
        .expect("static federation config")
        .validate_federated_operation_submission(&swap.operation)
        .expect_err("startup config should still see the rotation keyset as inactive");
    assert!(matches!(
        static_err,
        cdk::Error::Federation(FederationError::InactiveFederatedKeyset { keyset_id })
            if keyset_id == rotated_keyset.id
    ));

    state
        .validate_federation_envelope_admission(&swap)
        .await
        .expect("effective admission should accept rotated keyset outputs");
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
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let witness_member = public_config.member(member_id(3)).expect("witness member");
    let first = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    let peer_consensus =
        finalized_aleph_bft_history(&public_config, peer_member.member_id, [first, second]);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = aleph_bft_catch_up_response(
        &public_config,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    );
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
        peer_consensus
            .peer_status_for(public_config.federation_id, peer_member.member_id)
            .latest_order_digest
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
        .get_federation_catch_up_certificates()
        .await
        .expect("persisted catch-up certificates");
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
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    let peer_consensus = finalized_aleph_bft_history(
        &public_config,
        peer_member.member_id,
        [first, second.clone()],
    );
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = aleph_bft_catch_up_response(
        &public_config,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    );

    let peer_second_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_member.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(second)],
    )
    .expect("peer second finalized unit");
    state
        .apply_federation_aleph_bft_finalized_unit(peer_second_unit)
        .await
        .expect("background sync applied exact peer unit first");
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private))
        .with_catch_up_response(witness_response);
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
        .get_federation_catch_up_certificates()
        .await
        .expect("persisted catch-up certificates");
    assert_eq!(persisted_outcomes.len(), 2);
    assert!(persisted_outcomes
        .iter()
        .all(|outcome| outcome.from_index == 1));
    assert_eq!(
        persisted_outcomes
            .iter()
            .map(|outcome| outcome.signer)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([peer_private.member_id, witness_private.member_id])
    );
    assert_eq!(transport.catch_up_requests()[0].payload.next_index, 2);
}

#[tokio::test]
async fn rest_peer_client_rejects_unsigned_nonempty_catch_up_response() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    state
        .submit_federation_envelope(first.clone())
        .await
        .expect("local first operation");
    finalize_test_operation(&mut peer_consensus, first).expect("peer first operation");
    finalize_test_operation(&mut peer_consensus, second).expect("peer second operation");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("peer catch-up response");
    assert!(catch_up_response.catch_up_certificate.is_none());
    let transport = MockFederationTransport::default().with_catch_up_response(catch_up_response);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let err = client
        .catch_up_state_from_member(&state, peer_member)
        .await
        .expect_err("unsigned response must fail");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::CatchUpCertificateMismatch { .. })
    ));
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
    let envelope = test_operation_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let blocked = state.submit_federation_envelope(envelope.clone()).await;

    assert!(matches!(
        blocked,
        Err(cdk::Error::Federation(FederationError::NodeLagging {
            peer,
            local_operation_count: 0,
            peer_operation_count: 1,
        })) if peer == peer_member.member_id
    ));
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
    let restore_request = restore_request_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

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
async fn divergent_member_refuses_public_submit() {
    let (public_config, _, secondary_private) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), secondary_private).await;
    let primary_member = public_config.member(member_id(1)).expect("primary member");
    state
        .mark_federation_materialized_state_mismatch(primary_member.member_id, 1)
        .await;
    let envelope = test_operation_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("lagging non-primary member must not forward public submits");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeMaterializedStateMismatch {
            peer,
            operation_count: 1,
        }) if peer == primary_member.member_id
    ));
}

#[tokio::test]
async fn rest_peer_client_sync_requires_quorum_evidence_before_public_submit() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = test_operation_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let peer_consensus =
        finalized_aleph_bft_history(&public_config, peer_member.member_id, [peer_entry]);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = aleph_bft_catch_up_response_with_materialized_digest(
        &state,
        &public_config,
        &peer_consensus,
        &catch_up_request,
    )
    .await;
    let peer_status = aleph_bft_peer_status_with_materialized_digest(
        &state,
        &public_config,
        peer_member.member_id,
        &peer_consensus,
    )
    .await;
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
        Some(crate::FederationSyncStatus::VerifyingCatchUpCertificate {
            operation_count: 1,
            ..
        })
    ));

    let next = test_operation_envelope_for_state(
        &state,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let blocked = state.submit_federation_envelope(next).await;
    assert!(matches!(
        blocked,
        Err(cdk::Error::Federation(
            FederationError::NodeVerifyingCatchUpCertificate {
                peer,
                operation_count: 1,
            }
        )) if peer == peer_member.member_id
    ));
}

#[tokio::test]
async fn peer_sync_round_catches_up_from_configured_peer() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = test_operation_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let peer_consensus =
        finalized_aleph_bft_history(&public_config, peer_member.member_id, [peer_entry]);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = aleph_bft_catch_up_response_with_materialized_digest(
        &state,
        &public_config,
        &peer_consensus,
        &catch_up_request,
    )
    .await;
    let peer_status = aleph_bft_peer_status_with_materialized_digest(
        &state,
        &public_config,
        peer_member.member_id,
        &peer_consensus,
    )
    .await;
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

    let next = test_operation_envelope_for_state(
        &state,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    state
        .submit_federation_envelope(next)
        .await
        .expect("quorum-verified member can submit");
}

#[tokio::test]
async fn equal_height_peer_sync_does_not_enter_catch_up() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        peer_member.member_id,
        0,
        None,
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    let transport = MockFederationTransport::default().with_status_response(status);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("peer client");

    let synced = client
        .sync_once_from_member(&state, peer_member)
        .await
        .expect("equal-height sync poll");

    assert_eq!(synced.operation_count, 0);
    assert!(transport.catch_up_requests().is_empty());
    assert_eq!(
        state
            .federation_sync_transition_stats()
            .await
            .expect("sync transition stats"),
        crate::FederationSyncTransitionStats::default()
    );
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
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

    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
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
    let entry = test_operation_envelope_for_config(
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
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    let operation_count = 3;
    for index in 0..operation_count {
        let entry = indexed_test_operation_envelope_for_config(&public_config, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        finalize_test_operation(&mut peer_consensus, entry).expect("peer accepted operation");
    }
    state
        .mark_federation_verifying_catch_up_certificate(peer_member.member_id, operation_count)
        .await;

    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default().with_status_response(peer_status);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig {
            catch_up_certificate_chunk_size: 1,
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
async fn peer_sync_round_collects_catch_up_certificate_from_equal_peer() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    state
        .submit_federation_envelope(entry.clone())
        .await
        .expect("local accepted operation");
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, entry).expect("peer accepted operation");

    assert!(catch_up_certificate_quorum_status(&state.mint)
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
    let quorum = catch_up_certificate_quorum_status(&state.mint)
        .await
        .expect("quorum status")
        .expect("quorum evidence");
    assert_eq!(quorum.operation_count, 1);
    assert_eq!(quorum.signers.len(), 2);
    assert!(quorum.signers.contains(&local_private.member_id));
    assert!(quorum.signers.contains(&peer_private.member_id));
}

#[tokio::test]
async fn peer_sync_rejects_mismatched_catch_up_certificate_without_persisting_it() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    state
        .submit_federation_envelope(entry.clone())
        .await
        .expect("local accepted operation");
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, entry).expect("peer accepted operation");
    state
        .mark_federation_verifying_catch_up_certificate(peer_member.member_id, 1)
        .await;

    let catch_up_request = FederationJournalCatchUpRequest::new(0)
        .with_limit(1)
        .with_known_order_digest(FederationJournalDigest::genesis());
    let catch_up_response =
        catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
            .await;
    let mut catch_up_response = signed_catch_up_response(catch_up_response, &peer_private);
    let mismatched_operation_id = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    )
    .operation_id()
    .expect("mismatched operation id");
    catch_up_response
        .catch_up_certificate
        .as_mut()
        .expect("signed catch-up certificate")
        .operation_ids[0] = mismatched_operation_id;

    let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_catch_up_response(catch_up_response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    assert_eq!(transport.catch_up_requests().len(), 1);
    let persisted = state
        .mint
        .localstore()
        .get_federation_catch_up_certificates()
        .await
        .expect("persisted outcomes");
    assert!(
        persisted
            .iter()
            .all(|outcome| outcome.signer != peer_private.member_id),
        "mismatched peer evidence must never be persisted"
    );
    assert!(catch_up_certificate_quorum_status(&state.mint)
        .await
        .expect("quorum status")
        .is_none());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::VerifyingCatchUpCertificate { .. })
    ));
}

#[tokio::test]
async fn peer_sync_round_collects_configured_chunked_catch_up_certificate() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    let chunk_size = 2;
    let operation_count = chunk_size + 1;
    for index in 0..operation_count as u64 {
        let entry = indexed_test_operation_envelope_for_config(&public_config, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        finalize_test_operation(&mut peer_consensus, entry).expect("peer accepted operation");
    }
    state
        .mark_federation_verifying_catch_up_certificate(
            peer_member.member_id,
            operation_count as u64,
        )
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
            catch_up_certificate_chunk_size: chunk_size,
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
    let quorum = catch_up_certificate_quorum_status(&state.mint)
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
async fn peer_sync_round_collects_only_unverified_catch_up_certificate_suffix() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut peer_consensus = test_aleph_bft_peer(federation_id, peer_member.member_id);
    let chunk_size = 2;
    let operation_count = chunk_size + 1;
    for index in 0..operation_count as u64 {
        let entry = indexed_test_operation_envelope_for_config(&public_config, index);
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        finalize_test_operation(&mut peer_consensus, entry).expect("peer accepted operation");
    }

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let local_status = local_peer_status(&state).await.expect("local status");
    let runtime_config = state.mint.federation_config().expect("federation config");
    let local_chunks = persist_local_catch_up_certificate_chunks(
        &state,
        runtime_config,
        &entries[..chunk_size],
        local_status.latest_order_digest,
        local_status
            .materialized_state_digest
            .expect("local status has materialized-state digest"),
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
        .persist_federation_catch_up_certificate(
            first_response
                .catch_up_certificate
                .as_ref()
                .expect("first peer catch-up certificate"),
        )
        .await
        .expect("persist first peer catch-up certificate");

    let prefix = catch_up_certificate_quorum_status(&state.mint)
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
            catch_up_certificate_chunk_size: chunk_size,
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
    let quorum = catch_up_certificate_quorum_status(&state.mint)
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
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
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
    let (mut state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut state,
        FederationPeerSyncConfig::default(),
        FederationAlephBftLiveSessionOptions::default(),
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start live session before digest mismatch")
    .expect("live session supervisor");
    assert!(state.federation_aleph_bft_live_session_active().await);
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let peer_consensus =
        finalized_aleph_bft_history(&public_config, peer_member.member_id, [peer_entry]);
    state
        .submit_federation_envelope(local_entry)
        .await
        .expect("local accepted operation");
    let transport = MockFederationTransport::default().with_status_response(
        peer_consensus.peer_status_for(public_config.federation_id, peer_member.member_id),
    );
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    assert!(sync.is_err());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { .. })
    ));
    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);
    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn rest_peer_client_rejects_aleph_bft_status_without_finalized_cursor() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut status = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    status.aleph_bft_next_item_index = None;
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    let err = sync.expect_err("missing AlephBFT cursor should fail");
    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::AlephBftFinalizedCursorMissing { member_id })
            if *member_id == peer_member.member_id
    ));
}

#[tokio::test]
async fn rest_peer_client_uses_status_as_cheap_order_status() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::from_bytes([9; 32]),
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
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let peer_consensus =
        finalized_aleph_bft_history(&public_config, peer_member.member_id, [peer_entry]);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let mut catch_up_response =
        aleph_bft_catch_up_response(&public_config, &peer_consensus, &catch_up_request);
    catch_up_response.materialized_state_digest =
        cdk::federation::FederationMaterializedStateDigest::from_bytes([9; 32]);
    let witness_response = signed_catch_up_response(catch_up_response.clone(), &witness_private);
    let transport = MockFederationTransport::default()
        .with_status_response(
            peer_consensus.peer_status_for(public_config.federation_id, peer_member.member_id),
        )
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted quote");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_reports_conflicting_payment_observations() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let first_observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let conflicting_observation = mint_payment_observation_envelope_for_member(
        &state,
        &quote_id,
        2,
        2,
        "different-settlement",
    );
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    for envelope in [
        mint_quote_envelope(&state, &quote_id),
        first_observation,
        conflicting_observation,
    ] {
        finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted operation");
    }
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    assert_eq!(applied.len(), 3);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));

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
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
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
    let envelope = test_melt_quote_operation(&config, quote_id.clone(), request);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt quote");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted swap");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    for envelope in [first_envelope, conflicting_envelope, later_envelope] {
        finalize_test_operation(&mut peer_consensus, envelope)
            .expect("peer accepted swap operation");
    }
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].operation_id, conflicting_operation_id);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status(), FederationOperationStatus::Applied);

    let runtime_consensus = local_consensus.lock().await;
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(first_operation_id)
            .expect("first runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(conflicting_operation_id)
            .expect("conflicting runtime entry")
            .status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(later_operation_id)
            .expect("later runtime entry")
            .status(),
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
    let payment_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some(String::new()),
    );
    let payment_operation_id = payment_envelope
        .operation_id()
        .expect("payment operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt");
    finalize_test_operation(&mut peer_consensus, payment_envelope)
        .expect("peer accepted melt payment observation");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    assert_eq!(persisted.len(), 2);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let runtime_consensus = local_consensus.lock().await;
    for operation_id in [operation_id, payment_operation_id] {
        assert_eq!(
            runtime_consensus
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status(),
            FederationOperationStatus::Applied
        );
    }
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");
    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let runtime = local_consensus.lock().await;
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(melt_operation_id)
            .expect("melt runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(paid_operation_id)
            .expect("paid runtime entry")
            .status(),
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    assert_eq!(
        state
            .federation
            .consensus
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

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let replayed_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus");
    let paid_response = checked_test_catch_up_response(
        &peer_consensus,
        &replayed_consensus.lock().await.catch_up_request(),
    )
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
            .federation
            .consensus
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    assert_eq!(
        state
            .federation
            .consensus
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

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let replayed_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus");
    let failed_response = checked_test_catch_up_response(
        &peer_consensus,
        &replayed_consensus.lock().await.catch_up_request(),
    )
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
            .federation
            .consensus
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation.consensus.as_ref().expect("consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &local_consensus.lock().await.catch_up_request(),
        )
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
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation
            .consensus
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

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = {
        let replayed_consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &replayed_consensus.lock().await.catch_up_request(),
        )
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
            .federation
            .consensus
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation.consensus.as_ref().expect("consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &local_consensus.lock().await.catch_up_request(),
        )
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
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation
            .consensus
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

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = {
        let replayed_consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &replayed_consensus.lock().await.catch_up_request(),
        )
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
            .federation
            .consensus
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");
    finalize_test_operation(&mut peer_consensus, unpaid_envelope)
        .expect("peer accepted unpaid observation");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
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
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);

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
    let melt_quote_envelope = test_melt_quote_operation(
        &config,
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
            .submit_federation_envelope_to_consensus(envelope.clone())
            .await
            .expect("peer accepted operation");
        finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted operation");
    }

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
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
        peer_consensus.journal().latest_order_digest()
    );

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("restart federation runtime from durable catch-up state");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    activate_local_test_finality(&mut state);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 6);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));

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
        .submit_federation_envelope_to_consensus(melt_payment_envelope.clone())
        .await
        .expect("peer accepted delayed paid melt observation");
    finalize_test_operation(&mut peer_consensus, melt_payment_envelope)
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
    let conflicting_blinded_secret = conflicting_output.blinded_secret;
    let conflicting_mint = config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id,
            outputs: vec![conflicting_output],
            signature: None,
        }))
        .expect("conflicting mint envelope");
    let conflicting_operation_id = conflicting_mint
        .operation_id()
        .expect("conflicting mint operation id");
    let conflict = state.submit_federation_envelope(conflicting_mint).await;
    assert!(conflict.is_err());
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after conflicting mint");
    assert_eq!(entries.len(), 8);
    assert_eq!(entries[7].operation_id, conflicting_operation_id);
    assert_eq!(entries[7].status(), FederationOperationStatus::Rejected);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[conflicting_blinded_secret])
            .await
            .expect("conflicting mint signature lookup"),
        vec![None]
    );
}

#[tokio::test]
async fn diverged_member_rejects_peer_catch_up_digest_mismatch() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let local_entry = test_operation_envelope_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, peer_entry).expect("peer accepted operation");
    state
        .submit_federation_envelope(local_entry)
        .await
        .expect("local accepted divergent operation");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let request = local_consensus.lock().await.catch_up_request();
    assert_eq!(request.next_index, 1);
    assert!(matches!(
        checked_test_catch_up_response(&peer_consensus, &request),
        Err(FederationError::CatchUpDigestMismatch { .. })
    ));

    let unchecked_response = test_catch_up_response(&peer_consensus, &request);
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
    conflicting_config.payment_observation_policy.mint_quotes =
        FederationPaymentVerificationPolicy::member_quorum(
            conflicting_config.threshold_params.consensus_threshold,
        );
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
async fn federation_runtime_state_replays_persisted_journal_and_dkg_sequences() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    let envelope = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let response = state
        .submit_federation_envelope(envelope)
        .await
        .expect("persist operation");
    state
        .persist_federation_dkg_sequence(peer_member, 42)
        .await
        .expect("persist DKG sequence");

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
    assert_eq!(sequences.dkg_sequences().get(&peer_member), Some(&42));
}

#[tokio::test]
async fn federation_runtime_startup_skips_terminal_rejected_entries() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut conflict = envelope.clone();
    let FederationOperation::MintQuote(operation) = &mut conflict.operation else {
        panic!("mint quote operation");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("bolt11 mint quote response");
    };
    response.request.push_str("-conflict");
    let adapter = finalized_aleph_bft_history(
        state
            .mint
            .federation_config()
            .expect("federation config")
            .public_config(),
        member_id(1),
        [envelope, conflict],
    );
    let accepted_entry = adapter.journal().entries()[0].clone();
    let rejected_entry = adapter.journal().entries()[1].clone();
    assert_eq!(accepted_entry.status(), FederationOperationStatus::Accepted);
    assert_eq!(rejected_entry.status(), FederationOperationStatus::Rejected);

    persist_finalized_aleph_bft_history(&state, &adapter).await;

    let (consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("terminal rejected entry must not block startup");
    let consensus = consensus.expect("consensus");
    let consensus = consensus.lock().await;
    let recovered = consensus
        .journal()
        .entry_by_operation_id(rejected_entry.operation_id)
        .expect("recovered rejected entry");
    assert_eq!(recovered.status(), FederationOperationStatus::Rejected);
    assert_eq!(consensus.peer_status().operation_count, 2);
    drop(consensus);
    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("accepted quote applied during startup");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
}

#[tokio::test]
async fn federation_runtime_startup_reloads_aleph_bft_finalized_items_fail_closed() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut adapter = FederationConsensusRuntime::for_members(
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

    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].operation_id,
        entry.operation_id
    );
    assert_eq!(
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn router_startup_replays_aleph_bft_finalized_items_before_serving_local_status() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut adapter = FederationConsensusRuntime::for_members(
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
    assert_eq!(local_status.sync_state, "verifying_catch_up_certificate");
    assert!(
        local_status
            .sync_message
            .contains("verifying quorum catch-up certificates"),
        "unexpected sync message: {}",
        local_status.sync_message
    );
    assert!(local_status.aleph_bft_live_session_active);
    assert!(local_status.aleph_bft_network_inbound_active);
    let aleph_bft_status = local_status
        .aleph_bft_status
        .expect("AlephBFT local status");
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
        replayed_entries[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_replays_finalized_items_and_applies_entries() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(base_response, &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let applied = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
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
    assert_eq!(
        stored_entries[0].status(),
        FederationOperationStatus::Applied
    );
    let stored_certificates = state
        .mint
        .localstore()
        .get_federation_catch_up_certificates()
        .await
        .expect("stored catch-up certificates");
    assert_eq!(stored_certificates.len(), 2);

    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_retries_failed_local_finalized_overlap() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let stale_quote_id = QuoteId::new();
    let stale_envelope = mint_quote_envelope(&state, &stale_quote_id);
    let stale_operation_id = stale_envelope.operation_id().expect("operation id");
    let suffix_quote_id = QuoteId::new();
    let suffix_envelope = mint_quote_envelope(&state, &suffix_quote_id);
    let suffix_operation_id = suffix_envelope.operation_id().expect("operation id");
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        vec![FederationConsensusItem::operation(stale_envelope)],
    )
    .expect("stale AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize stale peer unit");
    let suffix_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(suffix_envelope)],
    )
    .expect("suffix AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(suffix_unit)
        .expect("finalize suffix peer unit");

    let mut local_consensus =
        FederationConsensusRuntime::authenticated_aleph_bft_from_finalized_items(
            &public_config,
            local_private.member_id,
            0,
            peer_adapter.finalized_items()[..1].to_vec(),
        )
        .expect("local recovered AlephBFT consensus");
    local_consensus
        .update_status(stale_operation_id, FederationOperationStatus::Failed)
        .expect("mark local operation failed");
    {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("local consensus");
        *consensus.lock().await = local_consensus;
    }
    {
        let mut failed_entry = peer_adapter.journal().entries()[0].clone();
        failed_entry.application = FederationApplicationState::Failed {};
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("failed overlap transaction");
        tx.add_federation_aleph_bft_finalized_items(&peer_adapter.finalized_items()[..1])
            .await
            .expect("store finalized items");
        tx.add_federation_journal_entry(&failed_entry)
            .await
            .expect("store failed local journal entry");
        tx.commit().await.expect("commit failed overlap");
    }

    let request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    assert_eq!(
        base_response.entries.len(),
        1,
        "test must exercise one stale finalized-item overlap plus one new suffix"
    );
    assert_eq!(base_response.entries[0].operation_id, suffix_operation_id);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(base_response, &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let applied_suffix = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
        )
        .await
        .expect("retry failed local overlap");

    assert_eq!(applied_suffix.len(), 1);
    assert_eq!(applied_suffix[0].operation_id, suffix_operation_id);
    let replayed_stale_quote = state
        .mint
        .localstore()
        .get_mint_quote(&stale_quote_id)
        .await
        .expect("quote lookup after catch-up")
        .expect("stale mint quote replayed from finalized overlap");
    assert_eq!(
        replayed_stale_quote.request,
        format!("invoice-{stale_quote_id}")
    );
    let replayed_suffix_quote = state
        .mint
        .localstore()
        .get_mint_quote(&suffix_quote_id)
        .await
        .expect("suffix quote lookup after catch-up")
        .expect("suffix mint quote replayed from catch-up");
    assert_eq!(
        replayed_suffix_quote.request,
        format!("invoice-{suffix_quote_id}")
    );
    let stored_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(stale_operation_id)
        .await
        .expect("journal lookup")
        .expect("stored journal entry");
    assert_eq!(stored_entry.status(), FederationOperationStatus::Applied);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn aleph_bft_catch_up_advances_recovered_active_session() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("local consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .begin_aleph_bft_session(1)
            .expect("local stale session begins");
        assert_eq!(consensus.aleph_bft_status().expect("status").session, 1);
    }
    {
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("active session transaction");
        tx.set_federation_aleph_bft_active_session(1)
            .await
            .expect("persist stale active session");
        tx.commit().await.expect("commit stale active session");
    }

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        peer_private.member_id,
        2,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        2,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    peer_adapter
        .finalize_unit(unit)
        .expect("finalize peer unit");

    let request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(base_response, &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let applied = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
        )
        .await
        .expect("apply AlephBFT catch-up");

    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].operation_id, operation_id);
    let status = state
        .federation_aleph_bft_status()
        .await
        .expect("AlephBFT status after catch-up");
    assert_eq!(status.session, 2);
    assert_eq!(status.next_item_index, 1);
    let stored_active_session = state
        .mint
        .localstore()
        .get_federation_aleph_bft_active_session()
        .await
        .expect("stored active session");
    assert_eq!(stored_active_session, Some(2));
    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after catch-up")
        .expect("mint quote replayed from catch-up evidence");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
}

#[tokio::test]
async fn aleph_bft_catch_up_applies_rejected_trailing_entry() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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

    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(peer_adapter.finalized_items().to_vec(), true);
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(base_response, &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let applied = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
}

#[tokio::test]
async fn aleph_bft_peer_sync_catches_up_finalized_cursor_without_new_operations() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    assert_eq!(request.next_index, 1);
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    );
    response.materialized_state_digest =
        materialized_state_digest(&state, peer_adapter.journal().entries())
            .await
            .expect("materialized state digest");
    let response =
        response.with_aleph_bft_finalized_items(peer_adapter.finalized_items()[1..].to_vec(), true);
    let transport = MockFederationTransport::default()
        .with_status_response(
            peer_adapter.peer_status_for(public_config.federation_id, peer_private.member_id),
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
    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let base_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    );
    let response = signed_catch_up_response(base_response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(base_response, &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let err = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
        )
        .await
        .expect_err("journal-only AlephBFT catch-up should fail");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
    ));
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
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request()
        .with_limit(1);
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        1,
        100,
    );
    response.materialized_state_digest =
        materialized_state_digest(&state, peer_adapter.journal().entries())
            .await
            .expect("materialized state digest");
    response = response.with_aleph_bft_finalized_items(Vec::new(), false);
    let response = signed_catch_up_response(response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(response.clone(), &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let err = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
        )
        .await
        .expect_err("incomplete finalized unit page should fail");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
    ));
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
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let mut peer_adapter = FederationConsensusRuntime::for_members(
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
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    );
    response.materialized_state_digest =
        materialized_state_digest(&state, peer_adapter.journal().entries())
            .await
            .expect("materialized state digest");
    response = response.with_aleph_bft_finalized_items(reordered_finalized_items, true);
    let response = signed_catch_up_response(response.clone(), &peer_private);
    let witness_certificate = signed_catch_up_response(response.clone(), &local_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");

    let err = state
        .apply_federation_catch_up_response_with_catch_up_certificate_quorum(
            &response,
            &[witness_certificate],
        )
        .await
        .expect_err("reordered finalized items should fail");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
    ));
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
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let finalized_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mismatched_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mut adapter = FederationConsensusRuntime::for_members(
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
        test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let mismatched_entry = finalize_test_operation(&mut mismatched_consensus, mismatched_envelope)
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
    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
    ));
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_persists_applies_and_notifies() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
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
    assert_eq!(
        stored_entries[0].status(),
        FederationOperationStatus::Applied
    );

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn federation_aleph_bft_duplicate_finalized_unit_applies_unmaterialized_entries() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let (unit, finalized_items, entries) = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        next_consensus
            .submit_aleph_bft_envelope(envelope)
            .expect("queue AlephBFT envelope");
        let unit = next_consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit");
        let finalized_item_start = next_consensus
            .aleph_bft_finalized_items()
            .expect("finalized items before replay")
            .len();
        let responses = next_consensus
            .finalize_aleph_bft_unit(unit.clone())
            .expect("finalize AlephBFT unit");
        let finalized_items = next_consensus
            .aleph_bft_finalized_items()
            .expect("finalized items after replay")[finalized_item_start..]
            .to_vec();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        *consensus = next_consensus;
        (unit, finalized_items, entries)
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Accepted);

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin partial finalized-unit transaction");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist finalized items");
    for entry in &entries {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist accepted journal entry");
    }
    tx.commit()
        .await
        .expect("commit partial finalized-unit transaction");
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before duplicate replay")
        .is_none());

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("duplicate finalized unit replays accepted materialization");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].operation_id, operation_id);
    assert_eq!(
        responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after duplicate replay")
        .expect("quote materialized");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after duplicate replay");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Applied);
}

#[tokio::test]
async fn duplicate_finalized_unit_recovers_lost_applied_response_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
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
            .expect("notified applied operation")
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    state
        .federation
        .suppress_next_terminal_response_publication
        .store(true, Ordering::SeqCst);

    let first_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("durably apply finalized unit before publication interruption");
    assert_eq!(first_responses.len(), 1);
    assert_eq!(
        first_responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert!(!waiter.is_finished());
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after interrupted publication")
        .is_some());
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .update_application_state(operation_id, FederationApplicationState::Accepted {})
        .expect("simulate interruption before in-memory terminal overlay");

    let duplicate_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("duplicate finalized unit republishes terminal response");
    assert_eq!(duplicate_responses.len(), 1);
    assert!(matches!(
        &duplicate_responses[0].outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(
        duplicate_responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let notified = time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("applied waiter notification timeout")
        .expect("applied waiter task");
    assert_eq!(notified.operation_id, operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for _ in 0..4 {
        state
            .apply_federation_aleph_bft_finalized_unit(unit.clone())
            .await
            .expect("idempotent duplicate terminal publication");
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
}

#[tokio::test]
async fn duplicate_finalized_unit_recovers_lost_rejected_response_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let input = proof_for_amount(&state, 1, "lost-rejected-publication");
    let accepted_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input.clone()],
            vec![swap_output(&state, "lost-rejected-winner")],
        ))
        .expect("accepted swap envelope");
    let rejected_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input],
            vec![swap_output(&state, "lost-rejected-loser")],
        ))
        .expect("rejected swap envelope");
    let rejected_operation_id = rejected_envelope
        .operation_id()
        .expect("rejected operation id");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(accepted_envelope),
            FederationConsensusItem::operation(rejected_envelope),
        ],
    )
    .expect("conflicting finalized unit");
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(rejected_operation_id)
            .await
            .expect("notified rejected operation")
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    state
        .federation
        .suppress_next_terminal_response_publication
        .store(true, Ordering::SeqCst);

    let first_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("durably reject finalized operation before publication interruption");
    let rejected_response = first_responses
        .iter()
        .find(|response| response.operation_id == rejected_operation_id)
        .expect("rejected terminal response");
    assert_eq!(
        rejected_response.outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    let rejection = rejected_response
        .outcome
        .entry()
        .rejection()
        .cloned()
        .expect("persisted Cashu error response");
    assert!(!waiter.is_finished());
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(rejected_operation_id)
        .await
        .expect("rejected journal lookup")
        .expect("persisted rejected journal entry");
    assert_eq!(persisted.rejection(), Some(&rejection));
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .update_application_state(
            rejected_operation_id,
            FederationApplicationState::Accepted {},
        )
        .expect("simulate interruption before rejected in-memory overlay");

    state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("duplicate finalized unit republishes rejection");
    let notified = time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("rejected waiter notification timeout")
        .expect("rejected waiter task");
    assert_eq!(notified.operation_id, rejected_operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(notified.outcome.entry().rejection(), Some(&rejection));

    for _ in 0..4 {
        state
            .apply_federation_aleph_bft_finalized_unit(unit.clone())
            .await
            .expect("idempotent duplicate rejection publication");
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 2);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_reports_state_rejected_swap() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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
        responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(responses[1].operation_id, conflicting_operation_id);
    assert_eq!(
        responses[1].outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(responses[2].operation_id, later_operation_id);
    assert_eq!(
        responses[2].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let notified = waiter.await.expect("waiter task");
    assert_eq!(notified.operation_id, conflicting_operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status(), FederationOperationStatus::Applied);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_applies_rejected_trailing_entry() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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
            .map(|response| response.outcome.entry().status())
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_units_serialize_cross_member_value_conflicts() {
    let (public_config, first_private, second_private) = transport_config();
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
    let melt_quote_envelope = test_melt_quote_operation(
        &config,
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
            FederationConsensusItem::operation(melt_payment_observation_envelope(
                &first_state,
                &melt_quote_id,
                MeltQuoteState::Paid,
                Some(String::new()),
            )),
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
                .map(|response| response.outcome.entry().status())
                .collect::<Vec<_>>(),
            vec![FederationOperationStatus::Applied; 7]
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
                .map(|response| response.outcome.entry().status())
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
        assert!(matches!(
            second_responses[0]
                .outcome
                .entry()
                .rejection()
                .cloned()
                .map(cdk::Error::from),
            Some(cdk::Error::IssuedQuote)
        ));
        assert!(matches!(
            second_responses[1]
                .outcome
                .entry()
                .rejection()
                .cloned()
                .map(cdk::Error::from),
            Some(cdk::Error::TokenAlreadySpent)
        ));
        assert!(second_responses[2].outcome.entry().rejection().is_none());

        let replay_responses = state
            .apply_federation_aleph_bft_finalized_unit(second_unit.clone())
            .await
            .expect("replay finalized unit");
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[0],
                conflicting_mint_operation_id,
            ),
            Err(cdk::Error::IssuedQuote)
        ));
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[1],
                conflicting_swap_operation_id,
            ),
            Err(cdk::Error::TokenAlreadySpent)
        ));
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[2],
                conflicting_melt_operation_id,
            ),
            Err(cdk::Error::Federation(
                FederationError::RejectedOperation { .. }
            ))
        ));

        for operation_id in [
            conflicting_mint_operation_id,
            conflicting_swap_operation_id,
            conflicting_melt_operation_id,
        ] {
            assert!(
                state
                    .mint
                    .localstore()
                    .get_completed_operation(&federation_completed_operation_uuid(operation_id))
                    .await
                    .expect("rejected completed-operation lookup")
                    .is_none(),
                "rejected federation operation must not be completed"
            );
        }

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
        assert_eq!(persisted.len(), 10);
        assert_eq!(
            persisted
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            vec![
                FederationOperationStatus::Applied,
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
        assert!(matches!(
            persisted[7].rejection().cloned().map(cdk::Error::from),
            Some(cdk::Error::IssuedQuote)
        ));
        assert!(matches!(
            persisted[8].rejection().cloned().map(cdk::Error::from),
            Some(cdk::Error::TokenAlreadySpent)
        ));
        assert!(persisted[9].rejection().is_none());
    }

    let first_entries = first_state
        .federation
        .consensus
        .as_ref()
        .expect("first consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
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
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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

    let mut conflicting_adapter = FederationConsensusRuntime::for_members(
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

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 0);
}

async fn queue_local_aleph_bft_unit(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationAlephBftUnitPayload {
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut consensus = consensus.lock().await;
    consensus
        .submit_aleph_bft_envelope(envelope)
        .expect("queue AlephBFT envelope");
    consensus
        .next_aleph_bft_unit_payload()
        .expect("drain AlephBFT unit")
        .expect("queued unit")
}

async fn wait_for_local_aleph_bft_unit(state: &MintState) -> FederationAlephBftUnitPayload {
    for _ in 0..50 {
        let maybe_unit = {
            let consensus = state.federation.consensus.as_ref().expect("consensus");
            let mut consensus = consensus.lock().await;
            consensus
                .next_aleph_bft_unit_payload()
                .expect("drain AlephBFT unit")
        };
        if let Some(unit) = maybe_unit {
            return unit;
        }
        tokio::task::yield_now().await;
    }

    panic!("AlephBFT unit was not queued");
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
async fn aleph_bft_live_session_startup_decision_spawns_for_federation_member() {
    let (public_config, local_private, _) = transport_config();
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let aleph_bft_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db")),
        &seed,
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
async fn aleph_bft_live_session_start_installs_inbound_queue() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
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

    assert!(state
        .federation
        .aleph_bft_network_inbound
        .read()
        .await
        .is_some());
    assert!(state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .is_some());
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);
    let stored_handle = state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .clone()
        .expect("stored live session handle");
    assert!(Arc::ptr_eq(&handle, &stored_handle));
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
async fn aleph_bft_session_supervisor_partial_startup_failure_publishes_no_tasks_or_inbound() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_pre_spawn_failure();

    start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect_err("injected startup failure");

    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);
}

#[tokio::test]
async fn aleph_bft_session_supervisor_halts_and_cleans_up_after_root_task_panic() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_session_task();
    let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start panicking live session")
    .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ) && !state.federation_aleph_bft_live_session_active().await
                && !state.federation_aleph_bft_network_inbound_active().await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("panicking session cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_session_tasks_wait_for_supervisor_publication_before_activation() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let publication_reached = Arc::new(tokio::sync::Notify::new());
    let publication_release = Arc::new(tokio::sync::Notify::new());
    let root_task_activated = Arc::new(AtomicBool::new(false));
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_session_task()
            .with_publication_pause(
                Arc::clone(&publication_reached),
                Arc::clone(&publication_release),
            )
            .with_root_task_activation_observer(Arc::clone(&root_task_activated));
    let observed_state = state.clone();
    let mut start_state = state;
    let start_task = tokio::spawn(async move {
        start_federation_aleph_bft_live_session_with_options_and_transport(
            &mut start_state,
            FederationPeerSyncConfig::default(),
            options,
            RouterFederationTransportRegistry::default(),
        )
        .await
    });

    time::timeout(Duration::from_secs(5), publication_reached.notified())
        .await
        .expect("startup reached the pre-publication barrier");
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(
        !root_task_activated.load(Ordering::SeqCst),
        "the root task must remain gated while its supervisor is constructing"
    );
    assert!(!start_task.is_finished());
    assert!(
        !observed_state
            .federation_aleph_bft_live_session_active()
            .await
    );
    assert!(
        !observed_state
            .federation_aleph_bft_network_inbound_active()
            .await
    );

    publication_release.notify_one();
    let supervisor = time::timeout(Duration::from_secs(5), start_task)
        .await
        .expect("startup completed after publication was released")
        .expect("startup task")
        .expect("published session")
        .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if root_task_activated.load(Ordering::SeqCst)
                && matches!(
                    observed_state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                )
                && !observed_state
                    .federation_aleph_bft_live_session_active()
                    .await
                && !observed_state
                    .federation_aleph_bft_network_inbound_active()
                    .await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("post-publication root panic cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_session_halt_during_construction_aborts_without_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let publication_reached = Arc::new(tokio::sync::Notify::new());
    let publication_release = Arc::new(tokio::sync::Notify::new());
    let root_task_activated = Arc::new(AtomicBool::new(false));
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_publication_pause(
                Arc::clone(&publication_reached),
                Arc::clone(&publication_release),
            )
            .with_root_task_activation_observer(Arc::clone(&root_task_activated));
    let observed_state = state.clone();
    let mut start_state = state;
    let start_task = tokio::spawn(async move {
        start_federation_aleph_bft_live_session_with_options_and_transport(
            &mut start_state,
            FederationPeerSyncConfig::default(),
            options,
            RouterFederationTransportRegistry::default(),
        )
        .await
    });

    time::timeout(Duration::from_secs(5), publication_reached.notified())
        .await
        .expect("startup reached the pre-publication barrier");
    halt_current_federation_aleph_bft_session(&observed_state, "test construction halt").await;
    publication_release.notify_one();

    let err = time::timeout(Duration::from_secs(5), start_task)
        .await
        .expect("halted startup completed")
        .expect("startup task")
        .expect_err("halt epoch change must reject constructing supervisor publication");
    assert!(err.to_string().contains("halted before publication"));
    assert!(!root_task_activated.load(Ordering::SeqCst));
    assert!(
        !observed_state
            .federation_aleph_bft_live_session_active()
            .await
    );
    assert!(
        !observed_state
            .federation_aleph_bft_network_inbound_active()
            .await
    );
}

#[tokio::test]
async fn aleph_bft_session_supervisor_halts_after_network_pump_panic() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_network_pump_task();
    let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start live session with panicking network pump")
    .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ) && !state.federation_aleph_bft_live_session_active().await
                && !state.federation_aleph_bft_network_inbound_active().await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("network pump panic cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_non_retryable_finalized_payload_failure_halts_exact_generation() {
    let options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    )
    .with_finalized_payload_apply_failure();
    let session = start_two_member_aleph_bft_live_session(options)
        .await
        .expect("start apply-failure live session");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&session.first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = session.first_state.clone();
    let submit_task = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    for state in [&session.first_state, &session.second_state] {
        time::timeout(Duration::from_secs(10), async {
            loop {
                if matches!(
                    state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                ) && !state.federation_aleph_bft_live_session_active().await
                    && !state.federation_aleph_bft_network_inbound_active().await
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("non-retryable apply failure cleanup");
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await
            .expect("journal lookup after apply failure")
            .is_none());
        let rejected = state
            .submit_federation_envelope(mint_quote_envelope(state, &QuoteId::new()))
            .await
            .expect_err("halted generation must reject public submission");
        assert!(matches!(
            rejected,
            cdk::Error::Federation(FederationError::NodeHalted { .. })
        ));
    }
    submit_task.abort();
    let _ = submit_task.await;
}

#[tokio::test]
async fn aleph_bft_automatic_rotation_failure_halts_without_live_resources() {
    let options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        6,
    )
    .with_automatic_rotation_limit(1)
    .with_automatic_rotation_start_failure();
    let session = start_two_member_aleph_bft_live_session(options)
        .await
        .expect("start rotation-failure live session");
    let rotation_activity = FederationOperationId::from_bytes([203; 32]);

    time::timeout(Duration::from_secs(10), async {
        loop {
            for state in [&session.first_state, &session.second_state] {
                if let Some(handle) = state.federation.aleph_bft_live_session.read().await.clone() {
                    handle.mark_peer_activity(rotation_activity);
                }
            }
            let mut all_halted_and_stopped = true;
            for state in [&session.first_state, &session.second_state] {
                all_halted_and_stopped &= matches!(
                    state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                );
                all_halted_and_stopped &= !state.federation_aleph_bft_live_session_active().await;
                all_halted_and_stopped &=
                    !state.federation_aleph_bft_network_inbound_active().await;
            }
            if all_halted_and_stopped {
                break;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic rotation failure did not halt both members");

    for state in [&session.first_state, &session.second_state] {
        assert!(!state.federation_aleph_bft_live_session_active().await);
        assert!(!state.federation_aleph_bft_network_inbound_active().await);
        let rejected = state
            .submit_federation_envelope(mint_quote_envelope(state, &QuoteId::new()))
            .await
            .expect_err("rotation-failure halt must reject public submission");
        assert!(matches!(
            rejected,
            cdk::Error::Federation(FederationError::NodeHalted { .. })
        ));
    }
}

#[tokio::test]
async fn aleph_bft_live_session_persists_and_recovers_empty_active_session() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    state
        .federation
        .consensus
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

    let mut restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (consensus, sequences) = crate::create_federation_runtime_state(&restarted_state.mint)
        .await
        .expect("recover active empty session");
    restarted_state.federation.consensus = consensus;
    restarted_state.federation.sequences = sequences;
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
async fn aleph_bft_restart_finalizes_atomically_rebound_pending_operations() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote);
    let second_envelope = mint_quote_envelope(&state, &second_quote);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let second_operation_id = second_envelope.operation_id().expect("second operation id");

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin crash fixture tx");
    tx.add_federation_aleph_bft_pending_operation(
        &cdk::federation::FederationAlephBftPendingOperationRecord {
            operation_id: first_operation_id,
            session_id: 0,
            envelope: first_envelope,
        },
    )
    .await
    .expect("persist first older-session operation");
    tx.add_federation_aleph_bft_pending_operation(
        &cdk::federation::FederationAlephBftPendingOperationRecord {
            operation_id: second_operation_id,
            session_id: 0,
            envelope: second_envelope,
        },
    )
    .await
    .expect("persist second older-session operation");
    tx.set_federation_aleph_bft_active_session(1)
        .await
        .expect("atomically advance and rebind pending operations");
    tx.commit().await.expect("commit crash fixture");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("recover and rebind pending operations");
    state.federation.consensus = consensus;
    state.federation.sequences = sequences;
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("recovered status")
            .queued_item_count,
        2
    );
    assert!(db
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("older pending rows")
        .is_empty());
    assert_eq!(
        db.get_federation_aleph_bft_pending_operations(1)
            .await
            .expect("rebound pending rows")
            .len(),
        2
    );

    let unit = state
        .federation
        .consensus
        .as_ref()
        .expect("recovered consensus")
        .lock()
        .await
        .next_aleph_bft_unit_payload()
        .expect("next recovered unit")
        .expect("recovered unit payload");
    state
        .federation
        .apply_finalized_unit(unit)
        .await
        .expect("finalize recovered pending operations");

    assert!(db
        .get_federation_aleph_bft_pending_operations(1)
        .await
        .expect("pending rows after finalization")
        .is_empty());
    for operation_id in [first_operation_id, second_operation_id] {
        assert_eq!(
            db.get_federation_journal_entry_by_operation_id(operation_id)
                .await
                .expect("journal lookup")
                .expect("recovered journal entry")
                .status(),
            FederationOperationStatus::Applied
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_session_rotates_to_next_empty_session_and_uses_new_network_pump() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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
    let initial_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
        );
    let initial_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        pump_config,
        initial_transport.clone(),
    )
    .await
    .expect("start initial live session")
    .expect("initial live session handle");
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("initial AlephBFT status")
            .session,
        0
    );

    let rotated_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
        );
    let rotated_handle = rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
        &mut state,
        sync_config,
        pump_config,
        rotated_transport.clone(),
    )
    .await
    .expect("rotate live session")
    .expect("rotated live session handle");

    assert!(!Arc::ptr_eq(&initial_handle, &rotated_handle));
    let status = state
        .federation_aleph_bft_status()
        .await
        .expect("rotated AlephBFT status");
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(status.next_unit_finalization_index, 0);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_active_session()
            .await
            .expect("persisted rotated active session"),
        Some(1)
    );
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("rotated startup decision"),
        FederationAlephBftLiveSessionStartupDecision::AlreadyStarted
    );
    let initial_request_count_after_rotation =
        initial_transport.aleph_bft_network_message_requests().len();

    let next_envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(next_envelope)
        .expect("queue rotated-session operation");
    time::timeout(
        Duration::from_secs(5),
        rotated_transport.wait_for_aleph_bft_network_message_request_matching(|_| true),
    )
    .await
    .expect("rotated-session network-message request");
    let requests = rotated_transport.aleph_bft_network_message_requests();

    assert!(
        requests
            .iter()
            .all(|request| request.payload.session_id == 1),
        "rotated live network pump must deliver only the new session"
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("rotated session network-message request verifies");
        assert_eq!(request.sender, local_private.member_id);
    }
    assert_eq!(
        initial_transport.aleph_bft_network_message_requests().len(),
        initial_request_count_after_rotation,
        "the old live-session pump must stay stopped after rotation"
    );

    let halt_epoch = state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst);
    halt_federation_aleph_bft_generation_for_test(&state, 0, initial_handle.generation()).await;
    assert_eq!(
        state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst),
        halt_epoch,
        "a stale generation failure must not publish a halt"
    );
    let installed = state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .clone()
        .expect("rotated generation remains installed");
    assert!(Arc::ptr_eq(&installed, &rotated_handle));
    assert!(state.federation_aleph_bft_network_inbound_active().await);

    drop(initial_handle);
    drop(rotated_handle);
}

#[tokio::test]
async fn aleph_bft_live_session_rotation_refuses_non_empty_pending_mempool() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
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
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        pump_config,
        transport,
    )
    .await
    .expect("start live session")
    .expect("live session handle");

    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(envelope)
        .expect("queue pending operation");

    let rotated_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
        );
    let err = rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
        &mut state,
        sync_config,
        pump_config,
        rotated_transport,
    )
    .await
    .expect_err("non-empty mempool must block live-session rotation");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::AlephBftSessionRotationBlocked {
            session: 0,
            pending_items: 1,
        })
    ));
    let status = state
        .federation_aleph_bft_status()
        .await
        .expect("unrotated AlephBFT status");
    assert_eq!(status.session, 0);
    assert_eq!(status.queued_item_count, 1);
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);

    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_session_uses_recovered_session_for_network_pump_messages() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    let initial_quote_id = QuoteId::new();
    let initial_envelope = mint_quote_envelope(&state, &initial_quote_id);
    let mut adapter = FederationConsensusRuntime::for_members(
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
    state.federation.consensus = consensus;
    state.federation.sequences = sequences;
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
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(next_envelope)
        .expect("queue session one operation");
    time::timeout(
        Duration::from_secs(5),
        transport.wait_for_aleph_bft_network_message_request_matching(|_| true),
    )
    .await
    .expect("session one network-message request");
    let requests = transport.aleph_bft_network_message_requests();

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
async fn aleph_bft_live_session_latency_probe_records_end_to_end_stage_order() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");

    let latency = CdkAlephBftLatencyRecorder::install().await;
    let operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_latency_stage(
        &latency,
        operation_id,
        CdkAlephBftLatencyStage::NetworkMessageDelivered,
    )
    .await;
    wait_for_applied_operation(&session.first_state, operation_id).await;
    wait_for_applied_operation(&session.second_state, operation_id).await;

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    eprintln!(
        "AlephBFT latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_three_serial_operations() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");
    let latency = CdkAlephBftLatencyRecorder::install().await;

    let mut operation_ids = Vec::new();
    for _ in 0..3 {
        let operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
        wait_for_applied_operation(&session.first_state, operation_id).await;
        wait_for_applied_operation(&session.second_state, operation_id).await;
        operation_ids.push(operation_id);
    }

    for operation_id in operation_ids {
        let operation_events = latency.events_for_operation(operation_id);
        assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
        let end_to_end = latency_stage_delta(
            &operation_events,
            CdkAlephBftLatencyStage::SubmitQueued,
            CdkAlephBftLatencyStage::WaiterReturned,
        )
        .expect("submit-to-waiter latency");
        assert!(
            end_to_end < Duration::from_secs(5),
            "serial operation {operation_id} took {end_to_end:?}: {}",
            format_latency_events(&operation_events)
        );
        eprintln!(
            "AlephBFT serial latency probe for {operation_id}: {}",
            format_latency_events(&operation_events)
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_duplicate_fanout_joins_one_operation() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");
    let latency = CdkAlephBftLatencyRecorder::install().await;

    let operation_id = submit_duplicate_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, operation_id).await;
    wait_for_applied_operation(&session.second_state, operation_id).await;

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    assert_eq!(
        latency_stage_count(&operation_events, CdkAlephBftLatencyStage::SubmitQueued),
        1,
        "duplicate fanout must queue one local AlephBFT operation: {}",
        format_latency_events(&operation_events)
    );
    assert_eq!(
        latency_stage_count(
            &operation_events,
            CdkAlephBftLatencyStage::DataProviderYielded
        ),
        1,
        "duplicate fanout must produce one CDK data payload: {}",
        format_latency_events(&operation_events)
    );
    assert_eq!(
        latency_stage_count(&operation_events, CdkAlephBftLatencyStage::WaiterReturned),
        2,
        "duplicate fanout must return both waiters: {}",
        format_latency_events(&operation_events)
    );
    let stats = session
        .first_state
        .federation_operation_notification_stats();
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
    assert_eq!(stats.duplicate_submissions, 1);
    eprintln!(
        "AlephBFT duplicate fanout latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_five_member_bft_operation() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let session = start_five_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start five-member live session");
    let latency = CdkAlephBftLatencyRecorder::install().await;

    let operation_id = submit_latency_probe_mint_quote(
        session
            .states
            .first()
            .expect("five-member latency probe has local state"),
    )
    .await;
    for state in &session.states {
        wait_for_applied_operation(state, operation_id).await;
    }

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    let end_to_end = latency_stage_delta(
        &operation_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("submit-to-waiter latency");
    assert!(
        end_to_end < Duration::from_secs(5),
        "five-member operation {operation_id} took {end_to_end:?}: {}",
        format_latency_events(&operation_events)
    );
    eprintln!(
        "AlephBFT five-member latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_seven_member_bft_operation() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 512,
        max_peer_deliveries_per_message: 512,
    };
    let session = start_multi_member_aleph_bft_live_session(
        7,
        5,
        fast_aleph_bft_live_session_options(pump_config, 500),
    )
    .await
    .expect("start seven-member live session");
    let latency = CdkAlephBftLatencyRecorder::install().await;

    let operation_id = submit_latency_probe_mint_quote(
        session
            .states
            .first()
            .expect("seven-member latency probe has local state"),
    )
    .await;
    for state in &session.states {
        wait_for_applied_operation(state, operation_id).await;
    }

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    let end_to_end = latency_stage_delta(
        &operation_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("submit-to-waiter latency");
    assert!(
        end_to_end < Duration::from_secs(10),
        "seven-member operation {operation_id} took {end_to_end:?}: {}",
        format_latency_events(&operation_events)
    );
    eprintln!(
        "AlephBFT seven-member latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_default_cadence_follow_up() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(
        FederationAlephBftLiveSessionOptions::new(pump_config).with_max_round(500),
    )
    .await
    .expect("start default-cadence live session");
    time::sleep(Duration::from_secs(3)).await;
    let latency = CdkAlephBftLatencyRecorder::install().await;

    let cold_operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, cold_operation_id).await;
    wait_for_applied_operation(&session.second_state, cold_operation_id).await;
    let warm_operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, warm_operation_id).await;
    wait_for_applied_operation(&session.second_state, warm_operation_id).await;

    let cold_events = latency.events_for_operation(cold_operation_id);
    let warm_events = latency.events_for_operation(warm_operation_id);
    assert_latency_stage_order(&cold_events, LATENCY_EXPECTED_STAGES);
    assert_latency_stage_order(&warm_events, LATENCY_EXPECTED_STAGES);
    let cold_end_to_end = latency_stage_delta(
        &cold_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("cold submit-to-waiter latency");
    let warm_end_to_end = latency_stage_delta(
        &warm_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("warm submit-to-waiter latency");
    assert!(
        cold_end_to_end < Duration::from_secs(10),
        "default-cadence cold operation {cold_operation_id} took {cold_end_to_end:?}: {}",
        format_latency_events(&cold_events)
    );
    assert!(
        warm_end_to_end < Duration::from_secs(10),
        "default-cadence warm operation {warm_operation_id} took {warm_end_to_end:?}: {}",
        format_latency_events(&warm_events)
    );
    eprintln!(
        "AlephBFT default-cadence cold latency probe for {cold_operation_id}: {}",
        format_latency_events(&cold_events)
    );
    eprintln!(
        "AlephBFT default-cadence warm latency probe for {warm_operation_id}: {}",
        format_latency_events(&warm_events)
    );
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
async fn aleph_bft_four_live_sessions_finalize_operation_over_private_rest_network_messages() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
    ];
    let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        threshold_params,
        federation_api_urls.clone(),
    );

    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
            )
            .await,
        );
    }

    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(30),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };

    let mut live_session_handles = Vec::new();
    for index in 0_usize..4 {
        let options = fast_aleph_bft_live_session_options(pump_config, 500);
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &mut states[index],
            sync_config,
            options,
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

    states[0].federation.peer_sync_config = Some(sync_config);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("four-member live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
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
async fn aleph_bft_live_network_pump_wakes_on_outbound_before_peer_sync_interval() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_secs(60),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(20),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 1,
        max_peer_deliveries_per_message: 128,
    };
    first_state.federation.peer_sync_config = Some(sync_config);

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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response without waiting for peer sync interval");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    assert!(transport
        .aleph_bft_network_message_requests()
        .iter()
        .any(|request| request.payload.session_id == 0));
}

#[tokio::test]
async fn aleph_bft_live_sessions_rotate_after_finality_through_router_clones() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_response = first_state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("session zero operation response");
    assert_eq!(first_response.operation_id, first_operation_id);
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    assert_eq!(
        first_state
            .federation_aleph_bft_status()
            .await
            .expect("first session zero status")
            .session,
        0
    );
    assert_eq!(
        second_state
            .federation_aleph_bft_status()
            .await
            .expect("second session zero status")
            .finalized_item_count,
        1
    );

    let rotated_first_handle =
        rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
            &mut first_state,
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("rotate first live session")
        .expect("rotated first live session handle");
    let rotated_second_handle =
        rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
            &mut second_state,
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("rotate second live session")
        .expect("rotated second live session handle");

    assert!(!Arc::ptr_eq(&first_handle, &rotated_first_handle));
    assert!(!Arc::ptr_eq(&second_handle, &rotated_second_handle));
    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("rotated AlephBFT status");
        assert_eq!(status.session, 1);
        assert_eq!(status.finalized_item_count, 1);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_aleph_bft_active_session()
                .await
                .expect("rotated active session"),
            Some(1)
        );
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let second_response = first_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("session one operation response");
    assert_eq!(second_response.operation_id, second_operation_id);
    wait_for_applied_operation(&first_state, second_operation_id).await;
    wait_for_applied_operation(&second_state, second_operation_id).await;

    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("post-rotation AlephBFT status");
        assert_eq!(status.session, 1);
        assert_eq!(status.finalized_item_count, 2);
        assert_eq!(status.accepted_operation_count, 2);
        assert_eq!(status.next_unit_finalization_index, 1);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&second_quote_id)
            .await
            .expect("rotated quote lookup")
            .is_some());
    }

    drop(first_handle);
    drop(second_handle);
    drop(rotated_first_handle);
    drop(rotated_second_handle);
}

#[tokio::test]
async fn aleph_bft_live_sessions_auto_rotate_after_session_completion() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(5),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let options =
        fast_aleph_bft_live_session_options(pump_config, 6).with_automatic_rotation_limit(2);

    let first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut first_state,
        sync_config,
        options.clone(),
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
    let second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut second_state,
        sync_config,
        options,
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

    let rotation_activity = FederationOperationId::from_bytes([202; 32]);
    time::timeout(Duration::from_secs(10), async {
        loop {
            let first_session = first_state
                .federation_aleph_bft_status()
                .await
                .expect("first auto-rotation status")
                .session;
            let second_session = second_state
                .federation_aleph_bft_status()
                .await
                .expect("second auto-rotation status")
                .session;
            if first_session >= 2 && second_session >= 2 {
                return;
            }

            // Empty sessions rotate only while bounded peer activity is present.
            for state in [&first_state, &second_state] {
                if let Some(handle) = state.federation.aleph_bft_live_session.read().await.clone() {
                    handle.mark_peer_activity(rotation_activity);
                }
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("active empty sessions did not auto-rotate before timeout");

    wait_for_aleph_bft_session(&first_state, 2).await;
    wait_for_aleph_bft_session(&second_state, 2).await;
    for state in [&first_state, &second_state] {
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_aleph_bft_active_session()
                .await
                .expect("auto-rotated active session"),
            Some(2)
        );
    }

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("session-one operation response");
    assert_eq!(response.operation_id, operation_id);
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;

    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("post-auto-rotation AlephBFT status");
        assert_eq!(status.session, 2);
        assert_eq!(status.accepted_operation_count, 1);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("auto-rotated quote lookup")
            .is_some());
    }

    drop(first_handle);
    drop(second_handle);
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response under transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
            .get_federation_dkg_member_sequences()
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response after malformed message");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
            .get_federation_dkg_member_sequences()
            .await
            .expect("durable sequences after valid finality")
            .is_empty(),
        "liveness-only network-message sequences should remain in memory"
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response before equivocation");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
            .get_federation_dkg_member_sequences()
            .await
            .expect("durable sequences after equivocated network message")
            .get(&first_private.member_id),
        None,
        "liveness-only network-message sequences should not be durably persisted"
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response under reordered transport");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("five-member AlephBFT operation response under combined transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
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
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first five-member AlephBFT operation response");

    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
    assert!(states[1]
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences before equivocated network message")
        .is_empty());

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
    assert!(states[1]
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences after equivocated network message")
        .is_empty());
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
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second five-member AlephBFT operation response after equivocation");

    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
async fn aleph_bft_live_five_member_sessions_converge_after_valid_increasing_sequence_fork() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 128);
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("five-member live session");
    let bootstrap_activity = mint_quote_envelope(&session.states[0], &QuoteId::new())
        .operation_id()
        .expect("bootstrap activity operation id");
    for state in &session.states {
        state
            .mark_aleph_bft_peer_activity(0, bootstrap_activity)
            .await
            .expect("mark bootstrap AlephBFT activity");
    }
    wait_for_aleph_bft_backup_bytes(&session.states[0], 0).await;
    for handle in &session._handles {
        handle.clear_network_activity();
    }
    time::sleep(Duration::from_millis(100)).await;
    let duplicated_member_backup = session.states[0]
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_bytes(0)
        .await
        .expect("settled duplicated member backup lookup")
        .expect("settled duplicated member backup");

    *session.states[0]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[0]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    drop(session._handles.remove(0));

    let honest_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("honest db"));
    let honest_mnemonic = Mnemonic::generate(12).expect("honest mnemonic");
    let mut honest_state = federation_route_state_for_private_with_db_and_aleph_bft(
        session.public_config.clone(),
        session.private_configs[0].clone(),
        honest_db,
        &honest_mnemonic.to_seed_normalized(""),
    )
    .await;
    seed_aleph_bft_backup(&honest_state, 0, &duplicated_member_backup).await;
    let honest_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut honest_state,
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("start restored honest member session")
    .expect("restored honest member live handle");
    session.states[0] = honest_state;
    session._handles.insert(0, honest_handle);
    session.transport.insert(
        session.federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[0].clone()),
    );

    let fork_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("fork db"));
    let fork_mnemonic = Mnemonic::generate(12).expect("fork mnemonic");
    let mut fork_state = federation_route_state_for_private_with_db_and_aleph_bft(
        session.public_config.clone(),
        session.private_configs[0].clone(),
        fork_db,
        &fork_mnemonic.to_seed_normalized(""),
    )
    .await;
    seed_aleph_bft_backup(&fork_state, 0, &duplicated_member_backup).await;
    let fork_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut fork_state,
        session.sync_config,
        live_options,
        session.transport.clone(),
    )
    .await
    .expect("start forked member session")
    .expect("forked member live handle");
    let duplicated_member = session.private_configs[0].member_id;
    session
        .transport
        .set_member_aleph_bft_data_hold(duplicated_member, true);
    session.transport.insert_shadow(
        session.federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(fork_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&session.states[0], &first_quote_id);
    let first_operation_id = first_envelope
        .operation_id()
        .expect("first fork operation id");
    let fork_quote_id = QuoteId::new();
    let fork_envelope = mint_quote_envelope(&fork_state, &fork_quote_id);
    let fork_operation_id = fork_envelope
        .operation_id()
        .expect("second fork operation id");
    assert_ne!(first_operation_id, fork_operation_id);

    let first_submit_state = session.states[0].clone();
    let first_submit = tokio::spawn(async move {
        first_submit_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    let fork_submit_state = fork_state.clone();
    let fork_submit = tokio::spawn(async move {
        fork_submit_state
            .submit_federation_envelope_to_consensus(fork_envelope)
            .await
    });
    let (first_unit_index, fork_unit_index) = tokio::join!(
        wait_for_aleph_bft_unit_with_operation(
            &session.transport,
            duplicated_member,
            first_operation_id,
        ),
        wait_for_aleph_bft_unit_with_operation(
            &session.transport,
            duplicated_member,
            fork_operation_id,
        ),
    );
    let first_unit_index = first_unit_index.expect("honest fork branch must enter transport");
    let fork_unit_index = fork_unit_index.expect("second fork branch must enter transport");

    assert_eq!(
        first_unit_index, fork_unit_index,
        "fork branches must use the same local unit index"
    );
    assert!(!first_submit.is_finished());
    assert!(!fork_submit.is_finished());

    let remote_members = session
        .public_config
        .members
        .values()
        .filter(|member| member.member_id != duplicated_member)
        .cloned()
        .collect::<Vec<_>>();
    let branch_payloads = time::timeout(Duration::from_secs(5), async {
        loop {
            let requests = session.transport.aleph_bft_network_message_requests();
            let selected = remote_members
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    let operation_id = if index % 2 == 0 {
                        first_operation_id
                    } else {
                        fork_operation_id
                    };
                    requests
                        .iter()
                        .find(|request| {
                            request.sender == duplicated_member
                                && request.receiver == member.member_id
                                && aleph_bft_network_request_contains_operation(
                                    request,
                                    operation_id,
                                )
                        })
                        .map(|request| (member.clone(), request.payload.clone()))
                })
                .collect::<Option<Vec<_>>>();
            if let Some(selected) = selected {
                return selected;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both fork variants must be captured for their target peers");

    *fork_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *fork_state.federation.aleph_bft_live_session.write().await = None;
    drop(fork_handle);
    fork_submit.abort();
    session
        .transport
        .remove_shadow(&session.federation_api_urls[0]);
    session
        .transport
        .set_member_aleph_bft_data_hold(duplicated_member, false);
    let replay_client = FederationRestPeerClient::new_with_config(
        session.transport.clone(),
        &session.public_config,
        session.private_configs[0].clone(),
        session.sync_config,
    )
    .expect("fork replay peer client");
    let mut replay_sequences = Vec::new();
    for (member, payload) in branch_payloads {
        let request_count = session.transport.aleph_bft_network_message_requests().len();
        replay_client
            .submit_aleph_bft_network_message(&member, &payload)
            .await
            .expect("deliver authenticated AlephBFT fork branch");
        let replay_sequence = session
            .transport
            .aleph_bft_network_message_requests()
            .into_iter()
            .skip(request_count)
            .find(|request| {
                request.sender == duplicated_member
                    && request.receiver == member.member_id
                    && request.payload == payload
            })
            .expect("freshly signed fork delivery request")
            .sequence;
        replay_sequences.push(replay_sequence);
    }
    assert!(
        replay_sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "fork branches must use distinct increasing private REST sequences"
    );

    let follow_up_quote_id = QuoteId::new();
    let follow_up_envelope = mint_quote_envelope(&session.states[1], &follow_up_quote_id);
    let follow_up_operation_id = follow_up_envelope
        .operation_id()
        .expect("follow-up operation id");
    let follow_up = time::timeout(
        Duration::from_secs(20),
        session.states[1].submit_federation_envelope_to_consensus(follow_up_envelope),
    )
    .await
    .expect("honest follow-up must finish after fork")
    .expect("honest follow-up response");
    assert_eq!(follow_up.operation_id, follow_up_operation_id);
    assert_eq!(
        follow_up.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for state in &session.states {
        wait_for_applied_operation(state, follow_up_operation_id).await;
    }
    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected post-fork journal");
    let expected_operation_ids = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    assert!(
        [first_operation_id, fork_operation_id]
            .iter()
            .filter(|operation_id| expected_operation_ids.contains(operation_id))
            .count()
            <= 1,
        "two variants of one AlephBFT unit must not both enter the journal"
    );
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected post-fork state digest");
    assert!(expected_operation_ids.contains(&follow_up_operation_id));
    for state in &session.states[1..] {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("post-fork journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("post-fork state digest"),
            expected_digest
        );
    }

    first_submit.abort();
    fork_submit.abort();
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    });
    let retry_envelope = mint_quote_envelope(&states[0], &quote_id);
    let response = states[0]
        .submit_federation_envelope_to_consensus(retry_envelope)
        .await
        .expect("returned member should allow live finality");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
async fn aleph_bft_live_partition_matrix_catches_up_and_certifies_lagging_members() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = FederationAlephBftLiveSessionOptions::new(pump_config);
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("five-member live partition session");

    let quote_id = QuoteId::new();
    let quote_envelope = mint_quote_envelope(&session.states[0], &quote_id);
    let quote_operation_id = quote_envelope.operation_id().expect("quote operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(quote_envelope)
        .await
        .expect("baseline mint quote finality");
    let paid_envelope = paid_observation_envelope(&session.states[0], &quote_id);
    let paid_operation_id = paid_envelope.operation_id().expect("paid operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(paid_envelope)
        .await
        .expect("baseline paid observation finality");
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            paid_operation_id,
            "baseline paid observation",
        )
        .await;
    }

    let (mint_envelope, mint_output) = mint_operation_envelope(
        &session.states[4],
        &quote_id,
        b"self-partitioned live mint output",
    );
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    session.transport.set_member_outbound_partition(
        &session.public_config,
        session.private_configs[4].member_id,
        true,
    );
    session.states[4].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..session.sync_config
    });
    let err = session.states[4]
        .submit_federation_envelope_to_consensus(mint_envelope.clone())
        .await
        .expect_err("self-partitioned proposer must time out locally");
    assert_pending_finalization_error(&err, mint_operation_id, 250);

    for state in &session.states {
        assert!(
            !operation_applied_within(state, mint_operation_id, Duration::from_millis(100)).await,
            "self-partitioned operation must not apply before outbound links heal"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote_id)
                .await
                .expect("mint quote lookup before partition heal")
                .expect("baseline mint quote before partition heal")
                .state(),
            MintQuoteState::Paid
        );
        assert!(!state
            .mint
            .localstore()
            .get_federation_aleph_bft_finalized_items()
            .await
            .expect("finalized item lookup before partition heal")
            .iter()
            .any(|item| item.operation_id == Some(mint_operation_id)));
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(mint_operation_id, 0)
            .await
            .expect("signature share lookup before partition heal")
            .is_empty());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(mint_operation_id))
            .await
            .expect("completed operation lookup before partition heal")
            .is_none());
    }
    let pending = session.states[4]
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("self-partitioned pending operation");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, mint_operation_id);

    *session.states[4]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[4]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    let proposer_handle = session._handles.remove(4);
    drop(proposer_handle);
    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[4]).await;
    let proposer_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[4],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart self-partitioned proposer")
    .expect("restarted self-partitioned proposer live session handle");
    session._handles.push(proposer_handle);
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[4].clone()),
    );

    let err = session.states[4]
        .submit_federation_envelope_to_consensus(mint_envelope.clone())
        .await
        .expect_err("restarted self-partitioned proposer must remain pending before heal");
    assert_pending_finalization_error(&err, mint_operation_id, 250);
    let pending = session.states[4]
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("restarted self-partitioned pending operation");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, mint_operation_id);
    for state in &session.states {
        assert!(
            !operation_applied_within(state, mint_operation_id, Duration::from_millis(100)).await,
            "restarted self-partitioned operation must remain unapplied before heal"
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(mint_operation_id, 0)
            .await
            .expect("signature share lookup after self-partitioned restart")
            .is_empty());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(mint_operation_id))
            .await
            .expect("completed operation lookup after self-partitioned restart")
            .is_none());
    }

    session.transport.set_member_outbound_partition(
        &session.public_config,
        session.private_configs[4].member_id,
        false,
    );
    for state in &session.states {
        wait_for_applied_operation_with_context(state, mint_operation_id, "self-partition heal")
            .await;
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after partition heal")
            .is_empty());
    }

    session.states[4].federation.peer_sync_config = Some(session.sync_config);
    let duplicate = session.states[4]
        .submit_federation_envelope_to_consensus(mint_envelope)
        .await
        .expect("healed mint retry returns durable response");
    assert_eq!(duplicate.operation_id, mint_operation_id);

    let shares = session.states[4]
        .mint
        .localstore()
        .get_federation_signature_shares_for_output(mint_operation_id, 0)
        .await
        .expect("healed mint share lookup");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].blinded_secret, mint_output.blinded_secret);

    *session.states[4]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[4]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(session.states[0].clone()),
    );
    let fifth_handle = session._handles.remove(4);
    drop(fifth_handle);

    let below_quorum_quote_id = QuoteId::new();
    let below_quorum_envelope = mint_quote_envelope(&session.states[0], &below_quorum_quote_id);
    let below_quorum_operation_id = below_quorum_envelope
        .operation_id()
        .expect("below-quorum operation id");
    let response = session.states[0]
        .submit_federation_envelope_to_consensus(below_quorum_envelope)
        .await
        .expect("four connected members retain BFT finality");
    assert_eq!(response.operation_id, below_quorum_operation_id);
    for state in &session.states[..4] {
        wait_for_applied_operation_with_context(
            state,
            below_quorum_operation_id,
            "one-member-offline finality",
        )
        .await;
    }
    assert!(
        !operation_applied_within(
            &session.states[4],
            below_quorum_operation_id,
            Duration::from_millis(250),
        )
        .await
    );
    session.states[4]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("offline member remains fail-closed before catch-up");

    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[4]).await;
    let fifth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[4],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart below-quorum member")
    .expect("below-quorum member live session handle");
    session._handles.push(fifth_handle);
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[4].clone()),
    );
    let peer = session
        .public_config
        .members
        .get(&member_id(1))
        .expect("catch-up peer")
        .clone();
    let client = FederationRestPeerClient::new_with_config(
        session.transport.clone(),
        &session.public_config,
        session.private_configs[4].clone(),
        session.sync_config,
    )
    .expect("partition catch-up client");
    sync_and_certify_partitioned_member(&session.states[4], &client, &peer, &session.public_config)
        .await;
    wait_for_applied_operation_with_context(
        &session.states[4],
        below_quorum_operation_id,
        "one-member-offline catch-up",
    )
    .await;
    assert!(session.states[4]
        .mint
        .localstore()
        .get_mint_quote(&below_quorum_quote_id)
        .await
        .expect("caught-up below-quorum quote lookup")
        .is_some());

    for index in [4_usize, 3] {
        *session.states[index]
            .federation
            .aleph_bft_network_inbound
            .write()
            .await = None;
        *session.states[index]
            .federation
            .aleph_bft_live_session
            .write()
            .await = None;
        session.transport.insert(
            session.federation_api_urls[index].clone(),
            Router::new()
                .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
                .with_state(session.states[0].clone()),
        );
    }
    let fifth_handle = session._handles.remove(4);
    let fourth_handle = session._handles.remove(3);
    drop(fifth_handle);
    drop(fourth_handle);

    let stalled_quote_id = QuoteId::new();
    let stalled_envelope = mint_quote_envelope(&session.states[0], &stalled_quote_id);
    let stalled_operation_id = stalled_envelope
        .operation_id()
        .expect("stalled operation id");
    session.states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..session.sync_config
    });
    let err = session.states[0]
        .submit_federation_envelope_to_consensus(stalled_envelope.clone())
        .await
        .expect_err("two offline members must stall five-member finality");
    assert_pending_finalization_error(&err, stalled_operation_id, 250);
    for state in &session.states {
        assert!(
            !operation_applied_within(state, stalled_operation_id, Duration::from_millis(100),)
                .await
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&stalled_quote_id)
            .await
            .expect("stalled quote lookup")
            .is_none());
    }

    session.states[4]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("offline member remains fail-closed");

    let activity_requests_before_refresh = session.transport.aleph_bft_activity_request_count();
    time::sleep(CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE / 2 + Duration::from_millis(250)).await;
    assert!(
        session.transport.aleph_bft_activity_request_count() > activity_requests_before_refresh,
        "durable pending work must renew activity before its lease expires"
    );
    for state in &session.states {
        assert!(
            !operation_applied_within(state, stalled_operation_id, Duration::from_millis(100))
                .await
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&stalled_quote_id)
            .await
            .expect("stalled quote lookup after activity refresh")
            .is_none());
    }

    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[3]).await;
    let fourth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[3],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart fourth live session")
    .expect("fourth live session handle");
    session._handles.push(fourth_handle);
    session.transport.insert(
        session.federation_api_urls[3].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[3].clone()),
    );

    session.states[0].federation.peer_sync_config = Some(session.sync_config);
    for state in &session.states[..4] {
        wait_for_applied_operation_with_context(
            state,
            stalled_operation_id,
            "above-quorum recovery",
        )
        .await;
    }
    assert!(
        !operation_applied_within(
            &session.states[4],
            stalled_operation_id,
            Duration::from_millis(250),
        )
        .await
    );

    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[4]).await;
    let fifth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[4],
        session.sync_config,
        live_options,
        session.transport.clone(),
    )
    .await
    .expect("restart fifth live session")
    .expect("fifth live session handle");
    session._handles.push(fifth_handle);
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[4].clone()),
    );
    sync_and_certify_partitioned_member(&session.states[4], &client, &peer, &session.public_config)
        .await;
    wait_for_applied_operation_with_context(
        &session.states[4],
        stalled_operation_id,
        "above-quorum lagging-member catch-up",
    )
    .await;

    session.states[4].federation.peer_sync_config = Some(session.sync_config);
    let duplicate = session.states[4]
        .submit_federation_envelope_to_consensus(stalled_envelope)
        .await
        .expect("caught-up stalled operation retry");
    assert_eq!(duplicate.operation_id, stalled_operation_id);

    let expected_operation_ids = vec![
        quote_operation_id,
        paid_operation_id,
        mint_operation_id,
        below_quorum_operation_id,
        stalled_operation_id,
    ];
    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected partition journal");
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected partition state digest");
    let expected_next_item_index = session.states[0]
        .federation_aleph_bft_status()
        .await
        .expect("expected AlephBFT cursor")
        .next_item_index;

    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("partition journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("partition state digest"),
            expected_digest
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("partition AlephBFT cursor")
                .next_item_index,
            expected_next_item_index
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("partition pending operation lookup")
            .is_empty());
    }

    let certified_outcomes = session.states[4]
        .mint
        .localstore()
        .get_federation_catch_up_certificates()
        .await
        .expect("certified catch-up certificates");
    let certified_signers = certified_outcomes
        .iter()
        .map(|outcome| outcome.signer)
        .collect::<BTreeSet<_>>();
    assert!(
        certified_signers.len()
            >= usize::from(session.public_config.threshold_params.consensus_threshold)
    );

    time::sleep(Duration::from_millis(250)).await;
    let idle_activity_requests = session.transport.aleph_bft_activity_request_count();
    time::sleep(CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE / 2 + Duration::from_millis(250)).await;
    assert_eq!(
        session.transport.aleph_bft_activity_request_count(),
        idle_activity_requests,
        "activity refresh must become fully idle after durable pending work clears"
    );
}

#[tokio::test]
async fn aleph_bft_live_below_finality_conflicts_and_melt_recover_without_leaking_effects() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = FederationAlephBftLiveSessionOptions::new(pump_config);
    let payment_policy = FederationPaymentObservationPolicy::member_quorum(4);
    let (mut session, make_payment_calls) =
        start_multi_member_aleph_bft_live_session_with_payment_policy(
            5,
            3,
            live_options.clone(),
            payment_policy,
        )
        .await
        .expect("five-member below-finality value session");
    let runtime_config = session.states[0]
        .mint
        .federation_config()
        .expect("below-finality federation config")
        .clone();
    let signature_threshold =
        usize::from(session.public_config.threshold_params.signature_threshold);

    let mint_quote_id = QuoteId::new();
    let mint_quote_envelope = runtime_config.mint_quote_operation(
        mint_quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(4),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: mint_quote_id.to_string(),
            request: format!("invoice-{mint_quote_id}"),
            amount: Some(Amount::from(4)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    );
    let mint_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_quote_envelope)
            .await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            mint_quote_operation_id,
            "below-finality baseline mint quote",
        )
        .await;
    }
    for observer in 1..=4 {
        let observation = mint_payment_observation_envelope_for_member(
            &session.states[0],
            &mint_quote_id,
            observer,
            4,
            "below-finality-mint-paid",
        );
        let operation_id =
            submit_live_operation_allowing_pending_timeout(&session.states[0], observation).await;
        for state in &session.states {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                "below-finality baseline mint payment observation",
            )
            .await;
        }
    }
    let keyset = session.public_config.keysets[0].clone();
    let mint_secrets = (0..4)
        .map(|index| cdk::secret::Secret::new(format!("below-finality proof {index}")))
        .collect::<Vec<_>>();
    let mint_outputs = mint_secrets
        .iter()
        .map(|secret| {
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(secret.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    let mint_envelope = runtime_config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id,
            outputs: mint_outputs.clone(),
            signature: None,
        }))
        .expect("below-finality baseline mint operation");
    let mint_operation_id = mint_envelope
        .operation_id()
        .expect("below-finality baseline mint operation id");
    submit_live_operation_allowing_pending_timeout(&session.states[0], mint_envelope).await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            mint_operation_id,
            "below-finality baseline mint",
        )
        .await;
    }
    let mut proofs = Vec::new();
    for (index, (output, secret)) in mint_outputs.iter().zip(mint_secrets).enumerate() {
        proofs.push(
            proof_from_accepted_output(
                &session.states,
                signature_threshold,
                mint_operation_id,
                u32::try_from(index).expect("below-finality mint output index"),
                output,
                secret,
            )
            .await,
        );
    }

    let melt_quote_id = QuoteId::new();
    let pending_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_envelope = test_melt_quote_operation(
        &runtime_config,
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(
                1_000,
                serde_json::to_string(&pending_description)
                    .expect("below-finality melt description"),
            ),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_quote_envelope)
            .await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            melt_quote_operation_id,
            "below-finality baseline melt quote",
        )
        .await;
    }

    let swap_input = proofs.remove(0);
    let swap_input_y = swap_input.y().expect("below-finality swap input y");
    let first_swap_output = swap_output(&session.states[0], "below-finality-swap-first");
    let second_swap_output = swap_output(&session.states[0], "below-finality-swap-second");
    let first_swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input.clone()],
            vec![first_swap_output.clone()],
        ))
        .expect("first below-finality swap envelope");
    let first_swap_operation_id = first_swap_envelope
        .operation_id()
        .expect("first below-finality swap operation id");
    let second_swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![second_swap_output.clone()],
        ))
        .expect("second below-finality swap envelope");
    let second_swap_operation_id = second_swap_envelope
        .operation_id()
        .expect("second below-finality swap operation id");

    let melt_inputs = proofs;
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("below-finality melt input y"))
        .collect::<Vec<_>>();
    let (melt_envelope, melt_change, elected_member) = (0..128)
        .find_map(|attempt| {
            let mut change = swap_output(
                &session.states[0],
                &format!("below-finality-melt-change-{attempt}"),
            );
            change.amount = Amount::ZERO;
            let envelope = runtime_config
                .melt_operation(cdk::nuts::MeltRequest::new(
                    melt_quote_id.clone(),
                    melt_inputs.clone(),
                    Some(vec![change.clone()]),
                ))
                .expect("below-finality melt envelope");
            let operation_id = envelope
                .operation_id()
                .expect("below-finality melt operation id");
            let elected_member = elected_melt_payment_member(&session.public_config, operation_id);
            let elected_index = session
                .public_config
                .members
                .keys()
                .position(|member_id| *member_id == elected_member)
                .expect("below-finality elected member index");
            (elected_index < 3).then_some((envelope, change, elected_member))
        })
        .expect("always-live member elected for below-finality melt");
    let melt_operation_id = melt_envelope
        .operation_id()
        .expect("below-finality melt operation id");

    for index in [4_usize, 3] {
        *session.states[index]
            .federation
            .aleph_bft_network_inbound
            .write()
            .await = None;
        *session.states[index]
            .federation
            .aleph_bft_live_session
            .write()
            .await = None;
        session.transport.insert(
            session.federation_api_urls[index].clone(),
            Router::new()
                .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
                .with_state(session.states[0].clone()),
        );
    }
    let fifth_handle = session._handles.remove(4);
    let fourth_handle = session._handles.remove(3);
    drop(fifth_handle);
    drop(fourth_handle);

    for index in 0..3 {
        session.states[index].federation.peer_sync_config = Some(FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(250),
            ..session.sync_config
        });
    }
    for (index, envelope, operation_id) in [
        (0_usize, first_swap_envelope, first_swap_operation_id),
        (1_usize, second_swap_envelope, second_swap_operation_id),
        (2_usize, melt_envelope, melt_operation_id),
    ] {
        let err = session.states[index]
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect_err("below-finality operation must remain pending");
        assert_pending_finalization_error(&err, operation_id, 250);
        let pending = session.states[index]
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("below-finality pending operation lookup");
        assert!(pending
            .iter()
            .any(|pending| pending.operation_id == operation_id));
    }

    let pending_operation_ids = [
        first_swap_operation_id,
        second_swap_operation_id,
        melt_operation_id,
    ];
    let pending_blinded_secrets = [
        first_swap_output.blinded_secret,
        second_swap_output.blinded_secret,
        melt_change.blinded_secret,
    ];
    for state in &session.states {
        for operation_id in pending_operation_ids {
            assert!(state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(operation_id)
                .await
                .expect("below-finality journal lookup")
                .is_none());
            assert!(state
                .mint
                .localstore()
                .get_completed_operation(&federation_completed_operation_uuid(operation_id))
                .await
                .expect("below-finality completed operation lookup")
                .is_none());
            assert!(state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("below-finality signature share lookup")
                .is_empty());
        }
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[swap_input_y])
                .await
                .expect("below-finality swap proof state"),
            vec![None]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("below-finality melt proof states"),
            vec![None, None, None]
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&pending_blinded_secrets)
            .await
            .expect("below-finality blind signature lookup")
            .iter()
            .all(Option::is_none));
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&melt_quote_id)
                .await
                .expect("below-finality melt quote lookup")
                .expect("below-finality melt quote")
                .state,
            MeltQuoteState::Unpaid
        );
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        0,
        "below-finality melt must not attempt payment"
    );

    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[3]).await;
    let fourth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[3],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart fourth below-finality member")
    .expect("fourth below-finality live session handle");
    session._handles.push(fourth_handle);
    session.transport.insert(
        session.federation_api_urls[3].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[3].clone()),
    );
    for state in &mut session.states[..4] {
        state.federation.peer_sync_config = Some(session.sync_config);
    }

    let first_status =
        wait_for_terminal_operation(&session.states[0], first_swap_operation_id).await;
    let second_status =
        wait_for_terminal_operation(&session.states[0], second_swap_operation_id).await;
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == FederationOperationStatus::Applied)
            .count(),
        1,
        "exactly one conflicting swap must apply"
    );
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == FederationOperationStatus::Rejected)
            .count(),
        1,
        "exactly one conflicting swap must be rejected"
    );
    assert_eq!(
        wait_for_terminal_operation(&session.states[0], melt_operation_id).await,
        FederationOperationStatus::Applied
    );
    let (
        winning_swap_operation_id,
        winning_swap_output,
        losing_swap_operation_id,
        losing_swap_output,
    ) = match first_status {
        FederationOperationStatus::Applied => (
            first_swap_operation_id,
            first_swap_output.clone(),
            second_swap_operation_id,
            second_swap_output.clone(),
        ),
        FederationOperationStatus::Rejected => (
            second_swap_operation_id,
            second_swap_output.clone(),
            first_swap_operation_id,
            first_swap_output.clone(),
        ),
        status => panic!("unexpected first swap status: {status}"),
    };

    for state in &session.states[..4] {
        assert_eq!(
            wait_for_terminal_operation(state, first_swap_operation_id).await,
            first_status
        );
        assert_eq!(
            wait_for_terminal_operation(state, second_swap_operation_id).await,
            second_status
        );
        assert_eq!(
            wait_for_terminal_operation(state, melt_operation_id).await,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(winning_swap_operation_id, 0)
                .await
                .expect("winning swap signature share lookup")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(losing_swap_operation_id, 0)
            .await
            .expect("losing swap signature share lookup")
            .is_empty());
    }

    let elected_index = session
        .public_config
        .members
        .keys()
        .position(|member_id| *member_id == elected_member)
        .expect("below-finality elected payment member index");
    let mut observation_operation_ids = Vec::new();
    let elected_observation = submit_melt_payment_observation_from_backend(
        &session.states[elected_index],
        &melt_quote_id,
    )
    .await;
    observation_operation_ids.push(elected_observation.operation_id);
    for state in &session.states[..4] {
        wait_for_applied_operation_with_context(
            state,
            elected_observation.operation_id,
            "below-finality elected melt payment observation",
        )
        .await;
    }
    for observer_index in (0..4).filter(|observer_index| *observer_index != elected_index) {
        let observation = matching_melt_payment_observation_envelope(
            &session.states[observer_index],
            &elected_observation.observation,
        );
        let operation_id = submit_live_operation_allowing_pending_timeout(
            &session.states[observer_index],
            observation,
        )
        .await;
        observation_operation_ids.push(operation_id);
        for state in &session.states[..4] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                "below-finality melt payment observation",
            )
            .await;
        }
    }
    assert_eq!(make_payment_calls[elected_index].load(Ordering::SeqCst), 1);
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "recovered melt must elect exactly one payment actor"
    );

    session.states[4]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("final below-finality lagger must fail closed");
    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[4]).await;
    let fifth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[4],
        session.sync_config,
        live_options,
        session.transport.clone(),
    )
    .await
    .expect("restart final below-finality lagger")
    .expect("final below-finality lagger live session handle");
    session._handles.push(fifth_handle);
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[4].clone()),
    );
    let peer = session
        .public_config
        .members
        .get(&member_id(1))
        .expect("below-finality catch-up peer")
        .clone();
    let client = FederationRestPeerClient::new_with_config(
        session.transport.clone(),
        &session.public_config,
        session.private_configs[4].clone(),
        session.sync_config,
    )
    .expect("below-finality catch-up client");
    sync_and_certify_partitioned_member(&session.states[4], &client, &peer, &session.public_config)
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect("caught-up below-finality lagger is ready");

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("below-finality expected journal");
    let expected_operation_ids = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let expected_statuses = expected_entries
        .iter()
        .map(|entry| entry.status())
        .collect::<Vec<_>>();
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("below-finality expected state digest");
    let expected_cursor = session.states[0]
        .federation_aleph_bft_status()
        .await
        .expect("below-finality expected cursor")
        .next_item_index;

    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("below-finality converged journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            expected_statuses
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("below-finality converged state digest"),
            expected_digest
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("below-finality converged cursor")
                .next_item_index,
            expected_cursor
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[swap_input_y])
                .await
                .expect("below-finality final swap proof state"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("below-finality final melt proof states"),
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
                winning_swap_output.blinded_secret,
                losing_swap_output.blinded_secret,
                melt_change.blinded_secret,
            ])
            .await
            .expect("below-finality final blind signatures");
        assert!(signatures[0].is_some());
        assert!(signatures[1].is_none());
        assert!(signatures[2].is_some());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(winning_swap_operation_id, 0)
                .await
                .expect("below-finality final winning share")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(losing_swap_operation_id, 0)
            .await
            .expect("below-finality final losing share")
            .is_empty());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(melt_operation_id, 0)
                .await
                .expect("below-finality final melt share")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(
                winning_swap_operation_id,
            ))
            .await
            .expect("below-finality winning completion")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(
                losing_swap_operation_id,
            ))
            .await
            .expect("below-finality losing completion")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(melt_operation_id))
            .await
            .expect("below-finality melt completion")
            .is_some());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&melt_quote_id)
                .await
                .expect("below-finality final melt quote lookup")
                .expect("below-finality final melt quote")
                .state,
            MeltQuoteState::Paid
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("below-finality final pending operation lookup")
            .is_empty());
    }
    for operation_id in observation_operation_ids {
        assert_eq!(
            wait_for_terminal_operation(&session.states[4], operation_id).await,
            FederationOperationStatus::Applied
        );
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "catch-up must not repeat the recovered melt payment"
    );
    assert_eq!(make_payment_calls[4].load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn aleph_bft_live_four_member_one_offline_value_window_catches_up_without_duplicate_payment()
{
    run_aleph_bft_live_one_member_offline_value_window(4).await;
}

#[tokio::test]
async fn aleph_bft_live_five_member_one_offline_value_window_catches_up_without_duplicate_payment()
{
    run_aleph_bft_live_one_member_offline_value_window(5).await;
}

async fn run_aleph_bft_live_one_member_offline_value_window(member_count: u16) {
    assert!(matches!(member_count, 4 | 5));
    let connected_member_count = usize::from(member_count - 1);
    let required_observations = member_count - 1;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 500);
    let payment_policy = FederationPaymentObservationPolicy::member_quorum(required_observations);
    let (mut session, make_payment_calls) =
        start_multi_member_aleph_bft_live_session_with_payment_policy(
            member_count,
            3,
            live_options.clone(),
            payment_policy,
        )
        .await
        .expect("multi-member live value-operation session");
    let runtime_config = session.states[0]
        .mint
        .federation_config()
        .expect("value-operation federation config")
        .clone();
    let signature_threshold =
        usize::from(session.public_config.threshold_params.signature_threshold);
    let mut expected_operation_ids = Vec::new();

    let offline_index = connected_member_count;
    let offline_member = session.private_configs[offline_index].member_id;
    *session.states[offline_index]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[offline_index]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    session.transport.insert(
        session.federation_api_urls[offline_index].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(session.states[0].clone()),
    );
    let offline_handle = session._handles.remove(offline_index);
    drop(offline_handle);

    let mint_quote_id = QuoteId::new();
    let mint_quote_envelope = runtime_config.mint_quote_operation(
        mint_quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(4),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: mint_quote_id.to_string(),
            request: format!("invoice-{mint_quote_id}"),
            amount: Some(Amount::from(4)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    );
    let mint_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_quote_envelope)
            .await;
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            mint_quote_operation_id,
            "one-offline mint quote",
        )
        .await;
    }
    expected_operation_ids.push(mint_quote_operation_id);

    for observer in 1..=required_observations {
        let observation = mint_payment_observation_envelope_for_member(
            &session.states[0],
            &mint_quote_id,
            observer,
            4,
            "live-value-window-paid",
        );
        let operation_id =
            submit_live_operation_allowing_pending_timeout(&session.states[0], observation).await;
        for state in &session.states[..offline_index] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                &format!("one-offline mint payment observation {observer}"),
            )
            .await;
        }
        expected_operation_ids.push(operation_id);
    }

    let keyset = session.public_config.keysets[0].clone();
    let mint_secrets = (0..4)
        .map(|index| cdk::secret::Secret::new(format!("live value proof {index}")))
        .collect::<Vec<_>>();
    let mint_outputs = mint_secrets
        .iter()
        .map(|secret| {
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(secret.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    let mint_envelope = runtime_config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id.clone(),
            outputs: mint_outputs.clone(),
            signature: None,
        }))
        .expect("baseline mint operation");
    let mint_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_envelope).await;
    expected_operation_ids.push(mint_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, mint_operation_id, "baseline mint").await;
    }

    let mut proofs = Vec::new();
    for (index, (output, secret)) in mint_outputs.iter().zip(mint_secrets).enumerate() {
        proofs.push(
            proof_from_accepted_output(
                &session.states[..offline_index],
                signature_threshold,
                mint_operation_id,
                u32::try_from(index).expect("mint output index"),
                output,
                secret,
            )
            .await,
        );
    }

    let swap_input = proofs.remove(0);
    let swap_input_y = swap_input.y().expect("live swap input y");
    let swap_blinded_output = swap_output(&session.states[0], "one-offline-value-window");
    let swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![swap_blinded_output.clone()],
        ))
        .expect("live swap operation");
    let swap_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], swap_envelope).await;
    expected_operation_ids.push(swap_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, swap_operation_id, "one-offline swap").await;
    }

    for state in &session.states[..offline_index] {
        let restored = state
            .mint
            .restore(RestoreRequest {
                outputs: vec![swap_blinded_output.clone()],
            })
            .await
            .expect("connected member restore");
        assert_eq!(restored.outputs, vec![swap_blinded_output.clone()]);
        assert_eq!(restored.signatures.len(), 1);
        let checked = state
            .mint
            .check_state(&CheckStateRequest {
                ys: vec![swap_input_y],
            })
            .await
            .expect("connected member check-state");
        assert_eq!(checked.states.len(), 1);
        assert_eq!(checked.states[0].state, cdk::nuts::State::Spent);
    }

    let melt_quote_id = QuoteId::new();
    let pending_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(
            1_000,
            serde_json::to_string(&pending_description).expect("pending melt description"),
        ),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let melt_quote_envelope =
        test_melt_quote_operation(&runtime_config, melt_quote_id.clone(), melt_quote_request);
    let melt_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_quote_envelope)
            .await;
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            melt_quote_operation_id,
            "one-offline melt quote",
        )
        .await;
    }
    expected_operation_ids.push(melt_quote_operation_id);

    let melt_input_ys = proofs
        .iter()
        .map(|proof| proof.y().expect("live melt input y"))
        .collect::<Vec<_>>();
    let (melt_envelope, melt_change, elected_member) = (0..128)
        .find_map(|attempt| {
            let mut change = swap_output(
                &session.states[0],
                &format!("one-offline-melt-change-{attempt}"),
            );
            change.amount = Amount::ZERO;
            let envelope = runtime_config
                .melt_operation(cdk::nuts::MeltRequest::new(
                    melt_quote_id.clone(),
                    proofs.clone(),
                    Some(vec![change.clone()]),
                ))
                .expect("live melt operation");
            let operation_id = envelope.operation_id().expect("live melt operation id");
            let elected_member = elected_melt_payment_member(&session.public_config, operation_id);
            (elected_member != offline_member).then_some((envelope, change, elected_member))
        })
        .expect("connected member elected for live melt payment");
    let melt_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_envelope).await;
    expected_operation_ids.push(melt_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, melt_operation_id, "one-offline melt").await;
    }

    let elected_index = session
        .public_config
        .members
        .keys()
        .position(|member_id| *member_id == elected_member)
        .expect("elected melt member index");
    let mut melt_payment_operation_ids = Vec::new();
    let elected_observation = submit_melt_payment_observation_from_backend(
        &session.states[elected_index],
        &melt_quote_id,
    )
    .await;
    expected_operation_ids.push(elected_observation.operation_id);
    melt_payment_operation_ids.push(elected_observation.operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            elected_observation.operation_id,
            "one-offline elected melt payment observation",
        )
        .await;
    }
    for observer_index in
        (0..connected_member_count).filter(|observer_index| *observer_index != elected_index)
    {
        let observation = matching_melt_payment_observation_envelope(
            &session.states[observer_index],
            &elected_observation.observation,
        );
        let operation_id = submit_live_operation_allowing_pending_timeout(
            &session.states[observer_index],
            observation,
        )
        .await;
        for state in &session.states[..offline_index] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                &format!("one-offline melt payment observation {observer_index}"),
            )
            .await;
        }
        expected_operation_ids.push(operation_id);
        melt_payment_operation_ids.push(operation_id);
    }
    assert_eq!(make_payment_calls[elected_index].load(Ordering::SeqCst), 1);
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "accepted melt must elect exactly one outgoing payment actor"
    );
    assert_eq!(make_payment_calls[offline_index].load(Ordering::SeqCst), 0);

    for state in &session.states[..offline_index] {
        for operation_id in &melt_payment_operation_ids {
            wait_for_applied_operation_with_context(
                state,
                *operation_id,
                "one-offline melt payment observation",
            )
            .await;
        }
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("connected melt quote lookup")
            .expect("connected melt quote");
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("connected melt proof states"),
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_payment_observations(
                    cdk::federation::FederationPaymentObservationContext::MeltQuote,
                    &melt_quote_id.to_string(),
                )
                .await
                .expect("connected melt payment observations")
                .len(),
            connected_member_count
        );
    }

    assert!(session.states[offline_index]
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(swap_operation_id)
        .await
        .expect("offline swap journal lookup")
        .is_none());
    session.states[offline_index]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[offline_index]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("offline value-operation member must remain fail-closed");

    let offline_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut session.states[offline_index],
        session.sync_config,
        live_options,
        session.transport.clone(),
    )
    .await
    .expect("restart offline value-operation member")
    .expect("offline value-operation live session handle");
    session._handles.push(offline_handle);
    session.transport.insert(
        session.federation_api_urls[offline_index].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[offline_index].clone()),
    );
    let peer = session
        .public_config
        .members
        .get(&member_id(1))
        .expect("value-operation catch-up peer")
        .clone();
    let client = FederationRestPeerClient::new_with_config(
        session.transport.clone(),
        &session.public_config,
        session.private_configs[offline_index].clone(),
        session.sync_config,
    )
    .expect("value-operation catch-up client");
    sync_and_certify_partitioned_member(
        &session.states[offline_index],
        &client,
        &peer,
        &session.public_config,
    )
    .await;

    for operation_id in &expected_operation_ids {
        wait_for_recorded_operation(&session.states[offline_index], *operation_id).await;
    }
    session.states[offline_index]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect("caught-up value-operation member is ready");
    for (index, state) in session.states.iter().enumerate() {
        let stats = state
            .federation_sync_transition_stats()
            .await
            .expect("value-operation sync transition stats");
        if index == offline_index {
            assert!(stats.lagging > 0);
            assert!(stats.catching_up > 0);
        } else {
            assert_eq!(
                stats,
                crate::FederationSyncTransitionStats::default(),
                "only the intentionally offline member should enter catch-up"
            );
        }
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "catch-up replay must not duplicate the elected outgoing payment"
    );
    assert_eq!(
        make_payment_calls[offline_index].load(Ordering::SeqCst),
        0,
        "non-elected lagging member must not pay while replaying melt history"
    );

    let caught_up = &session.states[offline_index];
    let restored = caught_up
        .mint
        .restore(RestoreRequest {
            outputs: vec![swap_blinded_output.clone()],
        })
        .await
        .expect("caught-up member restore");
    assert_eq!(restored.outputs, vec![swap_blinded_output.clone()]);
    assert_eq!(restored.signatures.len(), 1);
    let checked = caught_up
        .mint
        .check_state(&CheckStateRequest {
            ys: vec![swap_input_y],
        })
        .await
        .expect("caught-up member check-state");
    assert_eq!(checked.states.len(), 1);
    assert_eq!(checked.states[0].state, cdk::nuts::State::Spent);
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("caught-up melt proof states"),
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MintQuote,
                &mint_quote_id.to_string(),
            )
            .await
            .expect("caught-up mint payment observations")
            .len(),
        connected_member_count
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &melt_quote_id.to_string(),
            )
            .await
            .expect("caught-up melt payment observations")
            .len(),
        connected_member_count
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(swap_operation_id, 0)
            .await
            .expect("caught-up swap share provenance")
            .len(),
        1
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(melt_operation_id, 0)
            .await
            .expect("caught-up melt change share provenance")
            .first()
            .map(|record| record.blinded_secret),
        Some(melt_change.blinded_secret)
    );
    for operation_id in [mint_operation_id, swap_operation_id, melt_operation_id] {
        assert!(caught_up
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(operation_id))
            .await
            .expect("caught-up completed operation lookup")
            .is_some());
    }

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected value-operation journal");
    assert_eq!(
        expected_entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>(),
        expected_operation_ids
    );
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected value-operation state digest");
    let expected_cursor = session.states[0]
        .federation_aleph_bft_status()
        .await
        .expect("expected value-operation cursor")
        .next_item_index;
    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged value-operation journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged value-operation digest"),
            expected_digest
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("converged value-operation cursor")
                .next_item_index,
            expected_cursor
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(swap_operation_id, 0)
                .await
                .expect("converged swap share provenance")
                .len(),
            1,
            "restore and catch-up must not duplicate swap signing shares"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(melt_operation_id, 0)
                .await
                .expect("converged melt share provenance")
                .len(),
            1,
            "catch-up must not duplicate melt change signing shares"
        );
        for output_index in 0..4 {
            assert_eq!(
                state
                    .mint
                    .localstore()
                    .get_federation_signature_shares_for_output(mint_operation_id, output_index,)
                    .await
                    .expect("converged mint share provenance")
                    .len(),
                1,
                "catch-up must regenerate exactly one mint share per output"
            );
        }
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("converged pending operations")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_keyset_rotation_with_one_unavailable_member() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
    ];
    let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
    let active_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[41 + index as u8; 32]))
        .collect::<Vec<_>>();
    let inactive_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[51 + index as u8; 32]))
        .collect::<Vec<_>>();
    let active_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), active_coefficients)]),
    );
    let inactive_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        false,
        0,
        None,
        BTreeMap::from([(Amount::from(1), inactive_coefficients)]),
    );
    let (public_config, private_configs) =
        transport_config_with_member_federation_api_urls_and_keysets(
            threshold_params,
            federation_api_urls.clone(),
            vec![active_keyset, inactive_keyset],
        );
    let old_keyset_id = public_config
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("active keyset")
        .id;
    let mut next_keyset = public_config
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive keyset")
        .clone();
    next_keyset.active = true;

    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
            )
            .await,
        );
    }

    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(30),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    transport.insert(
        federation_api_urls[1].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    let mut live_session_handles = Vec::new();
    for index in [0_usize, 2, 3] {
        let options = fast_aleph_bft_live_session_options(pump_config, 500);
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &mut states[index],
            sync_config,
            options,
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
    assert_eq!(live_session_handles.len(), 3);

    for index in [0_usize, 2, 3] {
        states[index].federation.peer_sync_config = Some(sync_config);
    }

    let runtime_config =
        cdk::mint::FederationRuntimeConfig::new(public_config.clone(), private_configs[0].clone())
            .expect("runtime config");
    let envelope = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: next_keyset.clone(),
        },
    ));
    let operation_id = envelope.operation_id().expect("operation id");
    let first_envelope = envelope.clone();
    let third_envelope = envelope.clone();
    let fourth_envelope = envelope;
    let (first, third, fourth) = tokio::join!(
        states[0].submit_federation_envelope_to_consensus(first_envelope),
        states[2].submit_federation_envelope_to_consensus(third_envelope),
        states[3].submit_federation_envelope_to_consensus(fourth_envelope)
    );
    let responses = [first, third, fourth];
    if responses.iter().any(Result::is_err) {
        let errors = responses
            .iter()
            .filter_map(|response| response.as_ref().err())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if responses.iter().filter(|response| response.is_ok()).count() < 3 {
            let mut network_pairs = BTreeMap::<(MemberId, MemberId), (usize, usize)>::new();
            for request in transport.aleph_bft_network_message_requests() {
                let encoded_bytes = request.payload.encoded_message.len() / 2;
                let entry = network_pairs
                    .entry((request.sender, request.receiver))
                    .or_default();
                entry.0 = entry.0.saturating_add(1);
                entry.1 = entry.1.max(encoded_bytes);
            }
            let mut diagnostics = Vec::new();
            for index in [0_usize, 2, 3] {
                let status = states[index]
                    .federation_aleph_bft_status()
                    .await
                    .expect("AlephBFT status");
                let entries = states[index]
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("journal lookup");
                let pending = states[index]
                    .mint
                    .localstore()
                    .get_federation_aleph_bft_pending_operations(0)
                    .await
                    .expect("pending operation lookup");
                let entry_ids = entries
                    .iter()
                    .map(|entry| (entry.operation_id, entry.status()))
                    .collect::<Vec<_>>();
                let pending_ids = pending
                    .iter()
                    .map(|pending| pending.operation_id)
                    .collect::<Vec<_>>();
                diagnostics.push(format!(
                    "member {index}: status={status:?}, entries={entry_ids:?}, pending={pending_ids:?}"
                ));
            }
            panic!(
                "three live members should finalize keyset rotation: {}; diagnostics: {}; network_pairs={network_pairs:?}",
                errors.join("; "),
                diagnostics.join(" | ")
            );
        }
    }
    let response = responses
        .into_iter()
        .find_map(Result::ok)
        .expect("at least one keyset rotation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for index in [0_usize, 2, 3] {
        wait_for_applied_operation(&states[index], operation_id).await;
        let effective = states[index]
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federation config");
        assert!(
            effective
                .public_config()
                .keysets
                .iter()
                .any(|keyset| keyset.id == next_keyset.id && keyset.active),
            "rotated keyset is active for member {index}"
        );
        assert!(
            effective
                .public_config()
                .keysets
                .iter()
                .any(|keyset| keyset.id == old_keyset_id && !keyset.active),
            "old keyset is retired for member {index}"
        );
        assert!(states[index]
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after keyset rotation")
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let mut pending_operations = Vec::new();
    for _ in 0..3 {
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&states[0], &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let err = states[0]
            .submit_federation_envelope_to_consensus(envelope)
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
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

    *states[0].federation.aleph_bft_network_inbound.write().await = None;
    *states[0].federation.aleph_bft_live_session.write().await = None;
    live_session_handles.remove(0);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
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
            panic!(
                "member {index} did not apply restarted AlephBFT operation; status={status:?}; journal_entries={}; pending_operations={}",
                entries.len(),
                pending.len()
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

    let expected_entries = states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("unit-creation crash-window journal");
    assert_eq!(expected_entries.len(), 1);
    assert_eq!(expected_entries[0].operation_id, operation_id);
    let expected_digest = materialized_state_digest(&states[0], &expected_entries)
        .await
        .expect("unit-creation crash-window state digest");
    for state in &states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged unit-creation crash-window journal");
        assert_eq!(
            journal_convergence_projection(&entries),
            journal_convergence_projection(&expected_entries)
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged unit-creation crash-window state digest"),
            expected_digest
        );
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
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
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    assert!(!wait_for_aleph_bft_backup_bytes(&first_state, 0)
        .await
        .is_empty());

    *first_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *first_state.federation.aleph_bft_live_session.write().await = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        first_private,
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
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
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_seed,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
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
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first operation finality before crash-window recovery");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    let backup_bytes = wait_for_aleph_bft_backup_bytes(&first_state, 0).await;

    *first_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *first_state.federation.aleph_bft_live_session.write().await = None;
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
    let mut recovered_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        first_private,
        Arc::clone(&crash_window_db),
        &first_seed,
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
    recovered_first_state.federation.consensus = replayed_consensus;
    recovered_first_state.federation.sequences = replayed_sequences;
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
    let peer_entries = second_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal after backup-only recovery");
    assert_eq!(
        journal_convergence_projection(&recovered_entries),
        journal_convergence_projection(&peer_entries)
    );
    assert_eq!(
        materialized_state_digest(&recovered_first_state, &recovered_entries)
            .await
            .expect("recovered backup-only state digest"),
        materialized_state_digest(&second_state, &peer_entries)
            .await
            .expect("peer backup-only state digest")
    );

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&recovered_first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = recovered_first_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second operation finality after backup-only recovery");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
    let expected_entries = second_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal after backup-only continuation");
    let expected_digest = materialized_state_digest(&second_state, &expected_entries)
        .await
        .expect("peer state digest after backup-only continuation");
    let recovered_entries = recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("recovered journal after backup-only continuation");
    assert_eq!(
        journal_convergence_projection(&recovered_entries),
        journal_convergence_projection(&expected_entries)
    );
    assert_eq!(
        materialized_state_digest(&recovered_first_state, &recovered_entries)
            .await
            .expect("recovered state digest after backup-only continuation"),
        expected_digest
    );
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
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
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
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first five-member operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, first_operation_id).await;
    }
    let backup_before_restart = wait_for_aleph_bft_backup_bytes(&states[0], 0).await;

    *states[0].federation.aleph_bft_network_inbound.write().await = None;
    *states[0].federation.aleph_bft_live_session.write().await = None;
    let first_handle = live_session_handles.remove(0);
    drop(first_handle);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
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
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second five-member operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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
        .submit_federation_envelope_to_consensus(third_envelope)
        .await
        .expect("third five-member operation finality after restart");
    assert_eq!(response.operation_id, third_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
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

async fn wait_for_applied_operation(state: &MintState, operation_id: FederationOperationId) {
    assert!(
        operation_applied_within(state, operation_id, Duration::from_secs(15)).await,
        "operation applied before timeout"
    );
}

async fn wait_for_recorded_operation(state: &MintState, operation_id: FederationOperationId) {
    operation_terminal_within(state, operation_id, Duration::from_secs(15))
        .await
        .expect("operation recorded before timeout");
}

fn journal_convergence_projection(
    entries: &[FederationJournalEntry],
) -> Vec<(
    u64,
    FederationOperationId,
    FederationJournalDigest,
    FederationJournalDigest,
    FederationOperationStatus,
)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.index,
                entry.operation_id,
                entry.previous_order_digest,
                entry.order_digest,
                entry.status(),
            )
        })
        .collect()
}

async fn wait_for_terminal_operation(
    state: &MintState,
    operation_id: FederationOperationId,
) -> FederationOperationStatus {
    operation_terminal_within(state, operation_id, Duration::from_secs(30))
        .await
        .expect("operation reached terminal status before timeout")
}

async fn replay_aleph_bft_runtime_after_test_process_restart(state: &mut MintState) {
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("replay AlephBFT runtime after test process restart");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
}

async fn submit_live_operation_allowing_pending_timeout(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationOperationId {
    let operation_id = envelope.operation_id().expect("live operation id");
    match state
        .submit_federation_envelope_to_consensus(envelope)
        .await
    {
        Ok(response) => assert_eq!(response.operation_id, operation_id),
        Err(err) => assert_pending_finalization_error(&err, operation_id, 10_000),
    }

    operation_id
}

async fn wait_for_applied_operation_with_context(
    state: &MintState,
    operation_id: FederationOperationId,
    context: &str,
) {
    if operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
        return;
    }

    let member_id = state
        .mint
        .federation_config()
        .expect("contextual apply wait federation config")
        .member_id();
    let status = state.federation_aleph_bft_status().await;
    let journal_operation_ids = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("contextual apply wait journal")
        .into_iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let pending_operation_ids = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("contextual apply wait pending operations")
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect::<Vec<_>>();
    panic!(
        "{context}: operation {operation_id} did not apply on member {member_id} before timeout; status={status:?}; journal_operation_ids={journal_operation_ids:?}; pending_operation_ids={pending_operation_ids:?}"
    );
}

async fn sync_and_certify_partitioned_member(
    state: &MintState,
    client: &FederationRestPeerClient<RouterFederationTransportRegistry>,
    peer: &MemberConfigPublic,
    public_config: &FederationConfig,
) {
    let catch_up_deadline = time::Instant::now() + Duration::from_secs(15);
    loop {
        match client.sync_once_from_member(state, peer).await {
            Ok(_) => break,
            Err(_) if time::Instant::now() < catch_up_deadline => {
                time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("partitioned member finalized-item catch-up: {err}"),
        }
    }
    if state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .is_err()
    {
        let local_member = state
            .mint
            .federation_config()
            .expect("partitioned member federation config")
            .member_id();
        let peers = public_config
            .members
            .values()
            .filter(|member| member.member_id != local_member)
            .cloned()
            .collect::<Vec<_>>();
        let synced = run_federation_peer_sync_round(state, client, &peers).await;
        assert!(
            synced >= usize::from(public_config.threshold_params.consensus_threshold) - 1,
            "catch-up certification must reach a remote BFT quorum"
        );
    }
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect("certified caught-up member is ready");
}

const LATENCY_EXPECTED_STAGES: &[CdkAlephBftLatencyStage] = &[
    CdkAlephBftLatencyStage::SubmitQueued,
    CdkAlephBftLatencyStage::DataProviderYielded,
    CdkAlephBftLatencyStage::NetworkMessageDelivered,
    CdkAlephBftLatencyStage::FinalizedDataReceived,
    CdkAlephBftLatencyStage::ApplyLoopReceived,
    CdkAlephBftLatencyStage::ApplyCompleted,
    CdkAlephBftLatencyStage::OperationNotified,
    CdkAlephBftLatencyStage::WaiterReturned,
];

struct TwoMemberAlephBftLiveSession {
    first_state: MintState,
    second_state: MintState,
    _first_handle: Arc<AlephBftSessionSupervisor>,
    _second_handle: Arc<AlephBftSessionSupervisor>,
}

struct MultiMemberAlephBftLiveSession {
    states: Vec<MintState>,
    transport: RouterFederationTransportRegistry,
    federation_api_urls: Vec<MintUrl>,
    public_config: FederationConfig,
    private_configs: Vec<MemberConfigPrivate>,
    sync_config: FederationPeerSyncConfig,
    _handles: Vec<Arc<AlephBftSessionSupervisor>>,
}

async fn start_two_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<TwoMemberAlephBftLiveSession, cdk::Error> {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };

    let first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut first_state,
        sync_config,
        live_options.clone(),
        transport.clone(),
    )
    .await?
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut second_state,
        sync_config,
        live_options,
        transport.clone(),
    )
    .await?
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    Ok(TwoMemberAlephBftLiveSession {
        first_state,
        second_state,
        _first_handle: first_handle,
        _second_handle: second_handle,
    })
}

async fn start_five_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    start_multi_member_aleph_bft_live_session(5, 3, live_options).await
}

async fn start_multi_member_aleph_bft_live_session(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    let (session, _) = start_multi_member_aleph_bft_live_session_with_payment_policy(
        member_count,
        signature_threshold,
        live_options,
        FederationPaymentObservationPolicy::development_single_observation_for_testing(),
    )
    .await?;

    Ok(session)
}

async fn start_multi_member_aleph_bft_live_session_with_payment_policy(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
    payment_observation_policy: FederationPaymentObservationPolicy,
) -> Result<(MultiMemberAlephBftLiveSession, Vec<Arc<AtomicUsize>>), cdk::Error> {
    let federation_api_urls = (1..=member_count)
        .map(|value| {
            MintUrl::from_str(&format!("http://fed{value}.test")).expect("federation api url")
        })
        .collect::<Vec<_>>();
    let (mut public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(member_count, signature_threshold)
            .expect("threshold params"),
        federation_api_urls.clone(),
    );
    public_config.payment_observation_policy = payment_observation_policy;
    public_config.validate()?;
    let mut states = Vec::new();
    let mut make_payment_calls = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let (payment_processor, member_make_payment_calls) =
            CountingFakeWallet::with_call_counter();
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
                payment_processor,
            )
            .await,
        );
        make_payment_calls.push(member_make_payment_calls);
    }
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let mut handles = Vec::new();
    for (index, state) in states.iter_mut().enumerate() {
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            state,
            sync_config,
            live_options.clone(),
            transport.clone(),
        )
        .await?
        .expect("live session handle");
        handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
    }

    Ok((
        MultiMemberAlephBftLiveSession {
            states,
            transport,
            federation_api_urls,
            public_config,
            private_configs,
            sync_config,
            _handles: handles,
        },
        make_payment_calls,
    ))
}

async fn submit_healthy_federation_swap_batch(
    session: &MultiMemberAlephBftLiveSession,
    first_index: usize,
    count: usize,
    parallelism: usize,
) -> Vec<FederationOperationId> {
    let config = session.states[0]
        .mint
        .federation_config()
        .expect("stress federation config")
        .clone();
    let operations = (first_index..first_index + count)
        .map(|index| {
            let envelope = config
                .swap_operation(cdk::nuts::SwapRequest::new(
                    vec![proof_for_amount_with_aggregate_secret(
                        &session.states[0],
                        1,
                        &format!("healthy-stress-input-{index}"),
                        &BlsSecretKey::from_reduced_bytes(&[31; 32]),
                    )],
                    vec![swap_output(
                        &session.states[0],
                        &format!("healthy-stress-output-{index}"),
                    )],
                ))
                .expect("stress swap envelope");
            let operation_id = envelope.operation_id().expect("stress swap operation id");
            (operation_id, envelope)
        })
        .collect::<Vec<_>>();

    for chunk in operations.chunks(parallelism) {
        let mut submissions = tokio::task::JoinSet::new();
        for (offset, (operation_id, envelope)) in chunk.iter().cloned().enumerate() {
            let state = session.states[(first_index + offset) % session.states.len()].clone();
            submissions.spawn(async move {
                let response = state
                    .submit_federation_envelope_to_consensus(envelope)
                    .await
                    .expect("healthy stress swap submission");
                assert_eq!(response.operation_id, operation_id);
                assert_eq!(
                    response.outcome.entry().status(),
                    FederationOperationStatus::Applied
                );
            });
        }
        while let Some(result) = submissions.join_next().await {
            result.expect("healthy stress swap task");
        }
    }

    operations
        .into_iter()
        .map(|(operation_id, _)| operation_id)
        .collect()
}

fn spawn_healthy_federation_peer_sync_tasks(
    session: &MultiMemberAlephBftLiveSession,
) -> Vec<tokio::task::JoinHandle<()>> {
    session
        .states
        .iter()
        .cloned()
        .zip(session.private_configs.iter().cloned())
        .enumerate()
        .map(|(member_rank, (state, private_config))| {
            let public_config = session.public_config.clone();
            let local_member = private_config.member_id;
            let peers = public_config
                .members
                .values()
                .filter(|member| member.member_id != local_member)
                .cloned()
                .collect::<Vec<_>>();
            let sync_config = session.sync_config;
            let transport = session.transport.clone();
            tokio::spawn(async move {
                let client = FederationRestPeerClient::new_with_config(
                    transport,
                    &public_config,
                    private_config,
                    sync_config,
                )
                .expect("healthy stress peer client");
                let initial_delay = federation_peer_sync_initial_delay(
                    sync_config.peer_sync_interval,
                    member_rank,
                    public_config.members.len(),
                );
                if !initial_delay.is_zero() {
                    time::sleep(initial_delay).await;
                }
                let mut interval = federation_peer_sync_interval(sync_config);
                let mut reconnect_backoff = FederationPeerReconnectBackoff::default();
                loop {
                    interval.tick().await;
                    run_federation_peer_sync_round_with_backoff(
                        &state,
                        &client,
                        &peers,
                        &mut reconnect_backoff,
                    )
                    .await;
                }
            })
        })
        .collect()
}

#[tokio::test]
#[ignore = "process-level federation stress regression"]
async fn aleph_bft_live_healthy_swaps_do_not_trigger_catch_up() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let session = start_five_member_aleph_bft_live_session(
        FederationAlephBftLiveSessionOptions::new(pump_config),
    )
    .await
    .expect("five-member healthy stress session");
    for state in &session.states {
        state.restore_federation_startup_readiness().await;
    }
    let sync_tasks = spawn_healthy_federation_peer_sync_tasks(&session);

    let operation_ids = submit_healthy_federation_swap_batch(&session, 0, 5, 1).await;

    for operation_id in &operation_ids {
        for state in &session.states {
            wait_for_applied_operation_with_context(
                state,
                *operation_id,
                "healthy stress convergence",
            )
            .await;
        }
    }
    for task in &sync_tasks {
        task.abort();
    }
    for task in sync_tasks {
        let err = task.await.expect_err("healthy stress peer sync task abort");
        assert!(err.is_cancelled());
    }

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("healthy stress journal");
    assert_eq!(expected_entries.len(), operation_ids.len());
    assert!(expected_entries
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let expected_order = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("healthy stress materialized digest");
    let mut heights = Vec::new();

    for (member_index, state) in session.states.iter().enumerate() {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("member healthy stress journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_order
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("member healthy stress materialized digest"),
            expected_digest
        );
        let sync_status = state.federation_sync_status().await;
        let transition_stats = state
            .federation_sync_transition_stats()
            .await
            .expect("member sync transition stats");
        assert_eq!(
            transition_stats,
            crate::FederationSyncTransitionStats::default(),
            "healthy load must not enter lagging or catch-up states: member={}, status={sync_status:?}",
            member_index + 1,
        );
        heights.push(
            state
                .federation
                .consensus
                .as_ref()
                .expect("member consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
        );
    }

    let minimum_height = heights
        .iter()
        .min()
        .copied()
        .expect("minimum member height");
    let maximum_height = heights
        .iter()
        .max()
        .copied()
        .expect("maximum member height");
    assert_eq!(
        maximum_height - minimum_height,
        0,
        "healthy stress members must converge without height skew"
    );
}

async fn submit_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let response = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    })
    .await
    .expect("submit task")
    .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    operation_id
}

async fn submit_duplicate_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_state = state.clone();
    let first_envelope = envelope.clone();
    let first = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    let (first_response, second_response) = tokio::join!(first, second);
    let first_response = first_response
        .expect("first submit task")
        .expect("first live AlephBFT operation response");
    let second_response = second_response
        .expect("second submit task")
        .expect("second live AlephBFT operation response");

    for response in [first_response, second_response] {
        assert_eq!(response.operation_id, operation_id);
        assert_eq!(
            response.outcome.entry().status(),
            FederationOperationStatus::Applied
        );
    }

    operation_id
}

async fn wait_for_latency_stage(
    recorder: &CdkAlephBftLatencyRecorder,
    operation_id: FederationOperationId,
    stage: CdkAlephBftLatencyStage,
) {
    time::timeout(
        Duration::from_secs(5),
        recorder.wait_for_stage(operation_id, stage),
    )
    .await
    .expect("AlephBFT latency stage was not recorded before timeout")
}

fn assert_latency_stage_order(
    events: &[CdkAlephBftLatencyEvent],
    expected: &[CdkAlephBftLatencyStage],
) {
    let mut cursor = 0usize;
    for event in events {
        if event.stage == expected[cursor] {
            cursor += 1;
            if cursor == expected.len() {
                return;
            }
        }
    }

    panic!(
        "AlephBFT latency stages were not observed in order; expected {:?}, got {}",
        expected,
        format_latency_events(events)
    );
}

fn latency_stage_delta(
    events: &[CdkAlephBftLatencyEvent],
    start: CdkAlephBftLatencyStage,
    end: CdkAlephBftLatencyStage,
) -> Option<Duration> {
    let start_elapsed = events
        .iter()
        .find(|event| event.stage == start)
        .map(|event| event.elapsed)?;
    let end_elapsed = events
        .iter()
        .find(|event| event.stage == end)
        .map(|event| event.elapsed)?;

    end_elapsed.checked_sub(start_elapsed)
}

fn latency_stage_count(
    events: &[CdkAlephBftLatencyEvent],
    stage: CdkAlephBftLatencyStage,
) -> usize {
    events.iter().filter(|event| event.stage == stage).count()
}

fn format_latency_events(events: &[CdkAlephBftLatencyEvent]) -> String {
    events
        .iter()
        .map(|event| format!("{:?}@{}ms", event.stage, event.elapsed.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fast_aleph_bft_live_session_options(
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    delay_config.tick_interval = Duration::from_millis(5);
    delay_config.unit_rebroadcast_interval_min = Duration::from_millis(20);
    delay_config.unit_rebroadcast_interval_max = Duration::from_millis(40);
    delay_config.unit_creation_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.coord_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.parent_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.newest_request_delay = Arc::new(|_| Duration::from_millis(10));

    FederationAlephBftLiveSessionOptions::new(pump_config)
        .with_max_round(max_round)
        .with_delay_config(delay_config)
}

async fn wait_for_aleph_bft_session(state: &MintState, session_id: u64) {
    time::timeout(Duration::from_secs(10), async {
        loop {
            let status = state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status");
            if status.session == session_id {
                return;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT session did not advance before timeout")
}

async fn operation_applied_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> bool {
    operation_terminal_within(state, operation_id, timeout).await
        == Some(FederationOperationStatus::Applied)
}

async fn operation_terminal_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> Option<FederationOperationStatus> {
    if let Some(entry) = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await
        .expect("operation status lookup")
    {
        if matches!(
            entry.status(),
            FederationOperationStatus::Applied | FederationOperationStatus::Rejected
        ) {
            return Some(entry.status());
        }
    }

    let response = time::timeout(
        timeout,
        state.wait_for_federation_operation_acceptance(operation_id),
    )
    .await
    .ok()
    .flatten()?;
    let status = response.outcome.entry().status();
    matches!(
        status,
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )
    .then_some(status)
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

async fn wait_for_aleph_bft_network_message_request(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    receiver: MemberId,
) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest> {
    time::timeout(
        Duration::from_secs(5),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender && request.receiver == receiver
        }),
    )
    .await
    .expect("AlephBFT network message request was not captured")
}

async fn wait_for_aleph_bft_unit_with_operation(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let request = time::timeout(
        Duration::from_secs(10),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender
                && aleph_bft_network_request_contains_operation(request, operation_id)
        }),
    )
    .await
    .ok()?;
    aleph_bft_network_request_operation_unit_index(&request, operation_id)
}

fn aleph_bft_network_request_contains_operation(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> bool {
    aleph_bft_network_request_operation_unit_index(request, operation_id).is_some()
}

fn aleph_bft_network_request_operation_unit_index(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let bytes = request.payload.message_bytes().ok()?;
    let mut input = &bytes[..];
    let message = CdkAlephBftNetworkData::decode(&mut input).ok()?;
    if !input.is_empty() {
        return None;
    }
    message.included_data().iter().find_map(|data| {
        let payload = data.payload();
        payload
            .operation_ids
            .contains(&operation_id)
            .then_some(payload.unit_index)
    })
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

async fn seed_aleph_bft_backup(state: &MintState, session_id: u64, backup_bytes: &[u8]) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT backup seed transaction");
    tx.append_federation_aleph_bft_backup_bytes(session_id, backup_bytes)
        .await
        .expect("seed AlephBFT backup bytes");
    tx.set_federation_aleph_bft_active_session(session_id)
        .await
        .expect("seed AlephBFT active session");
    tx.commit()
        .await
        .expect("commit AlephBFT backup seed transaction");
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
async fn aleph_bft_public_submit_is_not_ready_until_live_session_is_started() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("AlephBFT public submit should be gated until a live session is started");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeHalted { .. })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_public_submit_stays_gated_when_sync_is_healthy_without_live_session() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.mark_federation_healthy().await;
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("healthy sync must not reopen AlephBFT submit without a live session");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::RuntimeUnavailable {
            component: "AlephBFT live session",
        })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_live_session_opens_queueing_but_still_waits_for_finalized_unit() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.mark_federation_healthy().await;
    let sync_config = FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    };
    state.federation.peer_sync_config = Some(sync_config);
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect_err("healthy sync without a live AlephBFT session must stay gated");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::RuntimeUnavailable {
            component: "AlephBFT live session",
        })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());
    {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        transport,
    )
    .await
    .expect("start AlephBFT live session")
    .expect("live session handle");
    assert!(state.federation_aleph_bft_live_session_active().await);
    assert!(state.federation_aleph_bft_network_inbound_active().await);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("live AlephBFT session should still wait for a finalized unit");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after unfinalized submit")
        .is_none());
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
    drop(handle);
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
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

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
        network
            .next_event()
            .await
            .expect("broadcast loops back to local AlephBFT network"),
        42
    );
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
async fn aleph_bft_network_pump_delivers_already_awaited_outbound_message() {
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
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(42, Recipient::Everyone);
    let first_outbound = handle.next_outgoing().await.expect("outbound message");
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        Some(first_outbound),
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        network
            .next_event()
            .await
            .expect("already-awaited broadcast loops back locally"),
        42
    );
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_pump_leaves_retry_queue_empty_after_success() {
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
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(42, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_requeued_members.is_empty());
    assert_eq!(outcome.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
}

#[tokio::test]
async fn aleph_bft_network_pump_quorum_fast_path_does_not_block_fresh_messages_on_slow_peer() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_network_message_response(response)
        .with_mock_aleph_bft_network_message_delays([
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(5),
        ]);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            peer_request_timeout: Duration::from_secs(10),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 7,
        ..CdkAlephBftNetworkPumpConfig::default()
    };

    network.send(41, Recipient::Everyone);
    let first = time::timeout(
        Duration::from_secs(1),
        run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
            &mut handle,
            &keychain,
            &client,
            pump_config,
            &mut retry_queue,
        ),
    )
    .await
    .expect("quorum delivery must not await the slow peer timeout")
    .expect("first pump tick");
    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert_eq!(first.remote_accepted_members.len(), 3);
    assert!(first.remote_failed_members.is_empty());
    assert_eq!(first.remote_requeued_members.len(), 1);
    assert_eq!(first.retry_queue_len, 1);

    network.send(42, Recipient::Everyone);
    let second = time::timeout(
        Duration::from_secs(1),
        run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
            &mut handle,
            &keychain,
            &client,
            pump_config,
            &mut retry_queue,
        ),
    )
    .await
    .expect("fresh message must bypass queued retry work")
    .expect("second pump tick");
    assert_eq!(second.messages_drained, 1);
    assert_eq!(second.retry_messages_drained, 0);
    assert_eq!(second.remote_accepted_members.len(), 4);
    assert_eq!(second.retry_queue_len, 1);

    let retry = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("idle retry tick");
    assert_eq!(retry.messages_drained, 0);
    assert_eq!(retry.retry_messages_drained, 1);
    assert_eq!(retry.remote_accepted_members.len(), 1);
    assert_eq!(retry.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
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
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

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
    assert_eq!(
        network
            .next_event()
            .await
            .expect("local node recipient loops back locally"),
        100
    );
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
async fn aleph_bft_network_retry_queue_retries_requeued_failure_once_per_tick() {
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
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 12,
        max_messages_per_tick: 8,
        max_peer_deliveries_per_message: 8,
    };

    network.send(789, Recipient::Everyone);
    let first = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("network pump first tick");
    let first_request_count = transport.aleph_bft_network_message_requests().len();

    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert_eq!(first_request_count, 4);
    assert_eq!(retry_queue.len(), 1);

    let second = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("network pump retry tick");

    assert_eq!(second.messages_drained, 0);
    assert_eq!(second.retry_messages_drained, 1);
    assert_eq!(second.retry_queue_len, 1);
    assert_eq!(retry_queue.len(), 1);
    assert_eq!(
        transport.aleph_bft_network_message_requests().len(),
        first_request_count + 4
    );
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_drops_non_retryable_status_failures() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_status_error(
        400,
        "Invalid federation AlephBFT network message: Codec error",
    );
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(789, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 12,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump tick handles non-retryable peer failures");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(outcome.retry_messages_drained, 0);
    assert!(outcome.remote_accepted_members.is_empty());
    assert_eq!(
        outcome.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_requeued_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(outcome.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_evicts_oldest_delivery_at_bound() {
    let (public_config, local_private, _) = transport_config();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
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
    assert_eq!(outcome.retry_deliveries_dropped, 1);
    assert_eq!(
        transport.aleph_bft_network_message_requests().len(),
        message_count
    );
}

#[tokio::test]
async fn aleph_bft_startup_does_not_restore_transient_healthy_status() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    first_state.mark_federation_healthy().await;
    assert!(matches!(
        first_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));

    let restored_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
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
        .expect_err("transient healthy status must not reopen AlephBFT public submit");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeHalted { .. })
    ));
    assert!(restored_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restored gated submit")
        .is_none());

    let consensus = restored_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_startup_restore_retires_startup_halt_after_live_session_for_empty_history() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut restored_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    assert!(matches!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { reason })
            if reason.contains("public federation submissions are disabled")
    ));

    let sync_config = FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    };
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restored_state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        transport,
    )
    .await
    .expect("start AlephBFT live session")
    .expect("live session handle");
    restored_state.restore_federation_startup_readiness().await;
    assert_eq!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );

    restored_state.federation.peer_sync_config = Some(sync_config);
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
        .federation
        .consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_restart_after_unfinalized_local_unit_requires_retry_without_state_mutation() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &seed,
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
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.pending_mempool_len(), 1);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }
    drop(state);

    let restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &seed,
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
            .federation
            .consensus
            .as_ref()
            .expect("restarted consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.pending_mempool_len(), 0);
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
async fn aleph_bft_submit_envelope_waits_for_finalization() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let submit_response = submit.await.expect("submit task");
    assert_eq!(submit_response.operation_id, operation_id);
    assert!(matches!(
        submit_response.outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));
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
async fn aleph_bft_submit_envelope_does_not_persist_dkg_sequence_state() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let sequences_before = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences before finalization");
    assert!(sequences_before.is_empty());

    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    submit.await.expect("submit task");
    let sequences_after = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after finalization");
    assert!(
        sequences_after.is_empty(),
        "operation finalization must not mutate durable DKG replay state"
    );
}

#[tokio::test]
async fn aleph_bft_submit_envelope_times_out_without_finalization() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_to_consensus(envelope)
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

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
}

#[tokio::test]
async fn aleph_bft_timed_out_submit_retry_receives_finalized_response_without_requeueing() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("unfinalized submit should time out");
    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let retry_state = state.clone();
    let retry_submit = tokio::spawn(async move {
        retry_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let retry_response = retry_submit
        .await
        .expect("retry submit task")
        .expect("retry submit response");
    assert_eq!(retry_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
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
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let first_err = state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect_err("first unfinalized submit should time out");
    assert_pending_finalization_error(&first_err, first_operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let second_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let second_operation_id = second_envelope.operation_id().expect("second operation id");

    let second_err = state
        .submit_federation_envelope_to_consensus(second_envelope)
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
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let rejected_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let rejected_operation_id = rejected_envelope
        .operation_id()
        .expect("rejected operation id");
    let err = state
        .submit_federation_envelope_to_consensus(rejected_envelope)
        .await
        .expect_err("full response waiter set should reject");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::ResponseWaiterLimitExceeded { waiting: 1, max: 1 })
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let session = consensus
        .aleph_bft_status()
        .expect("consensus status")
        .session;
    drop(consensus);
    let pending = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after waiter rejection");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, first_operation_id);
    assert!(pending
        .iter()
        .all(|operation| operation.operation_id != rejected_operation_id));

    first_submit.abort();
    let _ = first_submit.await;
    state
        .wait_for_federation_pending_response_waiter_count(0)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
}

#[tokio::test]
async fn aleph_bft_pending_persistence_failure_releases_waiter_and_allows_retry() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .federation
        .fail_next_pending_operation_persistence
        .store(true, std::sync::atomic::Ordering::SeqCst);

    state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("injected pending persistence failure");

    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    let session = consensus
        .aleph_bft_status()
        .expect("consensus status")
        .session;
    assert_eq!(consensus.queued_len(), 0);
    drop(consensus);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after persistence failure")
        .is_empty());

    let retry_error = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unfinalized retry should time out after successful admission");
    assert_pending_finalization_error(&retry_error, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);
    let pending = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after retry");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, operation_id);
}

#[tokio::test]
async fn aleph_bft_duplicate_pending_submit_joins_existing_waiter_without_requeueing() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
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
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let second_state = state.clone();
    let second_submit = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(2)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 2);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.response_cache_lookups, 2);
    assert_eq!(stats.response_cache_hits, 0);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

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
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_accepted_submit_waits_until_state_application_finishes() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let accepted_response = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(envelope.clone())
            .expect("queue AlephBFT envelope");
        let unit = consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit");
        let mut responses = consensus
            .finalize_aleph_bft_unit(unit)
            .expect("accept AlephBFT unit");
        assert_eq!(responses.len(), 1);
        let response = responses.pop().expect("accepted response");
        assert_eq!(
            response.outcome.entry().status(),
            FederationOperationStatus::Accepted
        );
        response
    };

    let duplicate_state = state.clone();
    let duplicate_submit = tokio::spawn(async move {
        duplicate_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert!(
        !duplicate_submit.is_finished(),
        "an accepted journal entry must not return before its state transition is applied"
    );

    {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        consensus
            .lock()
            .await
            .update_status(operation_id, FederationOperationStatus::Applied)
            .expect("mark operation applied");
    }
    let applied_response = state
        .federation_submit_responses_with_current_status(&[accepted_response])
        .await
        .pop()
        .expect("applied response");
    state
        .notify_federation_operation(applied_response.clone())
        .await;

    let duplicate_response = duplicate_submit
        .await
        .expect("duplicate submit task")
        .expect("duplicate submit response");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert_eq!(
        duplicate_response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_response_cache_eviction_returns_journal_duplicate() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        response_cache_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope.clone())
            .await
    });
    let first_unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(first_unit)
        .await
        .expect("apply first finalized unit");
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
            .submit_federation_envelope_to_consensus(second_envelope)
            .await
    });
    let second_unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(second_unit)
        .await
        .expect("apply second finalized unit");
    let second_submit_response = second_submit
        .await
        .expect("second submit task")
        .expect("second submit response");
    assert_eq!(second_submit_response.operation_id, second_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let duplicate_response = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &first_quote_id))
        .await
        .expect("duplicate submit after cache eviction");
    assert_eq!(duplicate_response.operation_id, first_operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    let accepted_response = submit.await.expect("submit task").expect("submit response");
    assert_eq!(accepted_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    // Simulate a restart after accepted apply where in-memory waiters/cache are gone.
    state.federation.notifications =
        Some(Arc::new(crate::FederationOperationNotifications::default()));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);

    let duplicate_response = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("duplicate submit after lost waiters");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
async fn aleph_bft_duplicate_after_sqlite_restart_returns_journal_duplicate_after_apply_without_response(
) {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-aleph-bft-applied-no-response-restart");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let seed = Mnemonic::generate(12)
        .expect("mnemonic")
        .to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("unfinalized submit should time out before response delivery");
    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after no-waiter apply")
        .is_some());
    let applied_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after no-waiter apply");
    let applied_digest = materialized_state_digest(&state, &applied_entries)
        .await
        .expect("state digest after no-waiter apply");
    drop(state);
    drop(db);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("replay restarted AlephBFT runtime");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    assert_eq!(
        restarted_state.federation_pending_response_waiter_count(),
        0
    );
    assert_eq!(restarted_state.federation_response_cache_entry_count(), 0);
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restart replay")
        .is_some());

    let duplicate_response = restarted_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("duplicate submit after SQLite restart");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(
        restarted_state.federation_pending_response_waiter_count(),
        0
    );
    assert_eq!(restarted_state.federation_response_cache_entry_count(), 0);
    let consensus = restarted_state
        .federation
        .consensus
        .as_ref()
        .expect("restarted consensus");
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
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("pending operations after duplicate restart")
        .is_empty());
    let restarted_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after applied-without-response restart");
    assert_eq!(
        journal_convergence_projection(&restarted_entries),
        journal_convergence_projection(&applied_entries)
    );
    assert_eq!(
        materialized_state_digest(&restarted_state, &restarted_entries)
            .await
            .expect("state digest after applied-without-response restart"),
        applied_digest
    );
}

#[tokio::test]
async fn aleph_bft_duplicate_after_admission_race_returns_journal_duplicate() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .validate_federation_envelope_admission(&envelope)
        .await
        .expect("operation admitted before competing acceptance");

    let competing_state = state.clone();
    let competing_envelope = envelope.clone();
    let competing_submit = tokio::spawn(async move {
        competing_state
            .submit_federation_envelope_to_consensus(competing_envelope)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    let accepted = competing_submit
        .await
        .expect("competing submit task")
        .expect("competing submit response");
    assert_eq!(accepted.operation_id, operation_id);

    let duplicate = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("post-admission race submit should return duplicate");
    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        pending_mempool_item_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let queued_quote_id = QuoteId::new();
    let queued_envelope = mint_quote_envelope(&state, &queued_quote_id);
    let queued_operation_id = queued_envelope.operation_id().expect("queued operation id");
    {
        let consensus = state
            .federation
            .consensus
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
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &rejected_quote_id))
        .await
        .expect_err("full pending mempool should reject new operation");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::PendingConsensusMempoolFull { pending: 1, max: 1 })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&rejected_quote_id)
        .await
        .expect("quote lookup after mempool rejection")
        .is_none());
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
    drop(consensus);

    let duplicate_err = state
        .submit_federation_envelope_to_consensus(queued_envelope)
        .await
        .expect_err("duplicate pending operation should wait on existing queue entry");
    assert_pending_finalization_error(&duplicate_err, queued_operation_id, 10);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let retained_unit = consensus
        .next_aleph_bft_unit_payload()
        .expect("drain retained AlephBFT unit")
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

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(retained_unit)
        .await
        .expect("apply retained finalized unit");
    assert_eq!(responses.len(), 1);
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
    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope.clone()).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let duplicate = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("duplicate response");
    let second_duplicate = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("second duplicate response");

    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(second_duplicate.operation_id, operation_id);
    assert!(matches!(
        second_duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable DKG sequences after operation duplicates");
    assert!(
        sequences.is_empty(),
        "operation duplicates must not mutate durable DKG replay state"
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 2);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
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

    let crashed_history =
        finalized_aleph_bft_history(&public_config, local_private.member_id, [envelope]);
    persist_finalized_aleph_bft_history(&restarted_state, &crashed_history).await;

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
    assert_ne!(
        missing_status.materialized_state_digest,
        control_status.materialized_state_digest
    );

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("runtime state replay");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

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
        replayed_entries[0].status(),
        FederationOperationStatus::Applied
    );

    let replayed_status = local_peer_status(&restarted_state)
        .await
        .expect("replayed status");
    assert_eq!(replayed_status.operation_count, 1);
    assert_eq!(
        replayed_status.materialized_state_digest,
        control_status.materialized_state_digest
    );
    let replayed_consensus = restarted_state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus")
        .lock()
        .await;
    assert_eq!(replayed_consensus.peer_status().operation_count, 1);
    assert_eq!(
        replayed_consensus.journal().entries()[0].status(),
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

    let crashed_history =
        finalized_aleph_bft_history(&public_config, local_private.member_id, [envelope]);
    persist_finalized_aleph_bft_history(&restarted_state, &crashed_history).await;
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
        local_status.peer_status.materialized_state_digest,
        control_status.materialized_state_digest
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
        replayed_entries[0].status(),
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
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
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
async fn submit_payment_observation_rejects_invalid_admission_without_dkg_sequence_mutation() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let mut envelope = paid_observation_envelope(&state, &quote_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("expected mint quote payment observation");
    };
    observation.method = PaymentMethod::Custom("bad/method".to_string());
    let err = state
        .submit_federation_envelope_to_consensus(envelope)
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequence lookup");
    assert!(
        sequences.is_empty(),
        "invalid operation admission must not mutate durable DKG replay state"
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
    assert_eq!(status.operation_count, 1);
    assert_ne!(
        status.materialized_state_digest,
        Some(FederationMaterializedStateDigest::genesis())
    );
}

#[tokio::test]
async fn materialized_state_digest_tracks_rejection_without_changing_ordering_hash() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let mut spent_entry = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup")
        .into_iter()
        .next()
        .expect("journal entry");
    spent_entry.application = FederationApplicationState::Rejected {
        response: cdk_common::error::ErrorResponse::from(cdk::Error::TokenAlreadySpent),
    };
    let mut duplicate_entry = spent_entry.clone();
    duplicate_entry.application = FederationApplicationState::Rejected {
        response: cdk_common::error::ErrorResponse::from(cdk::Error::DuplicateInputs),
    };

    assert_eq!(spent_entry.operation_id, duplicate_entry.operation_id);
    assert_eq!(spent_entry.order_digest, duplicate_entry.order_digest);

    let spent_digest = materialized_state_digest(&state, &[spent_entry])
        .await
        .expect("spent rejection digest");
    let duplicate_digest = materialized_state_digest(&state, &[duplicate_entry])
        .await
        .expect("duplicate rejection digest");

    assert_ne!(spent_digest, duplicate_digest);
}

#[tokio::test]
async fn catch_up_verification_rejects_mismatched_application_rejection() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let mut peer_entry = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup")
        .into_iter()
        .next()
        .expect("journal entry");
    let local_rejection = cdk_common::error::ErrorResponse::from(cdk::Error::TokenAlreadySpent);
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin rejection transaction");
    tx.set_federation_operation_application_state(
        peer_entry.operation_id,
        &FederationApplicationState::Rejected {
            response: local_rejection,
        },
    )
    .await
    .expect("persist local rejection");
    tx.commit().await.expect("commit local rejection");

    peer_entry.application = FederationApplicationState::Rejected {
        response: cdk_common::error::ErrorResponse::from(cdk::Error::DuplicateInputs),
    };
    let error = state
        .verify_federation_catch_up_rejections(&[peer_entry])
        .await
        .expect_err("mismatched catch-up rejection must fail");

    assert!(matches!(
        error,
        cdk::Error::Federation(FederationError::CatchUpCertificateMismatch { .. })
    ));
}

#[tokio::test]
async fn materialized_state_digest_tracks_keyset_rotation_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("before digest");
    let keyset = public_config.keysets[0].clone();
    let envelope = state
        .mint
        .federation_config()
        .expect("federation config")
        .operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation { keyset },
        ));
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
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

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
            .status(),
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
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
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
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state = federation_route_state_for_private(public_config, local_private).await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let consensus = state.federation.consensus.as_ref().expect("consensus");
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted swap");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

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
async fn failed_mint_apply_preserves_materialized_digest_and_replays_on_restart() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let seed = [41_u8; 64];
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let mut consensus = test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let quote_id = QuoteId::new();

    let quote_entry = finalize_test_entry(&mut consensus, mint_quote_envelope(&state, &quote_id));
    persist_test_journal_entry(&state, &quote_entry).await;
    state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&quote_entry))
        .await
        .expect("apply mint quote");
    let payment_entry =
        finalize_test_entry(&mut consensus, paid_observation_envelope(&state, &quote_id));
    persist_test_journal_entry(&state, &payment_entry).await;
    state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&payment_entry))
        .await
        .expect("apply mint payment");

    let (mint_envelope, output) =
        mint_operation_envelope(&state, &quote_id, b"atomic mint apply output");
    let mint_entry = finalize_test_entry(&mut consensus, mint_envelope);
    add_blinded_message_conflict(&state, Some(&quote_id), &output).await;
    persist_test_journal_entry(&state, &mint_entry).await;
    let before_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal before failed mint apply");
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before failed mint apply");

    let err = state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
        .await
        .expect_err("conflicting blinded message must fail mint apply");
    assert!(matches!(
        err,
        cdk::Error::Database(cdk_common::database::Error::Duplicate)
    ));

    let after_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after failed mint apply");
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after failed mint apply");
    assert_eq!(after_digest, before_digest);
    assert_eq!(after_entries.len(), before_entries.len());
    let failed_entry = after_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("mint entry after failure");
    assert_eq!(failed_entry.status(), FederationOperationStatus::Accepted);
    let quote = db
        .get_mint_quote(&quote_id)
        .await
        .expect("quote after failed mint apply")
        .expect("stored mint quote");
    assert_eq!(quote.state(), MintQuoteState::Paid);
    assert_eq!(
        quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(db
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("mint shares after failed apply")
        .is_empty());
    assert!(db
        .get_completed_operation(&federation_completed_operation_uuid(
            mint_entry.operation_id,
        ))
        .await
        .expect("mint completed operation after failed apply")
        .is_none());
    assert_eq!(
        db.get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("mint signature after failed apply"),
        vec![None]
    );

    remove_blinded_message_conflict(&state, &output).await;
    drop(state);
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("restart replay after failed mint apply");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    let replayed_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after mint restart replay");
    assert_eq!(
        replayed_entries
            .iter()
            .find(|entry| entry.operation_id == mint_entry.operation_id)
            .expect("replayed mint entry")
            .status(),
        FederationOperationStatus::Applied
    );
    let replayed_digest = materialized_state_digest(&restarted_state, &replayed_entries)
        .await
        .expect("digest after mint restart replay");
    assert_ne!(replayed_digest, before_digest);
    assert_eq!(
        db.get_mint_quote(&quote_id)
            .await
            .expect("replayed mint quote")
            .expect("mint quote after replay")
            .state(),
        MintQuoteState::Issued
    );
    assert!(db
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("mint signature after replay")[0]
        .is_some());
}

#[tokio::test]
async fn failed_swap_apply_preserves_materialized_digest_and_replays_on_restart() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let seed = [42_u8; 64];
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let mut consensus = test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let input = proof_for_amount(&state, 1, "atomic-swap-apply-input");
    let input_y = input.y().expect("swap input y");
    let output = swap_output(&state, "atomic-swap-apply-output");
    let config = state.mint.federation_config().expect("federation config");
    let envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input],
            vec![output.clone()],
        ))
        .expect("swap envelope");
    let swap_entry = finalize_test_entry(&mut consensus, envelope);
    add_blinded_message_conflict(&state, None, &output).await;
    persist_test_journal_entry(&state, &swap_entry).await;
    let before_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal before failed swap apply");
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before failed swap apply");

    let err = state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
        .await
        .expect_err("conflicting blinded message must fail swap apply");
    assert!(matches!(
        err,
        cdk::Error::Database(cdk_common::database::Error::Duplicate)
    ));

    let after_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after failed swap apply");
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after failed swap apply");
    assert_eq!(after_digest, before_digest);
    assert_eq!(after_entries.len(), before_entries.len());
    assert_eq!(
        after_entries
            .iter()
            .find(|entry| entry.operation_id == swap_entry.operation_id)
            .expect("swap entry after failure")
            .status(),
        FederationOperationStatus::Accepted
    );
    assert_eq!(
        db.get_proofs_states(&[input_y])
            .await
            .expect("proof state after failed swap apply"),
        vec![None]
    );
    assert!(db
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("swap shares after failed apply")
        .is_empty());
    assert!(db
        .get_completed_operation(&federation_completed_operation_uuid(
            swap_entry.operation_id,
        ))
        .await
        .expect("swap completed operation after failed apply")
        .is_none());
    assert_eq!(
        db.get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("swap signature after failed apply"),
        vec![None]
    );

    remove_blinded_message_conflict(&state, &output).await;
    drop(state);
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("restart replay after failed swap apply");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    let replayed_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after swap restart replay");
    assert_eq!(
        replayed_entries
            .iter()
            .find(|entry| entry.operation_id == swap_entry.operation_id)
            .expect("replayed swap entry")
            .status(),
        FederationOperationStatus::Applied
    );
    let replayed_digest = materialized_state_digest(&restarted_state, &replayed_entries)
        .await
        .expect("digest after swap restart replay");
    assert_ne!(replayed_digest, before_digest);
    assert_eq!(
        db.get_proofs_states(&[input_y])
            .await
            .expect("proof state after swap replay"),
        vec![Some(cdk::nuts::State::Spent)]
    );
    assert!(db
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("swap signature after replay")[0]
        .is_some());
}

#[tokio::test]
async fn submit_federation_envelope_notifies_operation_acceptance() {
    let (state, _, _) = federation_route_state().await;
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
        .await
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
    assert_eq!(entries[0].status(), FederationOperationStatus::Applied);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("mint quote lookup")
        .is_some());

    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn submit_federation_envelope_bounds_response_cache() {
    let (mut state, _, _) = federation_route_state().await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
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
    assert_eq!(stats.response_cache_lookups, 2);
    assert_eq!(stats.response_cache_hits, 0);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);
}

#[tokio::test]
async fn catch_up_replay_materializes_completed_operation_rows() {
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
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    finalize_test_operation(
        &mut peer_consensus,
        paid_observation_envelope(&state, &quote_id),
    )
    .expect("peer accepted paid observation");
    finalize_test_operation(&mut peer_consensus, mint_envelope).expect("peer accepted mint");
    let catch_up_request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_snapshot = materialized_state_snapshot(&state, peer_consensus.journal().entries())
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
    snapshot.digest().expect("materialized digest");
    let completed_row = completed_operation_snapshot_row(&snapshot, mint_operation_id);
    assert_eq!(
        completed_row.fields.get("kind").map(String::as_str),
        Some("mint")
    );
    assert!(!completed_row.fields.contains_key("missing"));
}

#[tokio::test]
async fn catch_up_replay_materializes_unresolved_payment_observation_rows() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    finalize_test_operation(&mut peer_consensus, observation)
        .expect("peer accepted unresolved payment observation");
    let catch_up_request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 2);

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
    assert!(!report.groups[0].quorum_reached);
    assert!(report.stale);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );
    let status_json = serde_json::to_string(&status).expect("status json");
    assert!(!status_json.contains("settled"));
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
            .status(),
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
            .submit_federation_envelope_to_consensus(quote_envelope.clone())
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope_to_consensus(paid_envelope.clone())
            .await
            .expect("submit paid observation");
        state
            .submit_federation_envelope_to_consensus(mint_envelope.clone())
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
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
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
    let payment_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some(String::new()),
    );
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt");
    finalize_test_operation(&mut peer_consensus, payment_envelope)
        .expect("peer accepted melt payment observation");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

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
            melt_quote_id.clone(),
            melt_inputs,
            Some(vec![melt_change]),
        ))
        .expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    state
        .submit_federation_envelope(melt_envelope)
        .await
        .expect("submit melt operation");
    state
        .submit_federation_envelope(melt_payment_observation_envelope(
            &state,
            &melt_quote_id,
            MeltQuoteState::Paid,
            Some(String::new()),
        ))
        .await
        .expect("submit melt payment observation");

    let entries = state
        .federation
        .consensus
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
    assert_ne!(digest, FederationMaterializedStateDigest::genesis());

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
#[ignore = "manual materialized-state projection benchmark"]
async fn benchmark_materialized_state_snapshot_growth() {
    for operation_count in [100_usize, 500, 1_000] {
        let (state, _, _) = federation_route_state().await;
        let mut journal = InMemoryFederationJournal::new();
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin benchmark transaction");

        for _ in 0..operation_count {
            let quote_id = QuoteId::new();
            let quote = MintQuote::new(
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
                1,
                Vec::new(),
                Vec::new(),
                None,
            );
            tx.add_mint_quote(quote).await.expect("add benchmark quote");
            journal
                .append(mint_quote_envelope(&state, &quote_id))
                .expect("append benchmark operation");
        }
        tx.commit().await.expect("commit benchmark quotes");

        let started = std::time::Instant::now();
        let snapshot = materialized_state_snapshot(&state, journal.entries())
            .await
            .expect("benchmark snapshot");
        let digest = snapshot.digest().expect("benchmark digest");
        let elapsed = started.elapsed();
        assert_eq!(snapshot.rows().len(), operation_count);
        assert_ne!(digest, FederationMaterializedStateDigest::genesis());
        eprintln!(
            "materialized_state_snapshot operations={operation_count} elapsed_us={} rows={}",
            elapsed.as_micros(),
            snapshot.rows().len()
        );
    }
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
    assert_eq!(response.catch_up_certificate_quorum_operation_count, None);
    assert_eq!(response.catch_up_certificate_quorum_from_index, None);
    assert_eq!(response.catch_up_certificate_quorum_latest_index, None);
    assert!(response.catch_up_certificate_quorum_signers.is_empty());
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
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
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
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first payment observation");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            2,
            2,
            "different-settlement",
        ))
        .await
        .expect("submit conflicting second payment observation");
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
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
    assert!(!status_json.contains("different-settlement"));
}

#[tokio::test]
async fn private_local_status_route_reports_stale_payment_observations() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
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
async fn private_local_status_route_reports_catch_up_certificate_quorum_evidence() {
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
    let materialized_state_digest = peer_status
        .materialized_state_digest
        .expect("local status has materialized-state digest");
    let operations = entries
        .iter()
        .map(FederationCatchUpOperation::from)
        .collect::<Vec<_>>();
    let local_certificate = FederationCatchUpCertificate::new_signed(
        public_config.federation_id,
        &operations,
        peer_status.latest_order_digest,
        materialized_state_digest,
        &local_private,
    )
    .expect("local catch-up certificate");
    let peer_certificate = FederationCatchUpCertificate::new_signed(
        public_config.federation_id,
        &operations,
        peer_status.latest_order_digest,
        materialized_state_digest,
        &peer_private,
    )
    .expect("peer catch-up certificate");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin transaction");
    tx.add_federation_catch_up_certificate(&local_certificate)
        .await
        .expect("persist local outcome");
    tx.add_federation_catch_up_certificate(&peer_certificate)
        .await
        .expect("persist peer outcome");
    tx.commit().await.expect("commit outcomes");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 1);
    assert_eq!(
        response.catch_up_certificate_quorum_operation_count,
        Some(1)
    );
    assert_eq!(response.catch_up_certificate_quorum_from_index, Some(0));
    assert_eq!(response.catch_up_certificate_quorum_latest_index, Some(0));
    assert_eq!(response.catch_up_certificate_quorum_signers.len(), 2);
    assert!(response
        .catch_up_certificate_quorum_signers
        .contains(&local_private.member_id));
    assert!(response
        .catch_up_certificate_quorum_signers
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
    let envelope = test_operation_envelope_for_config(
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
    assert_eq!(status.materialized_state_digest, None);

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
    assert!(local_status.peer_status.materialized_state_digest.is_some());
    assert!(!local_status.materialized_state_rows.is_empty());
}

#[tokio::test]
async fn private_status_route_records_sequence_without_persisting_liveness_watermark() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = signed_status_request(&public_config, &peer_private, local_member, 1);

    let status = post_status(State(state.clone()), Json(request.clone()))
        .await
        .expect("status response")
        .0;
    assert_eq!(status.member_id, local_member);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences");
    assert!(
        sequences.is_empty(),
        "status liveness requests should not persist sequence watermarks"
    );

    let response = post_status(State(state), Json(request))
        .await
        .expect_err("same status sequence should be rejected in memory");
    let detail = response_error_detail(response).await;
    assert!(
        detail.contains("stale sequence"),
        "unexpected detail: {detail}"
    );
}

#[tokio::test]
async fn private_status_route_rejects_inbound_overflow_before_sequence_persistence() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state.federation.private_rest_admission = Some(Arc::new(tokio::sync::Semaphore::new(1)));
    let held_permit = state
        .federation
        .private_rest_admission
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
        .get_federation_dkg_member_sequences()
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
async fn private_local_status_route_reports_catching_up_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state.mark_federation_catching_up(peer_member).await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.sync_state, "catching_up");
    assert!(!response.sync_ready);
    assert_eq!(response.sync_peer, Some(peer_member));
    assert!(response.sync_message.contains("catching up from peer"));
}

#[tokio::test]
async fn private_local_status_route_reports_redacted_halted_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state
        .mark_federation_halted("digest mismatch token=operator-secret")
        .await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.sync_state, "halted");
    assert!(!response.sync_ready);
    assert_eq!(response.sync_peer, None);
    assert!(response.sync_message.contains("digest mismatch"));
    assert!(!response.sync_message.contains("operator-secret"));
}

#[tokio::test]
async fn startup_discards_transient_federation_sync_status() {
    let (state, _, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    state.mark_federation_lagging(peer_member, 2, 5).await;

    let mut restarted = state.clone();
    restarted.federation.sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::for_aleph_bft(),
    )));
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );
}

#[tokio::test]
async fn startup_requires_catch_up_certificate_quorum_for_replayed_journal() {
    let (state, public_config, _, peer_private, _) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(test_operation_envelope_for_config(
            &public_config,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("persist accepted operation");

    let mut restarted = state.clone();
    restarted.federation.sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::VerifyingCatchUpCertificate {
            peer: peer_member,
            operation_count: 1,
        })
    );

    let blocked = restarted
        .submit_federation_envelope(test_operation_envelope_for_config(
            &public_config,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .await;
    assert!(matches!(
        blocked,
        Err(cdk::Error::Federation(
            FederationError::NodeVerifyingCatchUpCertificate {
                peer,
                operation_count: 1,
            }
        )) if peer == peer_member
    ));
}

#[tokio::test]
async fn startup_verifying_catch_up_certificate_blocks_restore_and_checkstate_reads() {
    let (state, public_config, _, peer_private, _) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(test_operation_envelope_for_config(
            &public_config,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("persist accepted operation");

    let mut restarted = state.clone();
    restarted.federation.sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    restarted.restore_federation_startup_readiness().await;
    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::VerifyingCatchUpCertificate {
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
        check_body.contains("verifying quorum catch-up certificates"),
        "check-state should fail closed while startup verifies catch-up certificate: {check_body}"
    );

    let restore_request = restore_request_for_config(
        &public_config,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let restore_error = crate::router_handlers::post_restore(
        AuthHeader::None,
        State(restarted),
        Json(restore_request),
    )
    .await
    .expect_err("startup verification must not serve authoritative restore");
    let restore_body = response_error_detail(restore_error).await;
    assert!(
        restore_body.contains("verifying quorum catch-up certificates"),
        "restore should fail closed while startup verifies catch-up certificate: {restore_body}"
    );
}

#[tokio::test]
async fn startup_returns_healthy_when_replayed_journal_has_catch_up_certificate_quorum() {
    let (state, public_config, _, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    let history_envelope = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let restore_request =
        restore_request_for_config(&public_config, "startup healthy restore output");
    state
        .submit_federation_envelope(history_envelope)
        .await
        .expect("persist accepted operation");
    let local_digest = local_peer_status(&state)
        .await
        .expect("local status")
        .materialized_state_digest
        .expect("local status has materialized-state digest");
    persist_startup_catch_up_certificate_quorum(
        &state,
        &public_config,
        &peer_private,
        &witness_private,
        local_digest,
    )
    .await;

    let mut restarted = state.clone();
    restarted.federation.sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::default(),
    )));
    restarted
        .mark_federation_verifying_catch_up_certificate(peer_member, 1)
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

async fn persist_startup_catch_up_certificate_quorum(
    state: &MintState,
    public_config: &cdk::federation::FederationConfig,
    peer_private: &MemberConfigPrivate,
    witness_private: &MemberConfigPrivate,
    materialized_state_digest: FederationMaterializedStateDigest,
) {
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
        FederationMaterializedStateDigest::genesis(),
        &FederationJournalCatchUpRequest::new(0),
        100,
        100,
    );
    response.materialized_state_digest = materialized_state_digest;
    let peer_certificate = signed_catch_up_response(response.clone(), peer_private)
        .catch_up_certificate
        .expect("peer catch-up certificate");
    let witness_certificate = signed_catch_up_response(response, witness_private)
        .catch_up_certificate
        .expect("witness catch-up certificate");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin evidence transaction");
    tx.add_federation_catch_up_certificate(&peer_certificate)
        .await
        .expect("persist peer outcome");
    tx.add_federation_catch_up_certificate(&witness_certificate)
        .await
        .expect("persist witness outcome");
    tx.commit().await.expect("commit evidence");
}

#[tokio::test]
async fn startup_rejects_catch_up_quorum_for_another_materialized_state() {
    let (state, public_config, _, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = peer_private.member_id;
    state
        .submit_federation_envelope(test_operation_envelope_for_config(
            &public_config,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .await
        .expect("persist accepted operation");
    let local_digest = local_peer_status(&state)
        .await
        .expect("local status")
        .materialized_state_digest
        .expect("local status has materialized-state digest");
    let certified_digest = FederationMaterializedStateDigest::from_bytes([42; 32]);
    assert_ne!(certified_digest, local_digest);
    persist_startup_catch_up_certificate_quorum(
        &state,
        &public_config,
        &peer_private,
        &witness_private,
        certified_digest,
    )
    .await;

    let mut restarted = state.clone();
    restarted.federation.sync = Some(Arc::new(tokio::sync::Mutex::new(
        crate::FederationSyncState::for_aleph_bft(),
    )));
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::MaterializedStateMismatch {
            peer: peer_member,
            operation_count: 1,
        })
    );

    let y = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"startup mismatched checkstate read",
    ));
    crate::router_handlers::post_check(
        AuthHeader::None,
        State(restarted.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect_err("mismatched startup member must not serve check-state");

    restarted
        .submit_federation_envelope(test_operation_envelope_for_config(
            &public_config,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .await
        .expect_err("mismatched startup member must reject new submissions");
    let restore_request =
        restore_request_for_config(&public_config, "startup mismatched restore output");
    crate::router_handlers::post_restore(AuthHeader::None, State(restarted), Json(restore_request))
        .await
        .expect_err("mismatched startup member must not serve restore");
}

#[tokio::test]
async fn private_local_status_route_reports_verifying_catch_up_certificate_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state
        .mark_federation_verifying_catch_up_certificate(peer_member, 3)
        .await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 0);
    assert_eq!(response.sync_state, "verifying_catch_up_certificate");
    assert!(!response.sync_ready);
    assert!(response
        .sync_message
        .contains("verifying quorum catch-up certificates"));
}

#[tokio::test]
async fn private_journal_route_returns_signed_catch_up_certificate() {
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

    let response = post_journal(State(state.clone()), Json(request.clone()))
        .await
        .expect("journal response")
        .0;

    assert_eq!(response.entries.len(), 1);
    assert!(response.catch_up_certificate.is_some());
    response
        .verify_catch_up_certificate(public_config.member(local_member).expect("local member"))
        .expect("catch-up certificate verifies");
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences");
    assert!(
        sequences.is_empty(),
        "read-only catch-up requests should not persist sequence watermarks"
    );

    let replay = post_journal(State(state), Json(request))
        .await
        .expect_err("same catch-up sequence should be rejected in memory");
    let detail = response_error_detail(replay).await;
    assert!(
        detail.contains("stale sequence"),
        "unexpected detail: {detail}"
    );
}

#[tokio::test]
async fn private_journal_route_returns_whole_aleph_bft_finalized_item_units() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "wrong-kind catch-up requests must not persist the sender sequence"
    );
}
