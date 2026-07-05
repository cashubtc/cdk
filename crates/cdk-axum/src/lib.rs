//! Axum server for Mint

#![doc = include_str!("../README.md")]

#[cfg(feature = "federation")]
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::Result;
use auth::create_auth_router;
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cache::HttpCache;
#[cfg(feature = "federation")]
use cdk::federation::{
    redact_federation_operator_message, FederationConfig, FederationConsensusEngine,
    FederationConsensusRuntime, FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgSetupState, FederationDkgTranscriptSignatureRequest, FederationError,
    FederationJournalAppendOutcome, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationOperationStatus, FederationRestSequenceTracker, FederationSessionOutcome,
    FederationSubmitOperationResponse, MemberId,
};
use cdk::mint::Mint;
use router_handlers::*;
#[cfg(feature = "federation")]
use sha2::{Digest, Sha256};

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
#[cfg(feature = "federation")]
mod federation;
#[cfg(feature = "federation")]
pub use self::federation::FederationPeerSyncConfig;
mod router_handlers;
mod ws;

#[cfg(feature = "federation")]
type FederationConsensusHandle = Arc<tokio::sync::Mutex<FederationConsensusRuntime>>;
#[cfg(feature = "federation")]
type FederationSequenceHandle = Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;
#[cfg(feature = "federation")]
type FederationSyncHandle = Arc<tokio::sync::Mutex<FederationSyncState>>;
#[cfg(feature = "federation")]
type FederationNotificationHandle = Arc<FederationOperationNotifications>;
#[cfg(feature = "federation")]
type FederationFinalizationHandle = tokio::sync::mpsc::Sender<FederationFinalizationRequest>;
#[cfg(feature = "federation")]
type FederationRestAdmissionHandle = Arc<tokio::sync::Semaphore>;
#[cfg(feature = "federation")]
type FederationInvalidOperationAlertHandle =
    Arc<tokio::sync::Mutex<FederationInvalidOperationAlertTracker>>;

#[cfg(feature = "federation")]
const FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD: u64 = 20;
#[cfg(feature = "federation")]
const FEDERATION_INVALID_OPERATION_ALERT_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(60);

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum FederationInvalidOperationSourceKind {
    AuthToken,
    ClientIp,
    Anonymous,
}

#[cfg(feature = "federation")]
impl FederationInvalidOperationSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::AuthToken => "auth_token",
            Self::ClientIp => "client_ip",
            Self::Anonymous => "anonymous",
        }
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FederationInvalidOperationAlertSummary {
    pub(crate) source: String,
    pub(crate) alerting_buckets: u64,
    pub(crate) max_count: u64,
    pub(crate) threshold: u64,
    pub(crate) window_secs: u64,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FederationInvalidOperationBucket {
    source: FederationInvalidOperationSourceKind,
    digest: [u8; 32],
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone)]
struct FederationInvalidOperationBucketState {
    window_started: std::time::Instant,
    count: u64,
    alerted: bool,
}

#[cfg(feature = "federation")]
impl FederationInvalidOperationBucketState {
    fn new(now: std::time::Instant) -> Self {
        Self {
            window_started: now,
            count: 0,
            alerted: false,
        }
    }

    fn reset_if_expired(&mut self, now: std::time::Instant, window: std::time::Duration) {
        if now.duration_since(self.window_started) >= window {
            self.window_started = now;
            self.count = 0;
            self.alerted = false;
        }
    }

    fn is_alerting(&self, threshold: u64) -> bool {
        self.count >= threshold
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone)]
struct FederationInvalidOperationAlertEvent {
    source: FederationInvalidOperationSourceKind,
    count: u64,
    threshold: u64,
    window_secs: u64,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone)]
struct FederationInvalidOperationAlertTracker {
    salt: [u8; 32],
    threshold: u64,
    window: std::time::Duration,
    buckets: HashMap<FederationInvalidOperationBucket, FederationInvalidOperationBucketState>,
}

#[cfg(feature = "federation")]
impl Default for FederationInvalidOperationAlertTracker {
    fn default() -> Self {
        Self::new(
            FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD,
            FEDERATION_INVALID_OPERATION_ALERT_WINDOW,
        )
    }
}

#[cfg(feature = "federation")]
impl FederationInvalidOperationAlertTracker {
    fn new(threshold: u64, window: std::time::Duration) -> Self {
        Self {
            salt: federation_invalid_operation_alert_salt(),
            threshold,
            window,
            buckets: HashMap::new(),
        }
    }

    fn record(
        &mut self,
        source: FederationInvalidOperationSource,
        now: std::time::Instant,
    ) -> Option<FederationInvalidOperationAlertEvent> {
        let bucket = self.bucket_for(&source);
        let state = self
            .buckets
            .entry(bucket)
            .or_insert_with(|| FederationInvalidOperationBucketState::new(now));
        state.reset_if_expired(now, self.window);
        state.count = state.count.saturating_add(1);
        if state.is_alerting(self.threshold) && !state.alerted {
            state.alerted = true;
            return Some(FederationInvalidOperationAlertEvent {
                source: source.kind,
                count: state.count,
                threshold: self.threshold,
                window_secs: self.window.as_secs(),
            });
        }

        None
    }

    fn summaries(
        &mut self,
        now: std::time::Instant,
    ) -> Vec<FederationInvalidOperationAlertSummary> {
        self.prune_expired(now);
        let mut summaries = BTreeMap::<
            FederationInvalidOperationSourceKind,
            FederationInvalidOperationAlertSummary,
        >::new();
        for (bucket, state) in &self.buckets {
            if !state.is_alerting(self.threshold) {
                continue;
            }
            let entry = summaries.entry(bucket.source).or_insert_with(|| {
                FederationInvalidOperationAlertSummary {
                    source: bucket.source.as_str().to_string(),
                    alerting_buckets: 0,
                    max_count: 0,
                    threshold: self.threshold,
                    window_secs: self.window.as_secs(),
                }
            });
            entry.alerting_buckets = entry.alerting_buckets.saturating_add(1);
            entry.max_count = entry.max_count.max(state.count);
        }

        summaries.into_values().collect()
    }

