use std::collections::BTreeMap;

use cdk::federation::MemberId;
use tokio::time::{self, Duration, Instant, MissedTickBehavior};

use super::FEDERATION_DISABLE_PEER_SYNC_ENV_VAR;

const DEFAULT_FEDERATION_PEER_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_FEDERATION_PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_FEDERATION_OPERATION_SUBMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_FEDERATION_CATCH_UP_LIMIT: u16 = 100;
const DEFAULT_FEDERATION_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_FEDERATION_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(60);
const DEFAULT_FEDERATION_PRIVATE_REST_INBOUND_CONCURRENCY_LIMIT: usize = 128;
const DEFAULT_FEDERATION_PRIVATE_REST_OUTBOUND_CONCURRENCY_LIMIT: usize = 64;
const DEFAULT_FEDERATION_PENDING_MEMPOOL_ITEM_LIMIT: usize = 1024;
const DEFAULT_FEDERATION_RESPONSE_WAITER_LIMIT: usize = 1024;
const DEFAULT_FEDERATION_RESPONSE_CACHE_LIMIT: usize = 4096;
const DEFAULT_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK: usize = 1;

/// Invalid federation peer synchronization configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FederationPeerSyncConfigError {
    /// A required duration or limit is zero.
    #[error("federation {field} must be greater than zero")]
    ZeroValue {
        /// Human-readable name of the invalid configuration field.
        field: &'static str,
    },
    /// The maximum reconnect backoff is below its initial value.
    #[error(
        "federation reconnect max backoff must be greater than or equal to the initial backoff"
    )]
    ReconnectBackoffOrder,
    /// A session rotation limit was configured without a session round cap.
    #[error("federation AlephBFT live session rotation limit requires a max round")]
    RotationLimitRequiresMaxRound,
}

/// Private federation peer synchronization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationPeerSyncConfig {
    /// Interval between background private REST peer sync rounds.
    pub peer_sync_interval: Duration,
    /// Timeout for private peer status, catch-up, DKG, and evidence requests.
    pub peer_request_timeout: Duration,
    /// Timeout for forwarding a locally received operation to the primary member.
    pub operation_submission_timeout: Duration,
    /// Maximum number of accepted journal entries requested per catch-up page.
    pub catch_up_limit: u16,
    /// Initial reconnect backoff after a peer sync failure.
    pub reconnect_initial_backoff: Duration,
    /// Maximum reconnect backoff after repeated peer sync failures.
    pub reconnect_max_backoff: Duration,
    /// Maximum concurrent private REST requests accepted by this member.
    pub private_rest_inbound_concurrency_limit: usize,
    /// Maximum concurrent private REST requests sent by one peer client.
    pub private_rest_outbound_concurrency_limit: usize,
    /// Maximum number of locally queued consensus items awaiting finalization.
    pub pending_mempool_item_limit: usize,
    /// Maximum number of local requests waiting for accepted operation responses.
    pub response_waiter_limit: usize,
    /// Maximum number of accepted operation responses retained for idempotent local waiters.
    pub response_cache_limit: usize,
    /// Unit-creation interval while an AlephBFT session has pending application work.
    pub aleph_bft_active_unit_interval: Duration,
    /// Maximum fresh outbound AlephBFT messages drained by one network-pump invocation.
    pub aleph_bft_network_pump_max_messages_per_tick: usize,
    /// Optional AlephBFT live-session round cap used to force natural session rollover.
    pub aleph_bft_live_session_max_round: Option<u16>,
    /// Optional number of capped AlephBFT live-session rollovers before restoring the normal cap.
    pub aleph_bft_live_session_rotation_limit: Option<usize>,
}

impl Default for FederationPeerSyncConfig {
    fn default() -> Self {
        Self {
            peer_sync_interval: DEFAULT_FEDERATION_PEER_SYNC_INTERVAL,
            peer_request_timeout: DEFAULT_FEDERATION_PEER_REQUEST_TIMEOUT,
            operation_submission_timeout: DEFAULT_FEDERATION_OPERATION_SUBMISSION_TIMEOUT,
            catch_up_limit: DEFAULT_FEDERATION_CATCH_UP_LIMIT,
            reconnect_initial_backoff: DEFAULT_FEDERATION_RECONNECT_INITIAL_BACKOFF,
            reconnect_max_backoff: DEFAULT_FEDERATION_RECONNECT_MAX_BACKOFF,
            private_rest_inbound_concurrency_limit:
                DEFAULT_FEDERATION_PRIVATE_REST_INBOUND_CONCURRENCY_LIMIT,
            private_rest_outbound_concurrency_limit:
                DEFAULT_FEDERATION_PRIVATE_REST_OUTBOUND_CONCURRENCY_LIMIT,
            pending_mempool_item_limit: DEFAULT_FEDERATION_PENDING_MEMPOOL_ITEM_LIMIT,
            response_waiter_limit: DEFAULT_FEDERATION_RESPONSE_WAITER_LIMIT,
            response_cache_limit: DEFAULT_FEDERATION_RESPONSE_CACHE_LIMIT,
            aleph_bft_active_unit_interval: DEFAULT_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL,
            aleph_bft_network_pump_max_messages_per_tick:
                DEFAULT_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK,
            aleph_bft_live_session_max_round: None,
            aleph_bft_live_session_rotation_limit: None,
        }
    }
}

