//! Mock, delaying and router-backed federation transports, and small shared helpers.

use super::*;

#[derive(Debug, Default)]
pub(super) struct MockFederationTransportState {
    pub(super) status_response: Option<FederationPeerStatus>,
    pub(super) checkpoint_summary_response:
        Option<(FederationCheckpointSummaryResponse, MemberConfigPrivate)>,
    pub(super) submit_response: Option<FederationSubmitOperationResponse>,
    pub(super) aleph_bft_network_message_response: Option<FederationAlephBftNetworkMessageResponse>,
    pub(super) aleph_bft_network_message_delays: VecDeque<Duration>,
    pub(super) aleph_bft_network_message_status_error: Option<(u16, String)>,
    pub(super) dkg_setup_responses: VecDeque<FederationDkgSetupResponse>,
    pub(super) aleph_bft_backup_responses:
        VecDeque<(FederationAlephBftBackupResponse, MemberConfigPrivate)>,
    pub(super) catch_up_responses: VecDeque<FederationJournalCatchUpResponse>,
    pub(super) status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
    pub(super) checkpoint_summary_requests:
        Vec<FederationSignedRequest<FederationCheckpointSummaryRequest>>,
    pub(super) dkg_commitment_requests:
        Vec<FederationSignedRequest<FederationDkgCommitmentRequest>>,
    pub(super) dkg_reveal_requests: Vec<FederationSignedRequest<FederationDkgRevealRequest>>,
    pub(super) dkg_secret_share_requests:
        Vec<FederationSignedRequest<FederationDkgSecretShareRequest>>,
    dkg_transcript_signature_requests:
        Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
    pub(super) submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
    aleph_bft_network_message_requests:
        Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>,
    pub(super) aleph_bft_backup_requests:
        Vec<FederationSignedRequest<FederationAlephBftBackupRequest>>,
    pub(super) catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct MockFederationTransport {
    pub(super) state: Arc<Mutex<MockFederationTransportState>>,
    pub(super) aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Clone)]
pub(super) struct DelayTransport {
    pub(super) delay: Duration,
    pub(super) request_started: Option<Arc<tokio::sync::Notify>>,
}

