use core::fmt;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::Recipient;
use cdk::federation::{
    FederationAlephBftError, FederationAlephBftNetworkMessageBatchRequest,
    FederationAlephBftNetworkMessageRequest, FederationOperationId, MemberConfigPublic, MemberId,
    FEDERATION_MAX_ALEPH_BFT_NETWORK_BATCH_BYTES, FEDERATION_MAX_ALEPH_BFT_NETWORK_BATCH_MESSAGES,
    FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
};
use cdk_http_client::{HttpError, Transport};
#[cfg(any(test, feature = "test-utils"))]
use futures::future::join_all;
#[cfg(any(test, feature = "test-utils"))]
use futures::stream::{FuturesUnordered, StreamExt};
use parity_scale_codec::{Decode, Encode};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinSet;

use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftInboundEnvelope, CdkAlephBftKeychain,
    CdkAlephBftNetworkData, CdkAlephBftNetworkHandle, CdkAlephBftOutboundMessage,
};
use super::peer_client::FederationRestPeerClient;

pub const CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT: usize = 4096;
const CDK_ALEPH_BFT_PEER_OUTBOUND_QUEUE_LIMIT: usize = 1024;
const CDK_ALEPH_BFT_SEEN_CONSENSUS_ACTIVITY_LIMIT: usize = 16_384;
#[cfg(any(test, feature = "test-utils"))]
const CDK_ALEPH_BFT_NETWORK_QUORUM_COMPLETION_GRACE: Duration = Duration::from_millis(25);
const CDK_ALEPH_BFT_NETWORK_RETRY_ATTEMPT_BUDGET: Duration = Duration::from_secs(1);
const CDK_ALEPH_BFT_PEER_RETRY_INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const CDK_ALEPH_BFT_PEER_RETRY_MAX_BACKOFF: Duration = Duration::from_secs(1);

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
pub trait CdkAlephBftNetworkActivityMarker {
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
pub trait CdkAlephBftNetworkInboundQueue: fmt::Debug + Send + Sync {
    /// Decode and queue one authenticated private-REST network message.
    fn queue_request(
        &self,
        request: &FederationAlephBftNetworkMessageRequest,
    ) -> Result<usize, cdk::Error>;

    /// Decode and queue a batch of authenticated private-REST network messages.
    fn queue_batch_request(
        &self,
        request: &FederationAlephBftNetworkMessageBatchRequest,
    ) -> Result<(usize, usize), cdk::Error>;
}

/// Typed inbound handle for a channel-backed AlephBFT network adapter.
#[derive(Clone)]
pub struct CdkAlephBftNetworkInboundHandle<D> {
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

impl<D> CdkAlephBftNetworkInboundHandle<D> {
    /// Create an inbound queue handle for a channel-backed network session.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new(session_id: u64, handle: &CdkAlephBftNetworkHandle<D>) -> Self {
        Self::with_activity_hint(session_id, handle, CdkAlephBftActivityHint::default())
    }

