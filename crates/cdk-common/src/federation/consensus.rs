//! Deterministic local consensus boundary.
//!
//! This is not a production quorum protocol. It validates federation identity,
//! appends operations to the accepted journal, and exposes the same
//! status/catch-up/submit payloads used by private REST. Production ordering
//! backends can replace the decision source while keeping this API shape.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{
    FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPeerStatus,
    FederationSessionOutcome, FederationStateDigest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, InMemoryFederationJournal, MemberId, Result,
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES,
};
use serde::{Deserialize, Serialize};

const DEFAULT_CATCH_UP_LIMIT: u16 = 100;
const MAX_CATCH_UP_LIMIT: u16 = 1000;

/// Version of consensus item payloads.
pub const FEDERATION_CONSENSUS_ITEM_VERSION: u16 = 1;

/// Default maximum canonical size for one consensus item.
pub const FEDERATION_MAX_CONSENSUS_ITEM_BYTES: usize =
    FEDERATION_MAX_OPERATION_ENVELOPE_BYTES + 4096;

/// One item that can enter the federation consensus pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationConsensusItem {
    /// A canonical Cashu mint operation envelope.
    Operation(FederationOperationEnvelope),
    /// A module-defined fact that must be ordered with operations.
    ModuleFact(FederationConsensusModuleFact),
    /// A member-signed session outcome.
    SessionSignature(FederationSessionOutcome),
    /// A generic consensus item for future compatibility tests.
    Default(FederationConsensusDefaultItem),
}

impl FederationConsensusItem {
    /// Wrap an operation envelope as a consensus item.
    #[must_use]
    pub fn operation(envelope: FederationOperationEnvelope) -> Self {
        Self::Operation(envelope)
    }

    /// Return canonical JSON bytes with all object keys sorted.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.ensure_supported_version()?;
        canonical_json_bytes(self)
    }

    /// Return this item's canonical size.
    pub fn canonical_size(&self) -> Result<usize> {
        Ok(self.canonical_bytes()?.len())
    }

    /// Reject this item if its canonical encoding exceeds `max_bytes`.
    pub fn ensure_within_size_limit(&self, max_bytes: usize) -> Result<usize> {
        self.ensure_supported_version()?;
        if let Self::Operation(envelope) = self {
            envelope.ensure_admissible_size()?;
        }

        let actual = self.canonical_size()?;
        if actual > max_bytes {
            return Err(FederationError::ConsensusItemTooLarge {
                actual,
                max: max_bytes,
            });
        }

        Ok(actual)
    }

    /// Reject this item if its canonical encoding exceeds the default limit.
    pub fn ensure_admissible_size(&self) -> Result<usize> {
        self.ensure_within_size_limit(FEDERATION_MAX_CONSENSUS_ITEM_BYTES)
    }

    /// Return the operation ID when this item wraps an operation envelope.
    pub fn operation_id(&self) -> Result<Option<FederationOperationId>> {
        match self {
            Self::Operation(envelope) => Ok(Some(envelope.operation_id()?)),
            Self::ModuleFact(_) | Self::SessionSignature(_) | Self::Default(_) => Ok(None),
        }
    }

    /// Reject consensus items from unsupported item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        match self {
            Self::Operation(envelope) => envelope.ensure_supported_version(),
            Self::ModuleFact(item) => item.ensure_supported_version(),
            Self::SessionSignature(outcome) => {
                if outcome.version != super::FEDERATION_SESSION_OUTCOME_VERSION {
                    return Err(FederationError::SessionOutcomeVersionMismatch {
                        expected: super::FEDERATION_SESSION_OUTCOME_VERSION,
                        found: outcome.version,
                    });
                }

                Ok(())
            }
            Self::Default(item) => item.ensure_supported_version(),
        }
    }
}

/// Module-defined fact to order through consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConsensusModuleFact {
    /// Item format version.
    pub version: u16,
    /// Module namespace.
    pub module: String,
    /// Fact kind within the module namespace.
    pub kind: String,
    /// Canonical JSON payload.
    pub payload: serde_json::Value,
}

impl FederationConsensusModuleFact {
    /// Create a module fact consensus item.
    #[must_use]
    pub fn new<M, K>(module: M, kind: K, payload: serde_json::Value) -> Self
    where
        M: Into<String>,
        K: Into<String>,
    {
        Self {
            version: FEDERATION_CONSENSUS_ITEM_VERSION,
            module: module.into(),
            kind: kind.into(),
            payload,
        }
    }

    /// Reject module facts from unsupported consensus-item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        ensure_consensus_item_version(self.version)
    }
}

/// Generic consensus item for forward-compatible item classes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConsensusDefaultItem {
    /// Item format version.
    pub version: u16,
    /// Item kind.
    pub kind: String,
    /// Canonical JSON payload.
    pub payload: serde_json::Value,
}

impl FederationConsensusDefaultItem {
    /// Create a default consensus item.
    #[must_use]
    pub fn new<K>(kind: K, payload: serde_json::Value) -> Self
    where
        K: Into<String>,
    {
        Self {
            version: FEDERATION_CONSENSUS_ITEM_VERSION,
            kind: kind.into(),
            payload,
        }
    }

    /// Reject default items from unsupported consensus-item encodings.
    pub fn ensure_supported_version(&self) -> Result<()> {
        ensure_consensus_item_version(self.version)
    }
}

fn ensure_consensus_item_version(version: u16) -> Result<()> {
    if version != FEDERATION_CONSENSUS_ITEM_VERSION {
        return Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found: version,
        });
    }

    Ok(())
}

/// Admission result for the pending consensus item queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationConsensusItemAdmission {
    /// The item was queued.
    Accepted,
    /// An operation with the same operation ID is already pending.
    DuplicateOperation,
}

/// Bounded pending-item queue for consensus data providers.
#[derive(Debug, Clone)]
pub struct FederationConsensusItemQueue {
    max_item_bytes: usize,
    pending: VecDeque<FederationConsensusItem>,
    pending_operation_ids: BTreeSet<FederationOperationId>,
}

impl Default for FederationConsensusItemQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationConsensusItemQueue {
    /// Create an empty queue with the default item size limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_item_bytes(FEDERATION_MAX_CONSENSUS_ITEM_BYTES)
    }

    /// Create an empty queue with a custom item size limit.
    #[must_use]
    pub fn with_max_item_bytes(max_item_bytes: usize) -> Self {
        Self {
            max_item_bytes,
            pending: VecDeque::new(),
            pending_operation_ids: BTreeSet::new(),
        }
    }

    /// Return the number of queued items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Return true when the queue has no pending items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Submit an item to the pending queue after size and duplicate checks.
    pub fn submit(
        &mut self,
        item: FederationConsensusItem,
    ) -> Result<FederationConsensusItemAdmission> {
        item.ensure_within_size_limit(self.max_item_bytes)?;
        if let Some(operation_id) = item.operation_id()? {
            if !self.pending_operation_ids.insert(operation_id) {
                return Ok(FederationConsensusItemAdmission::DuplicateOperation);
            }
        }

        self.pending.push_back(item);
        Ok(FederationConsensusItemAdmission::Accepted)
    }

    /// Drain up to `limit` items in FIFO order.
    pub fn drain_batch(&mut self, limit: usize) -> Vec<FederationConsensusItem> {
        let mut batch = Vec::new();
        for _ in 0..limit {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            if let Ok(Some(operation_id)) = item.operation_id() {
                self.pending_operation_ids.remove(&operation_id);
            }
            batch.push(item);
        }
        batch
    }
}