    fn bucket_for(
        &self,
        source: &FederationInvalidOperationSource,
    ) -> FederationInvalidOperationBucket {
        let mut hasher = Sha256::new();
        hasher.update(self.salt);
        hasher.update(source.kind.as_str().as_bytes());
        hasher.update(&source.material);
        FederationInvalidOperationBucket {
            source: source.kind,
            digest: hasher.finalize().into(),
        }
    }

    fn prune_expired(&mut self, now: std::time::Instant) {
        let window = self.window;
        self.buckets
            .retain(|_, state| now.duration_since(state.window_started) < window);
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone)]
struct FederationInvalidOperationSource {
    kind: FederationInvalidOperationSourceKind,
    material: Vec<u8>,
}

#[cfg(feature = "federation")]
impl FederationInvalidOperationSource {
    fn auth_token(token_kind: &'static str, token: &[u8]) -> Self {
        let mut material = Vec::with_capacity(token_kind.len() + 1 + token.len());
        material.extend_from_slice(token_kind.as_bytes());
        material.push(0);
        material.extend_from_slice(token);
        Self {
            kind: FederationInvalidOperationSourceKind::AuthToken,
            material,
        }
    }

    fn client_ip(ip: std::net::IpAddr) -> Self {
        Self {
            kind: FederationInvalidOperationSourceKind::ClientIp,
            material: ip.to_string().into_bytes(),
        }
    }

    fn anonymous() -> Self {
        Self {
            kind: FederationInvalidOperationSourceKind::Anonymous,
            material: b"anonymous".to_vec(),
        }
    }

    fn from_request(req: &axum::http::Request<axum::body::Body>) -> Self {
        if let Some(token) = req.headers().get(auth::CLEAR_AUTH_KEY) {
            return Self::auth_token("clear", token.as_bytes());
        }
        if let Some(token) = req.headers().get(auth::BLIND_AUTH_KEY) {
            return Self::auth_token("blind", token.as_bytes());
        }
        if let Some(axum::extract::ConnectInfo(address)) =
            req.extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        {
            return Self::client_ip(address.ip());
        }

        Self::anonymous()
    }
}

#[cfg(feature = "federation")]
fn federation_invalid_operation_alert_salt() -> [u8; 32] {
    let mut hasher = Sha256::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    hasher.update(now.to_be_bytes());
    hasher.finalize().into()
}

#[cfg(all(feature = "federation", feature = "prometheus"))]
fn usize_to_i64_saturating(value: usize) -> i64 {
    match i64::try_from(value) {
        Ok(value) => value,
        Err(_) => i64::MAX,
    }
}