impl FederationPeerSyncConfig {
    /// Validate the synchronization policy before starting private peer sync.
    pub fn validate(&self) -> Result<(), FederationPeerSyncConfigError> {
        if self.peer_sync_interval.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "peer sync interval",
            });
        }
        if self.peer_request_timeout.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "peer request timeout",
            });
        }
        if self.operation_submission_timeout.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "operation submission timeout",
            });
        }
        if self.catch_up_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "catch-up limit",
            });
        }
        if self.reconnect_initial_backoff.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect initial backoff",
            });
        }
        if self.reconnect_max_backoff.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect max backoff",
            });
        }
        if self.reconnect_max_backoff < self.reconnect_initial_backoff {
            return Err(FederationPeerSyncConfigError::ReconnectBackoffOrder);
        }
        if self.private_rest_inbound_concurrency_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "private REST inbound concurrency limit",
            });
        }
        if self.private_rest_outbound_concurrency_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "private REST outbound concurrency limit",
            });
        }
        if self.pending_mempool_item_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "pending mempool item limit",
            });
        }
        if self.response_waiter_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "response waiter limit",
            });
        }
        if self.response_cache_limit == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "response cache limit",
            });
        }
        if self.aleph_bft_active_unit_interval.is_zero() {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT active unit interval",
            });
        }
        if self.aleph_bft_network_pump_max_messages_per_tick == 0 {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT network pump max messages per tick",
            });
        }
        if self.aleph_bft_live_session_max_round == Some(0) {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session max round",
            });
        }
        if self.aleph_bft_live_session_rotation_limit == Some(0) {
            return Err(FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session rotation limit",
            });
        }
        if self.aleph_bft_live_session_rotation_limit.is_some()
            && self.aleph_bft_live_session_max_round.is_none()
        {
            return Err(FederationPeerSyncConfigError::RotationLimitRequiresMaxRound);
        }

        Ok(())
    }
}

pub(super) fn federation_peer_sync_interval(
    sync_config: FederationPeerSyncConfig,
) -> time::Interval {
    let mut interval = time::interval(sync_config.peer_sync_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval
}

pub(super) fn federation_peer_sync_initial_delay(
    peer_sync_interval: Duration,
    member_rank: usize,
    member_count: usize,
) -> Duration {
    if member_count <= 1 || member_rank == 0 {
        return Duration::ZERO;
    }

    let interval_nanos = peer_sync_interval.as_nanos();
    let phase_nanos = interval_nanos
        .saturating_mul(member_rank as u128)
        .checked_div(member_count as u128)
        .unwrap_or_default();
    Duration::from_nanos(u64::try_from(phase_nanos).unwrap_or(u64::MAX))
}

pub(super) fn federation_peer_sync_disabled_by_env() -> bool {
    match std::env::var(FEDERATION_DISABLE_PEER_SYNC_ENV_VAR) {
        Ok(value) => federation_peer_sync_disabled_value(&value),
        Err(_) => false,
    }
}

pub(super) fn federation_peer_sync_disabled_value(value: &str) -> bool {
    matches!(
        value,
        "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" | "on" | "ON" | "On"
    )
}

#[derive(Debug, Clone, Copy)]
struct FederationPeerReconnectBackoffEntry {
    failures: u32,
    retry_after: Instant,
}

#[derive(Debug, Default)]
pub(super) struct FederationPeerReconnectBackoff {
    entries: BTreeMap<MemberId, FederationPeerReconnectBackoffEntry>,
}

impl FederationPeerReconnectBackoff {
    pub(super) fn should_attempt(&self, peer: MemberId, now: Instant) -> bool {
        self.entries
            .get(&peer)
            .map(|entry| now >= entry.retry_after)
            .unwrap_or(true)
    }

    pub(super) fn record_success(&mut self, peer: MemberId) {
        self.entries.remove(&peer);
    }

    pub(super) fn record_failure(
        &mut self,
        peer: MemberId,
        sync_config: FederationPeerSyncConfig,
        now: Instant,
    ) -> Duration {
        let entry =
            self.entries
                .entry(peer)
                .or_insert_with(|| FederationPeerReconnectBackoffEntry {
                    failures: 0,
                    retry_after: now,
                });
        let delay = federation_reconnect_backoff_delay(sync_config, entry.failures);
        entry.failures = entry.failures.saturating_add(1);
        entry.retry_after = now + delay;
        delay
    }
}

pub(super) fn federation_reconnect_backoff_delay(
    sync_config: FederationPeerSyncConfig,
    failure_count: u32,
) -> Duration {
    let mut delay = sync_config.reconnect_initial_backoff;
    for _ in 0..failure_count {
        match delay.checked_mul(2) {
            Some(next) if next < sync_config.reconnect_max_backoff => {
                delay = next;
            }
            _ => return sync_config.reconnect_max_backoff,
        }
    }
    delay.min(sync_config.reconnect_max_backoff)
}