/// Minimal synchronous consensus engine surface used by federation runtimes.
///
/// The first implementation is the deterministic in-memory engine below. Real
/// networking or BFT finalization engines should preserve this state-machine
/// boundary: submit admissible items, expose accepted order, serve catch-up,
/// replay peer catch-up, and update only local application status.
pub trait FederationConsensusEngine: fmt::Debug + Send {
    /// Return the backend kind used by this consensus engine.
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    /// Return this member's federation ID.
    fn federation_id(&self) -> FederationId;

    /// Return this member's ID.
    fn member_id(&self) -> MemberId;

    /// Return the accepted operation journal.
    fn journal(&self) -> &InMemoryFederationJournal;

    /// Return this member's consensus-visible state digest.
    fn state_digest(&self) -> FederationStateDigest;

    /// Return a mutable accepted operation journal.
    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal;

    /// Submit an already-built operation envelope.
    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse>;

    /// Submit an operation request from a transport boundary.
    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse>;

    /// Return this member's current peer status.
    fn peer_status(&self) -> FederationPeerStatus;

    /// Return a catch-up request for the next local missing operation.
    fn catch_up_request(&self) -> FederationJournalCatchUpRequest;

    /// Return journal entries for a catch-up request.
    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse;

    /// Return journal entries for a checked catch-up request.
    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse>;

    /// Apply a peer catch-up response to the local journal.
    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>>;

    /// Update local application status for an accepted operation.
    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry>;
}

/// Runtime-owned consensus engine.
///
/// CDK keeps this small enum instead of exposing the in-memory backend through
/// server state. That lets Axum and storage code depend on the consensus engine
/// contract while future engines can be added behind this wrapper.
#[derive(Debug, Clone)]
pub struct FederationConsensusRuntime {
    backend: FederationConsensusRuntimeBackend,
}

/// Consensus backend selected for ordering federation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FederationConsensusBackendKind {
    /// Deterministic local append-log backend for tests and local smoke.
    #[default]
    InMemory,
    /// Future AlephBFT-backed production ordering backend.
    AlephBft,
}

impl FederationConsensusBackendKind {
    /// Return the canonical config string for this backend.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::AlephBft => "aleph_bft",
        }
    }

    /// Return true when this build can instantiate the backend.
    #[must_use]
    pub fn is_available(self) -> bool {
        matches!(self, Self::InMemory)
    }
}

impl fmt::Display for FederationConsensusBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FederationConsensusBackendKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().replace('-', "_").as_str() {
            "in_memory" | "inmemory" | "memory" => Ok(Self::InMemory),
            "aleph_bft" | "alephbft" => Ok(Self::AlephBft),
            _ => Err(format!(
                "Unknown federation consensus backend: {s}. Valid options: in_memory, aleph_bft"
            )),
        }
    }
}

#[derive(Debug, Clone)]
enum FederationConsensusRuntimeBackend {
    InMemory(InMemoryFederationConsensus),
}

impl FederationConsensusRuntime {
    /// Create an in-memory runtime for deterministic tests and local use.
    #[must_use]
    pub fn in_memory(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            backend: FederationConsensusRuntimeBackend::InMemory(InMemoryFederationConsensus::new(
                federation_id,
                member_id,
            )),
        }
    }

    /// Create a runtime for the requested backend.
    pub fn with_backend(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_id: MemberId,
    ) -> Result<Self> {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Ok(Self::in_memory(federation_id, member_id))
            }
            FederationConsensusBackendKind::AlephBft => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: backend.as_str().to_string(),
                })
            }
        }
    }

    /// Return the backend selected for this runtime.
    #[must_use]
    pub fn backend_kind(&self) -> FederationConsensusBackendKind {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(_) => {
                FederationConsensusBackendKind::InMemory
            }
        }
    }

    /// Rebuild an in-memory runtime from persisted journal entries.
    pub fn in_memory_from_entries(
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        Ok(Self {
            backend: FederationConsensusRuntimeBackend::InMemory(
                InMemoryFederationConsensus::from_entries(federation_id, member_id, entries)?,
            ),
        })
    }

    /// Rebuild a runtime for the requested backend from persisted journal entries.
    pub fn from_entries_with_backend(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Self::in_memory_from_entries(federation_id, member_id, entries)
            }
            FederationConsensusBackendKind::AlephBft => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: backend.as_str().to_string(),
                })
            }
        }
    }

    fn engine(&self) -> &dyn FederationConsensusEngine {
        match &self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine,
        }
    }

    fn engine_mut(&mut self) -> &mut dyn FederationConsensusEngine {
        match &mut self.backend {
            FederationConsensusRuntimeBackend::InMemory(engine) => engine,
        }
    }
}

impl FederationConsensusEngine for FederationConsensusRuntime {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        Self::backend_kind(self)
    }

    fn federation_id(&self) -> FederationId {
        self.engine().federation_id()
    }

    fn member_id(&self) -> MemberId {
        self.engine().member_id()
    }

    fn journal(&self) -> &InMemoryFederationJournal {
        self.engine().journal()
    }

    fn state_digest(&self) -> FederationStateDigest {
        self.engine().state_digest()
    }

    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        self.engine_mut().journal_mut()
    }

    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        self.engine_mut().submit_envelope(envelope)
    }

    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        self.engine_mut().submit_operation(request)
    }

    fn peer_status(&self) -> FederationPeerStatus {
        self.engine()
            .peer_status()
            .with_consensus_backend(self.backend_kind())
    }

    fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        self.engine().catch_up_request()
    }

    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        self.engine().catch_up(request)
    }

    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        self.engine().catch_up_checked(request)
    }

    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        self.engine_mut().apply_catch_up_response(response)
    }

    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.engine_mut().update_status(operation_id, status)
    }
}

/// Deterministic multi-member in-memory consensus simulator.
///
/// This is a test engine, not a BFT protocol. It lets CDK model the effects
/// that matter for the mint state machine before a real consensus backend is
/// plugged in: proposals can be delayed, duplicated, and delivered in a
/// different order; reachable members must satisfy the configured quorum; and
/// offline members can catch up from a peer journal.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensusNetwork {
    federation_id: FederationId,
    consensus_threshold: u16,
    members: BTreeMap<MemberId, FederationConsensusRuntime>,
    online_members: BTreeSet<MemberId>,
    pending: VecDeque<InMemoryFederationConsensusProposal>,
}

#[derive(Debug, Clone)]
struct InMemoryFederationConsensusProposal {
    sender: MemberId,
    envelope: FederationOperationEnvelope,
}

impl InMemoryFederationConsensusNetwork {
    /// Create a multi-member simulator with every member initially online.
    pub fn new<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let mut members = BTreeMap::new();
        for member_id in member_ids {
            if members
                .insert(
                    member_id,
                    FederationConsensusRuntime::in_memory(federation_id, member_id),
                )
                .is_some()
            {
                return Err(FederationError::DuplicateMember { member_id });
            }
        }

        let member_count = members.len() as u16;
        if consensus_threshold == 0 || consensus_threshold > member_count {
            return Err(FederationError::InvalidThresholdParams {
                members: member_count,
                signature_threshold: consensus_threshold,
                consensus_threshold,
            });
        }

