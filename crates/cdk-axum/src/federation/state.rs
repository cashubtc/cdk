//! Federation state integration.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;

use cdk::federation::{
    redact_federation_operator_message, FederationConfig, FederationConsensusRuntime,
    FederationDkgFinalizedMemberConfig, FederationError, FederationInvalidOperationAlertSummary,
    FederationOperationId, FederationOperationStatus, FederationRestSequenceTracker,
    FederationSubmitOperationResponse, MemberId,
};

use sha2::{Digest, Sha256};

use crate::auth;

pub(crate) type FederationConsensusHandle = Arc<tokio::sync::Mutex<FederationConsensusRuntime>>;
pub(crate) type FederationSequenceHandle = Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;
pub(crate) type FederationSyncHandle = Arc<tokio::sync::Mutex<FederationSyncState>>;
pub(crate) type FederationNotificationHandle = Arc<FederationOperationNotifications>;
pub(crate) type FederationRestAdmissionHandle = Arc<tokio::sync::Semaphore>;
pub(crate) type FederationDkgMemberAdmissionHandle = Arc<FederationDkgAdmissionState>;
pub(crate) type FederationInvalidOperationAlertHandle =
    Arc<tokio::sync::Mutex<FederationInvalidOperationAlertTracker>>;
pub(crate) type FederationAlephBftNetworkInboundHandle =
    Arc<dyn crate::federation::CdkAlephBftNetworkInboundQueue>;
pub(crate) type FederationAlephBftLiveSessionTaskHandle =
    Arc<crate::federation::AlephBftSessionSupervisor>;
pub(crate) type FederationAlephBftNetworkInboundState =
    Arc<tokio::sync::RwLock<Option<FederationAlephBftNetworkInboundHandle>>>;
pub(crate) type FederationAlephBftLiveSessionState =
    Arc<tokio::sync::RwLock<Option<FederationAlephBftLiveSessionTaskHandle>>>;

pub(crate) const FEDERATION_APPLY_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(25);
pub(crate) const FEDERATION_APPLY_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);
/// Terminal elapsed budget for retrying recognized transient storage
/// contention during finalized application and backup persistence. When the
/// budget is exhausted the caller transitions to a bounded operator-visible
/// state instead of retrying forever; the exact item is retained for replay.
pub(crate) const FEDERATION_APPLY_RETRY_BUDGET: Duration = Duration::from_secs(30);

#[cfg(all(test, feature = "federation"))]
pub(crate) static FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn federation_apply_retry_budget() -> Duration {
    #[cfg(test)]
    {
        let millis = FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS
            .load(std::sync::atomic::Ordering::Relaxed);
        if millis > 0 {
            return Duration::from_millis(millis);
        }
    }
    FEDERATION_APPLY_RETRY_BUDGET
}

/// Bounded message-count budget for each AlephBFT runtime channel
/// (inbound/outbound network, finalized units, and essential/activity
/// events). Bounds memory growth independent of the bounded
/// pending-operation mempool.
pub(crate) const FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY: usize = 4_096;
/// Bounded total buffered byte budget for the inbound AlephBFT network
/// queue. Together with the per-message size cap and the count budget, this
/// keeps an authenticated flood from growing process memory without limit.
pub(crate) const FEDERATION_ALEPH_BFT_INBOUND_BYTE_BUDGET: usize = 256 * 1024 * 1024;

#[cfg(all(test, feature = "federation"))]
pub(crate) static FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_OVERRIDE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(all(test, feature = "federation"))]
pub(crate) fn federation_aleph_bft_channel_capacity() -> usize {
    #[cfg(test)]
    {
        let override_capacity = FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_OVERRIDE
            .load(std::sync::atomic::Ordering::Relaxed);
        if override_capacity > 0 {
            return usize::try_from(override_capacity).unwrap_or(usize::MAX);
        }
    }
    FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY
}

