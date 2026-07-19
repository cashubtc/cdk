use core::fmt;

use std::collections::{BTreeSet, VecDeque};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::Recipient;
use cdk::federation::FederationOperationId;
use cdk::federation::{FederationAlephBftNetworkMessageRequest, MemberConfigPublic, MemberId};
use cdk_http_client::{HttpError, Transport};
use futures::stream::{FuturesUnordered, StreamExt};
use parity_scale_codec::{Decode, Encode};
use tokio::sync::mpsc;

use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftInboundEnvelope, CdkAlephBftKeychain,
    CdkAlephBftNetworkData, CdkAlephBftNetworkHandle, CdkAlephBftOutboundMessage,
};
use super::peer_client::FederationRestPeerClient;

pub(crate) const CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT: usize = 4096;
const CDK_ALEPH_BFT_SEEN_CONSENSUS_ACTIVITY_LIMIT: usize = 16_384;
const CDK_ALEPH_BFT_NETWORK_QUORUM_COMPLETION_GRACE: Duration = Duration::from_millis(25);
const CDK_ALEPH_BFT_NETWORK_RETRY_ATTEMPT_BUDGET: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct CdkAlephBftSeenConsensusActivity {
    ids: BTreeSet<[u8; 32]>,
    limit_reported: bool,
}

impl CdkAlephBftSeenConsensusActivity {
    fn mark_new(&mut self, activity_ids: Vec<[u8; 32]>, limit: usize) -> (Vec<[u8; 32]>, bool) {
        let mut new_activity_ids = Vec::new();
        let mut report_limit = false;
        for activity_id in activity_ids {
            if self.ids.contains(&activity_id) {
                continue;
            }
            if self.ids.len() >= limit {
                if !self.limit_reported {
                    self.limit_reported = true;
                    report_limit = true;
                }
                continue;
            }
            self.ids.insert(activity_id);
            new_activity_ids.push(activity_id);
        }
        (new_activity_ids, report_limit)
    }
}

/// Classifies inbound AlephBFT messages that should wake active proposal cadence.
pub(crate) trait CdkAlephBftNetworkActivityMarker {
    /// Return stable IDs for consensus payloads carried by this message.
    fn consensus_activity_ids(&self) -> Vec<[u8; 32]>;
}

impl CdkAlephBftNetworkActivityMarker for CdkAlephBftNetworkData {
    fn consensus_activity_ids(&self) -> Vec<[u8; 32]> {
        self.included_data()
            .into_iter()
            .map(|data| data.payload().body_hash.to_bytes())
            .collect()
    }
}

/// Cloneable sink for authenticated inbound AlephBFT network messages.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) trait CdkAlephBftNetworkInboundQueue: fmt::Debug + Send + Sync {
    /// Decode and queue one authenticated private-REST network message.
    fn queue_request(
        &self,
        request: &FederationAlephBftNetworkMessageRequest,
    ) -> Result<usize, cdk::Error>;
}

/// Typed inbound handle for a channel-backed AlephBFT network adapter.
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkInboundHandle<D> {
    session_id: u64,
    incoming: mpsc::Sender<CdkAlephBftInboundEnvelope<D>>,
    buffered_inbound_bytes: Arc<std::sync::atomic::AtomicUsize>,
    activity_hint: CdkAlephBftActivityHint,
    seen_consensus_activity: Arc<Mutex<CdkAlephBftSeenConsensusActivity>>,
    _message: PhantomData<fn() -> D>,
}