        Ok(Self {
            federation_id,
            consensus_threshold,
            online_members: members.keys().copied().collect(),
            members,
            pending: VecDeque::new(),
        })
    }

    /// Return the number of queued proposals.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Return the member IDs known to this simulator.
    pub fn member_ids(&self) -> impl Iterator<Item = MemberId> + '_ {
        self.members.keys().copied()
    }

    /// Mark a member online or offline.
    pub fn set_online(&mut self, member_id: MemberId, online: bool) -> Result<()> {
        self.ensure_member(member_id)?;
        if online {
            self.online_members.insert(member_id);
        } else {
            self.online_members.remove(&member_id);
        }
        Ok(())
    }

    /// Queue a proposal from a known online member.
    pub fn submit(
        &mut self,
        sender: MemberId,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationOperationId> {
        self.ensure_member(sender)?;
        self.ensure_online(sender)?;
        self.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        let operation_id = envelope.operation_id()?;
        self.pending
            .push_back(InMemoryFederationConsensusProposal { sender, envelope });
        Ok(operation_id)
    }

    /// Duplicate the next queued proposal at the back of the queue.
    ///
    /// Returns false when the queue is empty.
    pub fn duplicate_next_proposal(&mut self) -> bool {
        let Some(proposal) = self.pending.front().cloned() else {
            return false;
        };
        self.pending.push_back(proposal);
        true
    }

    /// Move the next queued proposal to the back of the queue.
    ///
    /// Returns false when the queue is empty.
    pub fn delay_next_proposal(&mut self) -> bool {
        let Some(proposal) = self.pending.pop_front() else {
            return false;
        };
        self.pending.push_back(proposal);
        true
    }

    /// Deliver one queued proposal to all currently online members.
    pub fn deliver_next(&mut self) -> Result<Option<FederationSubmitOperationResponse>> {
        self.ensure_quorum_reachable()?;
        let Some(proposal) = self.pending.pop_front() else {
            return Ok(None);
        };
        self.ensure_online(proposal.sender)?;

        let mut next_members = self.members.clone();
        let mut response = None;
        for member_id in &self.online_members {
            let member = next_members
                .get_mut(member_id)
                .ok_or(FederationError::UnknownMember {
                    member_id: *member_id,
                })?;
            let member_response = member.submit_envelope(proposal.envelope.clone())?;
            response.get_or_insert(member_response);
        }
        self.members = next_members;

        Ok(response)
    }

    /// Deliver all queued proposals that can be finalized with the current quorum.
    pub fn deliver_all(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        let mut responses = Vec::new();
        while self.pending_len() > 0 {
            if let Some(response) = self.deliver_next()? {
                responses.push(response);
            }
        }
        Ok(responses)
    }

    /// Catch one member up from another member's accepted journal.
    pub fn catch_up_member_from_peer(
        &mut self,
        member_id: MemberId,
        peer_id: MemberId,
    ) -> Result<Vec<FederationJournalEntry>> {
        let mut member = self.member(member_id)?.clone();
        let peer = self.member(peer_id)?.clone();
        let peer_status = peer.peer_status();
        let mut applied = Vec::new();

        while member.peer_status().operation_count < peer_status.operation_count {
            let before = member.peer_status().operation_count;
            let request = member.catch_up_request();
            let response = peer.catch_up_checked(&request)?;
            let next_applied = member.apply_catch_up_response(&response)?;
            if member.peer_status().operation_count == before {
                return Err(FederationError::CatchUpNoProgress { index: before });
            }
            applied.extend(next_applied);
        }

        self.members.insert(member_id, member);
        Ok(applied)
    }

    /// Return a member's current peer status.
    pub fn member_status(&self, member_id: MemberId) -> Result<FederationPeerStatus> {
        Ok(self.member(member_id)?.peer_status())
    }

    /// Return a member's accepted operation IDs in order.
    pub fn accepted_operation_ids(
        &self,
        member_id: MemberId,
    ) -> Result<Vec<FederationOperationId>> {
        Ok(self
            .member(member_id)?
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect())
    }

    /// Return true when every member has the same accepted sequence and state digest.
    #[must_use]
    pub fn all_members_converged(&self) -> bool {
        let mut members = self.members.values();
        let Some(first) = members.next() else {
            return true;
        };
        let first_ids: Vec<_> = first
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect();
        let first_state_digest = first.state_digest();
        members.all(|member| {
            member.state_digest() == first_state_digest
                && member
                    .journal()
                    .entries()
                    .iter()
                    .map(|entry| entry.operation_id)
                    .eq(first_ids.iter().copied())
        })
    }

    fn member(&self, member_id: MemberId) -> Result<&FederationConsensusRuntime> {
        self.members
            .get(&member_id)
            .ok_or(FederationError::UnknownMember { member_id })
    }

    fn ensure_member(&self, member_id: MemberId) -> Result<()> {
        self.member(member_id).map(|_| ())
    }

    fn ensure_online(&self, member_id: MemberId) -> Result<()> {
        if !self.online_members.contains(&member_id) {
            return Err(FederationError::ConsensusQuorumUnavailable {
                reachable: self.online_members.len() as u16,
                required: self.consensus_threshold,
            });
        }
        Ok(())
    }

    fn ensure_quorum_reachable(&self) -> Result<()> {
        let reachable = self.online_members.len() as u16;
        if reachable < self.consensus_threshold {
            return Err(FederationError::ConsensusQuorumUnavailable {
                reachable,
                required: self.consensus_threshold,
            });
        }
        Ok(())
    }

    fn ensure_federation(&self, found: FederationId) -> Result<()> {
        if found != self.federation_id {
            return Err(FederationError::OperationFederationMismatch {
                expected: self.federation_id,
                found,
            });
        }
        Ok(())
    }
}

/// Backend-neutral operation adapter used by consensus ordering engines.
///
/// The in-memory implementation dispatches into the deterministic simulator. A
/// production BFT backend should implement the same admission, dispatch, and
/// finalized-response boundary without changing public mint handlers.
pub trait FederationConsensusBackendAdapter: fmt::Debug + Send {
    /// Return the backend kind used by this adapter.
    fn backend_kind(&self) -> FederationConsensusBackendKind;

    /// Return the number of locally queued consensus items.
    fn queued_len(&self) -> usize;

    /// Return the number of items dispatched but not yet finalized.
    fn pending_finalization_len(&self) -> usize;

    /// Queue one operation envelope for later backend dispatch.
    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission>;

    /// Dispatch queued operation items to the backend without finalizing them.
    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize>;

    /// Deliver every dispatched operation finalized by the backend.
    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>>;

    /// Dispatch queued operations and return finalized responses.
    fn dispatch_and_finalize(
        &mut self,
        limit: usize,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.dispatch_queued_operations(limit)?;
        self.finalize_dispatched_operations()
    }
}

/// Runtime-selected consensus backend adapter.
#[derive(Debug, Clone)]
pub struct FederationConsensusBackendAdapterRuntime {
    backend: FederationConsensusBackendAdapterRuntimeBackend,
}

#[derive(Debug, Clone)]
enum FederationConsensusBackendAdapterRuntimeBackend {
    InMemory(InMemoryFederationConsensusAdapter),
}

impl FederationConsensusBackendAdapterRuntime {
    /// Create an in-memory backend adapter runtime.
    pub fn in_memory<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        Ok(Self {
            backend: FederationConsensusBackendAdapterRuntimeBackend::InMemory(
                InMemoryFederationConsensusAdapter::new(
                    federation_id,
                    member_ids,
                    consensus_threshold,
                    local_member,
                )?,
            ),
        })
    }

    /// Create an adapter runtime for the selected backend.
    pub fn with_backend<I>(
        backend: FederationConsensusBackendKind,
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        match backend {
            FederationConsensusBackendKind::InMemory => {
                Self::in_memory(federation_id, member_ids, consensus_threshold, local_member)
            }
            FederationConsensusBackendKind::AlephBft => {
                Err(FederationError::UnsupportedConsensusBackend {
                    backend: backend.as_str().to_string(),
                })
            }
        }
    }

    fn adapter(&self) -> &dyn FederationConsensusBackendAdapter {
        match &self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => adapter,
        }
    }

    fn adapter_mut(&mut self) -> &mut dyn FederationConsensusBackendAdapter {
        match &mut self.backend {
            FederationConsensusBackendAdapterRuntimeBackend::InMemory(adapter) => adapter,
        }
    }
}