#[cfg(feature = "federation")]
struct FederationFinalizationRequest {
    response: FederationSubmitOperationResponse,
    entry: FederationJournalEntry,
    sequence: Option<(MemberId, u64)>,
    result: tokio::sync::oneshot::Sender<Result<FederationSubmitOperationResponse, cdk::Error>>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotifications {
    inner: tokio::sync::Mutex<FederationOperationNotificationState>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotificationState {
    outcomes: HashMap<FederationOperationId, FederationSubmitOperationResponse>,
    notifiers: HashMap<FederationOperationId, Arc<tokio::sync::Notify>>,
}

#[cfg(feature = "federation")]
impl FederationOperationNotifications {
    async fn notify(&self, response: FederationSubmitOperationResponse) {
        let operation_id = response.operation_id;
        let mut inner = self.inner.lock().await;
        inner.outcomes.insert(operation_id, response);
        if let Some(notifier) = inner.notifiers.remove(&operation_id) {
            notifier.notify_waiters();
        }
    }

    async fn wait_for(
        &self,
        operation_id: FederationOperationId,
    ) -> FederationSubmitOperationResponse {
        loop {
            let notified = {
                let mut inner = self.inner.lock().await;
                if let Some(response) = inner.outcomes.get(&operation_id) {
                    return response.clone();
                }
                inner
                    .notifiers
                    .entry(operation_id)
                    .or_default()
                    .clone()
                    .notified_owned()
            };
            notified.await;
        }
    }
}

#[cfg(feature = "federation")]
fn catch_up_response_suffix_for_local_journal(
    local_entries: &[FederationJournalEntry],
    response: &FederationJournalCatchUpResponse,
) -> Result<Option<FederationJournalCatchUpResponse>, cdk::Error> {
    let local_next_index = local_entries.len() as u64;
    if response.from_index >= local_next_index {
        return Ok(Some(response.clone()));
    }

    let mut suffix_start = 0usize;
    for entry in &response.entries {
        if entry.index >= local_next_index {
            break;
        }
        let local_entry = local_entries.get(entry.index as usize).ok_or_else(|| {
            cdk::Error::Federation(FederationError::CatchUpIndexMismatch {
                expected: entry.index,
                found: local_next_index,
            })
        })?;
        ensure_catch_up_overlap_matches(local_entry, entry)?;
        suffix_start = suffix_start.saturating_add(1);
    }

    if suffix_start >= response.entries.len() {
        return Ok(None);
    }

    let mut suffix = response.clone();
    suffix.from_index = local_next_index;
    suffix.entries = response.entries[suffix_start..].to_vec();
    Ok(Some(suffix))
}

#[cfg(feature = "federation")]
fn ensure_catch_up_overlap_matches(
    local_entry: &FederationJournalEntry,
    peer_entry: &FederationJournalEntry,
) -> Result<(), cdk::Error> {
    if local_entry.index != peer_entry.index {
        return Err(cdk::Error::Federation(
            FederationError::CatchUpIndexMismatch {
                expected: local_entry.index,
                found: peer_entry.index,
            },
        ));
    }
    if local_entry.operation_id != peer_entry.operation_id {
        return Err(cdk::Error::Federation(
            FederationError::CatchUpOperationIdMismatch {
                index: peer_entry.index,
                expected: local_entry.operation_id,
                found: peer_entry.operation_id,
            },
        ));
    }
    if local_entry.previous_order_digest != peer_entry.previous_order_digest {
        return Err(cdk::Error::Federation(
            FederationError::CatchUpDigestMismatch {
                index: peer_entry.index,
                expected: local_entry.previous_order_digest,
                found: peer_entry.previous_order_digest,
            },
        ));
    }
    if local_entry.order_digest != peer_entry.order_digest {
        return Err(cdk::Error::Federation(
            FederationError::CatchUpDigestMismatch {
                index: peer_entry.index.saturating_add(1),
                expected: local_entry.order_digest,
                found: peer_entry.order_digest,
            },
        ));
    }

    Ok(())
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FederationSyncStatus {
    Healthy,
    Lagging {
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    },
    CatchingUp {
        peer: MemberId,
    },
    VerifyingSessionOutcome {
        peer: MemberId,
        operation_count: u64,
    },
    MaterializedStateMismatch {
        peer: MemberId,
        operation_count: u64,
    },
    Halted {
        reason: String,
    },
}

#[cfg(feature = "federation")]
impl Default for FederationSyncStatus {
    fn default() -> Self {
        Self::Healthy
    }
}

#[cfg(feature = "federation")]
impl FederationSyncStatus {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    pub(crate) fn state_name(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Lagging { .. } => "lagging",
            Self::CatchingUp { .. } => "catching_up",
            Self::VerifyingSessionOutcome { .. } => "verifying_session_outcome",
            Self::MaterializedStateMismatch { .. } => "materialized_state_mismatch",
            Self::Halted { .. } => "halted",
        }
    }

    pub(crate) fn not_ready_message(&self) -> String {
        match self {
            Self::Healthy => "federation member is healthy".to_string(),
            Self::Lagging {
                peer,
                local_operation_count,
                peer_operation_count,
            } => format!(
                "federation member is lagging behind peer {peer}: local_operations={local_operation_count}, peer_operations={peer_operation_count}"
            ),
            Self::CatchingUp { peer } => {
                format!("federation member is catching up from peer {peer}")
            }
            Self::VerifyingSessionOutcome {
                peer,
                operation_count,
            } => format!(
                "federation member is verifying quorum session outcome evidence after catch-up from peer {peer}: operation_count={operation_count}"
            ),
            Self::MaterializedStateMismatch {
                peer,
                operation_count,
            } => format!(
                "federation member is waiting for materialized state digest convergence with peer {peer}: operation_count={operation_count}"
            ),
            Self::Halted { reason } => {
                format!(
                    "federation member is halted: {}",
                    redact_federation_operator_message(reason)
                )
            }
        }
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Default)]
pub(crate) struct FederationSyncState {
    status: FederationSyncStatus,
}

#[cfg(feature = "federation")]
impl FederationSyncState {
    fn status(&self) -> FederationSyncStatus {
        self.status.clone()
    }

    fn mark_lagging(
        &mut self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.status = FederationSyncStatus::Lagging {
            peer,
            local_operation_count,
            peer_operation_count,
        };
    }

    fn mark_catching_up(&mut self, peer: MemberId) {
        self.status = FederationSyncStatus::CatchingUp { peer };
    }

    fn mark_verifying_session_outcome(&mut self, peer: MemberId, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > operation_count => {}
            _ => {
                self.status = FederationSyncStatus::VerifyingSessionOutcome {
                    peer,
                    operation_count,
                };
            }
        }
    }

    fn mark_materialized_state_mismatch(&mut self, peer: MemberId, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > operation_count => {}
            FederationSyncStatus::VerifyingSessionOutcome {
                operation_count: pending_operation_count,
                ..
            } if pending_operation_count > operation_count => {}
            _ => {
                self.status = FederationSyncStatus::MaterializedStateMismatch {
                    peer,
                    operation_count,
                };
            }
        }
    }

    fn mark_healthy_if_session_outcome_verified(&mut self, verified_operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > verified_operation_count => {}
            FederationSyncStatus::VerifyingSessionOutcome {
                operation_count, ..
            } if operation_count > verified_operation_count => {}
            _ => self.mark_healthy(),
        }
    }

    fn mark_healthy(&mut self) {
        self.status = FederationSyncStatus::Healthy;
    }

    fn mark_halted(&mut self, reason: String) {
        let reason = redact_federation_operator_message(&reason);
        self.status = FederationSyncStatus::Halted { reason };
    }
}

#[cfg(all(test, feature = "federation"))]
mod tests {
    use super::*;

    #[test]
    fn federation_halted_sync_status_redacts_operator_message() {
        let status = FederationSyncStatus::Halted {
            reason: concat!(
                "peer 2 digest mismatch at operation count 4 ",
                "password=hunter2 token=sync-token ",
                "private_config_path=/home/example/fed/private.json ",
                "database_url=postgresql://status:db-secret@db.example.invalid/cdk"
            )
            .to_string(),
        };

        let message = status.not_ready_message();

        assert!(message.contains("digest mismatch"));
        assert!(!message.contains("hunter2"));
        assert!(!message.contains("sync-token"));
        assert!(!message.contains("/home/example"));
        assert!(!message.contains("db-secret"));
    }

    #[test]
    fn federation_sync_state_stores_redacted_halted_reason() {
        let mut state = FederationSyncState::default();

        state.mark_halted(
            "halted while reading secret=sync-secret from /home/example/fed/private.json"
                .to_string(),
        );

        match state.status() {
            FederationSyncStatus::Halted { reason } => {
                assert!(reason.contains("secret=[REDACTED]"));
                assert!(reason.contains("[REDACTED_PATH]"));
                assert!(!reason.contains("sync-secret"));
                assert!(!reason.contains("/home/example"));
            }
            _ => panic!("expected halted status"),
        }
    }

