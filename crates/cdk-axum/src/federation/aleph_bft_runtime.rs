use core::fmt;

use std::future::Future;
use std::hash::{Hash, Hasher as StdHasher};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aleph_bft::{
    DataProvider, FinalizationHandler, Hasher as AlephHasher, Index, Keychain, LocalIO,
    MultiKeychain, Network, NetworkData, NodeCount, NodeIndex, OrderedUnit, Recipient,
    SignatureSet, SpawnHandle, TaskHandle, UnitFinalizationHandler,
};
use cdk::federation::{
    FederationAlephBftUnitPayload, FederationConfig, FederationConsensusRuntime, FederationError,
    MemberConfigPrivate, MemberConfigPublic, MemberId,
};
use cdk::nuts::SecretKey;
use cdk_common::database::DynMintDatabase;
use futures::future::BoxFuture;
use futures::io::{AsyncRead, AsyncWrite};
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input, Output};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::{
    is_retryable_federation_database_error, FEDERATION_APPLY_RETRY_INITIAL_DELAY,
    FEDERATION_APPLY_RETRY_MAX_DELAY,
};

/// How long inbound AlephBFT traffic keeps a member on the active proposal cadence.
///
/// Non-submitting members can have no local pending mempool while still needing
/// to create empty units so a peer's data can finalize. Keep that wake-up
/// bounded and purely in-memory.
pub(crate) const CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE: Duration = Duration::from_secs(10);

/// SHA-256 hasher used by CDK's AlephBFT session units.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftHasher;

impl AlephHasher for CdkAlephBftHasher {
    type Hash = [u8; 32];

    fn hash(s: &[u8]) -> Self::Hash {
        Sha256::digest(s).into()
    }
}

/// Concrete upstream AlephBFT network payload used by CDK federation sessions.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) type CdkAlephBftNetworkData = NetworkData<
    CdkAlephBftHasher,
    CdkAlephBftData,
    CdkAlephBftSignature,
    SignatureSet<CdkAlephBftSignature>,
>;

/// Outbound message emitted by the AlephBFT network adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftOutboundMessage<D> {
    /// Opaque AlephBFT payload to deliver.
    pub data: D,
    /// AlephBFT recipient chosen by the protocol.
    pub recipient: Recipient,
}

/// Channel-backed AlephBFT network implementation.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetwork<D> {
    incoming: mpsc::UnboundedReceiver<D>,
    outgoing: mpsc::UnboundedSender<CdkAlephBftOutboundMessage<D>>,
}

impl<D> fmt::Debug for CdkAlephBftNetwork<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftNetwork").finish_non_exhaustive()
    }
}

/// Test and transport bridge handle for a [`CdkAlephBftNetwork`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkHandle<D> {
    incoming: mpsc::UnboundedSender<D>,
    outgoing: mpsc::UnboundedReceiver<CdkAlephBftOutboundMessage<D>>,
}

impl<D> fmt::Debug for CdkAlephBftNetworkHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftNetworkHandle")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<D> CdkAlephBftNetwork<D> {
    /// Create a network plus bridge handle for injecting inbound and draining outbound messages.
    pub(crate) fn new_pair() -> (Self, CdkAlephBftNetworkHandle<D>) {
        let (incoming_sender, incoming_receiver) = mpsc::unbounded_channel();
        let (outgoing_sender, outgoing_receiver) = mpsc::unbounded_channel();
        (
            Self {
                incoming: incoming_receiver,
                outgoing: outgoing_sender,
            },
            CdkAlephBftNetworkHandle {
                incoming: incoming_sender,
                outgoing: outgoing_receiver,
            },
        )
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<D> CdkAlephBftNetworkHandle<D> {
    /// Queue an inbound message as if it arrived from the federation transport.
    pub(crate) fn queue_incoming(&self, data: D) -> Result<(), mpsc::error::SendError<D>> {
        self.incoming.send(data)
    }

    /// Return a cloneable sender for wiring authenticated inbound transport messages.
    pub(crate) fn incoming_sender(&self) -> mpsc::UnboundedSender<D> {
        self.incoming.clone()
    }

    /// Receive the next outbound message produced by AlephBFT.
    pub(crate) async fn next_outgoing(&mut self) -> Option<CdkAlephBftOutboundMessage<D>> {
        self.outgoing.recv().await
    }

    /// Try to receive the next outbound message without waiting.
    pub(crate) fn try_next_outgoing(&mut self) -> Option<CdkAlephBftOutboundMessage<D>> {
        self.outgoing.try_recv().ok()
    }
}

#[async_trait::async_trait]
impl<D> Network<D> for CdkAlephBftNetwork<D>
where
    D: Send + 'static,
{
    fn send(&self, data: D, recipient: Recipient) {
        if self
            .outgoing
            .send(CdkAlephBftOutboundMessage { data, recipient })
            .is_err()
        {
            tracing::warn!("AlephBFT network outbound bridge is closed");
        }
    }

    async fn next_event(&mut self) -> Option<D> {
        self.incoming.recv().await
    }
}

/// Schnorr signature bytes used by CDK's AlephBFT keychain adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftSignature([u8; 64]);

impl CdkAlephBftSignature {
    fn from_schnorr(signature: cdk::secp256k1::schnorr::Signature) -> Self {
        Self(signature.serialize())
    }

    fn as_schnorr(self) -> Option<cdk::secp256k1::schnorr::Signature> {
        cdk::secp256k1::schnorr::Signature::from_slice(&self.0).ok()
    }
}

/// AlephBFT keychain backed by the federation secp256k1 identity roster.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftKeychain {
    private_config: MemberConfigPrivate,
    members_by_node: Vec<(MemberId, MemberConfigPublic)>,
    local_index: NodeIndex,
    node_count: NodeCount,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftKeychain {
    /// Build an AlephBFT keychain from the validated federation config and local identity key.
    pub(crate) fn try_new(
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Result<Self, cdk::Error> {
        public_config.validate()?;
        if !matches!(&private_config.identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT keychain requires secp256k1 identity key for member {}",
                private_config.member_id
            )));
        }
        if let Some(federation_id) = private_config.federation_id {
            if federation_id != public_config.federation_id {
                return Err(cdk::Error::from(FederationError::FederationIdMismatch {
                    expected: public_config.federation_id,
                    found: federation_id,
                }));
            }
        }

        let node_count = NodeCount(public_config.members.len());
        let expected_consensus_threshold = u16::try_from(node_count.consensus_threshold().0)
            .map_err(|_| cdk::Error::Custom("AlephBFT node count exceeds u16".to_string()))?;
        if public_config.threshold_params.consensus_threshold != expected_consensus_threshold {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT keychain requires BFT consensus threshold {}, found {}",
                expected_consensus_threshold, public_config.threshold_params.consensus_threshold
            )));
        }

        let mut members_by_node = Vec::with_capacity(public_config.members.len());
        let mut local_index = None;
        for (index, (member_id, member)) in public_config.members.iter().enumerate() {
            if *member_id == private_config.member_id {
                local_index = Some(NodeIndex(index));
                if private_config.identity_secret_key.public_key() != member.identity_public_key {
                    return Err(cdk::Error::from(FederationError::PrivateIdentityMismatch {
                        member_id: *member_id,
                    }));
                }
            }
            members_by_node.push((*member_id, member.clone()));
        }
        let local_index = local_index.ok_or(cdk::Error::from(FederationError::UnknownMember {
            member_id: private_config.member_id,
        }))?;

        Ok(Self {
            private_config,
            members_by_node,
            local_index,
            node_count,
        })
    }

    /// Return the federation member represented by a given AlephBFT node index.
    pub(crate) fn member_for_node(&self, node: NodeIndex) -> Option<MemberId> {
        self.members_by_node
            .get(node.0)
            .map(|(member_id, _)| *member_id)
    }

    /// Return the federation member config represented by a given AlephBFT node index.
    pub(crate) fn member_config_for_node(&self, node: NodeIndex) -> Option<&MemberConfigPublic> {
        self.members_by_node.get(node.0).map(|(_, member)| member)
    }

    /// Return the local federation member represented by this keychain.
    pub(crate) fn local_member(&self) -> MemberId {
        self.private_config.member_id
    }

    /// Return all configured remote federation members.
    pub(crate) fn remote_members(&self) -> Vec<MemberConfigPublic> {
        self.members_by_node
            .iter()
            .filter(|(member_id, _)| *member_id != self.private_config.member_id)
            .map(|(_, member)| member.clone())
            .collect()
    }
}