impl FederationConsensusBackendAdapter for FederationConsensusBackendAdapterRuntime {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        self.adapter().backend_kind()
    }

    fn queued_len(&self) -> usize {
        self.adapter().queued_len()
    }

    fn pending_finalization_len(&self) -> usize {
        self.adapter().pending_finalization_len()
    }

    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.adapter_mut().submit_operation(envelope)
    }

    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        self.adapter_mut().dispatch_queued_operations(limit)
    }

    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.adapter_mut().finalize_dispatched_operations()
    }
}

/// Operation-focused adapter between the pending consensus item queue and the
/// deterministic multi-member simulator.
///
/// This in-memory adapter keeps public submit paths, pending consensus item
/// admission, and finalized operation delivery connected through one narrow
/// surface while production BFT backends are added behind the same trait.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensusAdapter {
    local_member: MemberId,
    queue: FederationConsensusItemQueue,
    network: InMemoryFederationConsensusNetwork,
}

impl InMemoryFederationConsensusAdapter {
    /// Create an adapter with an empty pending item queue.
    pub fn new<I>(
        federation_id: FederationId,
        member_ids: I,
        consensus_threshold: u16,
        local_member: MemberId,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = MemberId>,
    {
        let network = InMemoryFederationConsensusNetwork::new(
            federation_id,
            member_ids,
            consensus_threshold,
        )?;
        network.ensure_member(local_member)?;
        Ok(Self {
            local_member,
            queue: FederationConsensusItemQueue::new(),
            network,
        })
    }

    /// Return the number of locally queued consensus items.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }

    /// Return the number of proposals already dispatched to the simulator.
    #[must_use]
    pub fn network_pending_len(&self) -> usize {
        self.network.pending_len()
    }

    /// Mark a simulated member online or offline.
    pub fn set_online(&mut self, member_id: MemberId, online: bool) -> Result<()> {
        self.network.set_online(member_id, online)
    }

    /// Queue one operation envelope for later backend dispatch.
    pub fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        self.network.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        self.queue
            .submit(FederationConsensusItem::operation(envelope))
    }

    /// Dispatch queued operation items to the simulator without finalizing them.
    pub fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        self.network.ensure_online(self.local_member)?;
        let mut dispatched = 0;
        for item in self.queue.drain_batch(limit) {
            match item {
                FederationConsensusItem::Operation(envelope) => {
                    self.network.submit(self.local_member, envelope)?;
                    dispatched += 1;
                }
                FederationConsensusItem::ModuleFact(_)
                | FederationConsensusItem::SessionSignature(_)
                | FederationConsensusItem::Default(_) => {}
            }
        }

        Ok(dispatched)
    }

    /// Deliver every dispatched operation that can be finalized by quorum.
    pub fn finalize_dispatched_operations(
        &mut self,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.network.deliver_all()
    }

    /// Dispatch queued operation items and then deliver every finalized result.
    pub fn dispatch_and_finalize(
        &mut self,
        limit: usize,
    ) -> Result<Vec<FederationSubmitOperationResponse>> {
        self.dispatch_queued_operations(limit)?;
        self.finalize_dispatched_operations()
    }

    /// Return one member's accepted operation IDs in order.
    pub fn accepted_operation_ids(
        &self,
        member_id: MemberId,
    ) -> Result<Vec<FederationOperationId>> {
        self.network.accepted_operation_ids(member_id)
    }

    /// Return true when all simulated members agree on order and state digest.
    #[must_use]
    pub fn all_members_converged(&self) -> bool {
        self.network.all_members_converged()
    }
}

impl FederationConsensusBackendAdapter for InMemoryFederationConsensusAdapter {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    fn queued_len(&self) -> usize {
        Self::queued_len(self)
    }

    fn pending_finalization_len(&self) -> usize {
        Self::network_pending_len(self)
    }

    fn submit_operation(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationConsensusItemAdmission> {
        Self::submit_operation(self, envelope)
    }

    fn dispatch_queued_operations(&mut self, limit: usize) -> Result<usize> {
        Self::dispatch_queued_operations(self, limit)
    }

    fn finalize_dispatched_operations(&mut self) -> Result<Vec<FederationSubmitOperationResponse>> {
        Self::finalize_dispatched_operations(self)
    }
}

/// Deterministic local consensus driver used before real quorum networking.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensus {
    federation_id: FederationId,
    member_id: MemberId,
    journal: InMemoryFederationJournal,
}

impl InMemoryFederationConsensus {
    /// Create an empty local consensus driver for one federation member.
    #[must_use]
    pub fn new(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            federation_id,
            member_id,
            journal: InMemoryFederationJournal::new(),
        }
    }

    /// Rebuild local consensus from persisted journal entries.
    pub fn from_entries(
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        let journal = InMemoryFederationJournal::from_entries(entries)?;
        let consensus = Self {
            federation_id,
            member_id,
            journal,
        };

        for entry in consensus.journal.entries() {
            consensus.ensure_federation(entry.envelope.federation_id)?;
        }

        Ok(consensus)
    }

    /// Return this member's federation ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.federation_id
    }

    /// Return this member's ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Return the underlying journal.
    #[must_use]
    pub fn journal(&self) -> &InMemoryFederationJournal {
        &self.journal
    }

    /// Return a mutable reference to the underlying journal.
    pub fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        &mut self.journal
    }

    /// Submit an already-built operation envelope.
    pub fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        self.ensure_federation(envelope.federation_id)?;
        envelope.ensure_admissible_size()?;
        let outcome = self.journal.append(envelope)?;
        Ok(FederationSubmitOperationResponse::from_outcome(outcome))
    }

    /// Submit an operation request from the private REST transport.
    pub fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        self.submit_envelope(request.envelope)
    }

    /// Return this member's current peer status.
    #[must_use]
    pub fn peer_status(&self) -> FederationPeerStatus {
        FederationPeerStatus::from_journal_with_backend(
            self.federation_id,
            self.member_id,
            &self.journal,
            FederationConsensusBackendKind::InMemory,
        )
    }

    /// Return a catch-up request for the next local missing operation.
    #[must_use]
    pub fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        FederationJournalCatchUpRequest::new(self.journal.next_index())
            .with_known_order_digest(self.journal.latest_order_digest())
    }

    /// Return journal entries for a catch-up request.
    #[must_use]
    pub fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        FederationJournalCatchUpResponse::from_journal(
            self.federation_id,
            &self.journal,
            request,
            DEFAULT_CATCH_UP_LIMIT,
            MAX_CATCH_UP_LIMIT,
        )
    }

    /// Return journal entries for a catch-up request after checking the caller's digest.
    pub fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        if let Some(known_order_digest) = request.known_order_digest {
            if let Some(expected_digest) = self.journal.order_digest_before(request.next_index) {
                if expected_digest != known_order_digest {
                    return Err(FederationError::CatchUpDigestMismatch {
                        index: request.next_index,
                        expected: expected_digest,
                        found: known_order_digest,
                    });
                }
            }
        }

        Ok(self.catch_up(request))
    }

    /// Apply a peer catch-up response to the local journal.
    ///
    /// This only replays the accepted operation order. Local application status
    /// is not copied from the peer, because each member must apply entries to
    /// its own mint database and recovery state independently.
    pub fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        if response.federation_id != self.federation_id {
            return Err(FederationError::CatchUpFederationMismatch {
                expected: self.federation_id,
                found: response.federation_id,
            });
        }

        let expected_from_index = self.journal.next_index();
        if response.from_index != expected_from_index {
            return Err(FederationError::CatchUpIndexMismatch {
                expected: expected_from_index,
                found: response.from_index,
            });
        }
        if response.entries.is_empty() && !response.complete {
            return Err(FederationError::CatchUpNoProgress {
                index: expected_from_index,
            });
        }

        let mut applied = Vec::with_capacity(response.entries.len());
        for entry in &response.entries {
            self.ensure_federation(entry.envelope.federation_id)?;
            applied.push(self.journal.append_catch_up_entry(entry)?);
        }

        if response.complete && response.latest_order_digest != self.journal.latest_order_digest() {
            return Err(FederationError::CatchUpDigestMismatch {
                index: self.journal.next_index(),
                expected: self.journal.latest_order_digest(),
                found: response.latest_order_digest,
            });
        }

        Ok(applied)
    }

    /// Update local application status for an accepted operation.
    pub fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.journal.update_status(operation_id, status)
    }

    fn ensure_federation(&self, found: FederationId) -> Result<()> {
        if found != self.federation_id {
            return Err(FederationError::OperationFederationMismatch {
                expected: self.federation_id,
                found,
            });
        }

        Ok(())
    }
}