    #[test]
    fn invalid_operation_alert_tracker_reports_threshold_without_raw_source() {
        let mut tracker =
            FederationInvalidOperationAlertTracker::new(3, std::time::Duration::from_secs(60));
        let now = std::time::Instant::now();

        assert!(tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"secret-token"),
                now
            )
            .is_none());
        assert!(tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"secret-token"),
                now
            )
            .is_none());
        let event = tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"secret-token"),
                now,
            )
            .expect("threshold crossing alert");

        assert_eq!(
            event.source,
            FederationInvalidOperationSourceKind::AuthToken
        );
        assert_eq!(event.count, 3);
        let summaries = tracker.summaries(now);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].source, "auth_token");
        assert_eq!(summaries[0].alerting_buckets, 1);
        assert_eq!(summaries[0].max_count, 3);
        let serialized = serde_json::to_string(&summaries).expect("serialize summaries");
        assert!(!serialized.contains("secret-token"));
    }

    #[test]
    fn invalid_operation_alert_tracker_keeps_auth_buckets_separate() {
        let mut tracker =
            FederationInvalidOperationAlertTracker::new(2, std::time::Duration::from_secs(60));
        let now = std::time::Instant::now();

        assert!(tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"token-a"),
                now
            )
            .is_none());
        assert!(tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"token-b"),
                now
            )
            .is_none());
        assert!(tracker.summaries(now).is_empty());
        assert!(tracker
            .record(
                FederationInvalidOperationSource::auth_token("clear", b"token-a"),
                now
            )
            .is_some());

        let summaries = tracker.summaries(now);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].alerting_buckets, 1);
    }

    #[test]
    fn invalid_operation_alert_tracker_expires_old_buckets() {
        let mut tracker =
            FederationInvalidOperationAlertTracker::new(1, std::time::Duration::from_secs(5));
        let now = std::time::Instant::now();
        assert!(tracker
            .record(FederationInvalidOperationSource::anonymous(), now)
            .is_some());
        assert_eq!(tracker.summaries(now).len(), 1);

        assert!(tracker
            .summaries(now + std::time::Duration::from_secs(5))
            .is_empty());
    }

    #[test]
    fn invalid_operation_source_prefers_auth_token_then_connect_info() {
        let mut auth_req = axum::http::Request::builder()
            .uri("/v1/swap")
            .header(auth::CLEAR_AUTH_KEY, "clear-token")
            .body(axum::body::Body::empty())
            .expect("request");
        auth_req.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:8080"
                .parse::<std::net::SocketAddr>()
                .expect("socket address"),
        ));
        let auth_source = FederationInvalidOperationSource::from_request(&auth_req);
        assert_eq!(
            auth_source.kind,
            FederationInvalidOperationSourceKind::AuthToken
        );

        let mut ip_req = axum::http::Request::builder()
            .uri("/v1/swap")
            .body(axum::body::Body::empty())
            .expect("request");
        ip_req.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:8080"
                .parse::<std::net::SocketAddr>()
                .expect("socket address"),
        ));
        let ip_source = FederationInvalidOperationSource::from_request(&ip_req);
        assert_eq!(
            ip_source.kind,
            FederationInvalidOperationSourceKind::ClientIp
        );
    }
}

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
    #[cfg(feature = "federation")]
    federation_consensus: Option<FederationConsensusHandle>,
    #[cfg(feature = "federation")]
    federation_peer_sync_config: Option<FederationPeerSyncConfig>,
    #[cfg(feature = "federation")]
    federation_sequences: Option<FederationSequenceHandle>,
    #[cfg(feature = "federation")]
    federation_sync: Option<FederationSyncHandle>,
    #[cfg(feature = "federation")]
    federation_notifications: Option<FederationNotificationHandle>,
    #[cfg(feature = "federation")]
    federation_finalizer: Option<FederationFinalizationHandle>,
    #[cfg(feature = "federation")]
    federation_private_rest_admission: Option<FederationRestAdmissionHandle>,
    #[cfg(feature = "federation")]
    federation_invalid_operation_alerts: Option<FederationInvalidOperationAlertHandle>,
}