impl Index for CdkAlephBftKeychain {
    fn index(&self) -> NodeIndex {
        self.local_index
    }
}

impl Keychain for CdkAlephBftKeychain {
    type Signature = CdkAlephBftSignature;

    fn node_count(&self) -> NodeCount {
        self.node_count
    }

    fn sign(&self, msg: &[u8]) -> Self::Signature {
        CdkAlephBftSignature::from_schnorr(
            self.private_config
                .identity_secret_key
                .sign(msg)
                .expect("validated secp256k1 federation identity key signs AlephBFT messages"),
        )
    }

    fn verify(&self, msg: &[u8], signature: &Self::Signature, index: NodeIndex) -> bool {
        let Some((_, member)) = self.members_by_node.get(index.0) else {
            return false;
        };
        let Some(signature) = signature.as_schnorr() else {
            return false;
        };

        member.identity_public_key.verify(msg, &signature).is_ok()
    }
}

impl MultiKeychain for CdkAlephBftKeychain {
    type PartialMultisignature = SignatureSet<CdkAlephBftSignature>;

    fn bootstrap_multi(
        &self,
        signature: &Self::Signature,
        index: NodeIndex,
    ) -> Self::PartialMultisignature {
        let mut partial = SignatureSet::with_size(self.node_count());
        partial.insert(index, *signature);
        partial
    }

    fn is_complete(&self, msg: &[u8], partial: &Self::PartialMultisignature) -> bool {
        if partial.iter().count() < self.node_count().consensus_threshold().0 {
            return false;
        }

        partial
            .iter()
            .all(|(index, signature)| self.verify(msg, signature, index))
    }
}