impl FederationConsensusEngine for InMemoryFederationConsensus {
    fn backend_kind(&self) -> FederationConsensusBackendKind {
        FederationConsensusBackendKind::InMemory
    }

    fn federation_id(&self) -> FederationId {
        Self::federation_id(self)
    }

    fn member_id(&self) -> MemberId {
        Self::member_id(self)
    }

    fn journal(&self) -> &InMemoryFederationJournal {
        Self::journal(self)
    }

    fn state_digest(&self) -> FederationStateDigest {
        self.journal.state_digest()
    }

    fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        Self::journal_mut(self)
    }

    fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        Self::submit_envelope(self, envelope)
    }

    fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        Self::submit_operation(self, request)
    }

    fn peer_status(&self) -> FederationPeerStatus {
        Self::peer_status(self)
    }

    fn catch_up_request(&self) -> FederationJournalCatchUpRequest {
        Self::catch_up_request(self)
    }

    fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        Self::catch_up(self, request)
    }

    fn catch_up_checked(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> Result<FederationJournalCatchUpResponse> {
        Self::catch_up_checked(self, request)
    }

    fn apply_catch_up_response(
        &mut self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>> {
        Self::apply_catch_up_response(self, response)
    }

    fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        Self::update_status(self, operation_id, status)
    }
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    let mut value = serde_json::to_value(value)?;
    sort_json_value(&mut value);
    Ok(serde_json::to_vec(&value)?)
}