#[cfg(all(test, feature = "federation"))]
pub(crate) static FEDERATION_ALEPH_BFT_INBOUND_BYTE_BUDGET_OVERRIDE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn federation_aleph_bft_inbound_byte_budget() -> usize {
    #[cfg(test)]
    {
        let override_budget = FEDERATION_ALEPH_BFT_INBOUND_BYTE_BUDGET_OVERRIDE
            .load(std::sync::atomic::Ordering::Relaxed);
        if override_budget > 0 {
            return usize::try_from(override_budget).unwrap_or(usize::MAX);
        }
    }
    FEDERATION_ALEPH_BFT_INBOUND_BYTE_BUDGET
}

const FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD: u64 = 20;
const FEDERATION_INVALID_OPERATION_ALERT_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(60);
// The blocking closure owns the permit so cancellation cannot admit
// overlapping uncancellable DKG work.
static FEDERATION_DKG_CPU_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug)]
pub(crate) struct FederationDkgAdmissionState {
    pub(crate) members: BTreeMap<MemberId, Arc<FederationDkgMemberAdmission>>,
    pub(crate) finalized_config: tokio::sync::OnceCell<Arc<FederationDkgFinalizedMemberConfig>>,
}

impl FederationDkgAdmissionState {
    pub(crate) fn get(&self, member: &MemberId) -> Option<&Arc<FederationDkgMemberAdmission>> {
        self.members.get(member)
    }
}

#[derive(Debug)]
pub(crate) struct FederationDkgMemberAdmission {
    pub(crate) in_flight: Arc<tokio::sync::Semaphore>,
    pub(crate) rate_limit: Mutex<FederationDkgMemberRateLimit>,
}

#[derive(Debug)]
pub(crate) struct FederationDkgMemberRateLimit {
    pub(crate) capacity: usize,
    pub(crate) available: usize,
    pub(crate) refill_interval: Duration,
    pub(crate) last_refill: Instant,
}

impl FederationDkgMemberRateLimit {
    pub(crate) fn new(capacity: usize, refill_interval: Duration) -> Self {
        Self {
            capacity,
            available: capacity,
            refill_interval,
            last_refill: Instant::now(),
        }
    }

