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
    redact_federation_operator_message, FederationAlephBftFinalizedItem,
    FederationAlephBftUnitPayload, FederationConfig, FederationConfigSnapshot,
    FederationConsensusBackendKind, FederationConsensusEngine, FederationConsensusRuntime,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgSetupMethod, FederationDkgSetupState, FederationDkgTranscript,
    FederationDkgTranscriptHash, FederationDkgTranscriptSignatureRequest, FederationError,
    FederationJournalAppendOutcome, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationMemberStatusKind, FederationMemberStatusSnapshot, FederationOperation,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationRestSequenceTracker, FederationSessionOutcome, FederationStateDigestCheckpoint,
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

    fn to_member_status_snapshot(
        &self,
        member_id: MemberId,
        updated_time: u64,
    ) -> FederationMemberStatusSnapshot {
        match self {
            Self::Healthy => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::Healthy,
                peer: None,
                local_operation_count: None,
                peer_operation_count: None,
                operation_count: None,
                message: None,
                updated_time,
            },
            Self::Lagging {
                peer,
                local_operation_count,
                peer_operation_count,
            } => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::Lagging,
                peer: Some(*peer),
                local_operation_count: Some(*local_operation_count),
                peer_operation_count: Some(*peer_operation_count),
                operation_count: None,
                message: Some(self.not_ready_message()),
                updated_time,
            },
            Self::CatchingUp { peer } => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::CatchingUp,
                peer: Some(*peer),
                local_operation_count: None,
                peer_operation_count: None,
                operation_count: None,
                message: Some(self.not_ready_message()),
                updated_time,
            },
            Self::VerifyingSessionOutcome {
                peer,
                operation_count,
            } => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::VerifyingSessionOutcome,
                peer: Some(*peer),
                local_operation_count: None,
                peer_operation_count: None,
                operation_count: Some(*operation_count),
                message: Some(self.not_ready_message()),
                updated_time,
            },
            Self::MaterializedStateMismatch {
                peer,
                operation_count,
            } => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::MaterializedStateMismatch,
                peer: Some(*peer),
                local_operation_count: None,
                peer_operation_count: None,
                operation_count: Some(*operation_count),
                message: Some(self.not_ready_message()),
                updated_time,
            },
            Self::Halted { reason } => FederationMemberStatusSnapshot {
                member_id,
                status: FederationMemberStatusKind::Halted,
                peer: None,
                local_operation_count: None,
                peer_operation_count: None,
                operation_count: None,
                message: Some(reason.clone()),
                updated_time,
            },
        }
    }

    fn try_from_member_status_snapshot(
        snapshot: FederationMemberStatusSnapshot,
    ) -> std::result::Result<Self, String> {
        let peer = || {
            snapshot.peer.ok_or_else(|| {
                format!(
                    "persisted {} federation sync status is missing peer member id",
                    snapshot.status
                )
            })
        };
        let operation_count = || {
            snapshot.operation_count.ok_or_else(|| {
                format!(
                    "persisted {} federation sync status is missing operation count",
                    snapshot.status
                )
            })
        };

        match snapshot.status {
            FederationMemberStatusKind::Healthy => Ok(Self::Healthy),
            FederationMemberStatusKind::Lagging => Ok(Self::Lagging {
                peer: peer()?,
                local_operation_count: snapshot.local_operation_count.ok_or_else(|| {
                    "persisted lagging federation sync status is missing local operation count"
                        .to_string()
                })?,
                peer_operation_count: snapshot.peer_operation_count.ok_or_else(|| {
                    "persisted lagging federation sync status is missing peer operation count"
                        .to_string()
                })?,
            }),
            FederationMemberStatusKind::CatchingUp => Ok(Self::CatchingUp { peer: peer()? }),
            FederationMemberStatusKind::VerifyingSessionOutcome => {
                Ok(Self::VerifyingSessionOutcome {
                    peer: peer()?,
                    operation_count: operation_count()?,
                })
            }
            FederationMemberStatusKind::MaterializedStateMismatch => {
                Ok(Self::MaterializedStateMismatch {
                    peer: peer()?,
                    operation_count: operation_count()?,
                })
            }
            FederationMemberStatusKind::Halted => Ok(Self::Halted {
                reason: snapshot
                    .message
                    .as_deref()
                    .map(redact_federation_operator_message)
                    .unwrap_or_else(|| "persisted federation halt".to_string()),
            }),
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

    fn update_status<F>(&mut self, update: F) -> Option<FederationSyncStatus>
    where
        F: FnOnce(&mut Self),
    {
        let previous = self.status();
        update(self);
        let current = self.status();
        (current != previous).then_some(current)
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

#[cfg(feature = "federation")]
fn federation_status_unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
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
        if self.federation_consensus_backend().await? == FederationConsensusBackendKind::AlephBft {
            return self
                .submit_aleph_bft_federation_envelope_with_sequence(envelope, sequence)
                .await;
        }

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

    async fn federation_consensus_backend(
        &self,
    ) -> Result<FederationConsensusBackendKind, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let consensus = consensus.lock().await;
        Ok(consensus.backend_kind())
    }

    async fn submit_aleph_bft_federation_envelope_with_sequence(
        &self,
        envelope: FederationOperationEnvelope,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if sequence.is_some() {
            return Err(cdk::Error::Custom(
                "AlephBFT private REST sequence handoff is not wired yet".to_string(),
            ));
        }

        #[cfg(feature = "prometheus")]
        let operation_label = envelope.operation.kind().to_string();
        let operation_id = envelope.operation_id()?;
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        if let Some(entry) = consensus.journal().entry_by_operation_id(operation_id) {
            return Ok(FederationSubmitOperationResponse::from_outcome(
                FederationJournalAppendOutcome::Duplicate(entry.clone()),
            ));
        }

        let mut next_consensus = consensus.clone();
        if let Err(err) = next_consensus.submit_aleph_bft_envelope(envelope) {
            #[cfg(feature = "prometheus")]
            self.record_federation_rejected_operation_metric(&operation_label, "consensus");
            return Err(cdk::Error::from(err));
        }
        *consensus = next_consensus;
        drop(consensus);
        #[cfg(feature = "prometheus")]
        self.record_federation_pending_mempool_metric().await;

        let wait_timeout = self
            .federation_peer_sync_config
            .unwrap_or_default()
            .operation_submission_timeout;
        tokio::time::timeout(
            wait_timeout,
            self.wait_for_federation_operation_acceptance(operation_id),
        )
        .await
        .map_err(|_| {
            cdk::Error::Custom(format!(
                "timed out waiting for AlephBFT finalization of federation operation {operation_id}"
            ))
        })?
        .ok_or_else(|| {
            cdk::Error::Custom(
                "federation operation notifications are unavailable for AlephBFT submit"
                    .to_string(),
            )
        })
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn apply_federation_aleph_bft_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        self.apply_federation_aleph_bft_finalized_unit_with_sequence(payload, None)
            .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn apply_federation_aleph_bft_finalized_unit_with_sequence(
        &self,
        payload: FederationAlephBftUnitPayload,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let finalized_item_start = next_consensus.aleph_bft_finalized_items()?.len();
        let responses = next_consensus.finalize_aleph_bft_unit(payload)?;
        let finalized_items =
            next_consensus.aleph_bft_finalized_items()?[finalized_item_start..].to_vec();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();

        self.persist_federation_aleph_bft_finalization(&finalized_items, &entries, sequence)
            .await?;
        *consensus = next_consensus;
        drop(consensus);

        self.apply_federation_journal_entries(&entries).await?;
        if let Some((sender, sequence)) = sequence {
            self.record_federation_sequence(sender, sequence).await;
        }
        for response in &responses {
            self.notify_federation_operation(response.clone()).await;
        }

        Ok(responses)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn drain_and_apply_local_federation_aleph_bft_finalized_unit(
        &self,
        expected: &FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let drained = next_consensus.drain_next_aleph_bft_unit_payload_matching(expected)?;
        let finalized_item_start = next_consensus.aleph_bft_finalized_items()?.len();
        let responses = next_consensus.finalize_aleph_bft_unit(drained)?;
        let finalized_items =
            next_consensus.aleph_bft_finalized_items()?[finalized_item_start..].to_vec();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();

        self.persist_federation_aleph_bft_finalization(&finalized_items, &entries, None)
            .await?;
        *consensus = next_consensus;
        drop(consensus);

        self.apply_federation_journal_entries(&entries).await?;
        for response in &responses {
            self.notify_federation_operation(response.clone()).await;
        }

        Ok(responses)
    }

    #[cfg_attr(not(test), allow(dead_code))]
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

    pub(crate) async fn apply_federation_catch_up_response_with_session_outcome_quorum(
        &self,
        response: &FederationJournalCatchUpResponse,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let public_config = self.federation_public_config()?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let response_to_apply = catch_up_response_suffix_for_local_journal(
            next_consensus.journal().entries(),
            response,
        )?;
        let applied = match response_to_apply.as_ref() {
            Some(response_to_apply) => {
                if response_to_apply.from_index != response.from_index
                    || response_to_apply.entries.len() != response.entries.len()
                {
                    return Err(cdk::Error::Federation(
                        FederationError::SessionOutcomeMismatch {
                            reason: "signed catch-up evidence does not cover the local suffix"
                                .to_string(),
                        },
                    ));
                }
                next_consensus
                    .apply_catch_up_response_with_session_outcome_quorum(
                        response_to_apply,
                        &public_config,
                        additional_outcomes,
                    )?
                    .applied_entries
            }
            None => Vec::new(),
        };

        let mut session_outcomes = Vec::new();
        if let Some(session_outcome) = response.session_outcome.as_ref() {
            session_outcomes.push(session_outcome);
        }
        session_outcomes.extend(additional_outcomes.iter());
        for session_outcome in &session_outcomes {
            let member =
                public_config
                    .member(session_outcome.signer)
                    .ok_or(cdk::Error::Federation(FederationError::UnknownMember {
                        member_id: session_outcome.signer,
                    }))?;
            session_outcome.verify(
                member,
                response.federation_id,
                response.from_index,
                &response.entries,
                response.latest_order_digest,
                response.state_digest,
            )?;
        }
        self.persist_federation_journal_entries_with_session_outcomes(
            &applied,
            &session_outcomes,
            None,
        )
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
        let expected_hash = self
            .expected_federation_dkg_transcript_hash(&public_config)
            .await?;
        if signature.transcript_hash != expected_hash {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "transcript signature hash {} does not match locally derived DKG transcript {}",
                        signature.transcript_hash,
                        expected_hash
                    ),
                },
            ));
        }

        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_transcript_signature(signature)
            .await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    async fn expected_federation_dkg_transcript_hash(
        &self,
        setup_public_config: &FederationConfig,
    ) -> Result<FederationDkgTranscriptHash, cdk::Error> {
        let runtime_config = self.mint.federation_config().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its runtime config".to_string())
        })?;
        let local_member = runtime_config.member_id();
        let keyset_shares = self
            .mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await?;
        let mut keysets_by_id = BTreeMap::new();
        for share in keyset_shares {
            if share.federation_id != setup_public_config.federation_id {
                return Err(cdk::Error::Federation(
                    FederationError::FederationIdMismatch {
                        expected: setup_public_config.federation_id,
                        found: share.federation_id,
                    },
                ));
            }
            if share.member_id != local_member {
                return Err(cdk::Error::Federation(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                }));
            }
            let keyset_id = share.keyset.id;
            let keyset = share.keyset;
            match keysets_by_id.insert(keyset_id, keyset.clone()) {
                Some(existing) if existing != keyset => {
                    return Err(cdk::Error::Federation(
                        FederationError::DkgTranscriptMismatch {
                            reason: format!("conflicting local DKG keyset {keyset_id}"),
                        },
                    ));
                }
                Some(_) | None => {}
            }
        }
        if keysets_by_id.is_empty() {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: "missing local DKG keyset share before transcript signature"
                        .to_string(),
                },
            ));
        }

        let mut final_public_config = setup_public_config.clone();
        final_public_config.keysets = keysets_by_id.into_values().collect();
        final_public_config.setup_transcript = None;
        final_public_config.validate()?;

        Ok(FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
        )?)
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
        let session_outcomes = session_outcome.into_iter().collect::<Vec<_>>();
        self.persist_federation_journal_entries_with_session_outcomes(
            entries,
            &session_outcomes,
            sequence,
        )
        .await
    }

    async fn persist_federation_journal_entries_with_session_outcomes(
        &self,
        entries: &[FederationJournalEntry],
        session_outcomes: &[&FederationSessionOutcome],
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        for session_outcome in session_outcomes {
            tx.add_federation_session_outcome(session_outcome).await?;
        }
        if let Some((sender, sequence)) = sequence {
            tx.set_federation_member_sequence(sender, sequence).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn persist_federation_aleph_bft_finalization(
        &self,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_aleph_bft_finalized_items(finalized_items)
            .await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
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

    pub(crate) async fn restore_federation_sync_status(&self) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let Some(config) = self.mint.federation_config() else {
            return;
        };

        match self
            .mint
            .localstore()
            .get_federation_member_status(config.member_id())
            .await
        {
            Ok(Some(snapshot)) => {
                match FederationSyncStatus::try_from_member_status_snapshot(snapshot) {
                    Ok(status) => sync.lock().await.status = status,
                    Err(err) => {
                        tracing::warn!("Could not restore persisted federation sync status: {err}");
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!("Could not load persisted federation sync status: {err}");
            }
        }
    }

    pub(crate) async fn restore_federation_startup_readiness(&self) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let Some(config) = self.mint.federation_config() else {
            return;
        };
        let current_status = sync.lock().await.status();
        match current_status {
            FederationSyncStatus::Healthy
            | FederationSyncStatus::VerifyingSessionOutcome { .. } => {}
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::MaterializedStateMismatch { .. }
            | FederationSyncStatus::Halted { .. } => return,
        }

        let entries = match self
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
        {
            Ok(entries) => entries,
            Err(err) => {
                self.mark_federation_halted(format!(
                    "could not inspect federation startup journal for quorum evidence: {err}"
                ))
                .await;
                return;
            }
        };
        let operation_count = entries.len() as u64;
        if operation_count == 0 {
            return;
        }

        let verified_operation_count =
            match crate::federation::session_outcome_quorum_operation_count(self).await {
                Ok(Some(count)) => count,
                Ok(None) => 0,
                Err(err) => {
                    self.mark_federation_halted(format!(
                        "could not inspect federation startup session outcome quorum: {err}"
                    ))
                    .await;
                    return;
                }
            };
        if verified_operation_count >= operation_count {
            self.mark_federation_healthy_if_session_outcome_verified(verified_operation_count)
                .await;
            return;
        }

        let peer = config
            .public_config()
            .members
            .values()
            .find(|member| member.member_id != config.member_id())
            .map(|member| member.member_id)
            .unwrap_or_else(|| config.member_id());
        self.mark_federation_verifying_session_outcome(peer, operation_count)
            .await;
    }

    async fn persist_federation_sync_status(&self, status: &FederationSyncStatus) {
        let Some(config) = self.mint.federation_config() else {
            return;
        };
        let snapshot =
            status.to_member_status_snapshot(config.member_id(), federation_status_unix_time());
        let db = self.mint.localstore();
        let mut tx = match db.begin_transaction().await {
            Ok(tx) => tx,
            Err(err) => {
                tracing::warn!("Could not begin federation sync status transaction: {err}");
                return;
            }
        };

        if let Err(err) = tx.set_federation_member_status(&snapshot).await {
            tracing::warn!("Could not persist federation sync status: {err}");
            let _ = tx.rollback().await;
            return;
        }

        if let Err(err) = tx.commit().await {
            tracing::warn!("Could not commit federation sync status transaction: {err}");
        }
    }

    async fn apply_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        let applied = self.mint.apply_federation_journal_entries(entries).await?;
        self.mark_federation_operations_applied(&applied).await?;
        if let Some(entry) = entries.last() {
            self.record_federation_state_digest_checkpoint(entry)
                .await?;
        }
        Ok(applied)
    }

    async fn record_federation_state_digest_checkpoint(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let entries = db.get_federation_journal_entries().await?;
        let Some(latest_entry) = entries.last() else {
            return Ok(());
        };
        if latest_entry.index != entry.index || latest_entry.operation_id != entry.operation_id {
            return Ok(());
        }
        let state_digest = crate::federation::materialized_state_digest(self, &entries)
            .await
            .map_err(|err| {
                cdk::Error::Custom(format!(
                    "could not compute federation state digest checkpoint: {err}"
                ))
            })?;
        let checkpoint = FederationStateDigestCheckpoint::new(latest_entry, state_digest);
        let retention = self
            .federation_peer_sync_config
            .unwrap_or_default()
            .state_digest_checkpoint_retention;
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_state_digest_checkpoint(&checkpoint)
            .await?;
        tx.prune_federation_state_digest_checkpoints(retention)
            .await?;
        tx.commit().await?;
        Ok(())
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
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| {
                sync.mark_lagging(peer, local_operation_count, peer_operation_count);
            })
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    pub(crate) async fn mark_federation_catching_up(&self, peer: MemberId) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| sync.mark_catching_up(peer))
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    pub(crate) async fn mark_federation_verifying_session_outcome(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| sync.mark_verifying_session_outcome(peer, operation_count))
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
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
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| sync.mark_materialized_state_mismatch(peer, operation_count))
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    pub(crate) async fn mark_federation_healthy_if_session_outcome_verified(
        &self,
        verified_operation_count: u64,
    ) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| {
                sync.mark_healthy_if_session_outcome_verified(verified_operation_count);
            })
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn mark_federation_healthy(&self) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| sync.mark_healthy())
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    pub(crate) async fn mark_federation_healthy_if_caught_up(&self, local_operation_count: u64) {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| match sync.status() {
                FederationSyncStatus::Lagging {
                    peer_operation_count,
                    ..
                } if peer_operation_count > local_operation_count => {}
                FederationSyncStatus::VerifyingSessionOutcome { .. } => {}
                FederationSyncStatus::Halted { .. } => {}
                _ => sync.mark_healthy(),
            })
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
        }
    }

    pub(crate) async fn mark_federation_halted<S>(&self, reason: S)
    where
        S: Into<String>,
    {
        let Some(sync) = &self.federation_sync else {
            return;
        };
        let status = {
            let mut sync = sync.lock().await;
            sync.update_status(|sync| sync.mark_halted(reason.into()))
        };
        if let Some(status) = status {
            self.persist_federation_sync_status(&status).await;
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
    state.restore_federation_sync_status().await;
    #[cfg(feature = "federation")]
    state.restore_federation_startup_readiness().await;
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
        custom_handlers::spawn_federation_mint_payment_observer_loop(state.clone());
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
    let snapshot = FederationConfigSnapshot::new(config.public_config().clone())?;
    let mut tx = db.begin_transaction().await?;
    tx.set_federation_config(&snapshot).await?;
    tx.commit().await?;

    let persisted_entries = db.get_federation_journal_entries().await?;
    let sequences = db.get_federation_member_sequences().await?;
    let mut consensus = match config.consensus_backend() {
        FederationConsensusBackendKind::InMemory => {
            FederationConsensusRuntime::from_entries_with_backend(
                config.consensus_backend(),
                config.federation_id(),
                config.member_id(),
                persisted_entries.clone(),
            )?
        }
        FederationConsensusBackendKind::AlephBft => {
            let finalized_items = db.get_federation_aleph_bft_finalized_items().await?;
            let member_ids = config.public_config().members.keys().copied();
            let consensus = FederationConsensusRuntime::aleph_bft_from_finalized_items(
                config.federation_id(),
                config.member_id(),
                0,
                member_ids,
                config.public_config().threshold_params.consensus_threshold,
                finalized_items,
            )?;
            ensure_persisted_journal_matches_aleph_bft_recovery(
                &persisted_entries,
                consensus.journal().entries(),
            )?;
            consensus
        }
    };
    let entries = consensus.journal().entries().to_vec();
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

#[cfg(feature = "federation")]
fn ensure_persisted_journal_matches_aleph_bft_recovery(
    persisted_entries: &[FederationJournalEntry],
    recovered_entries: &[FederationJournalEntry],
) -> Result<()> {
    if persisted_entries.is_empty() {
        return Ok(());
    }
    if persisted_entries.len() != recovered_entries.len() {
        return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
            reason: format!(
                "journal entry count mismatch: persisted={}, recovered={}",
                persisted_entries.len(),
                recovered_entries.len()
            ),
        }
        .into());
    }

    for (persisted, recovered) in persisted_entries.iter().zip(recovered_entries) {
        if persisted.index != recovered.index
            || persisted.operation_id != recovered.operation_id
            || persisted.envelope.canonical_bytes()? != recovered.envelope.canonical_bytes()?
            || persisted.previous_order_digest != recovered.previous_order_digest
            || persisted.order_digest != recovered.order_digest
            || persisted.consensus_evidence != recovered.consensus_evidence
        {
            return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                reason: format!(
                    "journal entry mismatch at accepted operation index {}",
                    persisted.index
                ),
            }
            .into());
        }
    }

    Ok(())
}