impl<D> fmt::Debug for CdkAlephBftNetworkInboundHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftNetworkInboundHandle")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<D> CdkAlephBftNetworkInboundHandle<D> {
    /// Create an inbound queue handle for a channel-backed network session.
    pub(crate) fn new(session_id: u64, handle: &CdkAlephBftNetworkHandle<D>) -> Self {
        Self::with_activity_hint(session_id, handle, CdkAlephBftActivityHint::default())
    }

    /// Create an inbound queue handle with a shared AlephBFT activity hint.
    pub(crate) fn with_activity_hint(
        session_id: u64,
        handle: &CdkAlephBftNetworkHandle<D>,
        activity_hint: CdkAlephBftActivityHint,
    ) -> Self {
        Self {
            session_id,
            incoming: handle.incoming_sender(),
            buffered_inbound_bytes: handle.buffered_inbound_bytes(),
            activity_hint,
            seen_consensus_activity: Arc::new(Mutex::new(
                CdkAlephBftSeenConsensusActivity::default(),
            )),
            _message: PhantomData,
        }
    }

    fn mark_new_consensus_activity(&self, activity_ids: Vec<[u8; 32]>) {
        let (new_activity_ids, report_limit) = match self.seen_consensus_activity.lock() {
            Ok(mut seen) => {
                seen.mark_new(activity_ids, CDK_ALEPH_BFT_SEEN_CONSENSUS_ACTIVITY_LIMIT)
            }
            Err(_) => (Vec::new(), false),
        };
        if report_limit {
            tracing::warn!(
                session_id = self.session_id,
                limit = CDK_ALEPH_BFT_SEEN_CONSENSUS_ACTIVITY_LIMIT,
                "AlephBFT consensus-activity wakeup tracker reached its bound"
            );
        }
        if !new_activity_ids.is_empty() {
            self.activity_hint.mark_network_activity(new_activity_ids);
        }
    }
}

impl<D> CdkAlephBftNetworkInboundQueue for CdkAlephBftNetworkInboundHandle<D>
where
    D: CdkAlephBftNetworkActivityMarker + Decode + Send + Sync + 'static,
{
    fn queue_request(
        &self,
        request: &FederationAlephBftNetworkMessageRequest,
    ) -> Result<usize, cdk::Error> {
        if request.session_id != self.session_id {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBftUnitSessionMismatch {
                    expected: self.session_id,
                    found: request.session_id,
                },
            ));
        }
        let bytes = request.message_bytes().map_err(cdk::Error::from)?;
        let actual = bytes.len();
        request.ensure_admissible_size().map_err(cdk::Error::from)?;
        let encoded_prefix = request.encoded_message.chars().take(64).collect::<String>();
        let mut input = bytes.as_slice();
        let message = D::decode(&mut input).map_err(|err| {
            let reason = format!("{err:?}");
            tracing::warn!(
                session_id = self.session_id,
                encoded_bytes = actual,
                encoded_prefix = %encoded_prefix,
                "AlephBFT network message SCALE decode failed: {}",
                reason
            );
            cdk::Error::from(
                cdk::federation::FederationError::InvalidAlephBftNetworkMessage { reason },
            )
        })?;
        if !input.is_empty() {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::InvalidAlephBftNetworkMessage {
                    reason: "trailing bytes after SCALE network message".to_string(),
                },
            ));
        }
        let activity_ids = message.consensus_activity_ids();
        let byte_budget = crate::federation_aleph_bft_inbound_byte_budget();
        let buffered = self
            .buffered_inbound_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        if buffered.saturating_add(actual) > byte_budget {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBftBackpressure {
                    session_id: self.session_id,
                    detail: format!(
                        "inbound byte budget {byte_budget} exceeded (buffered={buffered}, incoming={actual})"
                    ),
                },
            ));
        }
        self.incoming
            .try_send(CdkAlephBftInboundEnvelope {
                data: message,
                byte_len: actual,
            })
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => {
                    cdk::Error::from(cdk::federation::FederationError::AlephBftBackpressure {
                        session_id: self.session_id,
                        detail: "inbound queue is at the bounded message-count budget".to_string(),
                    })
                }
                mpsc::error::TrySendError::Closed(_) => {
                    cdk::Error::Custom("AlephBFT network message receiver is closed".to_string())
                }
            })?;
        self.buffered_inbound_bytes
            .fetch_add(actual, std::sync::atomic::Ordering::Relaxed);
        self.mark_new_consensus_activity(activity_ids);

        Ok(actual)
    }
}

#[cfg(test)]
impl CdkAlephBftNetworkActivityMarker for u64 {
    fn consensus_activity_ids(&self) -> Vec<[u8; 32]> {
        let mut activity_id = [0u8; 32];
        activity_id[..8].copy_from_slice(&self.to_be_bytes());
        vec![activity_id]
    }
}

/// Bounded work policy for one AlephBFT network-pump tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkPumpConfig {
    /// AlephBFT session these opaque network messages belong to.
    pub session_id: u64,
    /// Maximum outbound AlephBFT messages to drain in one pump tick.
    pub max_messages_per_tick: usize,
    /// Maximum remote peer deliveries attempted for one outbound message.
    pub max_peer_deliveries_per_message: usize,
}