/// AlephBFT data wrapper that preserves CDK's canonical consensus encoding.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftData {
    canonical: Vec<u8>,
    payload: FederationAlephBftUnitPayload,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftData {
    /// Wrap a validated CDK unit payload for AlephBFT ordering.
    pub(crate) fn try_from_payload(
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Self, FederationError> {
        let canonical = payload.canonical_bytes()?;
        Ok(Self { canonical, payload })
    }

    /// Return the CDK unit payload ordered by AlephBFT.
    pub(crate) fn payload(&self) -> &FederationAlephBftUnitPayload {
        &self.payload
    }

    /// Return the ordered CDK unit payload.
    pub(crate) fn into_payload(self) -> FederationAlephBftUnitPayload {
        self.payload
    }

    /// Return the exact canonical bytes encoded into AlephBFT messages.
    pub(crate) fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
}

impl PartialEq for CdkAlephBftData {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

impl Eq for CdkAlephBftData {}

impl Hash for CdkAlephBftData {
    fn hash<H>(&self, state: &mut H)
    where
        H: StdHasher,
    {
        self.canonical.hash(state);
    }
}

impl Encode for CdkAlephBftData {
    fn size_hint(&self) -> usize {
        self.canonical.size_hint()
    }

    fn encode_to<T>(&self, dest: &mut T)
    where
        T: Output + ?Sized,
    {
        self.canonical.encode_to(dest);
    }
}

impl Decode for CdkAlephBftData {
    fn decode<I>(input: &mut I) -> Result<Self, CodecError>
    where
        I: Input,
    {
        let canonical = Vec::<u8>::decode(input)?;
        let payload = serde_json::from_slice::<FederationAlephBftUnitPayload>(&canonical)
            .map_err(|_| CodecError::from("invalid CDK AlephBFT data JSON"))?;
        let data = Self::try_from_payload(payload)
            .map_err(|_| CodecError::from("invalid CDK AlephBFT unit payload"))?;
        if data.canonical != canonical {
            return Err(CodecError::from("non-canonical CDK AlephBFT data encoding"));
        }

        Ok(data)
    }
}

#[derive(Debug, Clone)]
struct CdkAlephBftActivityHintState {
    local_pending: bool,
    active_until: Option<Instant>,
}

/// In-memory hint used to keep AlephBFT proposal cadence warm only around work.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftActivityHint {
    state: Arc<Mutex<CdkAlephBftActivityHintState>>,
}

impl Default for CdkAlephBftActivityHintState {
    fn default() -> Self {
        Self {
            local_pending: false,
            active_until: None,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftActivityHint {
    /// Mark this member as having locally queued or pending-finalization work.
    pub(crate) fn mark_local_pending(&self) {
        self.set_local_pending(true);
    }

    /// Update whether this member still has local pending consensus work.
    pub(crate) fn set_local_pending(&self, pending: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.local_pending = pending;
        }
    }

    /// Mark recent inbound AlephBFT network traffic from a peer.
    pub(crate) fn mark_network_activity(&self) {
        let deadline = Instant::now() + CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE;
        if let Ok(mut state) = self.state.lock() {
            state.active_until = Some(match state.active_until {
                Some(existing) => existing.max(deadline),
                None => deadline,
            });
        }
    }

    /// Return whether this session should use active proposal cadence.
    pub(crate) fn is_active(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.local_pending {
            return true;
        }

        match state.active_until {
            Some(deadline) if deadline > Instant::now() => true,
            Some(_) => {
                state.active_until = None;
                false
            }
            None => false,
        }
    }
}

/// Runtime-backed data provider for upstream AlephBFT sessions.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftDataProvider {
    consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>,
    activity_hint: CdkAlephBftActivityHint,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftDataProvider {
    /// Create a data provider with a shared AlephBFT activity hint.
    pub(crate) fn with_activity_hint(
        consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>,
        activity_hint: CdkAlephBftActivityHint,
    ) -> Self {
        Self {
            consensus,
            activity_hint,
        }
    }
}

#[async_trait::async_trait]
impl DataProvider for CdkAlephBftDataProvider {
    type Output = CdkAlephBftData;

    async fn get_data(&mut self) -> Option<Self::Output> {
        let (payload, pending_after) = {
            let mut consensus = self.consensus.lock().await;
            let payload = match consensus.next_aleph_bft_unit_payload() {
                Ok(payload) => payload,
                Err(err) => {
                    tracing::warn!("AlephBFT data provider could not build CDK unit: {}", err);
                    return None;
                }
            };
            (payload, consensus.pending_mempool_len())
        };
        self.activity_hint.set_local_pending(pending_after > 0);

        match payload {
            Some(payload) => match CdkAlephBftData::try_from_payload(payload) {
                Ok(data) => Some(data),
                Err(err) => {
                    tracing::warn!("AlephBFT data provider rejected CDK unit encoding: {}", err);
                    None
                }
            },
            None => None,
        }
    }
}

/// Finalization handler that hands ordered CDK units back to the mint runtime.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftFinalizationHandler {
    finalized_units: mpsc::UnboundedSender<FederationAlephBftUnitPayload>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftFinalizationHandler {
    /// Create a handler that forwards finalized unit payloads to an async apply loop.
    pub(crate) fn new(
        finalized_units: mpsc::UnboundedSender<FederationAlephBftUnitPayload>,
    ) -> Self {
        Self { finalized_units }
    }
}

impl FinalizationHandler<CdkAlephBftData> for CdkAlephBftFinalizationHandler {
    fn data_finalized(&mut self, data: CdkAlephBftData) {
        if self.finalized_units.send(data.into_payload()).is_err() {
            tracing::warn!("AlephBFT finalized CDK unit receiver is closed");
        }
    }
}

impl UnitFinalizationHandler for CdkAlephBftFinalizationHandler {
    type Data = CdkAlephBftData;
    type Hasher = CdkAlephBftHasher;

    fn batch_finalized(&mut self, batch: Vec<OrderedUnit<Self::Data, Self::Hasher>>) {
        for unit in batch {
            if let Some(data) = unit.data {
                self.data_finalized(data);
            }
        }
    }
}

/// Shared append-only AlephBFT backup bytes for one session.
///
/// Upstream AlephBFT owns the encoded unit format. CDK keeps this adapter
/// byte-preserving so the backing store can later move from memory to mint DB
/// persistence without changing the `run_session` IO boundary.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftBackup {
    bytes: Arc<Mutex<Vec<u8>>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftBackup {
    /// Create an empty backup byte store.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Create a backup byte store initialized from persisted bytes.
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(Mutex::new(bytes)),
        }
    }

    /// Create an append-only backup writer.
    pub(crate) fn writer(&self) -> CdkAlephBftBackupWriter {
        CdkAlephBftBackupWriter {
            bytes: Arc::clone(&self.bytes),
        }
    }

    /// Create a snapshot reader over the currently persisted backup bytes.
    pub(crate) fn try_reader(&self) -> Result<CdkAlephBftBackupReader, cdk::Error> {
        Ok(CdkAlephBftBackupReader {
            bytes: self.snapshot()?,
            position: 0,
        })
    }

    /// Return a snapshot of all currently persisted backup bytes.
    pub(crate) fn snapshot(&self) -> Result<Vec<u8>, cdk::Error> {
        self.bytes
            .lock()
            .map(|bytes| bytes.clone())
            .map_err(|_| cdk::Error::Custom("AlephBFT backup byte store is poisoned".to_string()))
    }
}

/// Snapshot reader for AlephBFT backup bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftBackupReader {
    bytes: Vec<u8>,
    position: usize,
}

impl AsyncRead for CdkAlephBftBackupReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = self.bytes.len().saturating_sub(self.position);
        let count = remaining.min(buf.len());
        if count == 0 {
            return Poll::Ready(Ok(0));
        }

        let start = self.position;
        let end = start.saturating_add(count);
        buf[..count].copy_from_slice(&self.bytes[start..end]);
        self.position = end;

        Poll::Ready(Ok(count))
    }
}

