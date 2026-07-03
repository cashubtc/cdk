//! Deterministic local consensus scaffold.
//!
//! This is not a quorum protocol. It is the first consensus-facing boundary:
//! validate federation identity, append operations to the journal, and expose
//! the same status/catch-up/submit payloads used by private REST. Real peer
//! networking can replace the decision source while keeping this API shape.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{
    FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPeerStatus, FederationStateDigest,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse, InMemoryFederationJournal,
    MemberId, Result,
};

const DEFAULT_CATCH_UP_LIMIT: u16 = 100;
const MAX_CATCH_UP_LIMIT: u16 = 1000;

/// Minimal synchronous consensus engine surface used by federation runtimes.
///
/// The first implementation is the deterministic in-memory engine below. Real
/// networking or BFT finalization engines should preserve this state-machine
/// boundary: submit admissible items, expose accepted order, serve catch-up,
/// replay peer catch-up, and update only local application status.
pub trait FederationConsensusEngine: fmt::Debug + Send {
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
/// CDK keeps this small enum instead of exposing the in-memory scaffold through
/// server state. That lets Axum and storage code depend on the consensus engine
/// contract while future engines can be added behind this wrapper.
#[derive(Debug, Clone)]
pub struct FederationConsensusRuntime {
    backend: FederationConsensusBackend,
}

#[derive(Debug, Clone)]
enum FederationConsensusBackend {
    InMemory(InMemoryFederationConsensus),
}

impl FederationConsensusRuntime {
    /// Create an in-memory runtime for deterministic tests and local MVP use.
    #[must_use]
    pub fn in_memory(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            backend: FederationConsensusBackend::InMemory(InMemoryFederationConsensus::new(
                federation_id,
                member_id,
            )),
        }
    }

    /// Rebuild an in-memory runtime from persisted journal entries.
    pub fn in_memory_from_entries(
        federation_id: FederationId,
        member_id: MemberId,
        entries: Vec<FederationJournalEntry>,
    ) -> Result<Self> {
        Ok(Self {
            backend: FederationConsensusBackend::InMemory(
                InMemoryFederationConsensus::from_entries(federation_id, member_id, entries)?,
            ),
        })
    }

    fn engine(&self) -> &dyn FederationConsensusEngine {
        match &self.backend {
            FederationConsensusBackend::InMemory(engine) => engine,
        }
    }

    fn engine_mut(&mut self) -> &mut dyn FederationConsensusEngine {
        match &mut self.backend {
            FederationConsensusBackend::InMemory(engine) => engine,
        }
    }
}

impl FederationConsensusEngine for FederationConsensusRuntime {
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
        self.engine().peer_status()
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
            let request = member.catch_up_request();
            let response = peer.catch_up_checked(&request)?;
            applied.extend(member.apply_catch_up_response(&response)?);
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
        FederationPeerStatus::from_journal(self.federation_id, self.member_id, &self.journal)
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

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