impl Default for CdkAlephBftNetworkPumpConfig {
    fn default() -> Self {
        Self {
            session_id: 0,
            max_messages_per_tick: 1,
            max_peer_deliveries_per_message: 64,
        }
    }
}

impl CdkAlephBftNetworkPumpConfig {
    pub(crate) fn validate(self) -> Result<(), cdk::Error> {
        if self.max_messages_per_tick == 0 {
            return Err(cdk::Error::Custom(
                "AlephBFT network pump max messages per tick must be greater than zero".to_string(),
            ));
        }
        if self.max_peer_deliveries_per_message == 0 {
            return Err(cdk::Error::Custom(
                "AlephBFT network pump max peer deliveries per message must be greater than zero"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

/// Summary of bounded AlephBFT network-pump work completed during one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkPumpTickOutcome {
    /// Number of outbound AlephBFT messages drained from the channel bridge.
    pub messages_drained: usize,
    /// Total encoded AlephBFT message bytes submitted to private REST.
    pub encoded_bytes: usize,
    /// Number of previously failed network messages retried in this tick.
    pub retry_messages_drained: usize,
    /// Remote members that accepted at least one message in this tick.
    pub remote_accepted_members: Vec<MemberId>,
    /// Remote members that failed at least one message delivery in this tick.
    pub remote_failed_members: Vec<MemberId>,
    /// Remote members whose failed delivery was retained for a later retry.
    pub remote_requeued_members: Vec<MemberId>,
    /// Remote members skipped by the bounded per-message delivery limit.
    pub remote_skipped_members: Vec<MemberId>,
    /// Retry messages waiting after this tick completes.
    pub retry_queue_len: usize,
    /// Oldest retry deliveries evicted by the bounded runtime queue.
    pub retry_deliveries_dropped: usize,
    /// True when the outbound channel was empty during this tick.
    pub reached_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryDelivery {
    request: FederationAlephBftNetworkMessageRequest,
    peers: Vec<MemberConfigPublic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryPushOutcome {
    member_ids: Vec<MemberId>,
    dropped_deliveries: usize,
}

/// In-memory retry queue for failed private REST AlephBFT network deliveries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkRetryQueue {
    pending: VecDeque<CdkAlephBftNetworkRetryDelivery>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftNetworkRetryQueue {
    /// Number of failed network messages waiting for retry.
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    /// True when no failed network messages are waiting for retry.
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn push_failed(
        &mut self,
        request: FederationAlephBftNetworkMessageRequest,
        peers: Vec<MemberConfigPublic>,
    ) -> CdkAlephBftNetworkRetryPushOutcome {
        let member_ids = peers.iter().map(|peer| peer.member_id).collect::<Vec<_>>();
        let mut outcome = CdkAlephBftNetworkRetryPushOutcome {
            member_ids,
            ..CdkAlephBftNetworkRetryPushOutcome::default()
        };
        if peers.is_empty() {
            return outcome;
        }

        while self.pending.len() >= CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT {
            let Some(dropped) = self.pending.pop_front() else {
                break;
            };
            outcome.dropped_deliveries = outcome.dropped_deliveries.saturating_add(1);
            tracing::warn!(
                dropped_members = dropped.peers.len(),
                retry_queue_limit = CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
                "AlephBFT network retry queue dropped oldest failed delivery"
            );
        }

        let delivery = CdkAlephBftNetworkRetryDelivery { request, peers };
        self.pending.push_back(delivery);

        outcome
    }
}

/// Drain outbound AlephBFT network messages and deliver them over signed private REST.
///
/// This one-shot pump is intentionally bounded and best-effort. A failed peer
/// delivery is recorded for operator visibility but does not make the whole tick
/// fail. Local recipients are fed back through the in-memory AlephBFT network
/// bridge and are never sent through private REST. Live sessions use
/// [`run_cdk_aleph_bft_network_pump_tick_with_retry_queue`] to retain failed
/// deliveries for later ticks.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_cdk_aleph_bft_network_pump_tick<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
) -> Result<CdkAlephBftNetworkPumpTickOutcome, cdk::Error>
where
    T: Transport,
    D: Clone + Encode + Send + 'static,
{
    config.validate()?;

    let mut outcome = CdkAlephBftNetworkPumpTickOutcome::default();
    for _ in 0..config.max_messages_per_tick {
        let Some(outbound) = handle.try_next_outgoing() else {
            outcome.reached_idle = true;
            break;
        };
        outcome.messages_drained = outcome.messages_drained.saturating_add(1);
        let delivery = cdk_aleph_bft_network_delivery(keychain, config, outbound)?;
        queue_local_aleph_bft_network_delivery(handle, delivery.local_message)?;
        let mut message_outcome = deliver_cdk_aleph_bft_network_request(
            client,
            config,
            &delivery.retry_delivery.request,
            &delivery.retry_delivery.peers,
            delivery.retry_delivery.peers.len(),
            None,
        )
        .await?;
        outcome.encoded_bytes = outcome
            .encoded_bytes
            .saturating_add(message_outcome.encoded_bytes);
        outcome
            .remote_accepted_members
            .append(&mut message_outcome.remote_accepted_members);
        outcome
            .remote_failed_members
            .append(&mut message_outcome.remote_failed_members);
        outcome
            .remote_skipped_members
            .append(&mut message_outcome.remote_skipped_members);
    }

    Ok(outcome)
}

/// Drain failed AlephBFT deliveries and new outbound messages with retry retention.
///
/// Successful new outbound messages stay on the fast path. Failed deliveries
/// remain only in the bounded queue for this live session.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_cdk_aleph_bft_network_pump_tick_with_retry_queue<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
    retry_queue: &mut CdkAlephBftNetworkRetryQueue,
) -> Result<CdkAlephBftNetworkPumpTickOutcome, cdk::Error>
where
    T: Transport,
    D: Clone + Encode + Send + 'static,
{
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound(
        handle,
        keychain,
        client,
        config,
        retry_queue,
        None,
    )
    .await
}

/// Drain failed AlephBFT deliveries and new outbound messages, starting with a
/// message that was already awaited by the live network task.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
    retry_queue: &mut CdkAlephBftNetworkRetryQueue,
    first_outbound: Option<CdkAlephBftOutboundMessage<D>>,
) -> Result<CdkAlephBftNetworkPumpTickOutcome, cdk::Error>
where
    T: Transport,
    D: Clone + Encode + Send + 'static,
{
    config.validate()?;

    let mut outcome = CdkAlephBftNetworkPumpTickOutcome::default();
    let mut first_outbound = first_outbound;
    for _ in 0..config.max_messages_per_tick {
        let outbound = match first_outbound.take() {
            Some(outbound) => Some(outbound),
            None => handle.try_next_outgoing(),
        };
        let Some(outbound) = outbound else {
            outcome.reached_idle = true;
            break;
        };
        outcome.messages_drained = outcome.messages_drained.saturating_add(1);
        let delivery = cdk_aleph_bft_network_delivery(keychain, config, outbound)?;
        queue_local_aleph_bft_network_delivery(handle, delivery.local_message)?;
        let mut message_outcome = deliver_cdk_aleph_bft_network_request(
            client,
            config,
            &delivery.retry_delivery.request,
            &delivery.retry_delivery.peers,
            keychain.remote_delivery_threshold(delivery.retry_delivery.peers.len()),
            None,
        )
        .await?;
        outcome.encoded_bytes = outcome
            .encoded_bytes
            .saturating_add(message_outcome.encoded_bytes);
        outcome
            .remote_accepted_members
            .append(&mut message_outcome.remote_accepted_members);
        outcome
            .remote_failed_members
            .append(&mut message_outcome.remote_failed_members);
        outcome
            .remote_skipped_members
            .append(&mut message_outcome.remote_skipped_members);
        let requeued =
            retry_queue.push_failed(message_outcome.request, message_outcome.remote_failed_peers);
        outcome.retry_deliveries_dropped = outcome
            .retry_deliveries_dropped
            .saturating_add(requeued.dropped_deliveries);
        outcome.remote_requeued_members.extend(requeued.member_ids);
    }

    // An idle bridge can spend the remainder of this tick on retry traffic.
    if outcome.messages_drained == 0 {
        let retry_messages_to_drain = retry_queue.pending.len().min(config.max_messages_per_tick);
        for _ in 0..retry_messages_to_drain {
            let Some(delivery) = retry_queue.pending.pop_front() else {
                break;
            };
            outcome.retry_messages_drained = outcome.retry_messages_drained.saturating_add(1);
            let mut retry_outcome = deliver_cdk_aleph_bft_network_request(
                client,
                config,
                &delivery.request,
                &delivery.peers,
                delivery.peers.len(),
                Some(CDK_ALEPH_BFT_NETWORK_RETRY_ATTEMPT_BUDGET),
            )
            .await?;
            outcome.encoded_bytes = outcome
                .encoded_bytes
                .saturating_add(retry_outcome.encoded_bytes);
            outcome
                .remote_accepted_members
                .append(&mut retry_outcome.remote_accepted_members);
            outcome
                .remote_failed_members
                .append(&mut retry_outcome.remote_failed_members);
            outcome
                .remote_skipped_members
                .append(&mut retry_outcome.remote_skipped_members);
            let requeued =
                retry_queue.push_failed(delivery.request, retry_outcome.remote_failed_peers);
            outcome.retry_deliveries_dropped = outcome
                .retry_deliveries_dropped
                .saturating_add(requeued.dropped_deliveries);
            outcome.remote_requeued_members.extend(requeued.member_ids);
        }
    }
    outcome.retry_queue_len = retry_queue.len();

    Ok(outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkMessageDeliveryOutcome {
    request: FederationAlephBftNetworkMessageRequest,
    encoded_bytes: usize,
    remote_accepted_members: Vec<MemberId>,
    remote_failed_members: Vec<MemberId>,
    remote_failed_peers: Vec<MemberConfigPublic>,
    remote_skipped_members: Vec<MemberId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkDelivery<D> {
    retry_delivery: CdkAlephBftNetworkRetryDelivery,
    local_message: Option<D>,
}

fn cdk_aleph_bft_network_delivery<D>(
    keychain: &CdkAlephBftKeychain,
    config: CdkAlephBftNetworkPumpConfig,
    outbound: CdkAlephBftOutboundMessage<D>,
) -> Result<CdkAlephBftNetworkDelivery<D>, cdk::Error>
where
    D: Clone + Encode,
{
    let encoded = outbound.data.encode();
    let request = FederationAlephBftNetworkMessageRequest::new(config.session_id, encoded);
    let peers = remote_members_for_recipient(keychain, &outbound.recipient)?;
    let local_message = match recipient_includes_local(keychain, &outbound.recipient)? {
        true => Some(outbound.data),
        false => None,
    };

    Ok(CdkAlephBftNetworkDelivery {
        retry_delivery: CdkAlephBftNetworkRetryDelivery { request, peers },
        local_message,
    })
}

fn queue_local_aleph_bft_network_delivery<D>(
    handle: &CdkAlephBftNetworkHandle<D>,
    local_message: Option<D>,
) -> Result<(), cdk::Error> {
    let Some(local_message) = local_message else {
        return Ok(());
    };

    handle
        .queue_incoming(local_message)
        .map_err(|_| cdk::Error::Custom("AlephBFT local network receiver is closed".to_string()))
}

async fn deliver_cdk_aleph_bft_network_request<T>(
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
    request: &FederationAlephBftNetworkMessageRequest,
    peers: &[MemberConfigPublic],
    minimum_remote_deliveries: usize,
    attempt_budget: Option<Duration>,
) -> Result<CdkAlephBftNetworkMessageDeliveryOutcome, cdk::Error>
where
    T: Transport,
{
    let encoded_bytes = request.ensure_admissible_size().map_err(cdk::Error::from)?;
    let mut outcome = CdkAlephBftNetworkMessageDeliveryOutcome {
        request: request.clone(),
        encoded_bytes,
        remote_accepted_members: Vec::new(),
        remote_failed_members: Vec::new(),
        remote_failed_peers: Vec::new(),
        remote_skipped_members: Vec::new(),
    };
    let mut attempted_members = BTreeSet::new();
    let mut attempted_peers = Vec::new();
    for peer in peers.iter() {
        if attempted_members.len() >= config.max_peer_deliveries_per_message {
            break;
        }
        if attempted_members.insert(peer.member_id) {
            attempted_peers.push(peer.clone());
        }
    }

    let minimum_remote_deliveries = match attempted_peers.is_empty() {
        true => 0,
        false => minimum_remote_deliveries.clamp(1, attempted_peers.len()),
    };
    let mut attempts = FuturesUnordered::new();
    for peer in attempted_peers.iter().cloned() {
        attempts.push(async move {
            let result = client
                .submit_aleph_bft_network_message(&peer, request)
                .await;
            (peer, result)
        });
    }
    let attempt_deadline = attempt_budget.map(|budget| tokio::time::Instant::now() + budget);
    let mut completion_deadline = None;
    let mut completed_members = BTreeSet::new();

    while !attempts.is_empty() {
        let deadline = match (attempt_deadline, completion_deadline) {
            (Some(attempt), Some(completion)) => Some(attempt.min(completion)),
            (Some(attempt), None) => Some(attempt),
            (None, Some(completion)) => Some(completion),
            (None, None) => None,
        };
        let next = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, attempts.next()).await {
                Ok(next) => next,
                Err(_) => break,
            },
            None => attempts.next().await,
        };
        let Some((peer, result)) = next else {
            break;
        };
        completed_members.insert(peer.member_id);
        match result {
            Ok(_) => {
                #[cfg(not(test))]
                if tracing::enabled!(
                    target: "cdk_axum::federation::aleph_bft_latency",
                    tracing::Level::TRACE
                ) {
                    crate::federation::aleph_bft_latency::record(
                        crate::federation::aleph_bft_latency::CdkAlephBftLatencyStage::NetworkMessageDelivered,
                        aleph_bft_network_request_operation_ids(request),
                    );
                }
                #[cfg(test)]
                crate::federation::aleph_bft_latency::record(
                    crate::federation::aleph_bft_latency::CdkAlephBftLatencyStage::NetworkMessageDelivered,
                    aleph_bft_network_request_operation_ids(request),
                );
                outcome.remote_accepted_members.push(peer.member_id);
            }
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    session_id = config.session_id,
                    encoded_bytes = outcome.encoded_bytes,
                    "AlephBFT network-message delivery failed: {}",
                    err
                );
                outcome.remote_failed_members.push(peer.member_id);
                if is_retryable_aleph_bft_network_delivery_error(&err) {
                    outcome.remote_failed_peers.push(peer.clone());
                } else {
                    tracing::warn!(
                        member_id = %peer.member_id,
                        session_id = config.session_id,
                        encoded_bytes = outcome.encoded_bytes,
                        "Dropping non-retryable AlephBFT network-message delivery failure"
                    );
                }
            }
        }

        if completion_deadline.is_none()
            && outcome.remote_accepted_members.len() >= minimum_remote_deliveries
            && !attempts.is_empty()
        {
            completion_deadline =
                Some(tokio::time::Instant::now() + CDK_ALEPH_BFT_NETWORK_QUORUM_COMPLETION_GRACE);
        }
    }

    for peer in attempted_peers
        .iter()
        .filter(|peer| !completed_members.contains(&peer.member_id))
    {
        outcome.remote_failed_peers.push(peer.clone());
    }

    outcome.remote_skipped_members = peers
        .iter()
        .filter(|peer| !attempted_members.contains(&peer.member_id))
        .map(|peer| peer.member_id)
        .collect();
    outcome.remote_accepted_members.sort();
    outcome.remote_failed_members.sort();
    outcome
        .remote_failed_peers
        .sort_by_key(|peer| peer.member_id);

    Ok(outcome)
}

fn is_retryable_aleph_bft_network_delivery_error(err: &anyhow::Error) -> bool {
    match err.downcast_ref::<HttpError>() {
        Some(HttpError::Status { status, .. }) if *status == 400 => false,
        _ => true,
    }
}

fn remote_members_for_recipient(
    keychain: &CdkAlephBftKeychain,
    recipient: &Recipient,
) -> Result<Vec<MemberConfigPublic>, cdk::Error> {
    match recipient {
        Recipient::Everyone => Ok(keychain.remote_members()),
        Recipient::Node(node) => {
            let Some(member) = keychain.member_config_for_node(*node) else {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT network recipient node {} is not in the federation roster",
                    node.0
                )));
            };
            if member.member_id == keychain.local_member() {
                return Ok(Vec::new());
            }

            Ok(vec![member.clone()])
        }
    }
}