/// Append-only writer for AlephBFT backup bytes.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftBackupWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl AsyncWrite for CdkAlephBftBackupWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.bytes.lock() {
            Ok(mut bytes) => {
                bytes.extend_from_slice(buf);
                Poll::Ready(Ok(buf.len()))
            }
            Err(_) => Poll::Ready(Err(io::Error::other(
                "AlephBFT backup byte store is poisoned",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Mint database backed AlephBFT backup bytes for one local session.
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftDbBackup {
    db: DynMintDatabase,
    session_id: u64,
}

impl fmt::Debug for CdkAlephBftDbBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftDbBackup")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftDbBackup {
    /// Create a database backed backup handle for a local AlephBFT session.
    pub(crate) fn new(db: DynMintDatabase, session_id: u64) -> Self {
        Self { db, session_id }
    }

    /// Create a reader over the currently persisted backup bytes.
    pub(crate) async fn try_reader(&self) -> Result<CdkAlephBftBackupReader, cdk::Error> {
        let bytes = self
            .db
            .get_federation_aleph_bft_backup_bytes(self.session_id)
            .await
            .map_err(cdk::Error::Database)?
            .unwrap_or_default();

        Ok(CdkAlephBftBackupReader { bytes, position: 0 })
    }

    /// Create an append-only writer that durably persists on flush or close.
    pub(crate) fn writer(&self) -> CdkAlephBftDbBackupWriter {
        CdkAlephBftDbBackupWriter {
            db: Arc::clone(&self.db),
            session_id: self.session_id,
            state: Arc::new(Mutex::new(CdkAlephBftDbBackupWriterState::default())),
        }
    }
}

#[derive(Default)]
struct CdkAlephBftDbBackupWriterState {
    pending: Vec<u8>,
    flush: Option<BoxFuture<'static, io::Result<()>>>,
}

/// Append-only AlephBFT backup writer that commits flushed bytes to mint DB.
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftDbBackupWriter {
    db: DynMintDatabase,
    session_id: u64,
    state: Arc<Mutex<CdkAlephBftDbBackupWriterState>>,
}

impl fmt::Debug for CdkAlephBftDbBackupWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftDbBackupWriter")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl AsyncWrite for CdkAlephBftDbBackupWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let mut state = match this.state.lock() {
            Ok(state) => state,
            Err(_) => {
                return Poll::Ready(Err(io::Error::other(
                    "AlephBFT database backup writer state is poisoned",
                )));
            }
        };

        if state.flush.is_some() {
            match poll_aleph_bft_db_backup_flush(&this.db, this.session_id, &mut state, cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }

        state.pending.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut state = match this.state.lock() {
            Ok(state) => state,
            Err(_) => {
                return Poll::Ready(Err(io::Error::other(
                    "AlephBFT database backup writer state is poisoned",
                )));
            }
        };

        poll_aleph_bft_db_backup_flush(&this.db, this.session_id, &mut state, cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

fn poll_aleph_bft_db_backup_flush(
    db: &DynMintDatabase,
    session_id: u64,
    state: &mut CdkAlephBftDbBackupWriterState,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    loop {
        if let Some(flush) = state.flush.as_mut() {
            match flush.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => {
                    state.flush = None;
                }
                Poll::Ready(Err(err)) => {
                    state.flush = None;
                    return Poll::Ready(Err(err));
                }
                Poll::Pending => return Poll::Pending,
            }
        } else if state.pending.is_empty() {
            return Poll::Ready(Ok(()));
        } else {
            let bytes = std::mem::take(&mut state.pending);
            let db = Arc::clone(db);
            state.flush = Some(Box::pin(async move {
                persist_aleph_bft_backup_bytes(db, session_id, bytes).await
            }));
        }
    }
}

async fn persist_aleph_bft_backup_bytes(
    db: DynMintDatabase,
    session_id: u64,
    bytes: Vec<u8>,
) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    let mut attempts = 0usize;
    let mut retry_delay = FEDERATION_APPLY_RETRY_INITIAL_DELAY;

    loop {
        match try_persist_aleph_bft_backup_bytes(Arc::clone(&db), session_id, &bytes).await {
            Ok(()) => {
                if attempts > 0 {
                    tracing::info!(
                        session_id,
                        retry_attempts = attempts,
                        "AlephBFT backup persistence recovered after transient storage contention"
                    );
                }
                return Ok(());
            }
            Err(err) if is_retryable_federation_database_error(&err) => {
                attempts = attempts.saturating_add(1);
                if attempts == 1 || attempts % 10 == 0 {
                    tracing::warn!(
                        session_id,
                        retry_attempts = attempts,
                        retry_delay_ms = retry_delay.as_millis(),
                        "AlephBFT backup persistence hit transient storage contention; retrying"
                    );
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(FEDERATION_APPLY_RETRY_MAX_DELAY);
            }
            Err(err) => return Err(database_io_error(err)),
        }
    }
}

async fn try_persist_aleph_bft_backup_bytes(
    db: DynMintDatabase,
    session_id: u64,
    bytes: &[u8],
) -> Result<(), cdk_common::database::Error> {
    let mut tx = db.begin_transaction().await?;
    tx.append_federation_aleph_bft_backup_bytes(session_id, &bytes)
        .await?;
    tx.commit().await
}

fn database_io_error(err: cdk_common::database::Error) -> io::Error {
    io::Error::other(format!("AlephBFT backup database error: {err}"))
}

/// Concrete local IO bundle passed into an upstream AlephBFT session.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) type CdkAlephBftLocalIo = LocalIO<
    CdkAlephBftDataProvider,
    CdkAlephBftFinalizationHandler,
    CdkAlephBftDbBackupWriter,
    CdkAlephBftBackupReader,
>;

/// Local AlephBFT session IO plus the receiver used by CDK's apply loop.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftSessionIo {
    local_io: CdkAlephBftLocalIo,
    finalized_units: mpsc::UnboundedReceiver<FederationAlephBftUnitPayload>,
}

impl fmt::Debug for CdkAlephBftSessionIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftSessionIo")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftSessionIo {
    /// Build local IO for a live session from the consensus runtime and mint DB backup.
    pub(crate) async fn from_db(
        consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>,
        db: DynMintDatabase,
        session_id: u64,
        activity_hint: CdkAlephBftActivityHint,
    ) -> Result<Self, cdk::Error> {
        let backup = CdkAlephBftDbBackup::new(db, session_id);
        let unit_loader = backup.try_reader().await?;
        let unit_saver = backup.writer();
        let data_provider = CdkAlephBftDataProvider::with_activity_hint(consensus, activity_hint);
        let (sender, finalized_units) = mpsc::unbounded_channel();
        let finalization_handler = CdkAlephBftFinalizationHandler::new(sender);
        let local_io = LocalIO::new_with_unit_finalization_handler(
            data_provider,
            finalization_handler,
            unit_saver,
            unit_loader,
        );

        Ok(Self {
            local_io,
            finalized_units,
        })
    }

    /// Split the upstream session IO from the receiver consumed by the CDK apply loop.
    pub(crate) fn into_parts(
        self,
    ) -> (
        CdkAlephBftLocalIo,
        mpsc::UnboundedReceiver<FederationAlephBftUnitPayload>,
    ) {
        (self.local_io, self.finalized_units)
    }
}