    pub(crate) fn try_acquire_at(&mut self, now: Instant) -> bool {
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

pub(crate) fn federation_dkg_member_admission(
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

pub(crate) async fn run_federation_dkg_cpu_task<T, E, F>(
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

pub(crate) async fn run_federation_dkg_cpu_task_with_admission<T, E, F>(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum FederationInvalidOperationSourceKind {
    AuthToken,
    ClientIp,
    Anonymous,
}

impl FederationInvalidOperationSourceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AuthToken => "auth_token",
            Self::ClientIp => "client_ip",
            Self::Anonymous => "anonymous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FederationInvalidOperationBucket {
    pub(crate) source: FederationInvalidOperationSourceKind,
    pub(crate) digest: [u8; 32],
}

#[derive(Debug, Clone)]
pub(crate) struct FederationInvalidOperationBucketState {
    pub(crate) window_started: std::time::Instant,
    pub(crate) count: u64,
    pub(crate) alerted: bool,
}

impl FederationInvalidOperationBucketState {
    pub(crate) fn new(now: std::time::Instant) -> Self {
        Self {
            window_started: now,
            count: 0,
            alerted: false,
        }
    }

    pub(crate) fn reset_if_expired(
        &mut self,
        now: std::time::Instant,
        window: std::time::Duration,
    ) {
        if now.duration_since(self.window_started) >= window {
            self.window_started = now;
            self.count = 0;
            self.alerted = false;
        }
    }

    pub(crate) fn is_alerting(&self, threshold: u64) -> bool {
        self.count >= threshold
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FederationInvalidOperationAlertEvent {
    pub(crate) source: FederationInvalidOperationSourceKind,
    pub(crate) count: u64,
    pub(crate) threshold: u64,
    pub(crate) window_secs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct FederationInvalidOperationAlertTracker {
    pub(crate) salt: [u8; 32],
    pub(crate) threshold: u64,
    pub(crate) window: std::time::Duration,
    pub(crate) buckets:
        HashMap<FederationInvalidOperationBucket, FederationInvalidOperationBucketState>,
}

impl Default for FederationInvalidOperationAlertTracker {
    fn default() -> Self {
        Self::new(
            FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD,
            FEDERATION_INVALID_OPERATION_ALERT_WINDOW,
        )
    }
}

impl FederationInvalidOperationAlertTracker {
    pub(crate) fn new(threshold: u64, window: std::time::Duration) -> Self {
        Self {
            salt: federation_invalid_operation_alert_salt(),
            threshold,
            window,
            buckets: HashMap::new(),
        }
    }

    pub(crate) fn record(
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

    pub(crate) fn summaries(
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

    pub(crate) fn bucket_for(
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

    pub(crate) fn prune_expired(&mut self, now: std::time::Instant) {
        let window = self.window;
        self.buckets
            .retain(|_, state| now.duration_since(state.window_started) < window);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FederationInvalidOperationSource {
    pub(crate) kind: FederationInvalidOperationSourceKind,
    pub(crate) material: Vec<u8>,
}

impl FederationInvalidOperationSource {
    pub(crate) fn auth_token(token_kind: &'static str, token: &[u8]) -> Self {
        let mut material = Vec::with_capacity(token_kind.len() + 1 + token.len());
        material.extend_from_slice(token_kind.as_bytes());
        material.push(0);
        material.extend_from_slice(token);
        Self {
            kind: FederationInvalidOperationSourceKind::AuthToken,
            material,
        }
    }

    pub(crate) fn client_ip(ip: std::net::IpAddr) -> Self {
        Self {
            kind: FederationInvalidOperationSourceKind::ClientIp,
            material: ip.to_string().into_bytes(),
        }
    }

    pub(crate) fn anonymous() -> Self {
        Self {
            kind: FederationInvalidOperationSourceKind::Anonymous,
            material: b"anonymous".to_vec(),
        }
    }

    pub(crate) fn from_request(req: &axum::http::Request<axum::body::Body>) -> Self {
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

pub(crate) fn federation_invalid_operation_alert_salt() -> [u8; 32] {
    let mut hasher = Sha256::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    hasher.update(now.to_be_bytes());
    hasher.finalize().into()
}

pub(crate) fn validate_federation_submit_response(
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

#[derive(Debug, Default)]
pub(crate) struct FederationOperationNotifications {
    pub(crate) inner: Mutex<FederationOperationNotificationState>,
    #[cfg(test)]
    pub(crate) waiter_count_changed: tokio::sync::Notify,
}

#[derive(Debug, Default)]
pub(crate) struct FederationOperationNotificationState {
    pub(crate) outcomes: HashMap<FederationOperationId, FederationSubmitOperationResponse>,
    pub(crate) outcome_order: VecDeque<FederationOperationId>,
    pub(crate) notifiers: HashMap<FederationOperationId, FederationOperationNotifier>,
    pub(crate) waiter_count: usize,
    pub(crate) response_cache_lookups: u64,
    pub(crate) response_cache_hits: u64,
    pub(crate) duplicate_submissions: u64,
    pub(crate) duplicate_pending_waiter_joins: u64,
}

impl FederationOperationNotificationState {
    pub(crate) fn record_response_cache_lookup(&mut self, hit: bool) {
        self.response_cache_lookups = self.response_cache_lookups.saturating_add(1);
        if hit {
            self.response_cache_hits = self.response_cache_hits.saturating_add(1);
        }
    }

    pub(crate) fn record_duplicate_submission(&mut self) {
        self.duplicate_submissions = self.duplicate_submissions.saturating_add(1);
    }

    pub(crate) fn record_duplicate_pending_waiter_join(&mut self) {
        self.record_duplicate_submission();
        self.duplicate_pending_waiter_joins = self.duplicate_pending_waiter_joins.saturating_add(1);
    }

    pub(crate) fn prune_outcomes(&mut self, response_cache_limit: usize) {
        while self.outcomes.len() > response_cache_limit {
            let Some(operation_id) = self.outcome_order.pop_front() else {
                break;
            };
            self.outcomes.remove(&operation_id);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct FederationOperationNotifier {
    pub(crate) notify: Arc<tokio::sync::Notify>,
    pub(crate) response: Option<FederationSubmitOperationResponse>,
    pub(crate) waiters: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FederationOperationNotificationStats {
    pub(crate) pending_response_waiters: usize,
    pub(crate) response_cache_items: usize,
    pub(crate) response_cache_lookups: u64,
    pub(crate) response_cache_hits: u64,
    pub(crate) duplicate_submissions: u64,
    pub(crate) duplicate_pending_waiter_joins: u64,
}

pub(crate) enum FederationOperationWaitRegistration<'a> {
    Outcome(Box<FederationSubmitOperationResponse>),
    Waiting(FederationOperationWaiter<'a>),
}

impl<'a> FederationOperationWaitRegistration<'a> {
    pub(crate) async fn wait(self) -> FederationSubmitOperationResponse {
        match self {
            Self::Outcome(response) => *response,
            Self::Waiting(waiter) => waiter.wait().await,
        }
    }
}

pub(crate) struct FederationOperationWaiter<'a> {
    pub(crate) notifications: &'a FederationOperationNotifications,
    pub(crate) operation_id: FederationOperationId,
    pub(crate) notified: Option<tokio::sync::futures::OwnedNotified>,
}

impl<'a> FederationOperationWaiter<'a> {
    pub(crate) async fn wait(mut self) -> FederationSubmitOperationResponse {
        if let Some(notified) = self.notified.take() {
            notified.await;
        }
        self.notifications
            .waiter_response(self.operation_id)
            .expect("notified federation operation has an accepted outcome")
    }
}

impl Drop for FederationOperationWaiter<'_> {
    fn drop(&mut self) {
        self.notifications.release_waiter(self.operation_id);
    }
}

impl FederationOperationNotifications {
    pub(crate) fn lock_inner(
        &self,
    ) -> std::sync::MutexGuard<'_, FederationOperationNotificationState> {
        self.inner
            .lock()
            .expect("federation operation notification state mutex poisoned")
    }

    pub(crate) fn notify(
        &self,
        response: FederationSubmitOperationResponse,
        response_cache_limit: usize,
    ) {
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

    pub(crate) fn register_waiter(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<FederationOperationWaitRegistration<'_>, cdk::Error> {
        let mut inner = self.lock_inner();
        if let Some(response) = inner.outcomes.get(&operation_id) {
            let response = response.clone();
            inner.record_response_cache_lookup(true);
            return Ok(FederationOperationWaitRegistration::Outcome(Box::new(
                response,
            )));
        }
        inner.record_response_cache_lookup(false);
        if let Some(response) = inner
            .notifiers
            .get(&operation_id)
            .and_then(|notifier| notifier.response.clone())
        {
            return Ok(FederationOperationWaitRegistration::Outcome(Box::new(
                response,
            )));
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

    pub(crate) fn release_waiter(&self, operation_id: FederationOperationId) {
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

    pub(crate) fn waiter_response(
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

    #[cfg(test)]
    pub(crate) fn pending_waiter_count(&self) -> usize {
        self.lock_inner().waiter_count
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_pending_waiter_count(&self, expected: usize) {
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

    #[cfg(test)]
    pub(crate) fn response_cache_count(&self) -> usize {
        self.lock_inner().outcomes.len()
    }

    pub(crate) fn record_duplicate_submission(&self) {
        self.lock_inner().record_duplicate_submission();
    }

    pub(crate) fn record_duplicate_pending_waiter_join(&self) {
        self.lock_inner().record_duplicate_pending_waiter_join();
    }

    pub(crate) fn stats(&self) -> FederationOperationNotificationStats {
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

    pub(crate) async fn wait_for(
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
pub(crate) fn catch_up_response_suffix_for_local_journal(
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
pub(crate) fn aleph_bft_finalized_item_suffix_for_local(
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
pub(crate) fn ensure_catch_up_entries_match_recovered_journal(
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

pub(crate) fn aleph_bft_catch_up_recovery_mismatch(reason: String) -> cdk::Error {
    cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum FederationSyncStatus {
    #[default]
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

#[derive(Debug, Clone, Default)]
pub(crate) struct FederationSyncState {
    pub(crate) status: FederationSyncStatus,
    #[cfg(test)]
    pub(crate) transition_stats: FederationSyncTransitionStats,
}

#[cfg(all(test, feature = "federation"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FederationSyncTransitionStats {
    pub(crate) lagging: u64,
    pub(crate) catching_up: u64,
}

impl FederationSyncState {
    pub(crate) fn for_aleph_bft() -> Self {
        Self {
            status: FederationSyncStatus::AwaitingQuorum { operation_count: 0 },
            #[cfg(test)]
            transition_stats: FederationSyncTransitionStats::default(),
        }
    }

    pub(crate) fn status(&self) -> FederationSyncStatus {
        self.status.clone()
    }

    #[cfg(test)]
    pub(crate) fn transition_stats(&self) -> FederationSyncTransitionStats {
        self.transition_stats
    }

    pub(crate) fn finish_startup_revalidation(
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

    pub(crate) fn update_status<F>(&mut self, update: F)
    where
        F: FnOnce(&mut Self),
    {
        update(self);
    }

    pub(crate) fn mark_lagging(
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

    pub(crate) fn mark_awaiting_quorum(&mut self, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::Halted { .. } => return,
            FederationSyncStatus::Healthy | FederationSyncStatus::AwaitingQuorum { .. } => {}
        }
        self.status = FederationSyncStatus::AwaitingQuorum { operation_count };
    }

    pub(crate) fn mark_catching_up(&mut self, peer: MemberId) {
        #[cfg(test)]
        {
            self.transition_stats.catching_up = self.transition_stats.catching_up.saturating_add(1);
        }
        self.status = FederationSyncStatus::CatchingUp { peer };
    }

    pub(crate) fn mark_healthy(&mut self) {
        self.status = FederationSyncStatus::Healthy;
    }

    pub(crate) fn mark_healthy_after_replay(&mut self) {
        if matches!(self.status, FederationSyncStatus::AwaitingQuorum { .. }) {
            self.status = FederationSyncStatus::Healthy;
        }
    }

    pub(crate) fn mark_halted(&mut self, reason: String) {
        let reason = redact_federation_operator_message(&reason);
        self.status = FederationSyncStatus::Halted { reason };
    }
}

#[cfg(all(test, feature = "federation-bdk"))]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn active_onchain_application_requires_local_watch_only_config() {
        let application_id = cdk::federation::FederationFrostApplicationId::from_bytes([7; 32])
            .expect("application id");
        let root = cdk::federation::FederationFrostRootPublicConfig {
            version: 2,
            federation_id: cdk::federation::FederationId::from_bytes([11; 32]),
            setup_authorization: cdk::federation::FederationSetupAuthorizationHash::from_bytes(
                [10; 32],
            ),
            ceremony_id: cdk::federation::FederationDkgCeremonyId::from_bytes([12; 32]),
            root_key_epoch: 1,
            dkg_protocol_version: 2,
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
        let wallet: cdk::federation::FederationFrostWalletConfig =
            serde_json::from_value(serde_json::json!({
                "version": 1,
                "root": root,
                "cdk_onchain_application": application,
                "extension_applications": {},
            }))
            .expect("test wallet config");
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
