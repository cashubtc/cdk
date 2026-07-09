use core::fmt;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::marker::PhantomData;

use aleph_bft::Recipient;
#[cfg(test)]
use cdk::federation::FederationOperationId;
use cdk::federation::{
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkRetryRecord,
    MemberConfigPublic, MemberId,
};
use cdk_common::database::DynMintDatabase;
use cdk_http_client::{HttpError, Transport};
use futures::future::join_all;
use parity_scale_codec::{Decode, Encode};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftKeychain, CdkAlephBftNetworkData, CdkAlephBftNetworkHandle,
    CdkAlephBftOutboundMessage,
};
use super::peer_client::FederationRestPeerClient;

pub(crate) const CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT: usize = 4096;

/// Classifies inbound AlephBFT messages that should wake active proposal cadence.
pub(crate) trait CdkAlephBftNetworkActivityMarker {
    /// Return true when this message carries consensus data, not only idle protocol chatter.
    fn marks_consensus_activity(&self) -> bool;
}

impl CdkAlephBftNetworkActivityMarker for CdkAlephBftNetworkData {
    fn marks_consensus_activity(&self) -> bool {
        !self.included_data().is_empty()
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
    incoming: mpsc::UnboundedSender<D>,
    activity_hint: CdkAlephBftActivityHint,
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
            activity_hint,
            _message: PhantomData,
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
        let marks_activity = message.marks_consensus_activity();
        self.incoming.send(message).map_err(|_| {
            cdk::Error::Custom("AlephBFT network message receiver is closed".to_string())
        })?;
        if marks_activity {
            self.activity_hint.mark_network_activity();
        }

        Ok(actual)
    }
}

#[cfg(test)]
impl CdkAlephBftNetworkActivityMarker for u64 {
    fn marks_consensus_activity(&self) -> bool {
        true
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
    /// True when the outbound channel was empty during this tick.
    pub reached_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryDelivery {
    retry_id: String,
    request: FederationAlephBftNetworkMessageRequest,
    peers: Vec<MemberConfigPublic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CdkAlephBftNetworkRetryPushOutcome {
    member_ids: Vec<MemberId>,
    dropped_retry_ids: Vec<String>,
    delivery: Option<CdkAlephBftNetworkRetryDelivery>,
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

    /// Load persisted failed deliveries for the current session.
    pub(crate) async fn from_db(
        db: &DynMintDatabase,
        keychain: &CdkAlephBftKeychain,
        session_id: u64,
    ) -> Result<Self, cdk::Error> {
        let retries = db
            .get_federation_aleph_bft_network_retries(session_id)
            .await
            .map_err(|err| {
                cdk::Error::Custom(format!(
                    "AlephBFT network retry database read failed: {err}"
                ))
            })?;
        let remote_members = keychain
            .remote_members()
            .into_iter()
            .map(|member| (member.member_id, member))
            .collect::<BTreeMap<_, _>>();
        let mut pending = VecDeque::new();
        for retry in retries {
            if let Err(err) = retry.request.ensure_admissible_size() {
                tracing::warn!(
                    retry_id = %retry.retry_id,
                    error = %err,
                    "Removing invalid AlephBFT network retry row"
                );
                remove_persisted_retry(Some(db), &retry.retry_id).await?;
                continue;
            }
            if retry.session_id != session_id || retry.request.session_id != session_id {
                tracing::warn!(
                    retry_id = %retry.retry_id,
                    retry_session = retry.session_id,
                    request_session = retry.request.session_id,
                    expected_session = session_id,
                    "Removing AlephBFT network retry row for a different session"
                );
                remove_persisted_retry(Some(db), &retry.retry_id).await?;
                continue;
            }
            let peers = retry
                .peer_member_ids
                .iter()
                .filter_map(|member_id| remote_members.get(member_id).cloned())
                .collect::<Vec<_>>();
            if peers.is_empty() {
                tracing::warn!(
                    retry_id = %retry.retry_id,
                    "Removing AlephBFT network retry row with no current remote peers"
                );
                remove_persisted_retry(Some(db), &retry.retry_id).await?;
                continue;
            }
            pending.push_back(CdkAlephBftNetworkRetryDelivery {
                retry_id: retry.retry_id,
                request: retry.request,
                peers,
            });
        }

        Ok(Self { pending })
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
            outcome.dropped_retry_ids.push(dropped.retry_id.clone());
            tracing::warn!(
                retry_id = %dropped.retry_id,
                dropped_members = dropped.peers.len(),
                retry_queue_limit = CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT,
                "AlephBFT network retry queue dropped oldest failed delivery"
            );
        }

        let delivery = CdkAlephBftNetworkRetryDelivery {
            retry_id: aleph_bft_network_retry_id(&request, &peers),
            request,
            peers,
        };
        self.pending.push_back(delivery.clone());
        outcome.delivery = Some(delivery);

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
/// Successful new outbound messages stay on the fast path and do not touch the
/// retry store. Only failed deliveries are persisted for crash recovery.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_cdk_aleph_bft_network_pump_tick_with_retry_queue<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
    retry_queue: &mut CdkAlephBftNetworkRetryQueue,
    retry_store: Option<&DynMintDatabase>,
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
        retry_store,
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
    retry_store: Option<&DynMintDatabase>,
    first_outbound: Option<CdkAlephBftOutboundMessage<D>>,
) -> Result<CdkAlephBftNetworkPumpTickOutcome, cdk::Error>
where
    T: Transport,
    D: Clone + Encode + Send + 'static,
{
    config.validate()?;

    let mut outcome = CdkAlephBftNetworkPumpTickOutcome::default();
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
        let requeued = retry_queue.push_failed(delivery.request, retry_outcome.remote_failed_peers);
        persist_retry_push_outcome(retry_store, &requeued).await?;
        remove_obsolete_persisted_retry(retry_store, &delivery.retry_id, &requeued).await?;
        outcome.remote_requeued_members.extend(requeued.member_ids);
    }

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
        persist_retry_push_outcome(retry_store, &requeued).await?;
        outcome.remote_requeued_members.extend(requeued.member_ids);
    }
    outcome.retry_queue_len = retry_queue.len();

    Ok(outcome)
}

async fn remove_obsolete_persisted_retry(
    retry_store: Option<&DynMintDatabase>,
    original_retry_id: &str,
    requeued: &CdkAlephBftNetworkRetryPushOutcome,
) -> Result<(), cdk::Error> {
    let still_pending = requeued
        .delivery
        .as_ref()
        .is_some_and(|delivery| delivery.retry_id == original_retry_id);
    if !still_pending {
        remove_persisted_retry(retry_store, original_retry_id).await?;
    }

    Ok(())
}

async fn persist_retry_push_outcome(
    retry_store: Option<&DynMintDatabase>,
    outcome: &CdkAlephBftNetworkRetryPushOutcome,
) -> Result<(), cdk::Error> {
    for retry_id in &outcome.dropped_retry_ids {
        remove_persisted_retry(retry_store, retry_id).await?;
    }
    let Some(delivery) = &outcome.delivery else {
        return Ok(());
    };
    persist_retry_delivery(retry_store, delivery).await
}

async fn persist_retry_delivery(
    retry_store: Option<&DynMintDatabase>,
    delivery: &CdkAlephBftNetworkRetryDelivery,
) -> Result<(), cdk::Error> {
    if delivery.peers.is_empty() {
        return Ok(());
    }
    let Some(db) = retry_store else {
        return Ok(());
    };
    db.add_federation_aleph_bft_network_retry(&retry_record_for_delivery(delivery))
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!(
                "AlephBFT network retry database write failed: {err}"
            ))
        })
}