    /// Create an inbound queue handle with a shared AlephBFT activity hint.
    pub fn with_activity_hint(
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
                cdk::federation::FederationError::AlephBft(
                    FederationAlephBftError::UnitSessionMismatch {
                        expected: self.session_id,
                        found: request.session_id,
                    },
                ),
            ));
        }
        let bytes = request.message_bytes().map_err(cdk::Error::from)?;
        let actual = bytes.len();
        request.ensure_admissible_size().map_err(cdk::Error::from)?;
        let encoded_prefix = request.encoded_message.chars().take(64).collect::<String>();
        let mut input = bytes.as_slice();
        let message = D::decode(&mut input).map_err(|err| {
            tracing::warn!(
                session_id = self.session_id,
                encoded_bytes = actual,
                encoded_prefix = %encoded_prefix,
                "AlephBFT network message SCALE decode failed: {err:?}"
            );
            cdk::Error::from(FederationAlephBftError::InvalidNetworkMessage {
                reason: "SCALE decode failed",
            })
        })?;
        if !input.is_empty() {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBft(
                    FederationAlephBftError::InvalidNetworkMessage {
                        reason: "trailing bytes after SCALE network message",
                    },
                ),
            ));
        }
        let activity_ids = message.consensus_activity_ids();
        let byte_budget = crate::federation_aleph_bft_inbound_byte_budget();
        let reservation = self.buffered_inbound_bytes.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |buffered| {
                buffered
                    .checked_add(actual)
                    .filter(|next| *next <= byte_budget)
            },
        );
        if let Err(buffered) = reservation {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBft(
                    FederationAlephBftError::InboundByteBudgetExceeded {
                        session_id: self.session_id,
                        byte_budget,
                        buffered,
                        incoming: actual,
                    },
                ),
            ));
        }
        let send_result = self.incoming.try_send(CdkAlephBftInboundEnvelope {
            data: message,
            byte_len: actual,
        });
        if let Err(err) = send_result {
            self.buffered_inbound_bytes
                .fetch_sub(actual, std::sync::atomic::Ordering::AcqRel);
            return Err(match err {
                mpsc::error::TrySendError::Full(_) => {
                    cdk::Error::from(cdk::federation::FederationError::AlephBft(
                        FederationAlephBftError::Backpressure {
                            session_id: self.session_id,
                            detail: "inbound queue is at the bounded message-count budget",
                        },
                    ))
                }
                mpsc::error::TrySendError::Closed(_) => {
                    cdk::Error::Custom("AlephBFT network message receiver is closed".to_string())
                }
            });
        }
        self.mark_new_consensus_activity(activity_ids);

        Ok(actual)
    }

    fn queue_batch_request(
        &self,
        request: &FederationAlephBftNetworkMessageBatchRequest,
    ) -> Result<(usize, usize), cdk::Error> {
        if request.session_id != self.session_id {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBft(
                    FederationAlephBftError::UnitSessionMismatch {
                        expected: self.session_id,
                        found: request.session_id,
                    },
                ),
            ));
        }

        let decoded_raw = request.decoded_messages().map_err(cdk::Error::from)?;
        let mut decoded = Vec::with_capacity(decoded_raw.len());
        let mut activity_ids = Vec::new();
        let mut total_bytes = 0usize;
        for (index, bytes) in decoded_raw.into_iter().enumerate() {
            let actual = bytes.len();
            let mut input = bytes.as_slice();
            let message = D::decode(&mut input).map_err(|err| {
                let encoded_prefix = request
                    .encoded_messages
                    .get(index)
                    .map(|encoded| encoded.chars().take(64).collect::<String>())
                    .unwrap_or_default();
                tracing::warn!(
                    session_id = self.session_id,
                    batch_index = index,
                    encoded_bytes = actual,
                    encoded_prefix = %encoded_prefix,
                    "AlephBFT batched network message SCALE decode failed: {err}"
                );
                cdk::Error::from(FederationAlephBftError::InvalidNetworkMessage {
                    reason: "SCALE decode failed",
                })
            })?;
            if !input.is_empty() {
                return Err(cdk::Error::from(
                    cdk::federation::FederationError::AlephBft(
                        FederationAlephBftError::InvalidNetworkMessage {
                            reason: "trailing bytes after SCALE network message",
                        },
                    ),
                ));
            }
            activity_ids.extend(message.consensus_activity_ids());
            total_bytes = total_bytes.saturating_add(actual);
            decoded.push((message, actual));
        }

        let byte_budget = crate::federation_aleph_bft_inbound_byte_budget();
        let reservation = self.buffered_inbound_bytes.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |buffered| {
                buffered
                    .checked_add(total_bytes)
                    .filter(|next| *next <= byte_budget)
            },
        );
        if let Err(buffered) = reservation {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::AlephBft(
                    FederationAlephBftError::InboundByteBudgetExceeded {
                        session_id: self.session_id,
                        byte_budget,
                        buffered,
                        incoming: total_bytes,
                    },
                ),
            ));
        }

        let permits = match self.incoming.try_reserve_many(decoded.len()) {
            Ok(permits) => permits,
            Err(err) => {
                self.buffered_inbound_bytes
                    .fetch_sub(total_bytes, Ordering::AcqRel);
                return Err(match err {
                    mpsc::error::TrySendError::Full(()) => {
                        cdk::Error::from(cdk::federation::FederationError::AlephBft(
                            FederationAlephBftError::Backpressure {
                                session_id: self.session_id,
                                detail: "inbound queue is at the bounded message-count budget",
                            },
                        ))
                    }
                    mpsc::error::TrySendError::Closed(()) => cdk::Error::Custom(
                        "AlephBFT network message receiver is closed".to_string(),
                    ),
                });
            }
        };

        let accepted_count = decoded.len();
        for (permit, (data, byte_len)) in permits.zip(decoded) {
            permit.send(CdkAlephBftInboundEnvelope { data, byte_len });
        }
        self.mark_new_consensus_activity(activity_ids);

        Ok((accepted_count, total_bytes))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl CdkAlephBftNetworkActivityMarker for u64 {
    fn consensus_activity_ids(&self) -> Vec<[u8; 32]> {
        let mut activity_id = [0u8; 32];
        activity_id[..8].copy_from_slice(&self.to_be_bytes());
        vec![activity_id]
    }
}

/// Bounded work policy for one AlephBFT network-pump tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdkAlephBftNetworkPumpConfig {
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
            max_messages_per_tick: 64,
            max_peer_deliveries_per_message: 64,
        }
    }
}

impl CdkAlephBftNetworkPumpConfig {
    pub fn validate(self) -> Result<(), cdk::Error> {
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
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CdkAlephBftNetworkPumpTickOutcome {
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

#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryDelivery {
    request: FederationAlephBftNetworkMessageRequest,
    peers: Vec<MemberConfigPublic>,
}

#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryPushOutcome {
    member_ids: Vec<MemberId>,
    dropped_deliveries: usize,
}

/// In-memory retry queue for failed private REST AlephBFT network deliveries.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CdkAlephBftNetworkRetryQueue {
    pending: VecDeque<CdkAlephBftNetworkRetryDelivery>,
}

#[cfg(any(test, feature = "test-utils"))]
impl CdkAlephBftNetworkRetryQueue {
    /// Number of failed network messages waiting for retry.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// True when no failed network messages are waiting for retry.
    pub fn is_empty(&self) -> bool {
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

#[derive(Debug, Clone)]
struct QueuedPeerAlephBftMessage {
    encoded_hex: String,
    byte_len: usize,
    operation_ids: Vec<FederationOperationId>,
}

#[derive(Debug)]
struct PeerOutboundQueueState {
    messages: VecDeque<Arc<QueuedPeerAlephBftMessage>>,
    closed: bool,
}

#[derive(Debug)]
struct PeerOutboundQueue {
    state: Mutex<PeerOutboundQueueState>,
    notify: Notify,
    limit: usize,
}

impl PeerOutboundQueue {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(PeerOutboundQueueState {
                messages: VecDeque::new(),
                closed: false,
            }),
            notify: Notify::new(),
            limit,
        }
    }

    /// Push a message into this peer's bounded outbound queue without blocking.
    /// Returns the number of oldest messages evicted to make room.
    fn push(&self, message: Arc<QueuedPeerAlephBftMessage>) -> usize {
        let mut dropped = 0usize;
        if let Ok(mut guard) = self.state.lock() {
            if guard.closed {
                return 0;
            }
            while guard.messages.len() >= self.limit {
                guard.messages.pop_front();
                dropped = dropped.saturating_add(1);
            }
            guard.messages.push_back(message);
        }
        self.notify.notify_one();
        dropped
    }

    fn drain_batch(
        &self,
        max_messages: usize,
        max_bytes: usize,
    ) -> Vec<Arc<QueuedPeerAlephBftMessage>> {
        let Ok(mut guard) = self.state.lock() else {
            return Vec::new();
        };
        let mut batch = Vec::with_capacity(guard.messages.len().min(max_messages));
        let mut total_bytes = 0usize;
        while batch.len() < max_messages {
            let Some(front) = guard.messages.front() else {
                break;
            };
            if !batch.is_empty() && total_bytes.saturating_add(front.byte_len) > max_bytes {
                break;
            }
            let Some(message) = guard.messages.pop_front() else {
                break;
            };
            total_bytes = total_bytes.saturating_add(message.byte_len);
            batch.push(message);
        }
        batch
    }