#[cfg(feature = "federation")]
impl MintState {
    fn start_federation_finalization_loop(&mut self) {
        if self.federation_consensus.is_none() || self.federation_finalizer.is_some() {
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::channel(128);
        self.federation_finalizer = Some(sender);
        let mut finalizer_state = self.clone();
        finalizer_state.federation_finalizer = None;
        tokio::spawn(async move {
            finalizer_state
                .run_federation_finalization_loop(receiver)
                .await;
        });
    }

    async fn run_federation_finalization_loop(
        &self,
        mut receiver: tokio::sync::mpsc::Receiver<FederationFinalizationRequest>,
    ) {
        while let Some(request) = receiver.recv().await {
            let result = self
                .finalize_federation_operation_acceptance_inline(
                    request.response,
                    request.entry,
                    request.sequence,
                )
                .await;
            let _ = request.result.send(result);
        }
    }

    #[cfg(feature = "prometheus")]
    fn federation_metric_member_label(&self) -> Option<String> {
        self.mint
            .federation_config()
            .map(|config| config.member_id().to_string())
    }

    #[cfg(feature = "prometheus")]
    fn record_federation_accepted_operation_metric(&self, operation: &str) {
        if let Some(member) = self.federation_metric_member_label() {
            cdk_prometheus::METRICS.record_federation_accepted_operation(&member, operation);
        }
    }

    #[cfg(feature = "prometheus")]
    fn record_federation_rejected_operation_metric(&self, operation: &str, reason: &str) {
        if let Some(member) = self.federation_metric_member_label() {
            cdk_prometheus::METRICS
                .record_federation_rejected_operation(&member, operation, reason);
        }
    }

    pub(crate) async fn federation_pending_mempool_len(&self) -> usize {
        match &self.federation_consensus {
            Some(consensus) => consensus.lock().await.pending_mempool_len(),
            None => 0,
        }
    }

    pub(crate) async fn federation_invalid_operation_alert_summaries(
        &self,
    ) -> Vec<FederationInvalidOperationAlertSummary> {
        match &self.federation_invalid_operation_alerts {
            Some(alerts) => alerts.lock().await.summaries(std::time::Instant::now()),
            None => Vec::new(),
        }
    }

    async fn record_federation_invalid_operation_alert(
        &self,
        source: FederationInvalidOperationSource,
    ) {
        let Some(alerts) = &self.federation_invalid_operation_alerts else {
            return;
        };
        let Some(event) = alerts
            .lock()
            .await
            .record(source, std::time::Instant::now())
        else {
            return;
        };
        tracing::warn!(
            source = event.source.as_str(),
            count = event.count,
            threshold = event.threshold,
            window_secs = event.window_secs,
            "Federation invalid operation alert threshold reached"
        );
    }

    #[cfg(feature = "prometheus")]
    async fn record_federation_pending_mempool_metric(&self) {
        let Some(member) = self.federation_metric_member_label() else {
            return;
        };
        let pending = self.federation_pending_mempool_len().await;
        cdk_prometheus::METRICS
            .set_federation_pending_mempool_items(&member, usize_to_i64_saturating(pending));
    }

    pub(crate) async fn submit_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.ensure_federation_can_forward_public_submit().await?;

        if let Some(response) =
            crate::federation::submit_operation_to_primary_if_needed(self, envelope.clone(), None)
                .await?
        {
            return Ok(response);
        }

        self.ensure_federation_ready_for_public_submit().await?;

        self.submit_federation_envelope_with_sequence(envelope, None)
            .await
    }

    pub(crate) async fn submit_federation_envelope_with_sequence(
        &self,
        envelope: FederationOperationEnvelope,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let operation_label = envelope.operation.kind().to_string();
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let response = match next_consensus.submit_envelope(envelope) {
            Ok(response) => response,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                self.record_federation_rejected_operation_metric(&operation_label, "consensus");
                return Err(cdk::Error::from(err));
            }
        };
        let _appended = matches!(
            &response.outcome,
            FederationJournalAppendOutcome::Appended(_)
        );
        let entry = response.outcome.entry().clone();

        if let Err(err) = self
            .persist_federation_append_outcome(&response.outcome, sequence)
            .await
        {
            return Err(err);
        }
        *consensus = next_consensus;
        drop(consensus);
        #[cfg(feature = "prometheus")]
        {
            if _appended {
                self.record_federation_accepted_operation_metric(&operation_label);
            }
            self.record_federation_pending_mempool_metric().await;
        }

        let operation_id = response.operation_id;
        self.finalize_federation_operation_acceptance(response.clone(), entry, sequence)
            .await?;