impl DelayTransport {
    pub(super) fn signal_request_started(&self) {
        if let Some(request_started) = &self.request_started {
            request_started.notify_one();
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct RouterFederationTransport {
    pub(super) router: Option<Router>,
}

#[derive(Clone, Default)]
pub(super) struct RouterFederationTransportRegistry {
    pub(super) routers: Arc<Mutex<BTreeMap<String, Router>>>,
    pub(super) shadow_routers: Arc<Mutex<BTreeMap<String, Router>>>,
    pub(super) shadow_delivery_lock: Arc<tokio::sync::Mutex<()>>,
    pub(super) missing_router_wait: Duration,
    pub(super) faults: Arc<Mutex<RouterFederationTransportFaults>>,
    pub(super) held_aleph_bft_network_message: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    aleph_bft_activity_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftActivityRequest>>>>,
    pub(super) dkg_readiness_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationDkgReadinessRequest>>>>,
    aleph_bft_network_message_requests:
        Arc<Mutex<Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>>>,
    pub(super) aleph_bft_network_message_changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Default)]
pub(super) struct RouterFederationTransportFaults {
    pub(super) post_response_failures_after_delivery: BTreeMap<String, usize>,
    pub(super) post_delays_before_delivery: BTreeMap<(String, MemberId), VecDeque<Duration>>,
    pub(super) aleph_bft_network_message_delays: VecDeque<Duration>,
    pub(super) aleph_bft_network_latency_model: Option<AlephBftNetworkLatencyModel>,
    pub(super) aleph_bft_network_message_duplicate_replays: usize,
    pub(super) aleph_bft_network_message_reorder_once: bool,
    pub(super) aleph_bft_network_message_reorders_completed: usize,
    pub(super) held_aleph_bft_data_senders: BTreeSet<MemberId>,
    pub(super) blocked_federation_links: BTreeSet<(MemberId, MemberId)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AlephBftNetworkLatencyModel {
    pub(super) nominal_round_trip: Duration,
    pub(super) jitter_percent: u8,
    pub(super) state: u64,
}

impl AlephBftNetworkLatencyModel {
    pub(super) fn next_delays(&mut self) -> (Duration, Duration) {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let nominal_micros =
            u64::try_from(self.nominal_round_trip.as_micros()).expect("test RTT fits u64");
        let jitter_micros = nominal_micros.saturating_mul(u64::from(self.jitter_percent)) / 100;
        let sample_width = jitter_micros.saturating_mul(2).saturating_add(1);
        let sampled_round_trip_micros = nominal_micros
            .saturating_sub(jitter_micros)
            .saturating_add(self.state % sample_width);
        let outbound_micros = sampled_round_trip_micros / 2;
        (
            Duration::from_micros(outbound_micros),
            Duration::from_micros(sampled_round_trip_micros - outbound_micros),
        )
    }
}

pub(super) enum AlephBftNetworkMessageReorderDecision {
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
                            faults.aleph_bft_network_latency_model,
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
    pub(super) fn new(router: Router) -> Self {
        Self {
            router: Some(router),
        }
    }
}

impl RouterFederationTransportRegistry {
    pub(super) fn fail_post_responses_after_delivery(&self, path_suffix: &str, count: usize) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .post_response_failures_after_delivery
            .insert(path_suffix.to_string(), count);
    }

    pub(super) fn take_post_response_failure_after_delivery(&self, path: &str) -> bool {
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

    pub(super) fn delay_next_posts_before_delivery<I>(
        &self,
        path_suffix: &str,
        receiver: MemberId,
        delays: I,
    ) where
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

    pub(super) fn take_post_delay_before_delivery(
        &self,
        path: &str,
        receiver: MemberId,
    ) -> Duration {
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

    pub(super) fn with_missing_router_wait(self, timeout: Duration) -> Self {
        Self {
            missing_router_wait: timeout,
            ..self
        }
    }

    pub(super) fn with_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
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

    pub(super) fn set_aleph_bft_network_latency_model(
        &self,
        nominal_round_trip: Duration,
        jitter_percent: u8,
        seed: u64,
    ) {
        assert!(
            jitter_percent <= 100,
            "network jitter percent must not exceed 100"
        );
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_latency_model = if nominal_round_trip.is_zero() {
            None
        } else {
            Some(AlephBftNetworkLatencyModel {
                nominal_round_trip,
                jitter_percent,
                state: seed,
            })
        };
    }

    pub(super) fn with_aleph_bft_network_message_duplicate_replays(self, count: usize) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_duplicate_replays = count;
        self
    }

    pub(super) fn with_aleph_bft_network_message_reorder_once(self) -> Self {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorder_once = true;
        self
    }

    pub(super) fn aleph_bft_network_message_reorders_completed(&self) -> usize {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed
    }

    pub(super) fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.aleph_bft_network_message_requests
            .lock()
            .expect("router transport network-message requests lock")
            .clone()
    }

    pub(super) async fn wait_for_aleph_bft_network_message_request_matching<F>(
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

    pub(super) fn dkg_readiness_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgReadinessRequest>> {
        self.dkg_readiness_requests
            .lock()
            .expect("router transport DKG readiness requests lock")
            .clone()
    }

    pub(super) fn insert(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .insert(endpoint, router);
    }

    pub(super) fn remove(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.routers
            .lock()
            .expect("router registry lock")
            .remove(&endpoint);
    }

    pub(super) fn insert_shadow(&self, url: MintUrl, router: Router) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .insert(endpoint, router);
    }

    pub(super) fn remove_shadow(&self, url: &MintUrl) {
        let endpoint = url::Url::parse(&url.to_string())
            .expect("federation api url")
            .origin()
            .ascii_serialization();
        self.shadow_routers
            .lock()
            .expect("shadow router registry lock")
            .remove(&endpoint);
    }

    pub(super) fn set_member_outbound_partition(
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

    pub(super) fn set_network_partition(
        &self,
        first: &[MemberId],
        second: &[MemberId],
        partitioned: bool,
    ) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        for &sender in first {
            for &receiver in second {
                for link in [(sender, receiver), (receiver, sender)] {
                    if partitioned {
                        faults.blocked_federation_links.insert(link);
                    } else {
                        faults.blocked_federation_links.remove(&link);
                    }
                }
            }
        }
    }

    pub(super) fn set_member_aleph_bft_data_hold(&self, sender: MemberId, held: bool) {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if held {
            faults.held_aleph_bft_data_senders.insert(sender);
        } else {
            faults.held_aleph_bft_data_senders.remove(&sender);
        }
    }

    pub(super) fn aleph_bft_data_is_held(&self, sender: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .held_aleph_bft_data_senders
            .contains(&sender)
    }

    pub(super) fn federation_link_is_blocked(&self, sender: MemberId, receiver: MemberId) -> bool {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .blocked_federation_links
            .contains(&(sender, receiver))
    }

    pub(super) fn pop_aleph_bft_network_message_delay(&self) -> Duration {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_delays
            .pop_front()
            .unwrap_or_default()
    }

    pub(super) fn next_aleph_bft_network_round_trip_delays(&self) -> (Duration, Duration) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_latency_model
            .as_mut()
            .map(AlephBftNetworkLatencyModel::next_delays)
            .unwrap_or_default()
    }

    pub(super) fn take_aleph_bft_network_message_duplicate_replay(&self) -> bool {
        let mut faults = self.faults.lock().expect("router transport faults lock");
        if faults.aleph_bft_network_message_duplicate_replays == 0 {
            return false;
        }
        faults.aleph_bft_network_message_duplicate_replays -= 1;
        true
    }

    pub(super) fn aleph_bft_network_message_reorder_decision(
        &self,
    ) -> AlephBftNetworkMessageReorderDecision {
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

    pub(super) fn mark_aleph_bft_network_message_reorder_completed(&self) {
        self.faults
            .lock()
            .expect("router transport faults lock")
            .aleph_bft_network_message_reorders_completed += 1;
    }
}

impl Default for DelayTransport {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(60),
            request_started: None,
        }
    }
}