    fn is_empty_and_open(&self) -> bool {
        match self.state.lock() {
            Ok(guard) => guard.messages.is_empty() && !guard.closed,
            Err(_) => false,
        }
    }

    fn is_closed_and_empty(&self) -> bool {
        match self.state.lock() {
            Ok(guard) => guard.closed && guard.messages.is_empty(),
            Err(_) => true,
        }
    }

    fn close(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.closed = true;
        }
        self.notify.notify_waiters();
    }
}

/// Continuously drain outbound AlephBFT messages and deliver them over per-peer
/// concurrent async queues with batched signed private-REST transport.
///
/// Local deliveries loop back immediately through the in-memory network bridge.
/// Remote deliveries are encoded once and fanned out non-blockingly into
/// independent bounded per-peer queues so a slow or partitioned peer never
/// stalls outbound draining or healthy peers.
pub async fn run_cdk_aleph_bft_network_pump<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
) -> Result<(), cdk::Error>
where
    T: Transport + 'static,
    D: Clone + Encode + Send + 'static,
{
    config.validate()?;

    let batch_limit = config
        .max_messages_per_tick
        .clamp(1, FEDERATION_MAX_ALEPH_BFT_NETWORK_BATCH_MESSAGES);
    let retry_queue_len = Arc::new(AtomicUsize::new(0));
    let retry_deliveries_dropped = Arc::new(AtomicUsize::new(0));

    let mut peer_queues = BTreeMap::new();
    let mut peer_tasks = JoinSet::new();
    for peer in keychain.remote_members() {
        let queue = Arc::new(PeerOutboundQueue::new(
            CDK_ALEPH_BFT_PEER_OUTBOUND_QUEUE_LIMIT,
        ));
        peer_queues.insert(peer.member_id, Arc::clone(&queue));
        peer_tasks.spawn(run_peer_aleph_bft_sender_task(
            peer,
            client.clone(),
            config.session_id,
            batch_limit,
            queue,
            Arc::clone(&retry_queue_len),
            Arc::clone(&retry_deliveries_dropped),
        ));
    }

    loop {
        let Some(first_outbound) = handle.next_outgoing().await else {
            tracing::warn!("AlephBFT network outbound bridge is closed");
            break;
        };

        let pump_timer = crate::latency::FederationLatencyTimer::start(
            crate::latency::FederationLatencyCorrelation::Session,
            config.session_id.to_string(),
            crate::latency::FederationLatencyBackend::AlephBft,
            crate::latency::FederationLatencyDirection::Internal,
            crate::latency::FederationLatencyPhase::AlephBftNetworkPump,
        );

        let mut messages_drained = 0usize;
        let mut encoded_bytes = 0usize;
        let mut current = Some(first_outbound);

        while messages_drained < batch_limit {
            let Some(outbound) = current.take().or_else(|| handle.try_next_outgoing()) else {
                break;
            };
            messages_drained = messages_drained.saturating_add(1);

            let peers = match remote_members_for_recipient(keychain, &outbound.recipient) {
                Ok(peers) => peers,
                Err(err) => {
                    tracing::warn!(
                        session_id = config.session_id,
                        "AlephBFT network pump could not resolve recipient: {}",
                        err
                    );
                    continue;
                }
            };
            let include_local = match recipient_includes_local(keychain, &outbound.recipient) {
                Ok(include_local) => include_local,
                Err(err) => {
                    tracing::warn!(
                        session_id = config.session_id,
                        "AlephBFT network pump could not check local recipient: {}",
                        err
                    );
                    continue;
                }
            };

            let encoded = outbound.data.encode();
            let byte_len = encoded.len();

            if include_local {
                if let Err(err) =
                    queue_local_aleph_bft_network_delivery(handle, Some(outbound.data))
                {
                    pump_timer.finish(crate::latency::FederationLatencyOutcome::Error);
                    for queue in peer_queues.values() {
                        queue.close();
                    }
                    peer_tasks.abort_all();
                    while peer_tasks.join_next().await.is_some() {}
                    return Err(err);
                }
            }

            if peers.is_empty() {
                continue;
            }
            if byte_len > FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES {
                tracing::warn!(
                    session_id = config.session_id,
                    encoded_bytes = byte_len,
                    max_bytes = FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
                    "Dropping oversized outbound AlephBFT network message"
                );
                continue;
            }

            encoded_bytes = encoded_bytes.saturating_add(byte_len);
            let operation_ids = extract_aleph_bft_operation_ids_for_latency(&encoded);
            let request = FederationAlephBftNetworkMessageRequest::new(config.session_id, encoded);
            let queued_message = Arc::new(QueuedPeerAlephBftMessage {
                encoded_hex: request.encoded_message,
                byte_len,
                operation_ids,
            });

            let mut attempted_members = BTreeSet::new();
            for peer in peers {
                if attempted_members.len() >= config.max_peer_deliveries_per_message {
                    break;
                }
                if !attempted_members.insert(peer.member_id) {
                    continue;
                }
                if let Some(queue) = peer_queues.get(&peer.member_id) {
                    let evicted = queue.push(Arc::clone(&queued_message));
                    if evicted > 0 {
                        retry_deliveries_dropped.fetch_add(evicted, Ordering::Relaxed);
                        tracing::debug!(
                            member_id = %peer.member_id,
                            session_id = config.session_id,
                            evicted,
                            queue_limit = CDK_ALEPH_BFT_PEER_OUTBOUND_QUEUE_LIMIT,
                            "AlephBFT per-peer outbound queue evicted oldest message"
                        );
                    }
                }
            }
        }

        pump_timer.finish(crate::latency::FederationLatencyOutcome::Success);

        #[cfg(any(test, feature = "test-utils"))]
        crate::aleph_bft_latency::record_network_pump_tick(
            messages_drained,
            encoded_bytes,
            0,
            retry_queue_len.load(Ordering::Relaxed),
            retry_deliveries_dropped.swap(0, Ordering::Relaxed),
            handle.outbound_queue_len(),
        );

        tracing::trace!(
            target: "cdk_axum::federation::aleph_bft_latency",
            stage = "network_pump_tick",
            session_id = config.session_id,
            messages_drained,
            encoded_bytes,
            retry_queue_len = retry_queue_len.load(Ordering::Relaxed),
            outbound_queue_len = handle.outbound_queue_len(),
            "AlephBFT per-peer network pump dispatch completed"
        );
    }

    for queue in peer_queues.values() {
        queue.close();
    }
    peer_tasks.abort_all();
    while peer_tasks.join_next().await.is_some() {}

    Ok(())
}