        Ok(self
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .unwrap_or(response))
    }

    pub(crate) async fn submit_federation_payment_observation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        match &envelope.operation {
            FederationOperation::MintQuotePayment(_) | FederationOperation::MeltQuotePayment(_) => {
            }
            _ => {
                return Err(cdk::Error::Custom(
                    "internal federation payment observation submit received non-observation operation"
                        .to_string(),
                ));
            }
        }

        if let Some(response) =
            crate::federation::submit_operation_to_primary_if_needed(self, envelope.clone(), None)
                .await?
        {
            return Ok(response);
        }

        self.submit_federation_envelope_with_sequence(envelope, None)
            .await
    }

    async fn finalize_federation_operation_acceptance(
        &self,
        response: FederationSubmitOperationResponse,
        entry: FederationJournalEntry,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let Some(finalizer) = &self.federation_finalizer else {
            return self
                .finalize_federation_operation_acceptance_inline(response, entry, sequence)
                .await;
        };
        let (result, receiver) = tokio::sync::oneshot::channel();
        finalizer
            .send(FederationFinalizationRequest {
                response,
                entry,
                sequence,
                result,
            })
            .await
            .map_err(|_| {
                cdk::Error::Custom(
                    "federation finalization loop stopped before accepting operation".to_string(),
                )
            })?;
        receiver.await.map_err(|_| {
            cdk::Error::Custom(
                "federation finalization loop stopped before completing operation".to_string(),
            )
        })?
    }

    async fn finalize_federation_operation_acceptance_inline(
        &self,
        response: FederationSubmitOperationResponse,
        entry: FederationJournalEntry,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if let Some((sender, sequence)) = sequence {
            self.record_federation_sequence(sender, sequence).await;
        }
        self.apply_federation_journal_entries(std::slice::from_ref(&entry))
            .await?;
        self.notify_federation_operation(response.clone()).await;

        Ok(response)
    }

    pub(crate) async fn wait_for_federation_operation_acceptance(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<FederationSubmitOperationResponse> {
        let notifications = self.federation_notifications.as_ref()?;
        Some(notifications.wait_for(operation_id).await)
    }

    async fn notify_federation_operation(&self, response: FederationSubmitOperationResponse) {
        if let Some(notifications) = &self.federation_notifications {
            notifications.notify(response).await;
        }
    }

    pub(crate) async fn apply_federation_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let response_to_apply = catch_up_response_suffix_for_local_journal(
            next_consensus.journal().entries(),
            response,
        )?;
        let applied = match response_to_apply.as_ref() {
            Some(response) => next_consensus.apply_catch_up_response(response)?,
            None => Vec::new(),
        };

        self.persist_federation_journal_entries(&applied, response.session_outcome.as_ref(), None)
            .await?;
        *consensus = next_consensus;
        drop(consensus);
        self.apply_federation_journal_entries(&applied).await?;

        Ok(applied)
    }

    pub(crate) async fn persist_federation_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.persist_federation_journal_entries(&[], None, Some((sender, sequence)))
            .await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.federation_public_config()?;
        let mut setup_state = self.load_federation_dkg_setup_state(&public_config).await?;
        setup_state.insert_commitment(&public_config, *commitment)?;

        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_commitment(commitment).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.federation_public_config()?;
        let mut setup_state = self.load_federation_dkg_setup_state(&public_config).await?;
        setup_state.insert_reveal(&public_config, reveal.clone())?;

        let db = self.mint.localstore();
        let completed_keyset_share = self
            .derive_federation_dkg_member_keyset_share_if_ready(&public_config, &setup_state, None)
            .await?;
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_reveal(reveal).await?;
        if let Some(keyset_share) = &completed_keyset_share {
            tx.add_federation_dkg_member_keyset_share(keyset_share)
                .await?;
        }
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let runtime_config = self.mint.federation_config().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its runtime config".to_string())
        })?;
        contribution.validate_sender_receiver(
            runtime_config.public_config(),
            sender,
            runtime_config.member_id(),
        )?;
        let public_config = runtime_config.public_config().clone();

        let db = self.mint.localstore();
        let existing_contributions = db.get_federation_dkg_secret_share_contributions().await?;
        if existing_contributions.iter().any(|existing| {
            existing.sender == contribution.sender
                && existing.receiver == contribution.receiver
                && existing != contribution
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: format!(
                        "conflicting secret-share contribution for receiver {}",
                        contribution.receiver
                    ),
                },
            ));
        }
        let setup_state = self.load_federation_dkg_setup_state(&public_config).await?;
        let completed_keyset_share = self
            .derive_federation_dkg_member_keyset_share_if_ready(
                &public_config,
                &setup_state,
                Some(contribution),
            )
            .await?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_secret_share_contribution(contribution)
            .await?;
        if let Some(keyset_share) = &completed_keyset_share {
            tx.add_federation_dkg_member_keyset_share(keyset_share)
                .await?;
        }
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_transcript_signature(
        &self,
        signature: &FederationDkgTranscriptSignatureRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.federation_public_config()?;
        signature.validate_sender(&public_config, sender)?;

        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_transcript_signature(signature)
            .await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    async fn derive_federation_dkg_member_keyset_share_if_ready(
        &self,
        public_config: &FederationConfig,
        setup_state: &FederationDkgSetupState,
        pending_contribution: Option<&FederationDkgSecretShareRequest>,
    ) -> Result<Option<FederationDkgMemberKeysetShare>, cdk::Error> {
        let Some(round) = setup_state.complete_round(public_config)? else {
            return Ok(None);
        };
        let local_member = self
            .mint
            .federation_config()
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })?
            .member_id();
        let mut participant_secret_shares = BTreeMap::new();
        let contributions = self
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await?;
        for contribution in &contributions {
            if contribution.receiver != local_member {
                continue;
            }
            contribution.validate_sender_receiver(
                public_config,
                contribution.sender,
                local_member,
            )?;
            match participant_secret_shares
                .insert(contribution.sender, contribution.secret_shares.clone())
            {
                Some(existing) if existing != contribution.secret_shares => {
                    return Err(cdk::Error::Federation(
                        FederationError::DkgCommitmentMismatch {
                            member_id: contribution.sender,
                            reason: format!(
                                "conflicting secret-share contribution for receiver {}",
                                contribution.receiver
                            ),
                        },
                    ));
                }
                Some(_) | None => {}
            }
        }
        if let Some(contribution) = pending_contribution {
            contribution.validate_sender_receiver(
                public_config,
                contribution.sender,
                local_member,
            )?;
            match participant_secret_shares
                .insert(contribution.sender, contribution.secret_shares.clone())
            {
                Some(existing) if existing != contribution.secret_shares => {
                    return Err(cdk::Error::Federation(
                        FederationError::DkgCommitmentMismatch {
                            member_id: contribution.sender,
                            reason: format!(
                                "conflicting secret-share contribution for receiver {}",
                                contribution.receiver
                            ),
                        },
                    ));
                }
                Some(_) | None => {}
            }
        }
        if !public_config
            .members
            .keys()
            .all(|member_id| participant_secret_shares.contains_key(member_id))
        {
            return Ok(None);
        }

        let keyset_share = round.derive_member_keyset_share(
            public_config,
            &setup_state.reveals,
            local_member,
            &participant_secret_shares,
            cdk::nuts::CurrencyUnit::Sat,
            true,
            0,
            None,
        )?;
        Ok(Some(keyset_share))
    }

    fn federation_public_config(&self) -> Result<FederationConfig, cdk::Error> {
        self.mint
            .federation_config()
            .map(|config| config.public_config().clone())
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })
    }

    async fn load_federation_dkg_setup_state(
        &self,
        public_config: &FederationConfig,
    ) -> Result<FederationDkgSetupState, cdk::Error> {
        let db = self.mint.localstore();
        let commitments = db.get_federation_dkg_commitments().await?;
        let reveals = db.get_federation_dkg_reveals().await?;
        let mut setup_state = FederationDkgSetupState::new(public_config);
        for commitment in commitments {
            setup_state.insert_commitment(public_config, commitment)?;
        }
        for reveal in reveals {
            setup_state.insert_reveal(public_config, reveal)?;
        }
        Ok(setup_state)
    }

    pub(crate) async fn persist_federation_session_outcome(
        &self,
        outcome: &FederationSessionOutcome,
    ) -> Result<(), cdk::Error> {
        self.persist_federation_journal_entries(&[], Some(outcome), None)
            .await
    }

    async fn persist_federation_append_outcome(
        &self,
        outcome: &FederationJournalAppendOutcome,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        match outcome {
            FederationJournalAppendOutcome::Appended(entry) => {
                self.persist_federation_journal_entries(std::slice::from_ref(entry), None, sequence)
                    .await
            }
            FederationJournalAppendOutcome::Duplicate(_) => {
                self.persist_federation_journal_entries(&[], None, sequence)
                    .await
            }
        }
    }

    async fn persist_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
        session_outcome: Option<&FederationSessionOutcome>,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        if let Some(session_outcome) = session_outcome {
            tx.add_federation_session_outcome(session_outcome).await?;
        }
        if let Some((sender, sequence)) = sequence {
            tx.set_federation_member_sequence(sender, sequence).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn record_federation_sequence(&self, sender: MemberId, sequence: u64) {
        if let Some(sequences) = &self.federation_sequences {
            sequences.lock().await.record_sequence(sender, sequence);
        }
    }

    async fn apply_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        let applied = self.mint.apply_federation_journal_entries(entries).await?;
        self.mark_federation_operations_applied(&applied).await?;
        Ok(applied)
    }

    async fn mark_federation_operations_applied(
        &self,
        operation_ids: &[FederationOperationId],
    ) -> Result<(), cdk::Error> {
        let Some(consensus) = &self.federation_consensus else {
            return Ok(());
        };
        let mut consensus = consensus.lock().await;
        for operation_id in operation_ids {
            consensus.update_status(*operation_id, FederationOperationStatus::Applied)?;
        }
        Ok(())
    }

    pub(crate) async fn federation_sync_status(&self) -> Option<FederationSyncStatus> {
        let sync = self.federation_sync.as_ref()?;
        Some(sync.lock().await.status())
    }

    pub(crate) async fn mark_federation_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_lagging(peer, local_operation_count, peer_operation_count);
        }
    }

    pub(crate) async fn mark_federation_catching_up(&self, peer: MemberId) {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_catching_up(peer);
        }
    }

    pub(crate) async fn mark_federation_verifying_session_outcome(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_verifying_session_outcome(peer, operation_count);
        }
    }

    pub(crate) async fn mark_federation_materialized_state_mismatch(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        #[cfg(feature = "prometheus")]
        {
            if let Some(member) = self.federation_metric_member_label() {
                cdk_prometheus::METRICS
                    .record_federation_state_digest_mismatch(&member, &peer.to_string());
            }
        }
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_materialized_state_mismatch(peer, operation_count);
        }
    }

    pub(crate) async fn mark_federation_healthy_if_session_outcome_verified(
        &self,
        verified_operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_healthy_if_session_outcome_verified(verified_operation_count);
        }
    }

    #[cfg(test)]
    pub(crate) async fn mark_federation_healthy(&self) {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_healthy();
        }
    }

    pub(crate) async fn mark_federation_healthy_if_caught_up(&self, local_operation_count: u64) {
        if let Some(sync) = &self.federation_sync {
            let mut sync = sync.lock().await;
            match sync.status() {
                FederationSyncStatus::Lagging {
                    peer_operation_count,
                    ..
                } if peer_operation_count > local_operation_count => {}
                FederationSyncStatus::VerifyingSessionOutcome { .. } => {}
                FederationSyncStatus::Halted { .. } => {}
                _ => sync.mark_healthy(),
            }
        }
    }

    pub(crate) async fn mark_federation_halted<S>(&self, reason: S)
    where
        S: Into<String>,
    {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_halted(reason.into());
        }
    }

    async fn ensure_federation_ready_for_public_submit(&self) -> Result<(), cdk::Error> {
        let Some(sync) = &self.federation_sync else {
            return Ok(());
        };
        let status = sync.lock().await.status();
        if status.is_ready() {
            return Ok(());
        }

        Err(cdk::Error::Custom(status.not_ready_message()))
    }

    async fn ensure_federation_can_forward_public_submit(&self) -> Result<(), cdk::Error> {
        let Some(sync) = &self.federation_sync else {
            return Ok(());
        };
        let status = sync.lock().await.status();
        match &status {
            FederationSyncStatus::MaterializedStateMismatch { .. }
            | FederationSyncStatus::Halted { .. } => {
                Err(cdk::Error::Custom(status.not_ready_message()))
            }
            _ => Ok(()),
        }
    }
}