pub(super) struct TempMintDbFile {
    pub(super) path: String,
}

impl TempMintDbFile {
    pub(super) fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();

        Self { path }
    }

    pub(super) fn path(&self) -> &str {
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
    pub(super) fn with_status_response(self, response: FederationPeerStatus) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_response = Some(response);
        self
    }

    #[cfg(feature = "prometheus")]
    pub(super) fn with_checkpoint_summary_response(
        self,
        response: FederationCheckpointSummaryResponse,
        signer: MemberConfigPrivate,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .checkpoint_summary_response = Some((response, signer));
        self
    }

    pub(super) fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .submit_response = Some(response);
        self
    }

    pub(super) fn with_aleph_bft_network_message_response(
        self,
        response: FederationAlephBftNetworkMessageResponse,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_response = Some(response);
        self
    }

    pub(super) fn with_mock_aleph_bft_network_message_delays<I>(self, delays: I) -> Self
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

    pub(super) fn with_aleph_bft_network_message_status_error(
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

    pub(super) fn with_dkg_setup_response(self, response: FederationDkgSetupResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_setup_responses
            .push_back(response);
        self
    }

    pub(super) fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_responses
            .push_back(response);
        self
    }

    pub(super) fn with_aleph_bft_backup_response(
        self,
        response: FederationAlephBftBackupResponse,
        signer: MemberConfigPrivate,
    ) -> Self {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_backup_responses
            .push_back((response, signer));
        self
    }

    pub(super) fn status_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .status_requests
            .clone()
    }

    #[cfg(feature = "prometheus")]
    pub(super) fn checkpoint_summary_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationCheckpointSummaryRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .checkpoint_summary_requests
            .clone()
    }

    pub(super) fn dkg_commitment_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgCommitmentRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_commitment_requests
            .clone()
    }

    pub(super) fn dkg_reveal_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgRevealRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_reveal_requests
            .clone()
    }

    pub(super) fn dkg_secret_share_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgSecretShareRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_secret_share_requests
            .clone()
    }

    pub(super) fn dkg_transcript_signature_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .dkg_transcript_signature_requests
            .clone()
    }

    pub(super) fn submitted_operations(
        &self,
    ) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .submitted_operations
            .clone()
    }

    pub(super) fn aleph_bft_network_message_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_network_message_requests
            .clone()
    }

    pub(super) async fn wait_for_aleph_bft_network_message_request_matching<F>(
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

    pub(super) fn catch_up_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .catch_up_requests
            .clone()
    }

    pub(super) fn aleph_bft_backup_requests(
        &self,
    ) -> Vec<FederationSignedRequest<FederationAlephBftBackupRequest>> {
        self.state
            .lock()
            .expect("mock transport lock")
            .aleph_bft_backup_requests
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

    async fn http_get(
        &self,
        _url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.signal_request_started();
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed raw GET completed".to_string()))
    }

    async fn http_post<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.signal_request_started();
        time::sleep(self.delay).await;
        Err(HttpError::Other("delayed POST completed".to_string()))
    }

    async fn http_post_form<P>(
        &self,
        _url: url::Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.signal_request_started();
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

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
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
            path if path.ends_with("/federation/v1/checkpoints/summary") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationCheckpointSummaryRequest>,
                >(serde_json::to_value(payload)?)?;
                state.checkpoint_summary_requests.push(request.clone());
                let (response, signer) =
                    state.checkpoint_summary_response.clone().ok_or_else(|| {
                        HttpError::Other("missing checkpoint summary response".to_string())
                    })?;
                let response = FederationSignedRequest::new_signed(
                    request.federation_id,
                    request.sender,
                    request.sequence,
                    FederationRestMessageKind::CheckpointSummary,
                    response,
                    &signer,
                )
                .map_err(|err| HttpError::Other(err.to_string()))?;
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
            path if path.ends_with("/federation/v1/aleph-bft/backup") => {
                let request = serde_json::from_value::<
                    FederationSignedRequest<FederationAlephBftBackupRequest>,
                >(serde_json::to_value(payload)?)?;
                state.aleph_bft_backup_requests.push(request.clone());
                let (response, signer) =
                    state
                        .aleph_bft_backup_responses
                        .pop_front()
                        .ok_or_else(|| {
                            HttpError::Other("missing AlephBFT backup response".to_string())
                        })?;
                let response = FederationSignedRequest::new_signed(
                    request.federation_id,
                    request.sender,
                    request.sequence,
                    FederationRestMessageKind::AlephBftBackup,
                    response,
                    &signer,
                )
                .map_err(|err| HttpError::Other(err.to_string()))?;
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

    async fn http_post_form<P>(
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

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
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
        Ok(RawResponse::new(status.as_u16(), body.to_vec()))
    }

    async fn http_post_form<P>(
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

    async fn http_get(
        &self,
        url: url::Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        Err(HttpError::Other(format!(
            "unexpected raw GET path {}",
            url.path()
        )))
    }

    async fn http_post<P>(
        &self,
        url: url::Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
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
        let (_, response_network_delay) = if is_aleph_bft_network_message {
            let delay = self.pop_aleph_bft_network_message_delay();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let (request_delay, response_delay) = self.next_aleph_bft_network_round_trip_delays();
            if !request_delay.is_zero() {
                tokio::time::sleep(request_delay).await;
            }
            (request_delay, response_delay)
        } else {
            (Duration::ZERO, Duration::ZERO)
        };
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
        if !response_network_delay.is_zero() {
            tokio::time::sleep(response_network_delay).await;
        }
        if is_aleph_bft_network_message && self.take_aleph_bft_network_message_duplicate_replay() {
            let _ = router_transport_post_once_bytes(&router, &path, payload_bytes).await;
        }
        if self.take_post_response_failure_after_delivery(&path) {
            return Err(HttpError::Other(format!(
                "simulated response loss after durable POST delivery for {path}"
            )));
        }

        Ok(RawResponse::new(200, response_body))
    }

    async fn http_post_form<P>(
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

pub(super) async fn router_transport_post_once_bytes(
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

pub(super) fn json_response<T>(value: T) -> Result<RawResponse, HttpError>
where
    T: Serialize,
{
    Ok(RawResponse::new(200, serde_json::to_vec(&value)?))
}

pub(super) fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("member id")
}

pub(super) fn zero_aleph_bft_unit_cursor(config: &FederationConfig) -> BTreeMap<MemberId, u64> {
    config
        .members
        .keys()
        .copied()
        .map(|member| (member, 0))
        .collect()
}

pub(super) async fn response_error_detail(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8_lossy(&bytes).to_string()
}
