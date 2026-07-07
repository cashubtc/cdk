use core::fmt;

use std::collections::BTreeSet;
use std::marker::PhantomData;

use aleph_bft::Recipient;
use cdk::federation::{FederationAlephBftNetworkMessageRequest, MemberConfigPublic, MemberId};
use cdk_http_client::Transport;
use parity_scale_codec::{Decode, Encode};
use tokio::sync::mpsc;

use super::aleph_bft_runtime::{
    CdkAlephBftKeychain, CdkAlephBftNetworkHandle, CdkAlephBftOutboundMessage,
};
use super::peer_client::FederationRestPeerClient;

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
        Self {
            session_id,
            incoming: handle.incoming_sender(),
            _message: PhantomData,
        }
    }
}

impl<D> CdkAlephBftNetworkInboundQueue for CdkAlephBftNetworkInboundHandle<D>
where
    D: Decode + Send + Sync + 'static,
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
        let mut input = bytes.as_slice();
        let message = D::decode(&mut input).map_err(|err| {
            cdk::Error::from(
                cdk::federation::FederationError::InvalidAlephBftNetworkMessage {
                    reason: err.to_string(),
                },
            )
        })?;
        if !input.is_empty() {
            return Err(cdk::Error::from(
                cdk::federation::FederationError::InvalidAlephBftNetworkMessage {
                    reason: "trailing bytes after SCALE network message".to_string(),
                },
            ));
        }
        self.incoming.send(message).map_err(|_| {
            cdk::Error::Custom("AlephBFT network message receiver is closed".to_string())
        })?;

        Ok(actual)
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
    /// Remote members that accepted at least one message in this tick.
    pub remote_accepted_members: Vec<MemberId>,
    /// Remote members that failed at least one message delivery in this tick.
    pub remote_failed_members: Vec<MemberId>,
    /// Remote members skipped by the bounded per-message delivery limit.
    pub remote_skipped_members: Vec<MemberId>,
    /// True when the outbound channel was empty during this tick.
    pub reached_idle: bool,
}

/// Drain outbound AlephBFT network messages and deliver them over signed private REST.
///
/// This pump is intentionally bounded and best-effort. AlephBFT has its own
/// retry and reliability mechanisms, so a failed peer delivery is recorded for
/// operator visibility but does not make the whole tick fail. Local recipients
/// are not looped back through REST; the upstream task already owns local state.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_cdk_aleph_bft_network_pump_tick<T, D>(
    handle: &mut CdkAlephBftNetworkHandle<D>,
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
) -> Result<CdkAlephBftNetworkPumpTickOutcome, cdk::Error>
where
    T: Transport,
    D: Encode + Send + 'static,
{
    config.validate()?;

    let mut outcome = CdkAlephBftNetworkPumpTickOutcome::default();
    for _ in 0..config.max_messages_per_tick {
        let Some(outbound) = handle.try_next_outgoing() else {
            outcome.reached_idle = true;
            break;
        };
        outcome.messages_drained = outcome.messages_drained.saturating_add(1);
        let mut message_outcome =
            deliver_cdk_aleph_bft_network_message(keychain, client, config, outbound).await?;
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CdkAlephBftNetworkMessageDeliveryOutcome {
    encoded_bytes: usize,
    remote_accepted_members: Vec<MemberId>,
    remote_failed_members: Vec<MemberId>,
    remote_skipped_members: Vec<MemberId>,
}

async fn deliver_cdk_aleph_bft_network_message<T, D>(
    keychain: &CdkAlephBftKeychain,
    client: &FederationRestPeerClient<T>,
    config: CdkAlephBftNetworkPumpConfig,
    outbound: CdkAlephBftOutboundMessage<D>,
) -> Result<CdkAlephBftNetworkMessageDeliveryOutcome, cdk::Error>
where
    T: Transport,
    D: Encode + Send + 'static,
{
    let encoded = outbound.data.encode();
    let request = FederationAlephBftNetworkMessageRequest::new(config.session_id, encoded);
    let encoded_bytes = request.ensure_admissible_size().map_err(cdk::Error::from)?;
    let peers = remote_members_for_recipient(keychain, &outbound.recipient)?;
    let mut outcome = CdkAlephBftNetworkMessageDeliveryOutcome {
        encoded_bytes,
        ..CdkAlephBftNetworkMessageDeliveryOutcome::default()
    };
    let mut attempted_members = BTreeSet::new();
    for peer in peers.iter() {
        if attempted_members.len() >= config.max_peer_deliveries_per_message {
            break;
        }
        attempted_members.insert(peer.member_id);
        match client
            .submit_aleph_bft_network_message(peer, &request)
            .await
        {
            Ok(_) => outcome.remote_accepted_members.push(peer.member_id),
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    session_id = config.session_id,
                    recipient = ?outbound.recipient,
                    "AlephBFT network-message delivery failed: {}",
                    err
                );
                outcome.remote_failed_members.push(peer.member_id);
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