/// Create mint [`Router`] with required endpoints for cashu mint with the default cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
pub async fn create_mint_router(mint: Arc<Mint>, custom_methods: Vec<String>) -> Result<Router> {
    create_mint_router_with_custom_cache(mint, Default::default(), custom_methods, false).await
}

async fn cors_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let allowed_headers = "*";

    // Handle preflight requests
    if req.method() == axum::http::Method::OPTIONS {
        let mut response = Response::new("".into());
        response.headers_mut().insert(
            "Access-Control-Allow-Origin",
            "*".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Methods",
            "GET, POST, OPTIONS".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Headers",
            allowed_headers.parse().expect("Valid header value"),
        );
        return response;
    }

    // Call the next handler
    let mut response = next.run(req).await;

    response.headers_mut().insert(
        "Access-Control-Allow-Origin",
        "*".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Headers",
        allowed_headers.parse().expect("Valid header value"),
    );

    response
}

#[cfg(feature = "federation")]
async fn federation_invalid_operation_alert_middleware(
    axum::extract::State(state): axum::extract::State<MintState>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let monitor = federation_invalid_operation_alert_path(req.method(), req.uri().path());
    let source = monitor.then(|| FederationInvalidOperationSource::from_request(&req));
    let response = next.run(req).await;
    if monitor && federation_invalid_operation_alert_status(response.status()) {
        if let Some(source) = source {
            state
                .record_federation_invalid_operation_alert(source)
                .await;
        }
    }

    response
}

