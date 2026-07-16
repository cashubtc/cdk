//! Axum server for Mint

#![doc = include_str!("../README.md")]

#[cfg(feature = "federation")]
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
#[cfg(feature = "federation")]
use std::sync::Mutex;
#[cfg(feature = "federation")]
use std::time::{Duration, Instant};

use anyhow::Result;
use auth::create_auth_router;
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cache::HttpCache;
#[cfg(feature = "federation")]
use cdk::federation::{
    redact_federation_operator_message, FederationAddressAllocationScope,
    FederationAddressKeychain, FederationAlephBftFinalizedItem,
    FederationAlephBftFinalizedItemStatus, FederationAlephBftUnitPayload,
    FederationChainObservation, FederationCheckpoint, FederationCheckpointFrontier,
    FederationCheckpointQuorum, FederationConfig, FederationConfigSnapshot,
    FederationConsensusItem, FederationConsensusRecoveryBase, FederationConsensusRuntime,
    FederationConsensusRuntimeStatus, FederationDkgActivationRequest,
    FederationDkgFinalizedMemberConfig, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgTranscriptSignatureRequest, FederationError, FederationFrostApplicationId,
    FederationFrostDkgContext, FederationInvalidOperationAlertSummary,
    FederationJournalAppendOutcome, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationOnchainWalletStatus, FederationOperationEnvelope, FederationOperationId,
    FederationOperationKind, FederationOperationStatus, FederationRestMessageKind,
    FederationRestSequenceTracker, FederationSubmitOperationResponse, FederationSuffixVerifier,
    MemberId, VerifiedFederationCheckpointQuorum,
};
use cdk::mint::Mint;
#[cfg(feature = "federation")]
use cdk_bdk::{
    FederatedBdkLocalConfig, FederatedBdkWallet, FederatedWalletConfig, FederatedWalletScope,
};
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
pub use self::federation::{
    run_federation_dkg_driver, FederationDkgAutoStartConfig, FederationDkgDriverRunConfig,
    FederationDkgDriverRunOutcome, FederationDkgDriverTickOutcome,
    FederationDkgSecretShareTransportPolicy, FederationPeerSyncConfig,
};
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
type FederationRestAdmissionHandle = Arc<tokio::sync::Semaphore>;
#[cfg(feature = "federation")]
type FederationDkgMemberAdmissionHandle = Arc<FederationDkgAdmissionState>;
#[cfg(feature = "federation")]
type FederationInvalidOperationAlertHandle =
    Arc<tokio::sync::Mutex<FederationInvalidOperationAlertTracker>>;
#[cfg(feature = "federation")]
type FederationAlephBftNetworkInboundHandle =
    Arc<dyn self::federation::CdkAlephBftNetworkInboundQueue>;
#[cfg(feature = "federation")]
type FederationAlephBftLiveSessionTaskHandle = Arc<self::federation::AlephBftSessionSupervisor>;
#[cfg(feature = "federation")]
type FederationAlephBftNetworkInboundState =
    Arc<tokio::sync::RwLock<Option<FederationAlephBftNetworkInboundHandle>>>;
#[cfg(feature = "federation")]
type FederationAlephBftLiveSessionState =
    Arc<tokio::sync::RwLock<Option<FederationAlephBftLiveSessionTaskHandle>>>;

#[cfg(feature = "federation")]
pub(crate) const FEDERATION_APPLY_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(25);
#[cfg(feature = "federation")]
pub(crate) const FEDERATION_APPLY_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);

#[cfg(feature = "federation")]
const FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD: u64 = 20;
#[cfg(feature = "federation")]
const FEDERATION_INVALID_OPERATION_ALERT_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(60);
#[cfg(feature = "federation")]
// The blocking closure owns the permit so cancellation cannot admit
// overlapping uncancellable DKG work.
static FEDERATION_DKG_CPU_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[cfg(feature = "federation")]
#[derive(Debug)]
struct FederationDkgAdmissionState {
    members: BTreeMap<MemberId, Arc<FederationDkgMemberAdmission>>,
    finalized_config: tokio::sync::OnceCell<Arc<FederationDkgFinalizedMemberConfig>>,
}

#[cfg(feature = "federation")]
impl FederationDkgAdmissionState {
    fn get(&self, member: &MemberId) -> Option<&Arc<FederationDkgMemberAdmission>> {
        self.members.get(member)
    }
}

#[cfg(feature = "federation")]
#[derive(Debug)]
struct FederationDkgMemberAdmission {
    in_flight: Arc<tokio::sync::Semaphore>,
    rate_limit: Mutex<FederationDkgMemberRateLimit>,
}

#[cfg(feature = "federation")]
#[derive(Debug)]
struct FederationDkgMemberRateLimit {
    capacity: usize,
    available: usize,
    refill_interval: Duration,
    last_refill: Instant,
}

#[cfg(feature = "federation")]
impl FederationDkgMemberRateLimit {
    fn new(capacity: usize, refill_interval: Duration) -> Self {
        Self {
            capacity,
            available: capacity,
            refill_interval,
            last_refill: Instant::now(),
        }
    }

    fn try_acquire_at(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last_refill);
        let refill_count = elapsed.as_nanos() / self.refill_interval.as_nanos();
        if refill_count >= self.capacity as u128 {
            self.available = self.capacity;
            self.last_refill = now;
        } else if refill_count > 0 {
            let refill_count =
                usize::try_from(refill_count).expect("bounded DKG request refill count fits usize");
            self.available = self.capacity.min(self.available + refill_count);
            self.last_refill += self.refill_interval
                * u32::try_from(refill_count).expect("bounded DKG request refill count fits u32");
        }

        match self.available.checked_sub(1) {
            Some(remaining) => {
                self.available = remaining;
                true
            }
            None => false,
        }
    }
}

#[cfg(feature = "federation")]
fn federation_dkg_member_admission(
    public_config: &FederationConfig,
    member_request_burst: usize,
    member_request_refill_interval: Duration,
) -> FederationDkgMemberAdmissionHandle {
    Arc::new(FederationDkgAdmissionState {
        members: public_config
            .members
            .keys()
            .copied()
            .map(|member_id| {
                (
                    member_id,
                    Arc::new(FederationDkgMemberAdmission {
                        in_flight: Arc::new(tokio::sync::Semaphore::new(1)),
                        rate_limit: Mutex::new(FederationDkgMemberRateLimit::new(
                            member_request_burst,
                            member_request_refill_interval,
                        )),
                    }),
                )
            })
            .collect(),
        finalized_config: tokio::sync::OnceCell::new(),
    })
}

#[cfg(feature = "federation")]
async fn run_federation_dkg_cpu_task<T, E, F>(
    join_error: &'static str,
    task: F,
) -> Result<T, cdk::Error>
where
    T: Send + 'static,
    E: Into<cdk::Error> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    run_federation_dkg_cpu_task_with_admission(&FEDERATION_DKG_CPU_ADMISSION, join_error, task)
        .await
}