fn recipient_includes_local(
    keychain: &CdkAlephBftKeychain,
    recipient: &Recipient,
) -> Result<bool, cdk::Error> {
    match recipient {
        Recipient::Everyone => Ok(true),
        Recipient::Node(node) => {
            let Some(member) = keychain.member_config_for_node(*node) else {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT network recipient node {} is not in the federation roster",
                    node.0
                )));
            };

            Ok(member.member_id == keychain.local_member())
        }
    }
}

fn aleph_bft_network_request_operation_ids(
    request: &FederationAlephBftNetworkMessageRequest,
) -> Vec<FederationOperationId> {
    let Ok(message_bytes) = request.message_bytes() else {
        return Vec::new();
    };
    let mut bytes = message_bytes.as_slice();
    match CdkAlephBftNetworkData::decode(&mut bytes) {
        Ok(message) => message
            .included_data()
            .iter()
            .flat_map(|data| data.payload().operation_ids.iter().copied())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use aleph_bft::Network;
    use parity_scale_codec::{Decode, Encode};

    use crate::federation::aleph_bft_runtime::CdkAlephBftNetwork;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Decode, Encode)]
    struct IdleNetworkMessage(u64);

    impl CdkAlephBftNetworkActivityMarker for IdleNetworkMessage {
        fn consensus_activity_ids(&self) -> Vec<[u8; 32]> {
            Vec::new()
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Decode, Encode)]
    struct ActiveNetworkMessage(u64);

    impl CdkAlephBftNetworkActivityMarker for ActiveNetworkMessage {
        fn consensus_activity_ids(&self) -> Vec<[u8; 32]> {
            let mut activity_id = [0u8; 32];
            activity_id[..8].copy_from_slice(&self.0.to_be_bytes());
            vec![activity_id]
        }
    }

    #[tokio::test]
    async fn inbound_idle_network_message_does_not_mark_activity_hint() {
        let (mut network, handle) = CdkAlephBftNetwork::<IdleNetworkMessage>::new_pair();
        let activity_hint = CdkAlephBftActivityHint::default();
        let inbound =
            CdkAlephBftNetworkInboundHandle::with_activity_hint(7, &handle, activity_hint.clone());
        let request =
            FederationAlephBftNetworkMessageRequest::new(7, IdleNetworkMessage(42).encode());

        let encoded_bytes = inbound.queue_request(&request).expect("queue request");

        assert_eq!(
            encoded_bytes,
            request.message_bytes().expect("message").len()
        );
        assert_eq!(network.next_event().await, Some(IdleNetworkMessage(42)));
        assert!(!activity_hint.is_active());
    }

    #[tokio::test]
    async fn inbound_replayed_consensus_data_does_not_refresh_activity_hint() {
        let (mut network, handle) = CdkAlephBftNetwork::<ActiveNetworkMessage>::new_pair();
        let activity_hint = CdkAlephBftActivityHint::default();
        let inbound =
            CdkAlephBftNetworkInboundHandle::with_activity_hint(7, &handle, activity_hint.clone());
        let first =
            FederationAlephBftNetworkMessageRequest::new(7, ActiveNetworkMessage(42).encode());

        inbound.queue_request(&first).expect("first request");
        assert_eq!(network.next_event().await, Some(ActiveNetworkMessage(42)));
        assert!(activity_hint.is_active());

        activity_hint.clear_network_activity();
        inbound.queue_request(&first).expect("replayed request");
        assert_eq!(network.next_event().await, Some(ActiveNetworkMessage(42)));
        assert!(!activity_hint.is_active());

        let second =
            FederationAlephBftNetworkMessageRequest::new(7, ActiveNetworkMessage(43).encode());
        inbound.queue_request(&second).expect("new request");
        assert_eq!(network.next_event().await, Some(ActiveNetworkMessage(43)));
        assert!(activity_hint.is_active());
    }

    #[test]
    fn seen_consensus_activity_stops_waking_after_its_bound() {
        let mut seen = CdkAlephBftSeenConsensusActivity::default();

        assert_eq!(seen.mark_new(vec![[1u8; 32]], 1), (vec![[1u8; 32]], false));
        assert_eq!(seen.mark_new(vec![[1u8; 32]], 1), (Vec::new(), false));
        assert_eq!(seen.mark_new(vec![[2u8; 32]], 1), (Vec::new(), true));
        assert_eq!(seen.mark_new(vec![[3u8; 32]], 1), (Vec::new(), false));
        assert_eq!(seen.ids.len(), 1);
    }
}