#[cfg(feature = "federation")]
fn federation_invalid_operation_alert_path(method: &axum::http::Method, path: &str) -> bool {
    if *method != axum::http::Method::POST {
        return false;
    }

    path == "/v1/swap"
        || path == "/v1/restore"
        || path == "/v1/checkstate"
        || path == "/v1/auth/blind/mint"
        || path == "/federation/v1/operations"
        || path.starts_with("/v1/mint/")
        || path.starts_with("/v1/melt/")
}

#[cfg(feature = "federation")]
fn federation_invalid_operation_alert_status(status: axum::http::StatusCode) -> bool {
    status.is_client_error()
        && status != axum::http::StatusCode::NOT_FOUND
        && status != axum::http::StatusCode::METHOD_NOT_ALLOWED
}

/// Create mint [`Router`] with required endpoints for cashu mint with a custom
/// backend for cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
) -> Result<Router> {
    #[cfg(feature = "federation")]
    {
        create_mint_router_with_custom_cache_and_federation_peer_sync_config(
            mint,
            cache,
            custom_methods,
            enable_info_page,
            FederationPeerSyncConfig::default(),
        )
        .await
    }
    #[cfg(not(feature = "federation"))]
    {
        create_mint_router_with_custom_cache_inner(mint, cache, custom_methods, enable_info_page)
            .await
    }
}

/// Create mint [`Router`] with a custom cache and federation peer sync policy.
#[cfg(feature = "federation")]
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache_and_federation_peer_sync_config(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    federation_peer_sync_config: FederationPeerSyncConfig,
) -> Result<Router> {
    federation_peer_sync_config.validate()?;
    create_mint_router_with_custom_cache_inner(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        federation_peer_sync_config,
    )
    .await
}

#[allow(unused_mut, unused_variables)]
async fn create_mint_router_with_custom_cache_inner(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    #[cfg(feature = "federation")] federation_peer_sync_config: FederationPeerSyncConfig,
) -> Result<Router> {
    #[cfg(feature = "federation")]
    let (federation_consensus, federation_sequences) =
        create_federation_runtime_state(&mint).await?;
    let mut state = MintState {
        mint,
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        federation_consensus: federation_consensus.clone(),
        #[cfg(feature = "federation")]
        federation_peer_sync_config: federation_consensus
            .as_ref()
            .map(|_| federation_peer_sync_config),
        #[cfg(feature = "federation")]
        federation_sequences,
        #[cfg(feature = "federation")]
        federation_sync: federation_consensus
            .as_ref()
            .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::default()))),
        #[cfg(feature = "federation")]
        federation_notifications: federation_consensus
            .as_ref()
            .map(|_| Arc::new(FederationOperationNotifications::default())),
        #[cfg(feature = "federation")]
        federation_finalizer: None,
        #[cfg(feature = "federation")]
        federation_private_rest_admission: federation_consensus.as_ref().map(|_| {
            Arc::new(tokio::sync::Semaphore::new(
                federation_peer_sync_config.private_rest_inbound_concurrency_limit,
            ))
        }),
        #[cfg(feature = "federation")]
        federation_invalid_operation_alerts: federation_consensus.as_ref().map(|_| {
            Arc::new(tokio::sync::Mutex::new(
                FederationInvalidOperationAlertTracker::default(),
            ))
        }),
    };
    #[cfg(feature = "federation")]
    state.start_federation_finalization_loop();

    let v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/{keyset_id}", get(get_keyset_pubkeys))
        .route("/swap", post(cache_post_swap))
        .route("/ws", get(ws_handler))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    let mut mint_router = Router::new().nest("/v1", v1_router);

    #[cfg(feature = "federation")]
    if state.federation_consensus.is_some() {
        federation::spawn_federation_peer_sync_loop_with_config(
            state.clone(),
            federation_peer_sync_config,
        );
        mint_router = mint_router.nest("/federation/v1", federation::create_federation_router());
    }

    #[cfg(feature = "info-page")]
    if enable_info_page {
        mint_router = mint_router.route("/", get(get_index));
    }

    let mint_router = {
        let auth_router = create_auth_router(state.clone());
        mint_router.nest("/v1", auth_router)
    };

    // Create and merge custom payment method routers
    // This now includes bolt11 and bolt12 if they are in custom_methods
    let mint_router = if !custom_methods.is_empty() {
        // Validate custom method names
        custom_router::validate_custom_method_names(&custom_methods)
            .map_err(|e| anyhow::anyhow!("Invalid custom method names: {}", e))?;

        tracing::info!(
            "Creating routes for {} payment methods: {:?}",
            custom_methods.len(),
            custom_methods
        );

        let custom_router = custom_router::create_custom_routers(state.clone(), custom_methods);
        mint_router.nest("/v1", custom_router)
    } else {
        mint_router
    };

    #[cfg(feature = "federation")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        federation_invalid_operation_alert_middleware,
    ));

    #[cfg(feature = "prometheus")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        metrics::global_metrics_middleware,
    ));
    let mint_router = mint_router
        .layer(from_fn(cors_middleware))
        .with_state(state);

    Ok(mint_router)
}

#[cfg(feature = "federation")]
async fn create_federation_runtime_state(
    mint: &Arc<Mint>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
)> {
    let Some(config) = mint.federation_config() else {
        return Ok((None, None));
    };

    let db = mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let sequences = db.get_federation_member_sequences().await?;
    let mut consensus = FederationConsensusRuntime::from_entries_with_backend(
        config.consensus_backend(),
        config.federation_id(),
        config.member_id(),
        entries.clone(),
    )?;
    let applied = mint.apply_federation_journal_entries(&entries).await?;
    for operation_id in applied {
        consensus.update_status(operation_id, FederationOperationStatus::Applied)?;
    }
    let sequences = FederationRestSequenceTracker::from_sequences(sequences);

    Ok((
        Some(Arc::new(tokio::sync::Mutex::new(consensus))),
        Some(Arc::new(tokio::sync::Mutex::new(sequences))),
    ))
}