async fn run_peer_aleph_bft_sender_task<T>(
    peer: MemberConfigPublic,
    client: FederationRestPeerClient<T>,
    session_id: u64,
    batch_limit: usize,
    queue: Arc<PeerOutboundQueue>,
    retry_queue_len: Arc<AtomicUsize>,
    retry_deliveries_dropped: Arc<AtomicUsize>,
) where
    T: Transport,
{
    let mut retry_queue: VecDeque<Arc<QueuedPeerAlephBftMessage>> = VecDeque::new();
    let mut retry_backoff = CDK_ALEPH_BFT_PEER_RETRY_INITIAL_BACKOFF;
    let mut next_retry_at: Option<tokio::time::Instant> = None;

    loop {
        let fresh_batch =
            queue.drain_batch(batch_limit, FEDERATION_MAX_ALEPH_BFT_NETWORK_BATCH_BYTES);
        if !fresh_batch.is_empty() {
            let batch_bytes = fresh_batch
                .iter()
                .map(|message| message.byte_len)
                .sum::<usize>();
            match deliver_peer_aleph_bft_batch(&client, &peer, session_id, &fresh_batch).await {
                Ok(()) => {
                    record_delivered_batch_latency(&fresh_batch);
                    retry_backoff = CDK_ALEPH_BFT_PEER_RETRY_INITIAL_BACKOFF;
                    if !retry_queue.is_empty() {
                        next_retry_at = Some(tokio::time::Instant::now());
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        member_id = %peer.member_id,
                        session_id,
                        batch_len = fresh_batch.len(),
                        encoded_bytes = batch_bytes,
                        "AlephBFT network-message delivery failed: {}",
                        err
                    );
                    if is_retryable_aleph_bft_network_delivery_error(&err) {
                        push_peer_retry_batch(
                            &mut retry_queue,
                            fresh_batch,
                            &retry_queue_len,
                            &retry_deliveries_dropped,
                        );
                        if next_retry_at.is_none() {
                            next_retry_at = Some(tokio::time::Instant::now() + retry_backoff);
                        }
                    } else {
                        tracing::warn!(
                            member_id = %peer.member_id,
                            session_id,
                            encoded_bytes = batch_bytes,
                            "Dropping non-retryable AlephBFT network-message delivery failure"
                        );
                    }
                }
            }

            // Interleave one due retry batch so retries never starve under continuous
            // fresh traffic.
            if !retry_queue.is_empty()
                && next_retry_at.is_none_or(|deadline| tokio::time::Instant::now() >= deadline)
            {
                attempt_peer_retry_batch(
                    &client,
                    &peer,
                    session_id,
                    batch_limit,
                    &mut retry_queue,
                    &mut retry_backoff,
                    &mut next_retry_at,
                    &retry_queue_len,
                    &retry_deliveries_dropped,
                )
                .await;
            }
            continue;
        }

        if queue.is_closed_and_empty() {
            break;
        }

        if retry_queue.is_empty() {
            next_retry_at = None;
            let notified = queue.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if queue.is_empty_and_open() {
                notified.await;
            }
            continue;
        }

        let retry_deadline =
            next_retry_at.unwrap_or_else(|| tokio::time::Instant::now() + retry_backoff);
        next_retry_at = Some(retry_deadline);

        if tokio::time::Instant::now() < retry_deadline {
            let notified = queue.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !queue.is_empty_and_open() {
                continue;
            }
            tokio::select! {
                () = notified => continue,
                () = tokio::time::sleep_until(retry_deadline) => {}
            }
        }

        if queue.is_closed_and_empty() {
            break;
        }
        if !queue.is_empty_and_open() {
            continue;
        }

        attempt_peer_retry_batch(
            &client,
            &peer,
            session_id,
            batch_limit,
            &mut retry_queue,
            &mut retry_backoff,
            &mut next_retry_at,
            &retry_queue_len,
            &retry_deliveries_dropped,
        )
        .await;
    }

    if !retry_queue.is_empty() {
        let remaining = retry_queue.len();
        retry_queue_len.fetch_sub(remaining, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt_peer_retry_batch<T>(
    client: &FederationRestPeerClient<T>,
    peer: &MemberConfigPublic,
    session_id: u64,
    batch_limit: usize,
    retry_queue: &mut VecDeque<Arc<QueuedPeerAlephBftMessage>>,
    retry_backoff: &mut Duration,
    next_retry_at: &mut Option<tokio::time::Instant>,
    retry_queue_len: &AtomicUsize,
    retry_deliveries_dropped: &AtomicUsize,
) where
    T: Transport,
{
    let retry_batch = drain_peer_retry_batch(
        retry_queue,
        batch_limit,
        FEDERATION_MAX_ALEPH_BFT_NETWORK_BATCH_BYTES,
        retry_queue_len,
    );
    if retry_batch.is_empty() {
        *next_retry_at = None;
        return;
    }
    let batch_bytes = retry_batch
        .iter()
        .map(|message| message.byte_len)
        .sum::<usize>();
    let attempt = tokio::time::timeout(
        CDK_ALEPH_BFT_NETWORK_RETRY_ATTEMPT_BUDGET,
        deliver_peer_aleph_bft_batch(client, peer, session_id, &retry_batch),
    )
    .await;
    match attempt {
        Ok(Ok(())) => {
            record_delivered_batch_latency(&retry_batch);
            #[cfg(any(test, feature = "test-utils"))]
            crate::aleph_bft_latency::record_network_pump_tick(
                0,
                batch_bytes,
                retry_batch.len(),
                retry_queue_len.load(Ordering::Relaxed),
                retry_deliveries_dropped.swap(0, Ordering::Relaxed),
                0,
            );
            *retry_backoff = CDK_ALEPH_BFT_PEER_RETRY_INITIAL_BACKOFF;
            *next_retry_at = (!retry_queue.is_empty()).then(tokio::time::Instant::now);
        }
        Ok(Err(err)) => {
            tracing::warn!(
                member_id = %peer.member_id,
                session_id,
                batch_len = retry_batch.len(),
                encoded_bytes = batch_bytes,
                "AlephBFT network-message retry failed: {}",
                err
            );
            if is_retryable_aleph_bft_network_delivery_error(&err) {
                push_peer_retry_batch(
                    retry_queue,
                    retry_batch,
                    retry_queue_len,
                    retry_deliveries_dropped,
                );
                *retry_backoff = retry_backoff
                    .saturating_mul(2)
                    .min(CDK_ALEPH_BFT_PEER_RETRY_MAX_BACKOFF);
                *next_retry_at = Some(tokio::time::Instant::now() + *retry_backoff);
            } else {
                tracing::warn!(
                    member_id = %peer.member_id,
                    session_id,
                    encoded_bytes = batch_bytes,
                    "Dropping non-retryable AlephBFT network-message retry failure"
                );
                *next_retry_at = (!retry_queue.is_empty()).then(tokio::time::Instant::now);
            }
        }
        Err(_) => {
            tracing::debug!(
                member_id = %peer.member_id,
                session_id,
                batch_len = retry_batch.len(),
                "AlephBFT network-message retry timed out"
            );
            push_peer_retry_batch(
                retry_queue,
                retry_batch,
                retry_queue_len,
                retry_deliveries_dropped,
            );
            *retry_backoff = retry_backoff
                .saturating_mul(2)
                .min(CDK_ALEPH_BFT_PEER_RETRY_MAX_BACKOFF);
            *next_retry_at = Some(tokio::time::Instant::now() + *retry_backoff);
        }
    }
}

async fn deliver_peer_aleph_bft_batch<T>(
    client: &FederationRestPeerClient<T>,
    peer: &MemberConfigPublic,
    session_id: u64,
    batch: &[Arc<QueuedPeerAlephBftMessage>],
) -> anyhow::Result<()>
where
    T: Transport,
{
    match batch {
        [] => Ok(()),
        [single] => {
            let request = FederationAlephBftNetworkMessageRequest {
                session_id,
                encoded_message: single.encoded_hex.clone(),
            };
            client
                .submit_aleph_bft_network_message(peer, &request)
                .await?;
            Ok(())
        }
        many => {
            let encoded_messages = many
                .iter()
                .map(|message| message.encoded_hex.clone())
                .collect::<Vec<_>>();
            let request = FederationAlephBftNetworkMessageBatchRequest::from_encoded_hex(
                session_id,
                encoded_messages,
            );
            client
                .submit_aleph_bft_network_message_batch(peer, &request)
                .await?;
            Ok(())
        }
    }
}

fn push_peer_retry_batch(
    retry_queue: &mut VecDeque<Arc<QueuedPeerAlephBftMessage>>,
    batch: Vec<Arc<QueuedPeerAlephBftMessage>>,
    retry_queue_len: &AtomicUsize,
    retry_deliveries_dropped: &AtomicUsize,
) {
    let mut dropped = 0usize;
    let mut added = 0usize;
    for message in batch {
        while retry_queue.len() >= CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT {
            if retry_queue.pop_front().is_some() {
                dropped = dropped.saturating_add(1);
                retry_queue_len.fetch_sub(1, Ordering::Relaxed);
            } else {
                break;
            }
        }
        retry_queue.push_back(message);
        added = added.saturating_add(1);
    }
    if added > 0 {
        retry_queue_len.fetch_add(added, Ordering::Relaxed);
    }
    if dropped > 0 {
        retry_deliveries_dropped.fetch_add(dropped, Ordering::Relaxed);
        tracing::warn!(
            dropped,
            retry_queue_limit = CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
            "AlephBFT per-peer retry queue dropped oldest failed deliveries"
        );
    }
}

fn drain_peer_retry_batch(
    retry_queue: &mut VecDeque<Arc<QueuedPeerAlephBftMessage>>,
    max_messages: usize,
    max_bytes: usize,
    retry_queue_len: &AtomicUsize,
) -> Vec<Arc<QueuedPeerAlephBftMessage>> {
    let mut batch = Vec::with_capacity(retry_queue.len().min(max_messages));
    let mut total_bytes = 0usize;
    while batch.len() < max_messages {
        let Some(front) = retry_queue.front() else {
            break;
        };
        if !batch.is_empty() && total_bytes.saturating_add(front.byte_len) > max_bytes {
            break;
        }
        let Some(message) = retry_queue.pop_front() else {
            break;
        };
        total_bytes = total_bytes.saturating_add(message.byte_len);
        batch.push(message);
    }
    if !batch.is_empty() {
        retry_queue_len.fetch_sub(batch.len(), Ordering::Relaxed);
    }
    batch
}

fn extract_aleph_bft_operation_ids_for_latency(encoded: &[u8]) -> Vec<FederationOperationId> {
    #[cfg(not(any(test, feature = "test-utils")))]
    if !tracing::enabled!(
        target: "cdk_axum::federation::aleph_bft_latency",
        tracing::Level::TRACE
    ) {
        return Vec::new();
    }

    let mut bytes = encoded;
    match CdkAlephBftNetworkData::decode(&mut bytes) {
        Ok(message) => message
            .included_data()
            .iter()
            .flat_map(|data| data.payload().operation_ids.iter().copied())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn record_delivered_batch_latency(batch: &[Arc<QueuedPeerAlephBftMessage>]) {
    #[cfg(not(any(test, feature = "test-utils")))]
    if !tracing::enabled!(
        target: "cdk_axum::federation::aleph_bft_latency",
        tracing::Level::TRACE
    ) {
        return;
    }

    for message in batch {
        crate::aleph_bft_latency::record(
            crate::aleph_bft_latency::CdkAlephBftLatencyStage::NetworkMessageDelivered,
            message.operation_ids.clone(),
        );
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
#[cfg(any(test, feature = "test-utils"))]
pub async fn run_cdk_aleph_bft_network_pump_tick<T, D>(
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
    let mut deliveries = Vec::with_capacity(config.max_messages_per_tick);
    for _ in 0..config.max_messages_per_tick {
        let Some(outbound) = handle.try_next_outgoing() else {
            outcome.reached_idle = true;
            break;
        };
        outcome.messages_drained = outcome.messages_drained.saturating_add(1);
        let delivery = cdk_aleph_bft_network_delivery(keychain, config, outbound)?;
        queue_local_aleph_bft_network_delivery(handle, delivery.local_message)?;
        deliveries.push(delivery.retry_delivery);
    }
    let delivery_outcomes = join_all(deliveries.iter().map(|delivery| {
        deliver_cdk_aleph_bft_network_request(
            client,
            config,
            &delivery.request,
            &delivery.peers,
            delivery.peers.len(),
            None,
        )
    }))
    .await;
    for message_outcome in delivery_outcomes {
        let mut message_outcome = message_outcome?;
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
#[cfg(any(test, feature = "test-utils"))]
pub async fn run_cdk_aleph_bft_network_pump_tick_with_retry_queue<T, D>(
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
#[cfg(any(test, feature = "test-utils"))]
pub async fn run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound<T, D>(
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
    let mut deliveries = Vec::with_capacity(config.max_messages_per_tick);
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
        deliveries.push(delivery.retry_delivery);
    }
    let delivery_outcomes = join_all(deliveries.iter().map(|delivery| {
        deliver_cdk_aleph_bft_network_request(
            client,
            config,
            &delivery.request,
            &delivery.peers,
            keychain.remote_delivery_threshold(delivery.peers.len()),
            None,
        )
    }))
    .await;
    for message_outcome in delivery_outcomes {
        let mut message_outcome = message_outcome?;
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
        let mut retry_deliveries = Vec::with_capacity(retry_messages_to_drain);
        for _ in 0..retry_messages_to_drain {
            let Some(delivery) = retry_queue.pending.pop_front() else {
                break;
            };
            outcome.retry_messages_drained = outcome.retry_messages_drained.saturating_add(1);
            retry_deliveries.push(delivery);
        }
        let retry_outcomes = join_all(retry_deliveries.iter().map(|delivery| {
            deliver_cdk_aleph_bft_network_request(
                client,
                config,
                &delivery.request,
                &delivery.peers,
                delivery.peers.len(),
                Some(CDK_ALEPH_BFT_NETWORK_RETRY_ATTEMPT_BUDGET),
            )
        }))
        .await;
        for (delivery, retry_outcome) in retry_deliveries.into_iter().zip(retry_outcomes) {
            let mut retry_outcome = retry_outcome?;
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

#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkMessageDeliveryOutcome {
    request: FederationAlephBftNetworkMessageRequest,
    encoded_bytes: usize,
    remote_accepted_members: Vec<MemberId>,
    remote_failed_members: Vec<MemberId>,
    remote_failed_peers: Vec<MemberConfigPublic>,
    remote_skipped_members: Vec<MemberId>,
}

#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkDelivery<D> {
    retry_delivery: CdkAlephBftNetworkRetryDelivery,
    local_message: Option<D>,
}

#[cfg(any(test, feature = "test-utils"))]
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

#[cfg(any(test, feature = "test-utils"))]
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
                #[cfg(not(any(test, feature = "test-utils")))]
                if tracing::enabled!(
                    target: "cdk_axum::federation::aleph_bft_latency",
                    tracing::Level::TRACE
                ) {
                    crate::aleph_bft_latency::record(
                        crate::aleph_bft_latency::CdkAlephBftLatencyStage::NetworkMessageDelivered,
                        aleph_bft_network_request_operation_ids(request),
                    );
                }
                #[cfg(any(test, feature = "test-utils"))]
                crate::aleph_bft_latency::record(
                    crate::aleph_bft_latency::CdkAlephBftLatencyStage::NetworkMessageDelivered,
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
    !matches!(
        err.downcast_ref::<HttpError>(),
        Some(HttpError::Status { status: 400, .. })
    )
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

#[cfg(any(test, feature = "test-utils"))]
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

    use crate::aleph_bft_runtime::CdkAlephBftNetwork;

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
        let buffered_inbound_bytes = handle.buffered_inbound_bytes();
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
        assert_eq!(
            buffered_inbound_bytes.load(std::sync::atomic::Ordering::Acquire),
            encoded_bytes
        );
        assert_eq!(network.next_event().await, Some(IdleNetworkMessage(42)));
        assert_eq!(
            buffered_inbound_bytes.load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert!(!activity_hint.is_active());
    }

    #[tokio::test]
    async fn inbound_full_queue_rolls_back_byte_reservation() {
        let (mut network, handle) =
            CdkAlephBftNetwork::<IdleNetworkMessage>::new_pair_with_capacity(1);
        let buffered_inbound_bytes = handle.buffered_inbound_bytes();
        let inbound = CdkAlephBftNetworkInboundHandle::new(7, &handle);
        let first =
            FederationAlephBftNetworkMessageRequest::new(7, IdleNetworkMessage(42).encode());
        let second =
            FederationAlephBftNetworkMessageRequest::new(7, IdleNetworkMessage(43).encode());

        let encoded_bytes = inbound.queue_request(&first).expect("first request");
        let err = inbound
            .queue_request(&second)
            .expect_err("full queue must reject the second request");

        assert!(err.to_string().contains("backpressure"));
        assert_eq!(
            buffered_inbound_bytes.load(std::sync::atomic::Ordering::Acquire),
            encoded_bytes
        );
        assert_eq!(network.next_event().await, Some(IdleNetworkMessage(42)));
        assert_eq!(
            buffered_inbound_bytes.load(std::sync::atomic::Ordering::Acquire),
            0
        );
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

    #[tokio::test]
    async fn inbound_batch_request_queues_all_messages_and_rolls_back_atomically_on_backpressure() {
        let (mut network, handle) =
            CdkAlephBftNetwork::<ActiveNetworkMessage>::new_pair_with_capacity(2);
        let buffered_inbound_bytes = handle.buffered_inbound_bytes();
        let activity_hint = CdkAlephBftActivityHint::default();
        let inbound =
            CdkAlephBftNetworkInboundHandle::with_activity_hint(7, &handle, activity_hint.clone());

        let too_large_batch = FederationAlephBftNetworkMessageBatchRequest::new(
            7,
            vec![
                ActiveNetworkMessage(1).encode(),
                ActiveNetworkMessage(2).encode(),
                ActiveNetworkMessage(3).encode(),
            ],
        );
        let err = inbound
            .queue_batch_request(&too_large_batch)
            .expect_err("batch exceeding channel capacity must fail atomically");
        assert!(err.to_string().contains("backpressure"));
        assert_eq!(buffered_inbound_bytes.load(Ordering::Acquire), 0);
        assert!(!activity_hint.is_active());

        let valid_batch = FederationAlephBftNetworkMessageBatchRequest::new(
            7,
            vec![
                ActiveNetworkMessage(10).encode(),
                ActiveNetworkMessage(11).encode(),
            ],
        );
        let (accepted_count, total_bytes) = inbound
            .queue_batch_request(&valid_batch)
            .expect("valid batch request");
        assert_eq!(accepted_count, 2);
        assert_eq!(
            total_bytes,
            ActiveNetworkMessage(10).encode().len() + ActiveNetworkMessage(11).encode().len()
        );
        assert_eq!(buffered_inbound_bytes.load(Ordering::Acquire), total_bytes);
        assert!(activity_hint.is_active());
        assert_eq!(network.next_event().await, Some(ActiveNetworkMessage(10)));
        assert_eq!(network.next_event().await, Some(ActiveNetworkMessage(11)));
        assert_eq!(buffered_inbound_bytes.load(Ordering::Acquire), 0);
    }

    #[derive(Debug, Clone)]
    struct RecordedPeerDelivery {
        receiver: MemberId,
        is_batch: bool,
        messages: Vec<u64>,
    }

    #[derive(Clone)]
    struct PeerFanoutTestTransport {
        federation_id: cdk::federation::FederationId,
        slow_member_port: u16,
        slow_release: Arc<Notify>,
        slow_started: Arc<Notify>,
        deliveries: Arc<Mutex<Vec<RecordedPeerDelivery>>>,
        delivery_notify: Arc<Notify>,
    }

    impl fmt::Debug for PeerFanoutTestTransport {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("PeerFanoutTestTransport")
                .field("slow_member_port", &self.slow_member_port)
                .finish_non_exhaustive()
        }
    }

    #[async_trait::async_trait]
    impl Transport for PeerFanoutTestTransport {
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
            _auth: Option<cdk::nuts::AuthToken>,
        ) -> Result<cdk_http_client::RawResponse, HttpError> {
            Err(HttpError::Other(format!("unexpected GET {}", url.path())))
        }

        async fn http_post<P>(
            &self,
            url: url::Url,
            _auth_token: Option<cdk::nuts::AuthToken>,
            payload: &P,
        ) -> Result<cdk_http_client::RawResponse, HttpError>
        where
            P: serde::Serialize + Send + Sync,
        {
            if url.port() == Some(self.slow_member_port) {
                self.slow_started.notify_waiters();
                self.slow_release.notified().await;
            }
            let body = serde_json::to_vec(payload)?;
            if url
                .path()
                .ends_with("/federation/v1/aleph-bft/messages/batch")
            {
                let signed: cdk::federation::FederationSignedRequest<
                    FederationAlephBftNetworkMessageBatchRequest,
                > = serde_json::from_slice(&body)?;
                let decoded_raw = signed
                    .payload
                    .decoded_messages()
                    .map_err(|err| HttpError::Other(err.to_string()))?;
                let messages = decoded_raw
                    .iter()
                    .map(|raw| u64::decode(&mut raw.as_slice()).expect("decode u64"))
                    .collect::<Vec<_>>();
                let count = messages.len();
                self.deliveries
                    .lock()
                    .expect("deliveries lock")
                    .push(RecordedPeerDelivery {
                        receiver: signed.receiver,
                        is_batch: true,
                        messages,
                    });
                self.delivery_notify.notify_waiters();
                let response = cdk::federation::FederationAlephBftNetworkMessageBatchResponse::new(
                    self.federation_id,
                    signed.payload.session_id,
                    count,
                );
                return Ok(cdk_http_client::RawResponse::new(
                    200,
                    serde_json::to_vec(&response)?,
                ));
            }
            if url.path().ends_with("/federation/v1/aleph-bft/messages") {
                let signed: cdk::federation::FederationSignedRequest<
                    FederationAlephBftNetworkMessageRequest,
                > = serde_json::from_slice(&body)?;
                let raw = signed
                    .payload
                    .message_bytes()
                    .map_err(|err| HttpError::Other(err.to_string()))?;
                let value = u64::decode(&mut raw.as_slice()).expect("decode u64");
                self.deliveries
                    .lock()
                    .expect("deliveries lock")
                    .push(RecordedPeerDelivery {
                        receiver: signed.receiver,
                        is_batch: false,
                        messages: vec![value],
                    });
                self.delivery_notify.notify_waiters();
                let response = cdk::federation::FederationAlephBftNetworkMessageResponse::new(
                    self.federation_id,
                    signed.payload.session_id,
                );
                return Ok(cdk_http_client::RawResponse::new(
                    200,
                    serde_json::to_vec(&response)?,
                ));
            }
            Err(HttpError::Other(format!("unexpected POST {}", url.path())))
        }

        async fn http_post_form<P>(
            &self,
            url: url::Url,
            _auth_token: Option<cdk::nuts::AuthToken>,
            _payload: &P,
        ) -> Result<cdk_http_client::RawResponse, HttpError>
        where
            P: serde::Serialize + Send + Sync,
        {
            Err(HttpError::Other(format!(
                "unexpected form POST {}",
                url.path()
            )))
        }
    }

    fn three_member_test_bundle() -> cdk_common::federation::TrustedDealerConfigBundle {
        use std::str::FromStr;

        use cdk::mint_url::MintUrl;
        use cdk::nuts::{CurrencyUnit, SecretKey};
        use cdk_common::federation::{
            generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
            TrustedDealerMember,
        };
        use cdk_common::nuts::nut01::BlsSecretKey;
        use cdk_common::Amount;

        let members = [1u16, 2, 3]
            .into_iter()
            .map(|id| {
                let mut key_bytes = [0u8; 32];
                key_bytes[31] = id as u8;
                let url =
                    MintUrl::from_str(&format!("http://127.0.0.1:{}", 18080 + id)).expect("url");
                TrustedDealerMember::new(
                    MemberId::new(id).expect("member id"),
                    url.clone(),
                    url,
                    SecretKey::from_slice(&key_bytes).expect("secret key"),
                )
            })
            .collect();
        let mut amount_coefficients = BTreeMap::new();
        amount_coefficients.insert(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[1u8; 32]),
                BlsSecretKey::from_reduced_bytes(&[2u8; 32]),
            ],
        );
        let keysets = vec![TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            amount_coefficients,
        )];
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            keysets,
        )
        .expect("three-member bundle")
    }

    #[tokio::test]
    async fn peer_network_pump_batches_outbound_messages_and_does_not_block_fast_peers_on_slow_peer(
    ) {
        let bundle = three_member_test_bundle();
        let local_id = MemberId::new(1).expect("member 1");
        let fast_id = MemberId::new(2).expect("member 2");
        let slow_id = MemberId::new(3).expect("member 3");
        let local_private = bundle.private_configs[&local_id].clone();
        let keychain = CdkAlephBftKeychain::try_new(&bundle.public_config, local_private.clone())
            .expect("keychain");
        let transport = PeerFanoutTestTransport {
            federation_id: bundle.public_config.federation_id,
            slow_member_port: 18083,
            slow_release: Arc::new(Notify::new()),
            slow_started: Arc::new(Notify::new()),
            deliveries: Arc::new(Mutex::new(Vec::new())),
            delivery_notify: Arc::new(Notify::new()),
        };
        let client =
            FederationRestPeerClient::new(transport.clone(), &bundle.public_config, local_private)
                .expect("peer client");
        let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

        // Queue a burst of 6 outbound messages before starting the pump so the
        // per-peer sender task coalesces them into a single batched request.
        for payload in 10u64..16 {
            network.send(payload, Recipient::Everyone);
        }

        let pump_task = tokio::spawn(async move {
            run_cdk_aleph_bft_network_pump(
                &mut handle,
                &keychain,
                &client,
                CdkAlephBftNetworkPumpConfig {
                    session_id: 7,
                    max_messages_per_tick: 64,
                    max_peer_deliveries_per_message: 64,
                },
            )
            .await
        });

        // Local loopback receives all 6 messages immediately.
        let mut local_received = Vec::new();
        for _ in 0..6 {
            local_received.push(
                tokio::time::timeout(Duration::from_secs(1), network.next_event())
                    .await
                    .expect("local loopback timeout")
                    .expect("local event"),
            );
        }
        assert_eq!(local_received, vec![10, 11, 12, 13, 14, 15]);

        // Fast peer receives all 6 messages quickly even though slow peer is stalled.
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notified = transport.delivery_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let fast_count: usize = transport
                    .deliveries
                    .lock()
                    .expect("deliveries lock")
                    .iter()
                    .filter(|delivery| delivery.receiver == fast_id)
                    .map(|delivery| delivery.messages.len())
                    .sum();
                if fast_count >= 6 {
                    break;
                }
                notified.await;
            }
        })
        .await
        .expect("fast peer must receive full burst without blocking on slow peer");

        let (fast_deliveries, slow_deliveries): (Vec<_>, Vec<_>) = {
            let guard = transport.deliveries.lock().expect("deliveries lock");
            (
                guard
                    .iter()
                    .filter(|delivery| delivery.receiver == fast_id)
                    .cloned()
                    .collect(),
                guard
                    .iter()
                    .filter(|delivery| delivery.receiver == slow_id)
                    .cloned()
                    .collect(),
            )
        };
        assert!(
            slow_deliveries.is_empty(),
            "slow peer should still be blocked while fast peer already finished"
        );
        assert!(
            fast_deliveries.iter().any(|delivery| delivery.is_batch),
            "burst to fast peer should ship via the batched network-message endpoint"
        );
        assert!(
            fast_deliveries.len() < 6,
            "expected fewer HTTP requests than messages, got {}",
            fast_deliveries.len()
        );
        let fast_messages = fast_deliveries
            .into_iter()
            .flat_map(|delivery| delivery.messages)
            .collect::<Vec<_>>();
        assert_eq!(fast_messages, vec![10, 11, 12, 13, 14, 15]);

        // Subsequent message still reaches fast peer without waiting for slow peer.
        network.send(99, Recipient::Everyone);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notified = transport.delivery_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let delivered_99 = transport
                    .deliveries
                    .lock()
                    .expect("deliveries lock")
                    .iter()
                    .filter(|delivery| delivery.receiver == fast_id)
                    .any(|delivery| delivery.messages.contains(&99));
                if delivered_99 {
                    break;
                }
                notified.await;
            }
        })
        .await
        .expect("fast peer must continue receiving fresh messages while slow peer is stalled");

        // Closing the outbound bridge shuts down the pump and aborts stalled peer tasks.
        drop(network);
        tokio::time::timeout(Duration::from_secs(1), pump_task)
            .await
            .expect("pump task shutdown timeout")
            .expect("pump task join")
            .expect("pump task ok");
    }
}