async fn remove_persisted_retry(
    retry_store: Option<&DynMintDatabase>,
    retry_id: &str,
) -> Result<(), cdk::Error> {
    let Some(db) = retry_store else {
        return Ok(());
    };
    db.remove_federation_aleph_bft_network_retry(retry_id)
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!(
                "AlephBFT network retry database delete failed: {err}"
            ))
        })
}

fn retry_record_for_delivery(
    delivery: &CdkAlephBftNetworkRetryDelivery,
) -> FederationAlephBftNetworkRetryRecord {
    FederationAlephBftNetworkRetryRecord {
        retry_id: delivery.retry_id.clone(),
        session_id: delivery.request.session_id,
        request: delivery.request.clone(),
        peer_member_ids: delivery.peers.iter().map(|peer| peer.member_id).collect(),
    }
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
        retry_delivery: CdkAlephBftNetworkRetryDelivery {
            retry_id: aleph_bft_network_retry_id(&request, &peers),
            request,
            peers,
        },
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

    let attempts = attempted_peers.iter().map(|peer| async move {
        (
            peer,
            client.submit_aleph_bft_network_message(peer, request).await,
        )
    });
    for (peer, result) in join_all(attempts).await {
        match result {
            Ok(_) => {
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
    }

    outcome.remote_skipped_members = peers
        .iter()
        .filter(|peer| !attempted_members.contains(&peer.member_id))
        .map(|peer| peer.member_id)
        .collect();

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

fn aleph_bft_network_retry_id(
    request: &FederationAlephBftNetworkMessageRequest,
    peers: &[MemberConfigPublic],
) -> String {
    let mut member_ids = peers.iter().map(|peer| peer.member_id).collect::<Vec<_>>();
    member_ids.sort();
    let mut hasher = Sha256::new();
    hasher.update(request.session_id.to_be_bytes());
    hasher.update(request.encoded_message.as_bytes());
    for member_id in member_ids {
        hasher.update(member_id.get().to_be_bytes());
    }

    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }

    encoded
}

#[cfg(test)]
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
        fn marks_consensus_activity(&self) -> bool {
            false
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
}