/// Tokio-backed task spawner for upstream AlephBFT.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftTokioSpawnHandle {
    spawned_tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl CdkAlephBftTokioSpawnHandle {
    /// Abort every child task spawned through this handle.
    pub(crate) fn abort_all(&self) {
        let Ok(mut tasks) = self.spawned_tasks.lock() else {
            return;
        };
        for task in tasks.drain(..) {
            task.abort();
        }
    }

    fn track(&self, abort_handle: AbortHandle) {
        match self.spawned_tasks.lock() {
            Ok(mut tasks) => tasks.push(abort_handle),
            Err(_) => abort_handle.abort(),
        }
    }
}

impl SpawnHandle for CdkAlephBftTokioSpawnHandle {
    fn spawn(&self, name: &'static str, task: impl Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(async move {
            tracing::trace!(task = name, "AlephBFT task started");
            task.await;
        });
        self.track(handle.abort_handle());
    }

    fn spawn_essential(
        &self,
        name: &'static str,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> TaskHandle {
        let handle = tokio::spawn(async move {
            tracing::trace!(task = name, "AlephBFT essential task started");
            task.await;
        });
        self.track(handle.abort_handle());
        Box::pin(async move {
            match handle.await {
                Ok(()) => Ok(()),
                Err(err) => {
                    tracing::error!("AlephBFT essential task failed: {}", err);
                    Err(())
                }
            }
        }) as Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use aleph_bft::{
        DataProvider, FinalizationHandler, Index, Keychain, MultiKeychain, Network, OrderedUnit,
        PartialMultisignature, Recipient, SpawnHandle, UnitFinalizationHandler,
    };
    use cdk::federation::{
        FederationAlephBftUnitPayload, FederationConfig, FederationConsensusBackendKind,
        FederationConsensusDefaultItem, FederationConsensusItem, FederationConsensusRuntime,
        FederationId, FederationMintQuoteOperation, FederationOperation,
        FederationOperationEnvelope, MemberConfigPrivate, MemberConfigPublic, MemberId,
        ThresholdParams,
    };
    use cdk::mint_url::MintUrl;
    use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response, SecretKey};
    use cdk::{Amount, MintQuoteRequest, MintQuoteResponse};
    use cdk_common::database::DynMintDatabase;
    use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use parity_scale_codec::{Decode, Encode};
    use serde_json::json;

    use super::*;

    fn federation_id(byte: u8) -> FederationId {
        FederationId::from_bytes([byte; 32])
    }

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(id: u16) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = u8::try_from(id).expect("test member id fits in u8");
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    fn public_member(id: u16, identity_secret: &SecretKey) -> MemberConfigPublic {
        MemberConfigPublic {
            member_id: member_id(id),
            public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.test"))
                .expect("mint url"),
            federation_api_url: MintUrl::from_str(&format!("https://fed{id}.example.test"))
                .expect("federation url"),
            identity_public_key: identity_secret.public_key(),
        }
    }

    fn private_member(id: u16, identity_secret: SecretKey) -> MemberConfigPrivate {
        MemberConfigPrivate {
            federation_id: None,
            member_id: member_id(id),
            identity_secret_key: identity_secret,
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        }
    }

    fn keychain_fixture() -> (FederationConfig, Vec<MemberConfigPrivate>) {
        let mut members = BTreeMap::new();
        let mut private_members = Vec::new();
        for id in 1..=4 {
            let secret = identity_secret(id);
            members.insert(member_id(id), public_member(id, &secret));
            private_members.push(private_member(id, secret));
        }
        let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
        let public_config =
            FederationConfig::new(threshold_params, members, Vec::new()).expect("public config");

        (public_config, private_members)
    }

    fn module_fact_payload(index: u64) -> FederationAlephBftUnitPayload {
        FederationAlephBftUnitPayload::new(
            federation_id(7),
            member_id(1),
            0,
            index,
            vec![FederationConsensusItem::Default(
                FederationConsensusDefaultItem::new("adapter-test", json!({ "index": index })),
            )],
        )
        .expect("unit payload")
    }

    fn ordered_unit(
        data: Option<CdkAlephBftData>,
        round: u16,
    ) -> OrderedUnit<CdkAlephBftData, CdkAlephBftHasher> {
        OrderedUnit {
            data,
            parents: Vec::new(),
            hash: [u8::try_from(round).expect("round fits in hash byte"); 32],
            creator: NodeIndex(0),
            round,
        }
    }

    fn mint_quote_envelope(quote: &str) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(8),
            FederationOperation::MintQuote(FederationMintQuoteOperation {
                quote: quote.to_string(),
                request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                    amount: Amount::from(1),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }),
                response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                    quote: quote.to_string(),
                    request: format!("invoice-{quote}"),
                    amount: Some(Amount::from(1)),
                    unit: Some(CurrencyUnit::Sat),
                    state: cdk::nuts::MintQuoteState::Unpaid,
                    expiry: None,
                    pubkey: None,
                }),
            }),
        )
    }

    #[test]
    fn cdk_aleph_bft_keychain_signs_and_verifies_roster_members() {
        let (public_config, private_members) = keychain_fixture();
        let first = CdkAlephBftKeychain::try_new(&public_config, private_members[0].clone())
            .expect("keychain");
        let second = CdkAlephBftKeychain::try_new(&public_config, private_members[1].clone())
            .expect("keychain");
        let message = b"cdk aleph bft keychain message";

        let signature = first.sign(message);

        assert_eq!(first.index(), NodeIndex(0));
        assert_eq!(first.member_for_node(NodeIndex(0)), Some(member_id(1)));
        assert_eq!(first.member_for_node(NodeIndex(3)), Some(member_id(4)));
        assert_eq!(first.member_for_node(NodeIndex(4)), None);
        assert!(first.verify(message, &signature, NodeIndex(0)));
        assert!(!first.verify(b"tampered", &signature, NodeIndex(0)));
        assert!(!first.verify(message, &signature, NodeIndex(99)));
        assert!(!second.verify(message, &signature, second.index()));
    }

    #[test]
    fn cdk_aleph_bft_keychain_requires_bft_multisignature_quorum() {
        let (public_config, private_members) = keychain_fixture();
        let keychains = private_members
            .into_iter()
            .map(|private_config| {
                CdkAlephBftKeychain::try_new(&public_config, private_config).expect("keychain")
            })
            .collect::<Vec<_>>();
        let message = b"cdk aleph bft multisignature message";
        let first = keychains[0].sign(message);
        let second = keychains[1].sign(message);
        let third = keychains[2].sign(message);

        let partial = keychains[0].bootstrap_multi(&first, keychains[0].index());
        assert!(!keychains[0].is_complete(message, &partial));
        let partial = partial.add_signature(&second, keychains[1].index());
        assert!(!keychains[0].is_complete(message, &partial));
        let partial = partial.add_signature(&third, keychains[2].index());
        assert!(keychains[0].is_complete(message, &partial));

        let bad_third = keychains[2].sign(b"different message");
        let bad_partial = keychains[0]
            .bootstrap_multi(&first, keychains[0].index())
            .add_signature(&second, keychains[1].index())
            .add_signature(&bad_third, keychains[2].index());
        assert!(!keychains[0].is_complete(message, &bad_partial));
    }

    #[test]
    fn cdk_aleph_bft_keychain_rejects_non_bft_consensus_threshold() {
        let mut members = BTreeMap::new();
        let mut private_members = Vec::new();
        for id in 1..=5 {
            let secret = identity_secret(id);
            members.insert(member_id(id), public_member(id, &secret));
            private_members.push(private_member(id, secret));
        }
        let public_config = FederationConfig::new(
            ThresholdParams::new(5, 3, 3).expect("threshold params"),
            members,
            Vec::new(),
        )
        .expect("public config");

        let err = CdkAlephBftKeychain::try_new(&public_config, private_members[0].clone())
            .expect_err("non-BFT threshold must fail");

        assert!(
            err.to_string().contains("BFT consensus threshold 4"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cdk_aleph_bft_hasher_uses_stable_sha256() {
        assert_eq!(
            CdkAlephBftHasher::hash(b"cdk aleph bft"),
            [
                0x11, 0xc9, 0xad, 0xb6, 0xa5, 0x45, 0x23, 0x9e, 0x59, 0x15, 0xc1, 0x6a, 0xdd, 0x83,
                0x74, 0x8a, 0x09, 0x51, 0x86, 0x82, 0x5c, 0xc5, 0x35, 0xc2, 0xdb, 0x50, 0xa6, 0xfe,
                0x5f, 0xe6, 0x53, 0xcf,
            ]
        );
    }

    #[test]
    fn cdk_aleph_bft_network_data_alias_is_network_compatible() {
        fn assert_network<N>(_: &N)
        where
            N: Network<CdkAlephBftNetworkData>,
        {
        }

        let (network, _handle) = CdkAlephBftNetwork::<CdkAlephBftNetworkData>::new_pair();

        assert_network(&network);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_preserves_outbound_recipient() {
        let (network, mut handle) = CdkAlephBftNetwork::<String>::new_pair();

        network.send("network-message".to_string(), Recipient::Node(NodeIndex(2)));
        let outbound = handle.next_outgoing().await.expect("outbound message");

        assert_eq!(outbound.data, "network-message");
        assert_eq!(outbound.recipient, Recipient::Node(NodeIndex(2)));
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_delivers_inbound_messages() {
        let (mut network, handle) = CdkAlephBftNetwork::<String>::new_pair();

        handle
            .queue_incoming("inbound-message".to_string())
            .expect("queue inbound message");

        assert_eq!(
            network.next_event().await.as_deref(),
            Some("inbound-message")
        );
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_returns_none_when_inbound_bridge_closes() {
        let (mut network, handle) = CdkAlephBftNetwork::<String>::new_pair();
        drop(handle);

        assert!(network.next_event().await.is_none());
    }

    #[test]
    fn cdk_aleph_bft_data_scale_round_trips_canonical_payload() {
        let payload = module_fact_payload(0);
        let data = CdkAlephBftData::try_from_payload(payload.clone()).expect("data");

        let encoded = data.encode();
        let decoded = CdkAlephBftData::decode(&mut encoded.as_slice()).expect("decode");

        assert_eq!(decoded, data);
        assert_eq!(
            decoded.canonical_bytes(),
            payload.canonical_bytes().expect("canonical")
        );
        assert_eq!(decoded.payload().body_hash, payload.body_hash);
    }

    #[test]
    fn cdk_aleph_bft_data_rejects_non_canonical_scale_bytes() {
        let payload = module_fact_payload(1);
        let mut value = serde_json::to_value(&payload).expect("json value");
        value["items"][0]["data"]["payload"] = json!({ "index": 1 });
        let non_canonical = serde_json::to_vec_pretty(&value).expect("pretty json");
        payload
            .ensure_supported_version()
            .expect("payload still valid");

        let encoded = non_canonical.encode();
        let result = CdkAlephBftData::decode(&mut encoded.as_slice());

        assert!(result.is_err(), "non-canonical bytes must fail");
    }

    #[tokio::test]
    async fn cdk_aleph_bft_data_provider_pulls_from_consensus_runtime_queue() {
        let runtime = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(8),
            member_id(1),
        )
        .expect("runtime");
        let consensus = Arc::new(tokio::sync::Mutex::new(runtime));
        let activity_hint = CdkAlephBftActivityHint::default();
        {
            let mut runtime = consensus.lock().await;
            let admission = runtime
                .submit_aleph_bft_envelope(mint_quote_envelope("quote-1"))
                .expect("submit");
            assert_eq!(
                admission,
                cdk::federation::FederationConsensusItemAdmission::Accepted
            );
        }

        let mut provider =
            CdkAlephBftDataProvider::with_activity_hint(consensus.clone(), activity_hint.clone());
        let data = provider.get_data().await.expect("queued data");

        assert_eq!(data.payload().federation_id, federation_id(8));
        assert_eq!(data.payload().submitting_member, member_id(1));
        assert_eq!(data.payload().unit_index, 0);
        assert!(activity_hint.is_active());
        assert!(provider.get_data().await.is_none());
        assert_eq!(consensus.lock().await.queued_len(), 0);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_data_provider_clears_activity_after_pending_finalization_drains() {
        let runtime = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(8),
            member_id(1),
        )
        .expect("runtime");
        let consensus = Arc::new(tokio::sync::Mutex::new(runtime));
        {
            let mut runtime = consensus.lock().await;
            runtime
                .submit_aleph_bft_envelope(mint_quote_envelope("quote-activity"))
                .expect("submit");
        }

        let activity_hint = CdkAlephBftActivityHint::default();
        let mut provider =
            CdkAlephBftDataProvider::with_activity_hint(consensus.clone(), activity_hint.clone());
        let data = provider.get_data().await.expect("queued data");
        assert!(activity_hint.is_active());
        {
            let mut runtime = consensus.lock().await;
            runtime
                .finalize_aleph_bft_unit(data.payload().clone())
                .expect("finalize unit");
        }

        assert!(provider.get_data().await.is_none());
        assert!(!activity_hint.is_active());
    }

    #[tokio::test]
    async fn cdk_aleph_bft_finalization_handler_forwards_payloads() {
        let payload = module_fact_payload(2);
        let data = CdkAlephBftData::try_from_payload(payload.clone()).expect("data");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut handler = CdkAlephBftFinalizationHandler::new(sender);

        handler.data_finalized(data);
        let finalized = receiver.recv().await.expect("finalized payload");

        assert_eq!(finalized.body_hash, payload.body_hash);
        assert_eq!(finalized.unit_index, payload.unit_index);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_finalization_handler_forwards_batch_payloads() {
        let first = module_fact_payload(3);
        let second = module_fact_payload(4);
        let first_data = CdkAlephBftData::try_from_payload(first.clone()).expect("first data");
        let second_data = CdkAlephBftData::try_from_payload(second.clone()).expect("second data");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut handler = CdkAlephBftFinalizationHandler::new(sender);

        handler.batch_finalized(vec![
            ordered_unit(None, 0),
            ordered_unit(Some(first_data), 1),
            ordered_unit(Some(second_data), 2),
        ]);

        let first_finalized = receiver.recv().await.expect("first finalized payload");
        let second_finalized = receiver.recv().await.expect("second finalized payload");

        assert_eq!(first_finalized.unit_index, first.unit_index);
        assert_eq!(second_finalized.unit_index, second.unit_index);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn cdk_aleph_bft_backup_reader_writer_satisfy_run_session_io_bounds() {
        fn assert_io<R, W>(_: R, _: W)
        where
            R: AsyncRead + Send + Sync + 'static,
            W: AsyncWrite + Send + Sync + 'static,
        {
        }

        let backup = CdkAlephBftBackup::new();

        assert_io(backup.try_reader().expect("backup reader"), backup.writer());
    }

    #[tokio::test]
    async fn cdk_aleph_bft_backup_writer_appends_and_snapshots_bytes() {
        let backup = CdkAlephBftBackup::new();
        let mut writer = backup.writer();

        writer.write_all(b"first").await.expect("write first bytes");
        writer
            .write_all(b"-second")
            .await
            .expect("write second bytes");
        writer.flush().await.expect("flush backup writer");

        assert_eq!(
            backup.snapshot().expect("backup snapshot"),
            b"first-second".to_vec()
        );
    }

    #[tokio::test]
    async fn cdk_aleph_bft_backup_reader_reads_partial_snapshot_to_end() {
        let backup = CdkAlephBftBackup::from_bytes(b"abcdef".to_vec());
        let mut reader = backup.try_reader().expect("backup reader");
        let mut first = [0u8; 2];
        let mut rest = Vec::new();

        reader
            .read_exact(&mut first)
            .await
            .expect("read first bytes");
        reader.read_to_end(&mut rest).await.expect("read rest");

        assert_eq!(&first, b"ab");
        assert_eq!(rest, b"cdef".to_vec());
    }

    #[tokio::test]
    async fn cdk_aleph_bft_backup_reader_isolated_from_later_appends() {
        let backup = CdkAlephBftBackup::from_bytes(b"before".to_vec());
        let mut old_reader = backup.try_reader().expect("old reader");
        backup
            .writer()
            .write_all(b"-after")
            .await
            .expect("append bytes");
        let mut old_snapshot = Vec::new();
        let mut new_snapshot = Vec::new();

        old_reader
            .read_to_end(&mut old_snapshot)
            .await
            .expect("read old snapshot");
        backup
            .try_reader()
            .expect("new reader")
            .read_to_end(&mut new_snapshot)
            .await
            .expect("read new snapshot");

        assert_eq!(old_snapshot, b"before".to_vec());
        assert_eq!(new_snapshot, b"before-after".to_vec());
    }

    #[tokio::test]
    async fn cdk_aleph_bft_session_io_loads_db_backup_and_forwards_finalized_units() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mut tx = db.begin_transaction().await.expect("begin");
        tx.append_federation_aleph_bft_backup_bytes(103, b"persisted")
            .await
            .expect("append persisted backup");
        tx.commit().await.expect("commit persisted backup");
        let runtime = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(8),
            member_id(1),
        )
        .expect("runtime");
        let consensus = Arc::new(tokio::sync::Mutex::new(runtime));
        let session_io = CdkAlephBftSessionIo::from_db(
            Arc::clone(&consensus),
            Arc::clone(&db),
            103,
            CdkAlephBftActivityHint::default(),
        )
        .await
        .expect("session io");
        let (local_io, mut finalized_units) = session_io.into_parts();
        let (mut data_provider, mut finalization_handler, mut unit_saver, mut unit_loader) =
            local_io.into_components();
        let mut loaded = Vec::new();

        unit_loader
            .read_to_end(&mut loaded)
            .await
            .expect("read persisted backup");
        unit_saver
            .write_all(b"-next")
            .await
            .expect("write next backup bytes");
        unit_saver.flush().await.expect("flush next backup bytes");
        finalization_handler.data_finalized(
            CdkAlephBftData::try_from_payload(module_fact_payload(5)).expect("finalized data"),
        );

        assert_eq!(loaded, b"persisted".to_vec());
        assert_eq!(
            db.get_federation_aleph_bft_backup_bytes(103)
                .await
                .expect("read appended backup"),
            Some(b"persisted-next".to_vec())
        );
        assert!(data_provider.get_data().await.is_none());
        assert_eq!(
            finalized_units
                .recv()
                .await
                .expect("forwarded finalized unit")
                .unit_index,
            5
        );
    }

    #[tokio::test]
    async fn cdk_aleph_bft_db_backup_writer_satisfies_run_session_io_bounds() {
        fn assert_writer<W>(_: W)
        where
            W: AsyncWrite + Send + Sync + 'static,
        {
        }

        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let backup = CdkAlephBftDbBackup::new(db, 100);

        assert_writer(backup.writer());
    }

    #[test]
    fn cdk_aleph_bft_db_backup_writer_retries_transient_storage_contention() {
        let sqlite_locked = cdk_common::database::Error::Database(Box::new(std::io::Error::other(
            "database is locked",
        )));
        let sqlite_busy =
            cdk_common::database::Error::Database(Box::new(std::io::Error::other("DatabaseBusy")));

        assert!(crate::is_retryable_federation_database_error(
            &cdk_common::database::Error::Locked
        ));
        assert!(crate::is_retryable_federation_database_error(
            &sqlite_locked
        ));
        assert!(crate::is_retryable_federation_database_error(&sqlite_busy));
        assert!(!crate::is_retryable_federation_database_error(
            &cdk_common::database::Error::InvalidDbResponse
        ));
    }

    #[tokio::test]
    async fn cdk_aleph_bft_db_backup_writer_persists_on_flush_and_close() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let backup = CdkAlephBftDbBackup::new(Arc::clone(&db), 101);
        let mut writer = backup.writer();

        writer.write_all(b"first").await.expect("write first bytes");
        assert_eq!(
            db.get_federation_aleph_bft_backup_bytes(101)
                .await
                .expect("read before flush"),
            None
        );

        writer.flush().await.expect("flush first bytes");
        assert_eq!(
            db.get_federation_aleph_bft_backup_bytes(101)
                .await
                .expect("read after flush"),
            Some(b"first".to_vec())
        );

        writer
            .write_all(b"-second")
            .await
            .expect("write second bytes");
        writer.close().await.expect("close writer");

        assert_eq!(
            db.get_federation_aleph_bft_backup_bytes(101)
                .await
                .expect("read after close"),
            Some(b"first-second".to_vec())
        );
    }

    #[tokio::test]
    async fn cdk_aleph_bft_db_backup_reader_loads_persisted_session_snapshot() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let backup = CdkAlephBftDbBackup::new(Arc::clone(&db), 102);
        let mut tx = db.begin_transaction().await.expect("begin");
        tx.append_federation_aleph_bft_backup_bytes(102, b"before")
            .await
            .expect("append before");
        tx.commit().await.expect("commit before");

        let mut old_reader = backup.try_reader().await.expect("old db reader");
        let mut writer = backup.writer();
        writer
            .write_all(b"-after")
            .await
            .expect("write after bytes");
        writer.flush().await.expect("flush after bytes");

        let mut old_snapshot = Vec::new();
        let mut new_snapshot = Vec::new();
        old_reader
            .read_to_end(&mut old_snapshot)
            .await
            .expect("read old db snapshot");
        backup
            .try_reader()
            .await
            .expect("new db reader")
            .read_to_end(&mut new_snapshot)
            .await
            .expect("read new db snapshot");

        assert_eq!(old_snapshot, b"before".to_vec());
        assert_eq!(new_snapshot, b"before-after".to_vec());
    }

    #[tokio::test]
    async fn cdk_aleph_bft_tokio_spawn_handle_reports_essential_completion() {
        let spawn = CdkAlephBftTokioSpawnHandle::default();
        let handle = spawn.spawn_essential("adapter-test", async {});

        assert!(handle.await.is_ok());
    }

    #[test]
    fn mint_quote_fixture_uses_expected_keysetless_quote_shape() {
        let envelope = mint_quote_envelope("quote-fixture");
        let canonical = envelope.canonical_bytes().expect("canonical envelope");
        let decoded = FederationOperationEnvelope::from_json_slice(&canonical).expect("decode");

        assert_eq!(
            decoded.operation.kind(),
            cdk::federation::FederationOperationKind::MintQuote
        );
        assert_eq!(decoded.federation_id, federation_id(8));
    }
}