fn sort_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.sort_keys();
            for value in map.values_mut() {
                sort_json_value(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sort_json_value(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use serde_json::json;

    use super::*;
    use crate::{
        Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey,
    };

    fn federation_id(byte: u8) -> FederationId {
        FederationId::from_bytes([byte; 32])
    }

    fn member_id() -> MemberId {
        MemberId::new(1).expect("member id")
    }

    fn member_id_value(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn keyset_id() -> Id {
        Id::from_str("00bfa73302d12ffd").expect("keyset id")
    }

    fn output(hex: &str) -> BlindedMessage {
        BlindedMessage::new(
            Amount::from(1),
            keyset_id(),
            PublicKey::from_hex(hex).expect("public key"),
        )
    }

    fn mint_envelope(
        federation_id: FederationId,
        quote: &str,
        hex: &str,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            super::super::FederationOperation::Mint(super::super::FederationMintOperation {
                request: super::super::FederationMintRequest::Single(MintRequest {
                    quote: quote.to_string(),
                    outputs: vec![output(hex)],
                    signature: None,
                }),
            }),
        )
    }

    fn mint_quote_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            super::super::FederationOperation::MintQuote(
                super::super::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                        amount: Amount::from(1),
                        unit: CurrencyUnit::Sat,
                        description: None,
                        pubkey: None,
                    }),
                    response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                        quote: "quote-1".to_string(),
                        request: "invoice-quote-1".to_string(),
                        amount: Some(Amount::from(1)),
                        unit: Some(CurrencyUnit::Sat),
                        state: crate::MintQuoteState::Unpaid,
                        expiry: None,
                        pubkey: None,
                    }),
                },
            ),
        )
    }

    fn oversized_mint_quote_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        let mut envelope = mint_quote_envelope(federation_id);
        let super::super::FederationOperation::MintQuote(operation) = &mut envelope.operation
        else {
            panic!("mint quote operation");
        };
        let crate::MintQuoteRequest::Bolt11(request) = &mut operation.request else {
            panic!("bolt11 request");
        };
        request.description =
            Some("x".repeat(super::super::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
        envelope
    }

    #[test]
    fn dummy_consensus_accepts_duplicates_and_rejects_conflicts() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let first = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let duplicate = first.clone();
        let conflict = mint_envelope(
            federation_id,
            "quote-1",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );

        let first_response = consensus.submit_envelope(first).expect("submit");
        let duplicate_response = consensus.submit_envelope(duplicate).expect("duplicate");

        assert_eq!(first_response.operation_id, duplicate_response.operation_id);
        assert_eq!(consensus.peer_status().operation_count, 1);
        assert!(matches!(
            consensus.submit_envelope(conflict),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn consensus_engine_trait_accepts_operations_in_submit_order() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let engine: &mut dyn FederationConsensusEngine = &mut consensus;

        let first = engine
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit first");
        let second = engine
            .submit_envelope(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit second");

        assert_eq!(engine.federation_id(), federation_id);
        assert_eq!(engine.member_id(), member_id());
        assert_eq!(first.outcome.entry().index, 0);
        assert_eq!(second.outcome.entry().index, 1);
        assert_eq!(engine.peer_status().operation_count, 2);
        assert_eq!(engine.state_digest(), engine.peer_status().state_digest);
        assert_eq!(
            engine.journal().entries()[0].operation_id,
            first.operation_id
        );
        assert_eq!(
            engine.journal().entries()[1].operation_id,
            second.operation_id
        );
    }

    #[test]
    fn consensus_runtime_replays_persisted_in_memory_entries() {
        let federation_id = federation_id(13);
        let mut runtime = FederationConsensusRuntime::in_memory(federation_id, member_id());
        let first = runtime
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit first");
        let second = runtime
            .submit_envelope(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit second");
        runtime
            .update_status(second.operation_id, FederationOperationStatus::Applied)
            .expect("update status");

        let replayed = FederationConsensusRuntime::in_memory_from_entries(
            federation_id,
            member_id(),
            runtime.journal().entries().to_vec(),
        )
        .expect("replay runtime");

        assert_eq!(replayed.peer_status().operation_count, 2);
        assert_eq!(
            replayed.peer_status().latest_order_digest,
            runtime.peer_status().latest_order_digest
        );
        assert_eq!(replayed.state_digest(), runtime.state_digest());
        assert_eq!(
            replayed
                .journal()
                .entry_by_operation_id(first.operation_id)
                .expect("first entry")
                .index,
            0
        );
        assert_eq!(
            replayed
                .journal()
                .entry_by_operation_id(second.operation_id)
                .expect("second entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[test]
    fn consensus_runtime_replay_rejects_future_operation_versions() {
        let federation_id = federation_id(13);
        let mut runtime = FederationConsensusRuntime::in_memory(federation_id, member_id());
        runtime
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit first");
        let mut entries = runtime.journal().entries().to_vec();
        entries[0].envelope.version = super::super::FEDERATION_OPERATION_VERSION + 1;

        assert!(matches!(
            FederationConsensusRuntime::in_memory_from_entries(federation_id, member_id(), entries),
            Err(FederationError::OperationVersionMismatch {
                expected: super::super::FEDERATION_OPERATION_VERSION,
                found,
            }) if found == super::super::FEDERATION_OPERATION_VERSION + 1
        ));
    }

    #[test]
    fn consensus_backend_kind_parses_known_values_and_rejects_unknown() {
        assert_eq!(
            "in_memory"
                .parse::<FederationConsensusBackendKind>()
                .expect("in-memory backend"),
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(
            "memory"
                .parse::<FederationConsensusBackendKind>()
                .expect("memory alias"),
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(
            "aleph-bft"
                .parse::<FederationConsensusBackendKind>()
                .expect("aleph alias"),
            FederationConsensusBackendKind::AlephBft
        );
        assert!("raft"
            .parse::<FederationConsensusBackendKind>()
            .expect_err("unknown backend should fail")
            .contains("Unknown federation consensus backend"));
    }

    #[test]
    fn consensus_runtime_rejects_unavailable_backend_kind() {
        let err = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(13),
            member_id(),
        )
        .expect_err("aleph-bft backend is not implemented yet");

        assert!(matches!(
            err,
            FederationError::UnsupportedConsensusBackend { ref backend }
                if backend == "aleph_bft"
        ));
    }

    #[test]
    fn consensus_runtime_replays_entries_with_selected_backend() {
        let federation_id = federation_id(13);
        let mut runtime = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::InMemory,
            federation_id,
            member_id(),
        )
        .expect("in-memory runtime");
        runtime
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit first");

        let replayed = FederationConsensusRuntime::from_entries_with_backend(
            FederationConsensusBackendKind::InMemory,
            federation_id,
            member_id(),
            runtime.journal().entries().to_vec(),
        )
        .expect("replay runtime");

        assert_eq!(
            replayed.backend_kind(),
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(
            replayed.peer_status().consensus_backend,
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(replayed.peer_status().operation_count, 1);
        assert_eq!(
            replayed.peer_status().latest_order_digest,
            runtime.peer_status().latest_order_digest
        );
    }

    #[test]
    fn consensus_item_queue_batches_and_deduplicates_operations() {
        let federation_id = federation_id(13);
        let envelope = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut queue = FederationConsensusItemQueue::new();

        assert_eq!(
            queue
                .submit(FederationConsensusItem::operation(envelope.clone()))
                .expect("submit operation"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(
            queue
                .submit(FederationConsensusItem::operation(envelope.clone()))
                .expect("submit duplicate operation"),
            FederationConsensusItemAdmission::DuplicateOperation
        );
        assert_eq!(
            queue
                .submit(FederationConsensusItem::ModuleFact(
                    FederationConsensusModuleFact::new("mint", "fact", json!({"ok": true}))
                ))
                .expect("submit module fact"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(queue.len(), 2);

        let first_batch = queue.drain_batch(1);
        assert_eq!(first_batch.len(), 1);
        assert_eq!(
            queue
                .submit(FederationConsensusItem::operation(envelope))
                .expect("resubmit drained operation"),
            FederationConsensusItemAdmission::Accepted
        );

        let second_batch = queue.drain_batch(10);
        assert_eq!(second_batch.len(), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn consensus_item_queue_rejects_oversized_items() {
        let mut queue = FederationConsensusItemQueue::with_max_item_bytes(128);
        let item = FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
            "large",
            json!({"body": "x".repeat(256)}),
        ));

        assert!(matches!(
            queue.submit(item),
            Err(FederationError::ConsensusItemTooLarge { .. })
        ));
        assert!(queue.is_empty());
    }

    #[test]
    fn consensus_item_queue_rejects_future_module_fact_versions() {
        let mut queue = FederationConsensusItemQueue::new();
        let mut fact = FederationConsensusModuleFact::new("mint", "fact", json!({"ok": true}));
        fact.version = FEDERATION_CONSENSUS_ITEM_VERSION + 1;
        let item = FederationConsensusItem::ModuleFact(fact);

        assert!(matches!(
            item.canonical_bytes(),
            Err(FederationError::ConsensusItemVersionMismatch {
                expected: FEDERATION_CONSENSUS_ITEM_VERSION,
                found,
            }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
        ));
        assert!(matches!(
            queue.submit(item),
            Err(FederationError::ConsensusItemVersionMismatch {
                expected: FEDERATION_CONSENSUS_ITEM_VERSION,
                found,
            }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
        ));
        assert!(queue.is_empty());
    }

    #[test]
    fn consensus_item_queue_rejects_future_default_item_versions() {
        let mut queue = FederationConsensusItemQueue::new();
        let mut item = FederationConsensusDefaultItem::new("future", json!({"ok": true}));
        item.version = FEDERATION_CONSENSUS_ITEM_VERSION + 1;

        assert!(matches!(
            queue.submit(FederationConsensusItem::Default(item)),
            Err(FederationError::ConsensusItemVersionMismatch {
                expected: FEDERATION_CONSENSUS_ITEM_VERSION,
                found,
            }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
        ));
        assert!(queue.is_empty());
    }

    #[test]
    fn consensus_item_canonical_bytes_sort_json_payload_keys() {
        let first = FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
            "mint",
            "fact",
            json!({"b": 2, "a": {"d": 4, "c": 3}}),
        ));
        let second = FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
            "mint",
            "fact",
            json!({"a": {"c": 3, "d": 4}, "b": 2}),
        ));

        assert_eq!(
            first.canonical_bytes().expect("first canonical bytes"),
            second.canonical_bytes().expect("second canonical bytes")
        );
    }

    #[test]
    fn multi_node_engine_converges_with_reordered_duplicate_delayed_messages() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut network =
            InMemoryFederationConsensusNetwork::new(federation_id, members, 2).expect("network");
        let first = network
            .submit(
                members[0],
                mint_envelope(
                    federation_id,
                    "quote-1",
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                ),
            )
            .expect("submit first");
        let second = network
            .submit(
                members[1],
                mint_envelope(
                    federation_id,
                    "quote-2",
                    "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
                ),
            )
            .expect("submit second");
        let third = network
            .submit(
                members[2],
                mint_envelope(
                    federation_id,
                    "quote-3",
                    "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
                ),
            )
            .expect("submit third");

        assert!(network.delay_next_proposal());
        assert!(network.duplicate_next_proposal());
        let responses = network.deliver_all().expect("deliver all");

        assert_eq!(responses.len(), 4);
        let expected_order = vec![second, third, first];
        for member in members {
            assert_eq!(
                network
                    .member_status(member)
                    .expect("member status")
                    .operation_count,
                3
            );
            assert_eq!(
                network
                    .accepted_operation_ids(member)
                    .expect("accepted ids"),
                expected_order
            );
        }
        assert!(network.all_members_converged());
    }

    #[test]
    fn multi_node_engine_requires_quorum_and_catches_up_offline_member() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut network =
            InMemoryFederationConsensusNetwork::new(federation_id, members, 2).expect("network");
        network
            .set_online(members[1], false)
            .expect("member 2 offline");
        network
            .set_online(members[2], false)
            .expect("member 3 offline");
        network
            .submit(
                members[0],
                mint_envelope(
                    federation_id,
                    "quote-1",
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                ),
            )
            .expect("submit");

        assert!(matches!(
            network.deliver_next(),
            Err(FederationError::ConsensusQuorumUnavailable {
                reachable: 1,
                required: 2
            })
        ));
        assert_eq!(network.pending_len(), 1);
        assert_eq!(
            network
                .member_status(members[0])
                .expect("member 1 status")
                .operation_count,
            0
        );

        network
            .set_online(members[1], true)
            .expect("member 2 online");
        network.deliver_next().expect("deliver with quorum");
        assert_eq!(
            network
                .member_status(members[0])
                .expect("member 1 status")
                .operation_count,
            1
        );
        assert_eq!(
            network
                .member_status(members[2])
                .expect("member 3 status")
                .operation_count,
            0
        );
        assert!(!network.all_members_converged());

        network
            .catch_up_member_from_peer(members[2], members[0])
            .expect("catch up member 3");
        assert!(network.all_members_converged());
    }

    #[test]
    fn multi_node_engine_catches_up_offline_member_across_default_limit() {
        let federation_id = federation_id(31);
        let members = (1..=4).map(member_id_value).collect::<Vec<_>>();
        let public_keys = [
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
        ];
        let mut network =
            InMemoryFederationConsensusNetwork::new(federation_id, members.clone(), 3)
                .expect("network");

        network
            .set_online(members[3], false)
            .expect("member 4 offline");

        let mut expected_order = Vec::new();
        for index in 0..105 {
            let quote = format!("offline-catch-up-{index}");
            let sender = members[index % 3];
            expected_order.push(
                network
                    .submit(
                        sender,
                        mint_envelope(
                            federation_id,
                            &quote,
                            public_keys[index % public_keys.len()],
                        ),
                    )
                    .expect("submit operation"),
            );
        }

        let delivered = network.deliver_all().expect("deliver operations");
        assert_eq!(delivered.len(), expected_order.len());
        for member in members.iter().take(3).copied() {
            assert_eq!(
                network
                    .accepted_operation_ids(member)
                    .expect("online accepted ids"),
                expected_order
            );
        }
        assert_eq!(
            network
                .member_status(members[3])
                .expect("offline member status")
                .operation_count,
            0
        );

        let applied = network
            .catch_up_member_from_peer(members[3], members[0])
            .expect("catch up offline member");

        assert_eq!(applied.len(), expected_order.len());
        assert_eq!(
            network
                .accepted_operation_ids(members[3])
                .expect("caught-up accepted ids"),
            expected_order
        );
        assert!(network.all_members_converged());
    }

    #[test]
    fn multi_node_engine_scales_to_seven_members_with_threshold_partition_catch_up() {
        let federation_id = federation_id(14);
        let members = (1..=7).map(member_id_value).collect::<Vec<_>>();
        let public_keys = [
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
        ];
        let mut network =
            InMemoryFederationConsensusNetwork::new(federation_id, members.clone(), 5)
                .expect("network");

        network
            .set_online(members[5], false)
            .expect("member 6 offline");
        network
            .set_online(members[6], false)
            .expect("member 7 offline");

        let mut operation_ids = Vec::new();
        for index in 0..6 {
            let quote = format!("quote-{index}");
            let sender = members[index % 5];
            operation_ids.push(
                network
                    .submit(
                        sender,
                        mint_envelope(
                            federation_id,
                            &quote,
                            public_keys[index % public_keys.len()],
                        ),
                    )
                    .expect("submit operation"),
            );
        }

        assert!(network.delay_next_proposal());
        assert!(network.duplicate_next_proposal());
        assert!(network.delay_next_proposal());
        let responses = network.deliver_all().expect("deliver at threshold");
        let mut expected_order = vec![
            operation_ids[2],
            operation_ids[3],
            operation_ids[4],
            operation_ids[5],
            operation_ids[0],
            operation_ids[1],
        ];

        assert_eq!(responses.len(), 7);
        for member in members.iter().take(5).copied() {
            assert_eq!(
                network
                    .accepted_operation_ids(member)
                    .expect("accepted ids"),
                expected_order
            );
        }
        for member in members.iter().skip(5).copied() {
            assert_eq!(
                network
                    .member_status(member)
                    .expect("offline member status")
                    .operation_count,
                0
            );
        }
        assert!(!network.all_members_converged());

        network
            .set_online(members[4], false)
            .expect("member 5 offline");
        let stalled_operation = network
            .submit(
                members[0],
                mint_envelope(
                    federation_id,
                    "quote-stalled",
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                ),
            )
            .expect("submit stalled operation");
        assert!(matches!(
            network.deliver_next(),
            Err(FederationError::ConsensusQuorumUnavailable {
                reachable: 4,
                required: 5
            })
        ));
        assert_eq!(network.pending_len(), 1);

        network
            .set_online(members[4], true)
            .expect("member 5 online");
        let response = network
            .deliver_next()
            .expect("deliver after quorum recovers")
            .expect("finalized response");
        expected_order.push(stalled_operation);

        assert_eq!(response.operation_id, stalled_operation);
        assert_eq!(network.pending_len(), 0);
        for member in members.iter().take(5).copied() {
            assert_eq!(
                network
                    .accepted_operation_ids(member)
                    .expect("accepted ids"),
                expected_order
            );
        }

        network
            .set_online(members[5], true)
            .expect("member 6 online");
        network
            .set_online(members[6], true)
            .expect("member 7 online");
        network
            .catch_up_member_from_peer(members[5], members[0])
            .expect("catch up member 6");
        network
            .catch_up_member_from_peer(members[6], members[4])
            .expect("catch up member 7");

        for member in members {
            assert_eq!(
                network
                    .accepted_operation_ids(member)
                    .expect("accepted ids"),
                expected_order
            );
        }
        assert!(network.all_members_converged());
    }

    #[test]
    fn backend_adapter_dispatches_queued_operations_to_finalized_order() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut adapter =
            InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
                .expect("adapter");
        let first_envelope = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let first = first_envelope.operation_id().expect("first operation id");
        let second_envelope = mint_envelope(
            federation_id,
            "quote-2",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let second = second_envelope.operation_id().expect("second operation id");

        assert_eq!(
            adapter
                .submit_operation(first_envelope.clone())
                .expect("submit first"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(
            adapter
                .submit_operation(first_envelope)
                .expect("duplicate first"),
            FederationConsensusItemAdmission::DuplicateOperation
        );
        assert_eq!(
            adapter
                .submit_operation(second_envelope)
                .expect("submit second"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(adapter.queued_len(), 2);

        let responses = adapter
            .dispatch_and_finalize(10)
            .expect("dispatch and finalize");

        assert_eq!(responses.len(), 2);
        assert_eq!(adapter.queued_len(), 0);
        assert_eq!(adapter.network_pending_len(), 0);
        for member in members {
            assert_eq!(
                adapter
                    .accepted_operation_ids(member)
                    .expect("accepted ids"),
                vec![first, second]
            );
        }
        assert!(adapter.all_members_converged());
    }

    #[test]
    fn backend_adapter_trait_dispatches_in_memory_operations() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut adapter =
            InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
                .expect("adapter");
        let envelope = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let operation_id = envelope.operation_id().expect("operation id");

        {
            let backend: &mut dyn FederationConsensusBackendAdapter = &mut adapter;
            assert_eq!(
                backend.backend_kind(),
                FederationConsensusBackendKind::InMemory
            );
            assert_eq!(
                backend.submit_operation(envelope).expect("submit"),
                FederationConsensusItemAdmission::Accepted
            );
            assert_eq!(backend.queued_len(), 1);
            assert_eq!(backend.pending_finalization_len(), 0);

            let responses = backend
                .dispatch_and_finalize(10)
                .expect("dispatch and finalize");

            assert_eq!(responses.len(), 1);
            assert_eq!(responses[0].operation_id, operation_id);
            assert_eq!(backend.queued_len(), 0);
            assert_eq!(backend.pending_finalization_len(), 0);
        }

        assert!(adapter.all_members_converged());
    }

    #[test]
    fn backend_adapter_runtime_dispatches_selected_in_memory_backend() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
            FederationConsensusBackendKind::InMemory,
            federation_id,
            members,
            2,
            members[0],
        )
        .expect("adapter runtime");
        let envelope = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );

        assert_eq!(
            adapter.backend_kind(),
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(
            adapter.submit_operation(envelope).expect("submit"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(adapter.queued_len(), 1);
        assert_eq!(adapter.dispatch_queued_operations(10).expect("dispatch"), 1);
        assert_eq!(adapter.queued_len(), 0);
        assert_eq!(adapter.pending_finalization_len(), 1);

        let responses = adapter.finalize_dispatched_operations().expect("finalize");

        assert_eq!(responses.len(), 1);
        assert_eq!(adapter.pending_finalization_len(), 0);
    }

    #[test]
    fn backend_adapter_runtime_rejects_unavailable_backend_kind() {
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];

        let err = FederationConsensusBackendAdapterRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(13),
            members,
            2,
            members[0],
        )
        .expect_err("aleph-bft adapter is not implemented yet");

        assert!(matches!(
            err,
            FederationError::UnsupportedConsensusBackend { ref backend }
                if backend == "aleph_bft"
        ));
    }

    #[test]
    fn backend_adapter_keeps_dispatched_operation_pending_without_quorum() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut adapter =
            InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
                .expect("adapter");
        adapter
            .set_online(members[1], false)
            .expect("member 2 offline");
        adapter
            .set_online(members[2], false)
            .expect("member 3 offline");
        adapter
            .submit_operation(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit");

        assert_eq!(adapter.dispatch_queued_operations(10).expect("dispatch"), 1);
        assert_eq!(adapter.queued_len(), 0);
        assert_eq!(adapter.network_pending_len(), 1);
        assert!(matches!(
            adapter.finalize_dispatched_operations(),
            Err(FederationError::ConsensusQuorumUnavailable {
                reachable: 1,
                required: 2
            })
        ));
        assert_eq!(adapter.network_pending_len(), 1);
    }

    #[test]
    fn backend_adapter_keeps_local_queue_when_proposer_is_offline() {
        let federation_id = federation_id(13);
        let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
        let mut adapter =
            InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
                .expect("adapter");
        adapter
            .set_online(members[0], false)
            .expect("local member offline");
        adapter
            .submit_operation(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit");

        assert!(adapter.dispatch_queued_operations(10).is_err());
        assert_eq!(adapter.queued_len(), 1);
        assert_eq!(adapter.network_pending_len(), 0);
    }

    #[test]
    fn dummy_consensus_rejects_wrong_federation() {
        let mut consensus = InMemoryFederationConsensus::new(federation_id(13), member_id());
        let envelope = mint_quote_envelope(federation_id(14));

        assert!(matches!(
            consensus.submit_operation(FederationSubmitOperationRequest::new(envelope)),
            Err(FederationError::OperationFederationMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_oversized_operation_before_journal_append() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let envelope = oversized_mint_quote_envelope(federation_id);

        assert!(matches!(
            consensus.submit_envelope(envelope),
            Err(FederationError::OperationEnvelopeTooLarge { .. })
        ));
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    #[test]
    fn dummy_consensus_serves_peer_status_and_catch_up() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        consensus
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit quote");
        consensus
            .submit_envelope(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit mint");

        let status = consensus.peer_status();
        let request = FederationJournalCatchUpRequest::new(1);
        let response = consensus.catch_up(&request);

        assert_eq!(status.federation_id, federation_id);
        assert_eq!(status.member_id, member_id());
        assert_eq!(status.operation_count, 2);
        assert_eq!(response.from_index, 1);
        assert_eq!(response.entries.len(), 1);
        assert!(response.complete);
        assert_eq!(response.latest_order_digest, status.latest_order_digest);
    }

    #[test]
    fn dummy_consensus_applies_peer_catch_up_response() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        let quote = mint_quote_envelope(federation_id);
        let mint = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );

        local
            .submit_envelope(quote.clone())
            .expect("local quote submit");
        peer.submit_envelope(quote).expect("peer quote submit");
        let mint_response = peer.submit_envelope(mint).expect("peer mint submit");
        peer.update_status(
            mint_response.operation_id,
            FederationOperationStatus::Applied,
        )
        .expect("peer status update");

        let request = local.catch_up_request();
        let response = peer.catch_up_checked(&request).expect("catch up");
        let applied = local
            .apply_catch_up_response(&response)
            .expect("apply catch up");

        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].operation_id, mint_response.operation_id);
        assert_eq!(applied[0].status, FederationOperationStatus::Accepted);
        assert_eq!(
            local.peer_status().latest_order_digest,
            peer.peer_status().latest_order_digest
        );
    }

    #[test]
    fn dummy_consensus_rejects_known_digest_mismatch() {
        let federation_id = federation_id(13);
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        peer.submit_envelope(mint_quote_envelope(federation_id))
            .expect("peer quote submit");
        let request = FederationJournalCatchUpRequest::new(1)
            .with_known_order_digest(super::super::FederationJournalDigest::genesis());

        assert!(matches!(
            peer.catch_up_checked(&request),
            Err(FederationError::CatchUpDigestMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_wrong_federation_catch_up_response() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        peer.submit_envelope(mint_quote_envelope(federation_id))
            .expect("peer quote submit");
        let mut response = peer
            .catch_up_checked(&local.catch_up_request())
            .expect("catch up");
        response.federation_id = super::super::FederationId::from_bytes([14; 32]);

        assert!(matches!(
            local.apply_catch_up_response(&response),
            Err(FederationError::CatchUpFederationMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_tampered_catch_up_operation_id() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        peer.submit_envelope(mint_quote_envelope(federation_id))
            .expect("peer quote submit");
        let mut response = peer
            .catch_up_checked(&local.catch_up_request())
            .expect("catch up");
        response.entries[0].operation_id = FederationOperationId::from_bytes([99; 32]);

        assert!(matches!(
            local.apply_catch_up_response(&response),
            Err(FederationError::CatchUpOperationIdMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_future_operation_version_catch_up_response() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        peer.submit_envelope(mint_quote_envelope(federation_id))
            .expect("peer quote submit");
        let mut response = peer
            .catch_up_checked(&local.catch_up_request())
            .expect("catch up");
        response.entries[0].envelope.version = super::super::FEDERATION_OPERATION_VERSION + 1;

        assert!(matches!(
            local.apply_catch_up_response(&response),
            Err(FederationError::OperationVersionMismatch {
                expected: super::super::FEDERATION_OPERATION_VERSION,
                found,
            }) if found == super::super::FEDERATION_OPERATION_VERSION + 1
        ));
        assert_eq!(local.peer_status().operation_count, 0);
    }

    #[test]
    fn dummy_consensus_rejects_tampered_catch_up_digest_chain() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
        peer.submit_envelope(mint_quote_envelope(federation_id))
            .expect("peer quote submit");
        let mut response = peer
            .catch_up_checked(&local.catch_up_request())
            .expect("catch up");
        response.entries[0].previous_order_digest =
            super::super::FederationJournalDigest::from_bytes([88; 32]);

        assert!(matches!(
            local.apply_catch_up_response(&response),
            Err(FederationError::CatchUpDigestMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_empty_incomplete_catch_up_response() {
        let federation_id = federation_id(13);
        let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
        let response = FederationJournalCatchUpResponse {
            version: super::super::FEDERATION_REST_API_VERSION,
            federation_id,
            from_index: 0,
            entries: Vec::new(),
            latest_index: Some(0),
            latest_order_digest: super::super::FederationJournalDigest::genesis(),
            state_digest: FederationStateDigest::genesis(),
            session_outcome: None,
            complete: false,
        };

        assert!(matches!(
            local.apply_catch_up_response(&response),
            Err(FederationError::CatchUpNoProgress { index: 0 })
        ));
    }

    #[test]
    fn dummy_consensus_updates_application_status() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let response = consensus
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit");
        let updated = consensus
            .update_status(response.operation_id, FederationOperationStatus::Applied)
            .expect("update status");

        assert_eq!(updated.status, FederationOperationStatus::Applied);
    }
}