#[cfg(feature = "federation")]
async fn run_federation_dkg_cpu_task_with_admission<T, E, F>(
    admission: &'static tokio::sync::Semaphore,
    join_error: &'static str,
    task: F,
) -> Result<T, cdk::Error>
where
    T: Send + 'static,
    E: Into<cdk::Error> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    let permit = admission
        .acquire()
        .await
        .map_err(|_| cdk::Error::Custom("federation DKG CPU admission closed".to_string()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
    .map_err(|_| cdk::Error::Custom(join_error.to_string()))?
    .map_err(Into::into)
}

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

#[cfg(feature = "federation")]
fn validate_federation_submit_response(
    response: &FederationSubmitOperationResponse,
    operation_id: FederationOperationId,
) -> Result<(), cdk::Error> {
    response.validate(operation_id)?;
    if response.outcome.entry().status() == FederationOperationStatus::Rejected {
        if let Some(rejection) = response.outcome.entry().rejection().cloned() {
            return Err(cdk::Error::from(rejection));
        }
    }
    response.ensure_accepted()?;
    Ok(())
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotifications {
    inner: Mutex<FederationOperationNotificationState>,
    #[cfg(test)]
    waiter_count_changed: tokio::sync::Notify,
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotificationState {
    outcomes: HashMap<FederationOperationId, FederationSubmitOperationResponse>,
    outcome_order: VecDeque<FederationOperationId>,
    notifiers: HashMap<FederationOperationId, FederationOperationNotifier>,
    waiter_count: usize,
    response_cache_lookups: u64,
    response_cache_hits: u64,
    duplicate_submissions: u64,
    duplicate_pending_waiter_joins: u64,
}

#[cfg(feature = "federation")]
impl FederationOperationNotificationState {
    fn record_response_cache_lookup(&mut self, hit: bool) {
        self.response_cache_lookups = self.response_cache_lookups.saturating_add(1);
        if hit {
            self.response_cache_hits = self.response_cache_hits.saturating_add(1);
        }
    }

    fn record_duplicate_submission(&mut self) {
        self.duplicate_submissions = self.duplicate_submissions.saturating_add(1);
    }

    fn record_duplicate_pending_waiter_join(&mut self) {
        self.record_duplicate_submission();
        self.duplicate_pending_waiter_joins = self.duplicate_pending_waiter_joins.saturating_add(1);
    }

    fn prune_outcomes(&mut self, response_cache_limit: usize) {
        while self.outcomes.len() > response_cache_limit {
            let Some(operation_id) = self.outcome_order.pop_front() else {
                break;
            };
            self.outcomes.remove(&operation_id);
        }
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotifier {
    notify: Arc<tokio::sync::Notify>,
    response: Option<FederationSubmitOperationResponse>,
    waiters: usize,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FederationOperationNotificationStats {
    pub(crate) pending_response_waiters: usize,
    pub(crate) response_cache_items: usize,
    pub(crate) response_cache_lookups: u64,
    pub(crate) response_cache_hits: u64,
    pub(crate) duplicate_submissions: u64,
    pub(crate) duplicate_pending_waiter_joins: u64,
}

#[cfg(feature = "federation")]
enum FederationOperationWaitRegistration<'a> {
    Outcome(FederationSubmitOperationResponse),
    Waiting(FederationOperationWaiter<'a>),
}

#[cfg(feature = "federation")]
impl<'a> FederationOperationWaitRegistration<'a> {
    async fn wait(self) -> FederationSubmitOperationResponse {
        match self {
            Self::Outcome(response) => response,
            Self::Waiting(waiter) => waiter.wait().await,
        }
    }
}

#[cfg(feature = "federation")]
struct FederationOperationWaiter<'a> {
    notifications: &'a FederationOperationNotifications,
    operation_id: FederationOperationId,
    notified: Option<tokio::sync::futures::OwnedNotified>,
}

#[cfg(feature = "federation")]
impl<'a> FederationOperationWaiter<'a> {
    async fn wait(mut self) -> FederationSubmitOperationResponse {
        if let Some(notified) = self.notified.take() {
            notified.await;
        }
        self.notifications
            .waiter_response(self.operation_id)
            .expect("notified federation operation has an accepted outcome")
    }
}

#[cfg(feature = "federation")]
impl Drop for FederationOperationWaiter<'_> {
    fn drop(&mut self) {
        self.notifications.release_waiter(self.operation_id);
    }
}

#[cfg(feature = "federation")]
impl FederationOperationNotifications {
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, FederationOperationNotificationState> {
        self.inner
            .lock()
            .expect("federation operation notification state mutex poisoned")
    }

    fn notify(&self, response: FederationSubmitOperationResponse, response_cache_limit: usize) {
        let operation_id = response.operation_id;
        let mut inner = self.lock_inner();
        if !inner.outcomes.contains_key(&operation_id) {
            inner.outcome_order.push_back(operation_id);
        }
        inner.outcomes.insert(operation_id, response.clone());
        if let Some(notifier) = inner.notifiers.get_mut(&operation_id) {
            notifier.response = Some(response);
            notifier.notify.notify_waiters();
        }
        inner.prune_outcomes(response_cache_limit);
    }

    fn register_waiter(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<FederationOperationWaitRegistration<'_>, cdk::Error> {
        let mut inner = self.lock_inner();
        if let Some(response) = inner.outcomes.get(&operation_id) {
            let response = response.clone();
            inner.record_response_cache_lookup(true);
            return Ok(FederationOperationWaitRegistration::Outcome(response));
        }
        inner.record_response_cache_lookup(false);
        if let Some(response) = inner
            .notifiers
            .get(&operation_id)
            .and_then(|notifier| notifier.response.clone())
        {
            return Ok(FederationOperationWaitRegistration::Outcome(response));
        }
        if inner.waiter_count >= waiter_limit {
            return Err(cdk::Error::from(
                FederationError::ResponseWaiterLimitExceeded {
                    waiting: inner.waiter_count,
                    max: waiter_limit,
                },
            ));
        }
        let notified = {
            let notifier = inner.notifiers.entry(operation_id).or_default();
            notifier.waiters = notifier.waiters.saturating_add(1);
            notifier.notify.clone().notified_owned()
        };
        inner.waiter_count = inner.waiter_count.saturating_add(1);
        #[cfg(test)]
        self.waiter_count_changed.notify_waiters();
        Ok(FederationOperationWaitRegistration::Waiting(
            FederationOperationWaiter {
                notifications: self,
                operation_id,
                notified: Some(notified),
            },
        ))
    }

    fn release_waiter(&self, operation_id: FederationOperationId) {
        let mut inner = self.lock_inner();
        inner.waiter_count = inner.waiter_count.saturating_sub(1);
        let remove_notifier = match inner.notifiers.get_mut(&operation_id) {
            Some(notifier) => {
                notifier.waiters = notifier.waiters.saturating_sub(1);
                notifier.waiters == 0
            }
            None => false,
        };
        if remove_notifier {
            inner.notifiers.remove(&operation_id);
        }
        #[cfg(test)]
        self.waiter_count_changed.notify_waiters();
    }

    fn waiter_response(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<FederationSubmitOperationResponse> {
        let inner = self.lock_inner();
        inner
            .notifiers
            .get(&operation_id)
            .and_then(|notifier| notifier.response.clone())
            .or_else(|| inner.outcomes.get(&operation_id).cloned())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn pending_waiter_count(&self) -> usize {
        self.lock_inner().waiter_count
    }

    #[cfg(test)]
    async fn wait_for_pending_waiter_count(&self, expected: usize) {
        let wait = async {
            loop {
                let changed = self.waiter_count_changed.notified();
                if self.pending_waiter_count() == expected {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {expected} federation response waiters; found {}",
                    self.pending_waiter_count()
                )
            });
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn response_cache_count(&self) -> usize {
        self.lock_inner().outcomes.len()
    }

    fn record_duplicate_submission(&self) {
        self.lock_inner().record_duplicate_submission();
    }

    fn record_duplicate_pending_waiter_join(&self) {
        self.lock_inner().record_duplicate_pending_waiter_join();
    }

    fn stats(&self) -> FederationOperationNotificationStats {
        let inner = self.lock_inner();
        FederationOperationNotificationStats {
            pending_response_waiters: inner.waiter_count,
            response_cache_items: inner.outcomes.len(),
            response_cache_lookups: inner.response_cache_lookups,
            response_cache_hits: inner.response_cache_hits,
            duplicate_submissions: inner.duplicate_submissions,
            duplicate_pending_waiter_joins: inner.duplicate_pending_waiter_joins,
        }
    }

    #[cfg(test)]
    async fn wait_for(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        Ok(self
            .register_waiter(operation_id, waiter_limit)?
            .wait()
            .await)
    }
}

#[cfg(all(feature = "federation", test))]
fn catch_up_response_suffix_for_local_journal(
    local_entries: &[FederationJournalEntry],
    local_next_index: u64,
    response: &FederationJournalCatchUpResponse,
) -> Result<Option<FederationJournalCatchUpResponse>, cdk::Error> {
    if response.from_index >= local_next_index {
        return Ok(Some(response.clone()));
    }

    let local_base_index = local_entries
        .first()
        .map_or(local_next_index, |entry| entry.index);
    let mut suffix_start = 0usize;
    for entry in &response.entries {
        if entry.index >= local_next_index {
            break;
        }
        let offset = entry
            .index
            .checked_sub(local_base_index)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(|| {
                cdk::Error::Federation(FederationError::CatchUpIndexMismatch {
                    expected: local_base_index,
                    found: entry.index,
                })
            })?;
        let local_entry = local_entries.get(offset).ok_or_else(|| {
            cdk::Error::Federation(FederationError::CatchUpIndexMismatch {
                expected: entry.index,
                found: local_next_index,
            })
        })?;
        entry.ensure_matches_journal_entry(local_entry)?;
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

#[cfg(all(feature = "federation", test))]
fn aleph_bft_finalized_item_suffix_for_local(
    local_items: &[FederationAlephBftFinalizedItem],
    local_next_item_index: u64,
    response: &FederationJournalCatchUpResponse,
) -> Result<Vec<FederationAlephBftFinalizedItem>, cdk::Error> {
    let local_base_item_index = local_items
        .first()
        .map_or(local_next_item_index, |item| item.accepted_item_index);
    let mut suffix = Vec::new();
    for peer_item in &response.aleph_bft_finalized_items {
        if peer_item.accepted_item_index < local_next_item_index {
            let local_index = peer_item
                .accepted_item_index
                .checked_sub(local_base_item_index)
                .and_then(|index| usize::try_from(index).ok())
                .ok_or_else(|| {
                    aleph_bft_catch_up_recovery_mismatch(format!(
                        "finalized item {} precedes retained base {}",
                        peer_item.accepted_item_index, local_base_item_index
                    ))
                })?;
            let local_item = local_items.get(local_index).ok_or_else(|| {
                aleph_bft_catch_up_recovery_mismatch(format!(
                    "missing local finalized item {} for catch-up overlap",
                    peer_item.accepted_item_index
                ))
            })?;
            local_item.ensure_matches(peer_item)?;
        } else {
            suffix.push(peer_item.clone());
        }
    }

    if suffix.is_empty() && !response.aleph_bft_finalized_items_complete {
        return Err(aleph_bft_catch_up_recovery_mismatch(format!(
            "AlephBFT catch-up made no finalized-item progress at item index {local_next_item_index}"
        )));
    }

    Ok(suffix)
}

#[cfg(all(feature = "federation", test))]
fn ensure_catch_up_entries_match_recovered_journal(
    recovered_entries: &[FederationJournalEntry],
    response_to_apply: Option<&FederationJournalCatchUpResponse>,
) -> Result<(), cdk::Error> {
    match response_to_apply {
        Some(response) => {
            if response.entries.len() != recovered_entries.len() {
                return Err(aleph_bft_catch_up_recovery_mismatch(format!(
                    "finalized-item catch-up recovered {} journal entries but response carried {}",
                    recovered_entries.len(),
                    response.entries.len()
                )));
            }
            for (recovered_entry, response_entry) in
                recovered_entries.iter().zip(response.entries.iter())
            {
                response_entry.ensure_matches_journal_entry(recovered_entry)?;
            }
        }
        None => {
            if !recovered_entries.is_empty() {
                return Err(aleph_bft_catch_up_recovery_mismatch(format!(
                    "finalized-item catch-up recovered {} new journal entries without response entries",
                    recovered_entries.len()
                )));
            }
        }
    }

    Ok(())
}

#[cfg(feature = "federation")]
fn aleph_bft_catch_up_recovery_mismatch(reason: String) -> cdk::Error {
    cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FederationSyncStatus {
    Healthy,
    AwaitingQuorum {
        operation_count: u64,
    },
    Lagging {
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    },
    CatchingUp {
        peer: MemberId,
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
            Self::AwaitingQuorum { .. } => "awaiting_quorum",
            Self::Lagging { .. } => "lagging",
            Self::CatchingUp { .. } => "catching_up",
            Self::Halted { .. } => "halted",
        }
    }

    pub(crate) fn peer(&self) -> Option<MemberId> {
        match self {
            Self::Lagging { peer, .. } | Self::CatchingUp { peer } => Some(*peer),
            Self::Healthy | Self::AwaitingQuorum { .. } | Self::Halted { .. } => None,
        }
    }

    pub(crate) fn local_operation_count(&self) -> Option<u64> {
        match self {
            Self::Lagging {
                local_operation_count,
                ..
            } => Some(*local_operation_count),
            Self::Healthy
            | Self::AwaitingQuorum { .. }
            | Self::CatchingUp { .. }
            | Self::Halted { .. } => None,
        }
    }

    pub(crate) fn peer_operation_count(&self) -> Option<u64> {
        match self {
            Self::Lagging {
                peer_operation_count,
                ..
            } => Some(*peer_operation_count),
            Self::Healthy
            | Self::AwaitingQuorum { .. }
            | Self::CatchingUp { .. }
            | Self::Halted { .. } => None,
        }
    }

    pub(crate) fn operation_count(&self) -> Option<u64> {
        match self {
            Self::AwaitingQuorum { operation_count } => Some(*operation_count),
            Self::Healthy
            | Self::Lagging { .. }
            | Self::CatchingUp { .. }
            | Self::Halted { .. } => None,
        }
    }

    pub(crate) fn not_ready_message(&self) -> String {
        match self {
            Self::Healthy => "federation member is healthy".to_string(),
            Self::AwaitingQuorum { operation_count } => format!(
                "federation member is awaiting a quorum-compatible peer view: operation_count={operation_count}"
            ),
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
    #[cfg(test)]
    transition_stats: FederationSyncTransitionStats,
}

#[cfg(all(test, feature = "federation"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FederationSyncTransitionStats {
    pub(crate) lagging: u64,
    pub(crate) catching_up: u64,
}

#[cfg(feature = "federation")]
impl FederationSyncState {
    fn for_aleph_bft() -> Self {
        Self {
            status: FederationSyncStatus::AwaitingQuorum { operation_count: 0 },
            #[cfg(test)]
            transition_stats: FederationSyncTransitionStats::default(),
        }
    }

    fn status(&self) -> FederationSyncStatus {
        self.status.clone()
    }

    #[cfg(test)]
    fn transition_stats(&self) -> FederationSyncTransitionStats {
        self.transition_stats
    }

    fn finish_startup_revalidation(
        &mut self,
        expected: &FederationSyncStatus,
        next: FederationSyncStatus,
    ) {
        if &self.status != expected {
            return;
        }
        self.status = match next {
            FederationSyncStatus::Halted { reason } => FederationSyncStatus::Halted {
                reason: redact_federation_operator_message(&reason),
            },
            status => status,
        };
    }

    fn update_status<F>(&mut self, update: F)
    where
        F: FnOnce(&mut Self),
    {
        update(self);
    }

    fn mark_lagging(
        &mut self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        #[cfg(test)]
        {
            self.transition_stats.lagging = self.transition_stats.lagging.saturating_add(1);
        }
        self.status = FederationSyncStatus::Lagging {
            peer,
            local_operation_count,
            peer_operation_count,
        };
    }

    fn mark_awaiting_quorum(&mut self, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::Halted { .. } => return,
            FederationSyncStatus::Healthy | FederationSyncStatus::AwaitingQuorum { .. } => {}
        }
        self.status = FederationSyncStatus::AwaitingQuorum { operation_count };
    }

    fn mark_catching_up(&mut self, peer: MemberId) {
        #[cfg(test)]
        {
            self.transition_stats.catching_up = self.transition_stats.catching_up.saturating_add(1);
        }
        self.status = FederationSyncStatus::CatchingUp { peer };
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
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn active_onchain_application_requires_local_watch_only_config() {
        let application_id = cdk::federation::FederationFrostApplicationId::from_bytes([7; 32])
            .expect("application id");
        let root = cdk::federation::FederationFrostRootPublicConfig {
            version: 1,
            federation_id: cdk::federation::FederationId::from_bytes([11; 32]),
            ceremony_id: cdk::federation::FederationDkgCeremonyId::from_bytes([12; 32]),
            root_key_epoch: 1,
            dkg_protocol_version: 1,
            ciphersuite_version: 1,
            threshold: 2,
            participant_mapping: BTreeMap::new(),
            verification_shares: BTreeMap::new(),
            untweaked_group_key: [2; 33],
            transcript_binding: [3; 32],
            canonical_public_package: Vec::new(),
        };
        let application = cdk::federation::FederationFrostApplicationConfig {
            version: 1,
            application_id,
            application_key_epoch: 1,
            root_key_epoch: 1,
            derivation_version: 1,
            authorization_policy: [8; 32],
            threshold: 2,
            verification_shares: BTreeMap::new(),
            derived_group_key: [2; 33],
            chain_code: [9; 32],
            external_descriptor: "unused-invalid-external".to_string(),
            internal_descriptor: "unused-invalid-internal".to_string(),
            descriptor_commitment: [10; 32],
            network: cdk_common::bitcoin::Network::Regtest,
            confirmation_policy: cdk::federation::FederationOnchainConfirmationPolicy::new(1)
                .expect("confirmation policy"),
            fee_policy_version: 1,
        };
        let wallet = cdk::federation::FederationFrostWalletConfig {
            version: 1,
            root,
            cdk_onchain_application: application,
        };
        let federation_id = cdk::federation::FederationId::from_bytes([11; 32]);

        let error = initialize_accepted_federation_onchain_wallets(federation_id, &wallet, None)
            .expect_err("active application must require local BDK config");
        assert!(error.to_string().contains("member-local BDK"));
    }

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
    fn dkg_member_rate_limit_refills_without_exceeding_capacity() {
        let mut rate_limit = FederationDkgMemberRateLimit::new(2, Duration::from_millis(100));
        let start = rate_limit.last_refill;

        assert!(rate_limit.try_acquire_at(start));
        assert!(rate_limit.try_acquire_at(start));
        assert!(!rate_limit.try_acquire_at(start));
        assert!(!rate_limit.try_acquire_at(start + Duration::from_millis(99)));
        assert!(rate_limit.try_acquire_at(start + Duration::from_millis(100)));
        assert!(!rate_limit.try_acquire_at(start + Duration::from_millis(100)));
        assert!(rate_limit.try_acquire_at(start + Duration::from_secs(1)));
        assert!(rate_limit.try_acquire_at(start + Duration::from_secs(1)));
        assert!(!rate_limit.try_acquire_at(start + Duration::from_secs(1)));
    }

    #[cfg(feature = "federation")]
    fn test_mint_quote_envelope() -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            cdk::federation::FederationId::from_bytes([1; 32]),
            cdk::federation::FederationOperation::MintQuote(
                cdk::federation::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request_lookup_id: cdk::cdk_payment::PaymentIdentifier::CustomId(
                        "quote-1".to_string(),
                    ),
                    request: cdk::mint::MintQuoteRequest::Bolt11(
                        cdk::nuts::MintQuoteBolt11Request {
                            amount: cdk::Amount::from(1),
                            unit: cdk::nuts::CurrencyUnit::Sat,
                            description: None,
                            pubkey: None,
                        },
                    ),
                    response: cdk::mint::MintQuoteResponse::Bolt11(
                        cdk::nuts::MintQuoteBolt11Response {
                            quote: "quote-1".to_string(),
                            request: "invoice-quote-1".to_string(),
                            amount: Some(cdk::Amount::from(1)),
                            unit: Some(cdk::nuts::CurrencyUnit::Sat),
                            method: cdk::nuts::PaymentMethod::Known(
                                cdk::nuts::nut00::KnownMethod::Bolt11,
                            ),
                            amount_paid: cdk::Amount::ZERO,
                            amount_issued: cdk::Amount::ZERO,
                            updated_at: 0,
                            state: cdk::nuts::MintQuoteState::Unpaid,
                            expiry: None,
                            pubkey: None,
                        },
                    ),
                },
            ),
        )
    }

    #[cfg(feature = "federation")]
    #[test]
    fn federation_submit_validation_rejects_terminal_status_before_state_read() {
        let envelope = test_mint_quote_envelope();
        let operation_id = envelope.operation_id().expect("operation id");
        let mut journal = cdk::federation::InMemoryFederationJournal::new();
        let mut response = FederationSubmitOperationResponse::from_outcome(
            journal.append(envelope).expect("append"),
        );
        match &mut response.outcome {
            FederationJournalAppendOutcome::Appended(entry)
            | FederationJournalAppendOutcome::Duplicate(entry) => {
                entry.application =
                    cdk::federation::FederationApplicationState::ConsensusRejected {};
            }
        }

        assert!(matches!(
            validate_federation_submit_response(&response, operation_id),
            Err(cdk::Error::Federation(
                FederationError::RejectedOperation {
                    operation_id: found_operation_id,
                    status: FederationOperationStatus::Rejected,
                }
            )) if found_operation_id == operation_id
        ));
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
    federation: federation::FederationNodeRuntime,
}

#[cfg(feature = "federation")]
impl MintState {
    /// Validate one bounded local BDK observation against accepted allocation
    /// state, then submit it through normal federation consensus. Raw BDK scan
    /// results never mutate the canonical UTXO projection directly.
    pub async fn submit_federation_chain_observation(
        &self,
        observation: FederationChainObservation,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if let FederationChainObservation::Deposit(deposit) = &observation {
            let allocation = self
                .mint
                .federation_address_allocation(
                    deposit.allocation.scope(),
                    &deposit.allocation.quote,
                )
                .await?
                .ok_or_else(|| {
                    cdk::Error::Custom(
                        "federation chain observation has no accepted address allocation"
                            .to_string(),
                    )
                })?;
            deposit.allocation.validate_allocation(&allocation)?;
            let runtime = self.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let wallet_config = runtime
                .public_config()
                .frost_wallet
                .as_ref()
                .ok_or(cdk::Error::Internal)?;
            let expected_script = wallet_config.derive_cdk_onchain_script(
                runtime.public_config(),
                deposit.allocation.application_id,
                deposit.allocation.application_key_epoch,
                deposit.allocation.keychain,
                deposit.allocation.derivation_index,
            )?;
            if deposit.txout.script_pubkey != expected_script {
                return Err(cdk::Error::Federation(
                    FederationError::ChainObservationInvalid {
                        reason: "deposit script does not match accepted allocation".to_string(),
                    },
                ));
            }
            let local_wallet = self
                .federation
                .onchain_wallets()
                .iter()
                .find(|wallet| {
                    let scope = wallet.scope();
                    scope.application_id() == deposit.allocation.application_id.to_bytes()
                        && scope.application_key_epoch() == deposit.allocation.application_key_epoch
                })
                .ok_or_else(|| {
                    cdk::Error::Custom(
                        "federation chain observation has no local watch-only wallet".to_string(),
                    )
                })?;
            let required_height = deposit.confirmation.map_or(0, |confirmation| {
                confirmation
                    .block_height
                    .saturating_add(confirmation.confirmation_depth.saturating_sub(1))
            });
            local_wallet
                .ensure_ready(required_height)
                .await
                .map_err(|_| {
                    cdk::Error::Custom(
                        "local federation chain view is not ready for observation".to_string(),
                    )
                })?;
        }
        let envelope = self
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?
            .chain_observation_operation(observation)?;
        self.submit_federation_envelope(envelope).await
    }

    /// Allocate or recover one consensus-controlled address. No BDK reveal is
    /// attempted until the operation has a terminal consensus result and the
    /// accepted mapping is readable from materialized state.
    pub async fn allocate_federation_onchain_address(
        &self,
        quote: &str,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<String, cdk::Error> {
        if let Some(address) = self
            .reveal_accepted_federation_onchain_address(
                quote,
                application_id,
                application_key_epoch,
                keychain,
            )
            .await?
        {
            return Ok(address);
        }
        let envelope = self
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?
            .address_allocation_operation(quote, application_id, application_key_epoch, keychain)?;
        let response = self.submit_federation_envelope(envelope).await?;
        if let Some(address) = self
            .reveal_accepted_federation_onchain_address(
                quote,
                application_id,
                application_key_epoch,
                keychain,
            )
            .await?
        {
            return Ok(address);
        }
        response.ensure_accepted()?;
        Err(cdk::Error::Custom(
            "accepted federation address allocation is not materialized".to_string(),
        ))
    }

    async fn reveal_accepted_federation_onchain_address(
        &self,
        quote: &str,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
    ) -> Result<Option<String>, cdk::Error> {
        let scope = FederationAddressAllocationScope {
            application_id,
            application_key_epoch,
        };
        let allocation = match self
            .mint
            .federation_address_allocation(scope, quote)
            .await?
        {
            Some(allocation) if allocation.keychain == keychain => allocation,
            Some(_) => {
                return Err(cdk::Error::Federation(
                    FederationError::AddressAllocationConflict {
                        quote: quote.to_string(),
                    },
                ));
            }
            None => return Ok(None),
        };
        let wallet = self
            .federation
            .onchain_wallets()
            .iter()
            .find(|wallet| {
                let wallet_scope = wallet.scope();
                wallet_scope.application_id() == application_id.to_bytes()
                    && wallet_scope.application_key_epoch() == application_key_epoch
            })
            .ok_or_else(|| {
                cdk::Error::Custom(
                    "accepted federation address allocation has no local watch-only wallet"
                        .to_string(),
                )
            })?;
        let bdk_keychain = match keychain {
            FederationAddressKeychain::External => cdk_bdk::FederatedWalletKeychain::External,
            FederationAddressKeychain::Internal => cdk_bdk::FederatedWalletKeychain::Internal,
        };
        let address = wallet
            .reveal_address(bdk_keychain, allocation.derivation_index)
            .await
            .map_err(|_| {
                cdk::Error::Custom(
                    "accepted federation address allocation could not be revealed locally"
                        .to_string(),
                )
            })?;
        Ok(Some(address))
    }

    pub(crate) async fn federation_onchain_wallet_statuses(
        &self,
    ) -> Vec<FederationOnchainWalletStatus> {
        let mut statuses = Vec::with_capacity(self.federation.onchain_wallets().len());
        for wallet in self.federation.onchain_wallets() {
            let status = wallet.sync_status().await;
            let observation_source = wallet.observation_source();
            let correlated_source_warning = matches!(
                observation_source.scope,
                cdk_bdk::FederatedChainSourceScope::Shared
            );
            statuses.push(FederationOnchainWalletStatus {
                source: match status.source {
                    cdk_bdk::ChainSourceKind::Esplora => "esplora",
                    cdk_bdk::ChainSourceKind::BitcoinRpc => "bitcoin_rpc",
                }
                .to_string(),
                source_scope: if correlated_source_warning {
                    "shared"
                } else {
                    "independent"
                }
                .to_string(),
                source_id: observation_source.source_id.clone(),
                correlated_source_warning,
                health: match status.health {
                    cdk_bdk::FederatedWalletSyncHealth::NeverSynced => "never_synced",
                    cdk_bdk::FederatedWalletSyncHealth::Healthy => "healthy",
                    cdk_bdk::FederatedWalletSyncHealth::Degraded => "degraded",
                }
                .to_string(),
                local_tip_height: status.local_tip_height,
                local_tip_hash: status.local_tip_hash,
                last_success_unix: status.last_success_unix,
                consecutive_failures: status.consecutive_failures,
            });
        }
        statuses
    }

    pub(crate) async fn federation_pending_mempool_len(&self) -> usize {
        self.federation.pending_mempool_len().await
    }

    pub(crate) async fn federation_aleph_bft_status(
        &self,
    ) -> Option<FederationConsensusRuntimeStatus> {
        self.federation.aleph_bft_status().await
    }

    pub(crate) async fn federation_aleph_bft_live_session_active(&self) -> bool {
        self.federation.live_session_active().await
    }

    pub(crate) async fn federation_aleph_bft_network_inbound_active(&self) -> bool {
        self.federation.network_inbound_active().await
    }

    pub(crate) async fn mark_aleph_bft_peer_activity(
        &self,
        session_id: u64,
        operation_id: FederationOperationId,
    ) -> Result<(), cdk::Error> {
        self.federation
            .mark_peer_activity(session_id, operation_id)
            .await
    }

    pub(crate) async fn federation_invalid_operation_alert_summaries(
        &self,
    ) -> Vec<FederationInvalidOperationAlertSummary> {
        match &self.federation.invalid_operation_alerts {
            Some(alerts) => alerts.lock().await.summaries(std::time::Instant::now()),
            None => Vec::new(),
        }
    }

    async fn record_federation_invalid_operation_alert(
        &self,
        source: FederationInvalidOperationSource,
    ) {
        let Some(alerts) = &self.federation.invalid_operation_alerts else {
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

    pub(crate) async fn submit_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.validate_federation_envelope_admission(&envelope)
            .await?;
        let operation_id = envelope.operation_id()?;
        self.ensure_federation_ready_for_public_submit().await?;

        let response = self
            .submit_federation_envelope_to_consensus(envelope)
            .await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    pub(crate) async fn submit_federation_envelope_to_consensus(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.validate_federation_envelope_admission(&envelope)
            .await?;
        let operation_id = envelope.operation_id()?;
        let response = self.federation.submit_envelope(envelope).await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    pub(crate) async fn validate_federation_envelope_admission(
        &self,
        envelope: &FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        self.federation.validate_envelope_admission(envelope).await
    }

    pub(crate) async fn submit_federation_payment_observation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        match envelope.operation.kind() {
            FederationOperationKind::MintQuotePayment
            | FederationOperationKind::MeltQuotePayment => {}
            other => {
                return Err(cdk::Error::Custom(format!(
                    "federation payment observation expected payment operation, got {other}"
                )));
            }
        }
        let operation_id = envelope.operation_id()?;
        let response = self
            .submit_federation_envelope_to_consensus(envelope)
            .await?;
        validate_federation_submit_response(&response, operation_id)?;
        Ok(response)
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_federation_operation_acceptance(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<FederationSubmitOperationResponse> {
        self.wait_for_federation_operation_acceptance_with_limit(operation_id, usize::MAX)
            .await
            .ok()
            .flatten()
    }

    #[cfg(test)]
    async fn wait_for_federation_operation_acceptance_with_limit(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        self.federation
            .wait_for_operation(operation_id, waiter_limit)
            .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn federation_pending_response_waiter_count(&self) -> usize {
        self.federation.pending_response_waiter_count()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_federation_pending_response_waiter_count(&self, expected: usize) {
        self.federation
            .wait_for_pending_response_waiter_count(expected)
            .await;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn federation_response_cache_entry_count(&self) -> usize {
        self.federation.response_cache_entry_count()
    }

    pub(crate) fn federation_operation_notification_stats(
        &self,
    ) -> FederationOperationNotificationStats {
        self.federation.operation_notification_stats()
    }

    #[cfg(test)]
    async fn notify_federation_operation(&self, response: FederationSubmitOperationResponse) {
        self.federation.notify_operation(response);
    }

    #[cfg(test)]
    async fn federation_submit_responses_with_current_status(
        &self,
        responses: &[FederationSubmitOperationResponse],
    ) -> Vec<FederationSubmitOperationResponse> {
        self.federation
            .responses_with_current_status(responses)
            .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn apply_federation_aleph_bft_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        self.federation.apply_finalized_unit(payload).await
    }

    #[cfg(test)]
    pub(crate) async fn apply_federation_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        self.federation.apply_test_catch_up_response(response).await
    }

    pub(crate) async fn apply_federation_checkpoint_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        self.federation
            .apply_checkpoint_catch_up_response(response, quorum)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn persist_federation_dkg_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation.persist_dkg_sequence(sender, sequence).await
    }

    pub(crate) async fn persist_federation_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_commitment(commitment, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_reveal(reveal, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_round1(
        &self,
        request: &cdk::federation::FederationFrostDkgRound1Request,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_round1(request, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_round2(
        &self,
        record: &cdk::federation::FederationFrostDkgRound2Record,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_round2(record, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_frost_dkg_confirmation(
        &self,
        request: &cdk::federation::FederationFrostDkgConfirmationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_frost_dkg_confirmation(request, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_secret_share_contribution(contribution, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_transcript_signature(
        &self,
        signature: &FederationDkgTranscriptSignatureRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_transcript_signature(signature, sender, sequence)
            .await
    }

    pub(crate) async fn persist_federation_dkg_activation(
        &self,
        activation: &FederationDkgActivationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.federation
            .persist_dkg_activation(activation, sender, sequence)
            .await
    }

    pub(crate) fn ready_federation_dkg_finalized_member_config(
        &self,
    ) -> Option<Arc<FederationDkgFinalizedMemberConfig>> {
        self.federation.ready_dkg_finalized_member_config()
    }

    pub(crate) async fn remember_federation_dkg_finalized_member_config(
        &self,
        finalized: Arc<FederationDkgFinalizedMemberConfig>,
    ) -> Result<(), cdk::Error> {
        self.federation
            .remember_dkg_finalized_member_config(finalized)
            .await
    }

    async fn record_federation_sequence(
        &self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) {
        self.federation
            .record_sequence(kind, sender, sequence)
            .await;
    }

    pub(crate) async fn restore_federation_startup_readiness(&self) {
        self.federation.restore_startup_readiness().await;
    }

    pub(crate) async fn federation_sync_status(&self) -> Option<FederationSyncStatus> {
        self.federation.sync_status().await
    }

    #[cfg(test)]
    pub(crate) async fn federation_sync_transition_stats(
        &self,
    ) -> Option<FederationSyncTransitionStats> {
        let sync = self.federation.sync_handle()?;
        Some(sync.lock().await.transition_stats())
    }

    pub(crate) async fn mark_federation_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.federation
            .mark_lagging(peer, local_operation_count, peer_operation_count)
            .await;
    }

    pub(crate) async fn mark_federation_awaiting_quorum(&self, operation_count: u64) {
        self.federation.mark_awaiting_quorum(operation_count).await;
    }

    pub(crate) async fn mark_federation_catching_up(&self, peer: MemberId) {
        self.federation.mark_catching_up(peer).await;
    }

    pub(crate) async fn mark_federation_healthy(&self) {
        self.federation.mark_healthy().await;
    }

    pub(crate) async fn mark_federation_halted<S>(&self, reason: S)
    where
        S: Into<String>,
    {
        self.federation.mark_halted(reason.into()).await;
    }

    pub(crate) async fn ensure_federation_ready_for_public_submit(&self) -> Result<(), cdk::Error> {
        self.ensure_federation_ready_for_authoritative_read().await
    }

    pub(crate) async fn ensure_federation_ready_for_authoritative_read(
        &self,
    ) -> Result<(), cdk::Error> {
        self.federation.ensure_ready_for_authoritative_read().await
    }

    pub(crate) async fn audit_trusted_federation_checkpoint(
        &self,
    ) -> Result<cdk::federation::FederationCheckpointAuditResponse, cdk::Error> {
        let audit = self
            .federation
            .audit_trusted_checkpoint_materialized_state()
            .await;
        if matches!(
            &audit,
            Err(cdk::Error::Federation(
                cdk::federation::FederationError::MaterializedStateAuditMismatch { .. }
            ))
        ) {
            federation::halt_current_federation_aleph_bft_session(
                self,
                "failed an operator-requested trusted-checkpoint materialized-state audit",
            )
            .await;
        }
        audit
    }
}

#[cfg(feature = "federation")]
fn accepted_federation_journal_entries(
    entries: &[FederationJournalEntry],
) -> Vec<FederationJournalEntry> {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.status(),
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            )
        })
        .cloned()
        .collect()
}

#[cfg(all(feature = "federation", test))]
fn should_overlay_local_status_after_aleph_bft_recovery(status: FederationOperationStatus) -> bool {
    matches!(
        status,
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )
}

#[cfg(feature = "federation")]
#[cfg(test)]
fn stale_local_aleph_bft_entries_to_retry(
    local_entries: &[FederationJournalEntry],
    recovered_entries: &[FederationJournalEntry],
) -> Vec<FederationJournalEntry> {
    local_entries
        .iter()
        .zip(recovered_entries.iter())
        .filter_map(|(local_entry, recovered_entry)| {
            let finalized_entry_is_accepted = matches!(
                recovered_entry.status(),
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            );
            let local_entry_needs_retry = matches!(
                local_entry.status(),
                FederationOperationStatus::Accepted
                    | FederationOperationStatus::Applying
                    | FederationOperationStatus::Failed
            );
            (finalized_entry_is_accepted && local_entry_needs_retry)
                .then(|| recovered_entry.clone())
        })
        .collect()
}

#[cfg(feature = "federation")]
pub(crate) fn is_retryable_federation_apply_error(err: &cdk::Error) -> bool {
    match err {
        cdk::Error::ConcurrentUpdate => true,
        cdk::Error::Database(err) => is_retryable_federation_database_error(err),
        _ => false,
    }
}

#[cfg(feature = "federation")]
pub(crate) fn is_retryable_federation_database_error(err: &cdk::cdk_database::Error) -> bool {
    match err {
        cdk::cdk_database::Error::ConcurrentUpdate | cdk::cdk_database::Error::Locked => true,
        cdk::cdk_database::Error::Database(source) => {
            let detail = source.to_string().to_ascii_lowercase();
            detail.contains("database is locked")
                || detail.contains("database locked")
                || detail.contains("database is busy")
                || detail.contains("database busy")
                || detail.contains("databasebusy")
        }
        _ => false,
    }
}

#[cfg(feature = "federation")]
fn federation_submit_response_with_entry(
    response: &FederationSubmitOperationResponse,
    current_entry: FederationJournalEntry,
) -> FederationSubmitOperationResponse {
    let mut response = response.clone();
    match &mut response.outcome {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => {
            *entry = current_entry;
        }
    }
    response
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
        create_mint_router_with_custom_cache_inner(
            mint,
            cache,
            custom_methods,
            enable_info_page,
            true,
        )
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
    create_mint_router_with_custom_cache_and_federation_dkg(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        federation_peer_sync_config,
        None,
    )
    .await
}

/// Create mint [`Router`] with federation peer sync and optional DKG auto-start policy.
#[cfg(feature = "federation")]
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache_and_federation_dkg(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    federation_peer_sync_config: FederationPeerSyncConfig,
    federation_dkg_config: Option<FederationDkgAutoStartConfig>,
) -> Result<Router> {
    create_mint_router_with_custom_cache_and_federation_dkg_and_wallet(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        federation_peer_sync_config,
        federation_dkg_config,
        None,
    )
    .await
}

/// Create mint [`Router`] with federation DKG and member-local watch-only BDK policy.
#[cfg(feature = "federation")]
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache_and_federation_dkg_and_wallet(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    federation_peer_sync_config: FederationPeerSyncConfig,
    federation_dkg_config: Option<FederationDkgAutoStartConfig>,
    federation_bdk_config: Option<FederatedBdkLocalConfig>,
) -> Result<Router> {
    federation_peer_sync_config.validate()?;
    create_mint_router_with_custom_cache_inner(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        true,
        federation_peer_sync_config,
        federation_dkg_config,
        federation_bdk_config,
        #[cfg(test)]
        false,
    )
    .await
}

#[cfg(all(test, feature = "federation"))]
/// Builds member HTTP routes without starting cross-member synchronization.
///
/// Wallet route tests use independent in-memory member stores; AlephBFT networking and
/// convergence are exercised by the federation integration tests.
pub(crate) async fn create_mint_router_without_federation_peer_sync(
    mint: Arc<Mint>,
    custom_methods: Vec<String>,
) -> Result<Router> {
    create_mint_router_with_custom_cache_inner(
        mint,
        HttpCache::default(),
        custom_methods,
        false,
        false,
        FederationPeerSyncConfig::default(),
        None,
        None,
        true,
    )
    .await
}

#[allow(unused_mut, unused_variables)]
async fn create_mint_router_with_custom_cache_inner(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    spawn_federation_peer_sync: bool,
    #[cfg(feature = "federation")] federation_peer_sync_config: FederationPeerSyncConfig,
    #[cfg(feature = "federation")] federation_dkg_config: Option<FederationDkgAutoStartConfig>,
    #[cfg(feature = "federation")] federation_bdk_config: Option<FederatedBdkLocalConfig>,
    #[cfg(all(test, feature = "federation"))] federation_local_finality_test: bool,
) -> Result<Router> {
    #[cfg(all(feature = "federation", feature = "prometheus"))]
    let federation_startup_started_at = std::time::Instant::now();
    #[cfg(feature = "federation")]
    let (
        federation_consensus,
        federation_sequences,
        federation_onchain_wallets,
        federation_frost_signing,
    ) = create_federation_runtime_state_with_wallet(&mint, federation_bdk_config).await?;
    #[cfg(feature = "federation")]
    let federation_sync = federation_consensus
        .as_ref()
        .and_then(|_| mint.federation_config())
        .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::for_aleph_bft())));
    let mut state = MintState {
        mint: Arc::clone(&mint),
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        federation: federation::FederationNodeRuntime {
            mint: federation_consensus.as_ref().map(|_| Arc::clone(&mint)),
            consensus: federation_consensus.clone(),
            peer_sync_config: federation_consensus
                .as_ref()
                .map(|_| federation_peer_sync_config),
            sequences: federation_sequences,
            notifications: federation_consensus
                .as_ref()
                .map(|_| Arc::new(FederationOperationNotifications::default())),
            private_rest_admission: federation_consensus.as_ref().map(|_| {
                Arc::new(tokio::sync::Semaphore::new(
                    federation_peer_sync_config.private_rest_inbound_concurrency_limit,
                ))
            }),
            invalid_operation_alerts: federation_consensus.as_ref().map(|_| {
                Arc::new(tokio::sync::Mutex::new(
                    FederationInvalidOperationAlertTracker::default(),
                ))
            }),
            payment_observer: federation::FederationPaymentObserver::default(),
            transaction_broadcaster: federation::FederationTransactionBroadcaster::default(),
            wallet_liveness: federation::FederationWalletLivenessWorker::default(),
            components: federation::runtime_components::FederationRuntimeComponents::new(
                federation_sync,
                None,
                None,
                federation_onchain_wallets,
                federation_frost_signing,
            ),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(test)]
            local_finality_test: federation_local_finality_test,
            #[cfg(test)]
            manual_finality_test: false,
        },
    };
    #[cfg(all(test, feature = "federation"))]
    if state.federation.local_finality_test {
        state.federation.set_sync_handle(
            state
                .federation
                .consensus
                .as_ref()
                .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::default()))),
        );
    }
    #[cfg(feature = "federation")]
    let (federation_dkg_config, spawn_federation_dkg_driver) = match federation_dkg_config {
        Some(config) => {
            let runtime_config = state.mint.federation_config().ok_or_else(|| {
                anyhow::anyhow!(
                    "federation DKG auto-start requires a mint federation runtime config"
                )
            })?;
            let public_config = runtime_config.public_config();
            if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
                tracing::info!(
                    member_id = %runtime_config.member_id(),
                    ceremony_id = %config.ceremony_id,
                    "Federation DKG auto-start skipped because finalized key material is already active"
                );
                (None, false)
            } else {
                config.validate_for_public_config(public_config)?;
                (Some(config), true)
            }
        }
        None => (None, false),
    };
    #[cfg(feature = "federation")]
    {
        let dkg_member_admission = match federation_dkg_config.as_ref() {
            Some(config) => state.mint.federation_config().map(|runtime_config| {
                federation_dkg_member_admission(
                    runtime_config.public_config(),
                    config.driver.member_request_burst,
                    config.driver.member_request_refill_interval,
                )
            }),
            None => None,
        };
        state.federation.set_dkg_components(
            federation_dkg_config.clone().map(Arc::new),
            dkg_member_admission,
        );
    }
    #[cfg(feature = "federation")]
    let federation_application_ready = state
        .mint
        .federation_config()
        .is_some_and(|config| !config.public_config().keysets.is_empty());
    #[cfg(feature = "federation")]
    if state.federation.consensus.is_some() {
        state
            .federation
            .recover_scheduled_checkpoint_candidate_at_startup()
            .await?;
        state
            .federation
            .recover_recovery_checkpoint_candidate_at_startup()
            .await?;
    }
    #[cfg(feature = "federation")]
    if spawn_federation_peer_sync {
        state.restore_federation_startup_readiness().await;
    }
    #[cfg(all(feature = "federation", feature = "prometheus"))]
    if let Some(config) = state.mint.federation_config() {
        cdk_prometheus::METRICS.record_federation_checkpoint_duration(
            &config.member_id().to_string(),
            cdk_prometheus::FederationCheckpointOperation::StartupReadiness,
            federation_startup_started_at.elapsed().as_secs_f64(),
        );
    }
    #[cfg(feature = "federation")]
    let expose_federation_routes =
        state.federation.consensus.is_some() || federation_dkg_config.is_some();
    #[cfg(feature = "federation")]
    if spawn_federation_dkg_driver {
        let config = federation_dkg_config
            .expect("enabled federation DKG driver must retain its validated configuration");
        federation::spawn_federation_dkg_driver(state.clone(), federation_peer_sync_config, config);
    }

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
    if federation_application_ready && state.federation.consensus.is_some() {
        state
            .federation
            .payment_observer
            .clone()
            .spawn(state.clone());
        state
            .federation
            .transaction_broadcaster
            .clone()
            .spawn(state.clone());
        state
            .federation
            .wallet_liveness
            .clone()
            .spawn(state.clone());
        if spawn_federation_peer_sync {
            federation::spawn_federation_peer_sync_loop_with_config(
                state.clone(),
                federation_peer_sync_config,
            );
        }
    }
    #[cfg(feature = "federation")]
    if expose_federation_routes {
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
fn checkpoint_is_exact_startup_frontier(
    checkpoint: &FederationCheckpoint,
    frontier: &FederationCheckpointFrontier,
) -> std::result::Result<bool, cdk::Error> {
    if frontier.materialized_state_audit_mismatch {
        return Err(FederationError::CheckpointInvalid {
            reason: "materialized-state audit mismatch is latched".to_string(),
        }
        .into());
    }
    frontier.ensure_covers_checkpoint_position(checkpoint)?;
    frontier.ensure_covers_checkpoint(checkpoint)?;
    Ok(frontier.is_at_checkpoint_position(checkpoint))
}

#[cfg(feature = "federation")]
async fn historical_entries_referenced_by_finalized_items(
    mint: &Mint,
    finalized_items: &[FederationAlephBftFinalizedItem],
    checkpoint_journal_next_index: u64,
) -> std::result::Result<Vec<FederationJournalEntry>, cdk::Error> {
    let db = mint.localstore();
    let mut historical_entries = BTreeMap::new();
    for finalized_item in finalized_items {
        if finalized_item.status == FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
        {
            continue;
        }
        let FederationConsensusItem::Operation(envelope) = &finalized_item.item else {
            continue;
        };
        let operation_id = envelope.operation_id()?;
        if let Some(entry) = db
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
            .filter(|entry| entry.index < checkpoint_journal_next_index)
        {
            historical_entries.insert(entry.index, entry);
        }
        for conflict_key in envelope.operation.conflict_keys() {
            if let Some(entry) = db
                .get_federation_journal_entry_by_conflict_key(&conflict_key)
                .await?
                .filter(|entry| entry.index < checkpoint_journal_next_index)
            {
                historical_entries.insert(entry.index, entry);
            }
        }
    }
    Ok(historical_entries.into_values().collect())
}

#[cfg(feature = "federation")]
struct FederationCheckpointStartupVerifier<'a> {
    mint: &'a Mint,
    config: &'a FederationConfig,
    member_id: MemberId,
    active_session: u64,
    checkpoint: &'a VerifiedFederationCheckpointQuorum,
}

#[cfg(feature = "federation")]
impl FederationCheckpointStartupVerifier<'_> {
    async fn verify(
        &self,
        frontier: &FederationCheckpointFrontier,
        journal_entries: &[FederationJournalEntry],
        finalized_items: &[FederationAlephBftFinalizedItem],
    ) -> std::result::Result<cdk::federation::VerifiedFederationSuffix, cdk::Error> {
        let checkpoint_payload = self.checkpoint.checkpoint();
        let historical_entries = historical_entries_referenced_by_finalized_items(
            self.mint,
            finalized_items,
            checkpoint_payload.journal_next_index,
        )
        .await?;
        let mut verifier = FederationSuffixVerifier::new(
            self.config,
            self.member_id,
            self.active_session,
            FederationConsensusRecoveryBase::Checkpoint(self.checkpoint),
        )?;
        verifier.register_historical_entries(historical_entries)?;
        verifier.push_finalized_items(finalized_items)?;
        verifier.push_journal_entries(journal_entries)?;
        Ok(verifier.finish_for_durable_frontier(frontier)?)
    }

    async fn repair_application(
        &self,
        initial_frontier: &FederationCheckpointFrontier,
        initial_journal_entries: &[FederationJournalEntry],
        finalized_items: &[FederationAlephBftFinalizedItem],
        terminal_prefix_len: usize,
        consensus: &mut FederationConsensusRuntime,
    ) -> std::result::Result<usize, cdk::Error> {
        let checkpoint_payload = self.checkpoint.checkpoint();
        let entries_to_replay = &initial_journal_entries[terminal_prefix_len..];
        if !entries_to_replay.is_empty() {
            tracing::info!(
                trusted_checkpoint_journal_index = checkpoint_payload.journal_next_index,
                application_next_index = initial_frontier.application_next_index,
                journal_next_index = initial_frontier.journal_next_index,
                replay_entries = entries_to_replay.len(),
                "Repairing terminal application after trusted checkpoint startup"
            );
        } else {
            return Ok(0);
        }
        for entry in entries_to_replay {
            federation::apply_federation_journal_entry_for_replay(self.mint, entry).await?;
        }

        let db = self.mint.localstore();
        let repaired_frontier = db.get_federation_checkpoint_frontier().await?;
        if !repaired_frontier.ordered_history_matches(initial_frontier) {
            return Err(FederationError::CheckpointStartupSuffixMismatch {
                reason: "durable ordered frontier changed during startup application replay"
                    .to_string(),
            }
            .into());
        }
        let repaired_journal_entries = db
            .get_federation_journal_entries_from(checkpoint_payload.journal_next_index)
            .await?;
        let repaired_suffix = self
            .verify(
                &repaired_frontier,
                &repaired_journal_entries,
                finalized_items,
            )
            .await?;
        let repaired_terminal_prefix_len =
            usize::try_from(repaired_suffix.terminal_application_count()).map_err(|_| {
                FederationError::CheckpointStartupSuffixMismatch {
                    reason: "terminal application suffix length exceeds local address space"
                        .to_string(),
                }
            })?;
        if !repaired_frontier.is_fully_terminal()
            || repaired_terminal_prefix_len != repaired_journal_entries.len()
        {
            return Err(
                FederationError::CheckpointStartupApplicationReplayIncomplete {
                    expected: repaired_frontier.journal_next_index,
                    found: repaired_frontier.application_next_index,
                }
                .into(),
            );
        }
        *consensus = FederationConsensusRuntime::recover_verified_suffix(repaired_suffix);
        Ok(entries_to_replay.len())
    }
}

#[cfg(feature = "federation")]
async fn create_federation_runtime_state_with_wallet(
    mint: &Arc<Mint>,
    local_wallet_config: Option<FederatedBdkLocalConfig>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
    Vec<FederatedBdkWallet>,
    Option<federation::frost_signing::FederationFrostSigningRuntime>,
)> {
    let Some(config) = mint.federation_config() else {
        if local_wallet_config.is_some() {
            return Err(anyhow::anyhow!(
                "federated BDK configuration requires an active federation mint"
            ));
        }
        return Ok((None, None, Vec::new(), None));
    };
    let db = mint.localstore();
    validate_and_materialize_frost_wallet_config(mint).await?;
    let frost_signing = match local_wallet_config.as_ref() {
        Some(local) => {
            federation::frost_signing::FederationFrostSigningRuntime::open(
                mint,
                &local.storage_root,
            )
            .await?
        }
        None => None,
    };
    let onchain_wallets = initialize_federation_onchain_wallets(config, local_wallet_config)?;
    let snapshot = FederationConfigSnapshot::new(config.public_config().clone())?;
    let mut tx = db.begin_transaction().await?;
    tx.set_federation_config(&snapshot).await?;
    tx.commit().await?;

    let dkg_sequences = db.get_federation_dkg_member_sequences().await?;
    let active_session = db
        .get_federation_aleph_bft_active_session()
        .await?
        .unwrap_or(0);
    let frontier = db.get_federation_checkpoint_frontier().await?;
    let trusted_quorum = db.get_trusted_federation_checkpoint_quorum().await?;
    let checkpoint_consensus = match (frontier.trusted_checkpoint_id, trusted_quorum) {
        (None, None) => None,
        (Some(_), None) => {
            return Err(FederationError::CheckpointInvalid {
                reason: "trusted checkpoint metadata references a missing quorum".to_string(),
            }
            .into());
        }
        (None, Some(_)) => {
            return Err(FederationError::CheckpointInvalid {
                reason: "trusted checkpoint quorum is not bound to the durable frontier"
                    .to_string(),
            }
            .into());
        }
        (Some(trusted_checkpoint_id), Some(quorum)) => {
            let verified_quorum = federation::verify_checkpoint_quorum_with_metrics(
                &quorum,
                config.public_config(),
                config.member_id(),
            )?;
            let checkpoint = verified_quorum.checkpoint();
            if checkpoint.id()? != trusted_checkpoint_id {
                return Err(FederationError::CheckpointInvalid {
                    reason: "trusted checkpoint ID does not match its stored quorum".to_string(),
                }
                .into());
            }
            if active_session < checkpoint.aleph_bft_session {
                return Err(FederationError::CheckpointInvalid {
                    reason: "active AlephBFT session is behind the trusted checkpoint".to_string(),
                }
                .into());
            }
            let exact_checkpoint = checkpoint_is_exact_startup_frontier(checkpoint, &frontier)?;
            #[cfg(feature = "prometheus")]
            let tail_started_at = std::time::Instant::now();
            let (journal_entries, finalized_items) = if exact_checkpoint {
                (Vec::new(), Vec::new())
            } else {
                let journal_entries = db
                    .get_federation_journal_entries_from(checkpoint.journal_next_index)
                    .await?;
                let finalized_items = db
                    .get_federation_aleph_bft_finalized_items_from(
                        checkpoint.finalized_next_item_index,
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                {
                    let member = config.member_id().to_string();
                    let journal_rows = u64::try_from(journal_entries.len()).unwrap_or(u64::MAX);
                    let finalized_rows = u64::try_from(finalized_items.len()).unwrap_or(u64::MAX);
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::JournalRowsLoaded,
                        journal_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::FinalizedItemsLoaded,
                        finalized_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::StartupTailJournalRowsLoaded,
                        journal_rows,
                    );
                    cdk_prometheus::METRICS.record_federation_checkpoint_work(
                        &member,
                        cdk_prometheus::FederationCheckpointWork::StartupTailFinalizedItemsLoaded,
                        finalized_rows,
                    );
                }
                (journal_entries, finalized_items)
            };
            let startup_verifier = FederationCheckpointStartupVerifier {
                mint,
                config: config.public_config(),
                member_id: config.member_id(),
                active_session,
                checkpoint: &verified_quorum,
            };
            let verified_suffix = startup_verifier
                .verify(&frontier, &journal_entries, &finalized_items)
                .await?;
            let terminal_prefix_len = usize::try_from(verified_suffix.terminal_application_count())
                .map_err(|_| FederationError::CheckpointStartupSuffixMismatch {
                    reason: "terminal application suffix length exceeds local address space"
                        .to_string(),
                })?;
            let mut consensus =
                FederationConsensusRuntime::recover_verified_suffix(verified_suffix);
            if !exact_checkpoint {
                let replayed_entries = startup_verifier
                    .repair_application(
                        &frontier,
                        &journal_entries,
                        &finalized_items,
                        terminal_prefix_len,
                        &mut consensus,
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &config.member_id().to_string(),
                    cdk_prometheus::FederationCheckpointWork::StartupEntriesReplayed,
                    u64::try_from(replayed_entries).unwrap_or(u64::MAX),
                );
                #[cfg(not(feature = "prometheus"))]
                let _ = replayed_entries;
                #[cfg(feature = "prometheus")]
                cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                    &config.member_id().to_string(),
                    cdk_prometheus::FederationCheckpointOperation::StartupTail,
                    tail_started_at.elapsed().as_secs_f64(),
                );
            }
            Some(consensus)
        }
    };
    let used_checkpoint = checkpoint_consensus.is_some();
    let mut consensus = match checkpoint_consensus {
        Some(consensus) => consensus,
        None => {
            let persisted_entries = db.get_federation_journal_entries().await?;
            let finalized_items = db.get_federation_aleph_bft_finalized_items().await?;
            #[cfg(feature = "prometheus")]
            {
                let member = config.member_id().to_string();
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::JournalRowsLoaded,
                    u64::try_from(persisted_entries.len()).unwrap_or(u64::MAX),
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::FinalizedItemsLoaded,
                    u64::try_from(finalized_items.len()).unwrap_or(u64::MAX),
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::StartupEntriesReplayed,
                    u64::try_from(persisted_entries.len()).unwrap_or(u64::MAX),
                );
            }
            let recovered_session =
                latest_aleph_bft_finalized_session(&finalized_items).max(active_session);
            let mut verifier = FederationSuffixVerifier::new(
                config.public_config(),
                config.member_id(),
                recovered_session,
                FederationConsensusRecoveryBase::Genesis,
            )?;
            verifier.push_finalized_items(&finalized_items)?;
            verifier.push_journal_entries(&persisted_entries)?;
            FederationConsensusRuntime::recover_verified_suffix(
                verifier.finish_for_durable_frontier(&frontier)?,
            )
        }
    };
    let session_id = consensus.aleph_bft_status()?.session;
    let mut tx = db.begin_transaction().await?;
    tx.set_federation_aleph_bft_active_session(session_id)
        .await?;
    if used_checkpoint {
        tx.prune_federation_aleph_bft_backups_before(session_id)
            .await?;
    }
    tx.commit().await?;
    for pending in db
        .get_federation_aleph_bft_pending_operations(session_id)
        .await?
    {
        let operation_id = pending.envelope.operation_id()?;
        if operation_id != pending.operation_id {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT pending operation ID mismatch: stored {}, envelope {}",
                pending.operation_id, operation_id
            ))
            .into());
        }
        if consensus
            .journal()
            .entry_by_operation_id(pending.operation_id)
            .is_some()
            || db
                .get_federation_journal_entry_by_operation_id(pending.operation_id)
                .await?
                .is_some()
        {
            continue;
        }
        consensus.submit_aleph_bft_envelope(pending.envelope)?;
    }
    let entries = consensus.journal().entries().to_vec();
    if !used_checkpoint {
        let entries_to_apply = accepted_federation_journal_entries(&entries);
        let applied = mint
            .apply_federation_journal_entries(&entries_to_apply)
            .await?;
        for operation_id in applied {
            consensus.update_status(operation_id, FederationOperationStatus::Applied)?;
        }
    }
    reconcile_federation_onchain_wallets(mint, &onchain_wallets).await?;
    let sequences = FederationRestSequenceTracker::from_dkg_sequences(dkg_sequences);

    Ok((
        Some(Arc::new(tokio::sync::Mutex::new(consensus))),
        Some(Arc::new(tokio::sync::Mutex::new(sequences))),
        onchain_wallets,
        frost_signing,
    ))
}

#[cfg(feature = "federation")]
async fn reconcile_federation_onchain_wallets(
    mint: &Mint,
    wallets: &[FederatedBdkWallet],
) -> Result<()> {
    let address_state = mint.federation_address_allocation_state().await?;
    let transaction_state = mint.federation_transaction_state().await?;
    for wallet in wallets {
        let wallet_scope = wallet.scope();
        let application_id = cdk::federation::FederationFrostApplicationId::from_bytes(
            wallet_scope.application_id(),
        )?;
        let scope = cdk::federation::FederationAddressAllocationScope {
            application_id,
            application_key_epoch: wallet_scope.application_key_epoch(),
        };
        if let Some(materialized) = address_state.wallet(scope) {
            wallet
                .reconcile_revealed_counters(
                    materialized.external_next_index,
                    materialized.internal_next_index,
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "failed to reconcile accepted federated address allocation into local BDK state"
                    )
                })?;
        }
        for accepted in transaction_state
            .accepted_transactions()
            .values()
            .filter(|accepted| {
                accepted.proposal.scope.root_key_epoch == wallet_scope.root_key_epoch()
                    && accepted.proposal.scope.application_id.to_bytes()
                        == wallet_scope.application_id()
                    && accepted.proposal.scope.application_key_epoch
                        == wallet_scope.application_key_epoch()
            })
        {
            let Some(signed) = &accepted.signed else {
                continue;
            };
            signed.verify_signatures(&accepted.proposal)?;
            wallet
                .engine()
                .apply_accepted_transaction(
                    signed.transaction()?,
                    accepted.accepted_operation_index,
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "failed to reconcile an accepted federated transaction into local BDK state"
                    )
                })?;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "federation"))]
async fn create_federation_runtime_state(
    mint: &Arc<Mint>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
)> {
    let (consensus, sequences, _, _) =
        create_federation_runtime_state_with_wallet(mint, None).await?;
    Ok((consensus, sequences))
}

#[cfg(feature = "federation")]
fn initialize_federation_onchain_wallets(
    runtime: &cdk::mint::FederationRuntimeConfig,
    local_config: Option<FederatedBdkLocalConfig>,
) -> Result<Vec<FederatedBdkWallet>> {
    let public = runtime.public_config();
    let Some(wallet_config) = &public.frost_wallet else {
        if local_config.is_some() {
            return Err(anyhow::anyhow!(
                "federated BDK configuration requires accepted FROST wallet config"
            ));
        }
        return Ok(Vec::new());
    };
    initialize_accepted_federation_onchain_wallets(
        public.federation_id,
        wallet_config,
        local_config,
    )
}

#[cfg(feature = "federation")]
fn initialize_accepted_federation_onchain_wallets(
    federation_id: cdk::federation::FederationId,
    wallet_config: &cdk::federation::FederationFrostWalletConfig,
    local_config: Option<FederatedBdkLocalConfig>,
) -> Result<Vec<FederatedBdkWallet>> {
    let local_config = local_config.ok_or_else(|| {
        anyhow::anyhow!(
            "active federated on-chain application requires member-local BDK configuration"
        )
    })?;

    let application = &wallet_config.cdk_onchain_application;
    [application]
        .into_iter()
        .map(|application| {
            let scope = FederatedWalletScope::new(
                federation_id.to_bytes(),
                application.application_id.to_bytes(),
                application.root_key_epoch,
                application.application_key_epoch,
                application.descriptor_commitment,
            )?;
            let config = FederatedWalletConfig::new(
                scope,
                &application.external_descriptor,
                &application.internal_descriptor,
                application.network,
            )?;
            let wallet = FederatedBdkWallet::open_with_observation_source(
                config,
                local_config.chain_source.clone(),
                &local_config.storage_root,
                local_config.observation_source.clone(),
            )?;
            if matches!(
                wallet.observation_source().scope,
                cdk_bdk::FederatedChainSourceScope::Shared
            ) {
                tracing::warn!(
                    source = ?wallet.engine().chain_source_kind(),
                    source_id = %wallet.observation_source().source_id,
                    "Federated BDK chain source is intentionally shared; member quorum does not provide independent upstream observations"
                );
            }
            let sync_wallet = wallet.clone();
            let sync_interval = local_config.sync_interval;
            tokio::spawn(async move {
                const MAX_SYNC_RETRY_BACKOFF: std::time::Duration =
                    std::time::Duration::from_secs(300);
                let initial_backoff = sync_interval.max(std::time::Duration::from_secs(5));
                let mut retry_backoff = initial_backoff;
                let mut delay = std::time::Duration::ZERO;
                loop {
                    tokio::time::sleep(delay).await;
                    match sync_wallet.sync_once().await {
                        Ok(()) => {
                            delay = sync_interval;
                            retry_backoff = initial_backoff;
                        }
                        Err(_) => {
                            let status = sync_wallet.sync_status().await;
                            if status.consecutive_failures == 1
                                || status.consecutive_failures.is_power_of_two()
                            {
                                tracing::warn!(
                                    source = ?status.source,
                                    consecutive_failures = status.consecutive_failures,
                                    retry_delay_secs = retry_backoff.as_secs(),
                                    "Federated BDK synchronization did not complete; wallet remains unavailable for consensus-dependent actions"
                                );
                            }
                            delay = retry_backoff;
                            retry_backoff = (retry_backoff * 2).min(MAX_SYNC_RETRY_BACKOFF);
                        }
                    }
                }
            });
            Ok(wallet)
        })
        .collect()
}

#[cfg(feature = "federation")]
async fn validate_and_materialize_frost_wallet_config(mint: &Arc<Mint>) -> Result<()> {
    let runtime = mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public = runtime.public_config();
    let Some(wallet) = &public.frost_wallet else {
        return Ok(());
    };
    wallet.validate(public)?;

    let db = mint.localstore();
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_frost_root_public_config(&wallet.root)
        .await?;
    tx.add_federation_frost_application_config(&wallet.cdk_onchain_application)
        .await?;
    tx.commit().await?;

    let stored_roots = db.get_federation_frost_root_public_configs().await?;
    let stored_applications = db.get_federation_frost_application_configs().await?;
    if stored_roots.as_slice() != std::slice::from_ref(&wallet.root)
        || stored_applications.as_slice() != std::slice::from_ref(&wallet.cdk_onchain_application)
    {
        return Err(cdk::Error::Custom(
            "persisted federation FROST wallet configuration mismatch".to_string(),
        )
        .into());
    }

    let private_roots = db.get_federation_frost_root_private_configs().await?;
    let [local] = private_roots.as_slice() else {
        return Err(cdk::Error::Custom(
            "missing or unknown participant-local FROST root configuration".to_string(),
        )
        .into());
    };
    let root = &wallet.root;
    let context = FederationFrostDkgContext::new(public, root.ceremony_id, root.root_key_epoch)?;
    let storage_key = self::federation::frost_storage_key(runtime.private_config(), &context)?;
    local.open(public, root, runtime.member_id(), &storage_key)?;
    Ok(())
}

#[cfg(feature = "federation")]
fn latest_aleph_bft_finalized_session(finalized_items: &[FederationAlephBftFinalizedItem]) -> u64 {
    finalized_items
        .iter()
        .map(|item| item.session)
        .max()
        .unwrap_or(0)
}
