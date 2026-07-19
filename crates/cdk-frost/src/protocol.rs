//! Durable participant state for non-FROST custody protocols.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::nonce::CustodyProtocol;
use crate::{
    ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch, ApplicationRegistration,
    EncryptedSecret, Error, MemberId, MemoryNonceStore, NonceBurnReason, RootKeyEpoch,
    SignatureMode, SigningAuthorizationScope, SigningDerivation, SigningSessionContext,
    SigningSessionId, SigningSlotId, ThresholdDhProof, ThresholdDhShare,
    VirtualMusigNonceCommitment, VirtualMusigNonceId, VirtualMusigPartialShare,
};

const PROTOCOL_RECORD_VERSION: u16 = 1;
const MAX_PROTOCOL_RECORD_BYTES: usize = 512 * 1024;

/// Durable virtual MuSig2 nonce lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum VirtualMusigLifecycle {
    /// Secret nonce material has been generated and sealed.
    Generated,
    /// The authorization slot durably owns this nonce.
    Reserved,
    /// The public nonce may have left the participant.
    CommitmentSent,
    /// Final signing is durably locked and the secret cannot be loaded again.
    PartialSigning {
        /// Hash of the exact finalized BIP327 transcript.
        transcript_binding: [u8; 32],
    },
    /// A verified partial is durably cached for safe recovery.
    PartialProduced {
        /// Hash of the exact finalized BIP327 transcript.
        transcript_binding: [u8; 32],
    },
    /// Terminal success; an exact retry may return the cached partial.
    Consumed {
        /// Hash required for an exact idempotent retry.
        transcript_binding: [u8; 32],
    },
    /// Terminal failure; this nonce can never be used again.
    Burned {
        /// Reason the nonce became unusable.
        reason: NonceBurnReason,
    },
}

impl VirtualMusigLifecycle {
    /// Whether this state permanently prevents secret nonce use.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Consumed { .. } | Self::Burned { .. })
    }
}

/// Participant-local encrypted virtual MuSig2 record.
#[derive(Clone, PartialEq, Eq)]
pub struct VirtualMusigRecord {
    context: SigningSessionContext,
    derivation: SigningDerivation,
    commitment: VirtualMusigNonceCommitment,
    lifecycle: VirtualMusigLifecycle,
    encrypted_nonce: Option<EncryptedSecret>,
    cached_partial: Option<VirtualMusigPartialShare>,
    revision: u64,
}

impl fmt::Debug for VirtualMusigRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VirtualMusigRecord")
            .field("session_id", &self.context.session_id())
            .field("slot_id", &self.context.slot_id())
            .field("nonce_id", &self.commitment.nonce_id())
            .field("lifecycle", &self.lifecycle)
            .field("secret", &self.encrypted_nonce.as_ref().map(|_| "<sealed>"))
            .field(
                "cached_partial",
                &self.cached_partial.as_ref().map(|_| "<redacted>"),
            )
            .field("revision", &self.revision)
            .finish()
    }
}

impl VirtualMusigRecord {
    pub(crate) fn generated(
        context: SigningSessionContext,
        derivation: SigningDerivation,
        commitment: VirtualMusigNonceCommitment,
        encrypted_nonce: EncryptedSecret,
    ) -> Self {
        Self {
            context,
            derivation,
            commitment,
            lifecycle: VirtualMusigLifecycle::Generated,
            encrypted_nonce: Some(encrypted_nonce),
            cached_partial: None,
            revision: 0,
        }
    }

    /// Fully bound protocol session context.
    pub fn context(&self) -> &SigningSessionContext {
        &self.context
    }

    /// Single-use virtual MuSig2 nonce identifier.
    pub fn nonce_id(&self) -> VirtualMusigNonceId {
        self.commitment.nonce_id()
    }

    /// Public two-column nonce commitment.
    pub fn commitment(&self) -> &VirtualMusigNonceCommitment {
        &self.commitment
    }

    /// Current durable lifecycle.
    pub fn lifecycle(&self) -> &VirtualMusigLifecycle {
        &self.lifecycle
    }

    /// Monotonic compare-and-swap revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn derivation(&self) -> &SigningDerivation {
        &self.derivation
    }

    pub(crate) fn encrypted_nonce(&self) -> Option<&EncryptedSecret> {
        self.encrypted_nonce.as_ref()
    }

    pub(crate) fn cached_partial(&self) -> Option<&VirtualMusigPartialShare> {
        self.cached_partial.as_ref()
    }

    pub(crate) fn transition(
        &self,
        lifecycle: VirtualMusigLifecycle,
        keep_secret: bool,
        cached_partial: Option<VirtualMusigPartialShare>,
    ) -> Result<Self, Error> {
        let revision = self.revision.checked_add(1).ok_or_else(|| {
            Error::NoncePersistence("virtual MuSig2 revision overflow".to_string())
        })?;
        Ok(Self {
            context: self.context.clone(),
            derivation: self.derivation.clone(),
            commitment: self.commitment.clone(),
            lifecycle,
            encrypted_nonce: keep_secret.then(|| self.encrypted_nonce.clone()).flatten(),
            cached_partial,
            revision,
        })
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let wire = VirtualMusigRecordWire {
            version: PROTOCOL_RECORD_VERSION,
            common: ProtocolRecordCommonWire::from_parts(&self.context, &self.derivation),
            nonce_id: self.nonce_id().to_bytes(),
            participant: self.commitment.participant().get(),
            public_nonce: self.commitment.public_nonce().to_vec(),
            lifecycle: self.lifecycle.clone(),
            encrypted_nonce: self.encrypted_nonce.as_ref().map(EncryptedSecret::to_bytes),
            cached_partial: self.cached_partial.as_ref().map(PartialShareWire::from),
            revision: self.revision,
        };
        bounded_encode("virtual MuSig2 record", &wire)
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let wire: VirtualMusigRecordWire = bounded_decode("virtual MuSig2 record", bytes)?;
        if wire.version != PROTOCOL_RECORD_VERSION {
            return Err(Error::malformed(
                "virtual MuSig2 record",
                "unsupported record version",
            ));
        }
        let (context, derivation) = wire.common.into_parts(CustodyProtocol::VirtualMusig2)?;
        let participant = MemberId::new(wire.participant)?;
        let public_nonce: [u8; 66] = wire.public_nonce.try_into().map_err(|_| {
            Error::malformed("virtual MuSig2 record", "public nonce is not 66 bytes")
        })?;
        let commitment = VirtualMusigNonceCommitment::new(&context, participant, public_nonce)?;
        if commitment.nonce_id() != VirtualMusigNonceId::from_bytes(wire.nonce_id)? {
            return Err(Error::malformed(
                "virtual MuSig2 record",
                "nonce identifier differs from public commitment",
            ));
        }
        let encrypted_nonce = wire
            .encrypted_nonce
            .map(EncryptedSecret::from_bytes)
            .transpose()?;
        let cached_partial = wire
            .cached_partial
            .map(PartialShareWire::into_share)
            .transpose()?;
        validate_virtual_payload(
            &wire.lifecycle,
            encrypted_nonce.as_ref(),
            cached_partial.as_ref(),
            &commitment,
        )?;
        Ok(Self {
            context,
            derivation,
            commitment,
            lifecycle: wire.lifecycle,
            encrypted_nonce,
            cached_partial,
            revision: wire.revision,
        })
    }
}

/// Result of atomically reserving a virtual MuSig2 session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualMusigReservation {
    /// A new generated record was inserted.
    Inserted,
    /// The exact session already exists for retry handling.
    Existing(Box<VirtualMusigRecord>),
}

/// Compact evidence that a virtual MuSig2 nonce cannot be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMusigTombstone {
    /// Claimed protocol session.
    pub session_id: SigningSessionId,
    /// Claimed authorization slot.
    pub slot_id: SigningSlotId,
    /// Permanently unusable nonce identifier.
    pub nonce_id: VirtualMusigNonceId,
    /// Terminal outcome retained after detail cleanup.
    pub lifecycle: VirtualMusigLifecycle,
}

/// Durable threshold-DH request lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum ThresholdDhLifecycle {
    /// The exact request owns the authorization slot but has no result yet.
    Pending,
    /// A verified result is durably cached for exact retries.
    Completed,
    /// The exact request is terminal and cannot be retried.
    Burned {
        /// Reason result production became ambiguous or invalid.
        reason: NonceBurnReason,
    },
}

impl ThresholdDhLifecycle {
    /// Whether no further computation may occur for this record.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Burned { .. })
    }
}

/// Durable threshold-DH authorization and result audit record.
#[derive(Clone, PartialEq, Eq)]
pub struct ThresholdDhRecord {
    context: SigningSessionContext,
    derivation: SigningDerivation,
    request_binding: [u8; 32],
    lifecycle: ThresholdDhLifecycle,
    cached_result: Option<ThresholdDhShare>,
    revision: u64,
}

impl fmt::Debug for ThresholdDhRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ThresholdDhRecord")
            .field("session_id", &self.context.session_id())
            .field("slot_id", &self.context.slot_id())
            .field("request_binding", &self.request_binding)
            .field("lifecycle", &self.lifecycle)
            .field(
                "cached_result",
                &self.cached_result.as_ref().map(|_| "<redacted>"),
            )
            .field("revision", &self.revision)
            .finish()
    }
}

impl ThresholdDhRecord {
    pub(crate) fn pending(
        context: SigningSessionContext,
        derivation: SigningDerivation,
        request_binding: [u8; 32],
    ) -> Self {
        Self {
            context,
            derivation,
            request_binding,
            lifecycle: ThresholdDhLifecycle::Pending,
            cached_result: None,
            revision: 0,
        }
    }

    /// Fully bound protocol session context.
    pub fn context(&self) -> &SigningSessionContext {
        &self.context
    }

    /// Commitment to the exact authorized request.
    pub fn request_binding(&self) -> [u8; 32] {
        self.request_binding
    }

    /// Current durable lifecycle.
    pub fn lifecycle(&self) -> &ThresholdDhLifecycle {
        &self.lifecycle
    }

    /// Monotonic compare-and-swap revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn derivation(&self) -> &SigningDerivation {
        &self.derivation
    }

    pub(crate) fn cached_result(&self) -> Option<&ThresholdDhShare> {
        self.cached_result.as_ref()
    }

    pub(crate) fn transition(
        &self,
        lifecycle: ThresholdDhLifecycle,
        cached_result: Option<ThresholdDhShare>,
    ) -> Result<Self, Error> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::NoncePersistence("threshold-DH revision overflow".to_string()))?;
        Ok(Self {
            context: self.context.clone(),
            derivation: self.derivation.clone(),
            request_binding: self.request_binding,
            lifecycle,
            cached_result,
            revision,
        })
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let wire = ThresholdDhRecordWire {
            version: PROTOCOL_RECORD_VERSION,
            common: ProtocolRecordCommonWire::from_parts(&self.context, &self.derivation),
            request_binding: self.request_binding,
            lifecycle: self.lifecycle.clone(),
            cached_result: self.cached_result.as_ref().map(DhShareWire::from),
            revision: self.revision,
        };
        bounded_encode("threshold-DH record", &wire)
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let wire: ThresholdDhRecordWire = bounded_decode("threshold-DH record", bytes)?;
        if wire.version != PROTOCOL_RECORD_VERSION {
            return Err(Error::malformed(
                "threshold-DH record",
                "unsupported record version",
            ));
        }
        let (context, derivation) = wire.common.into_parts(CustodyProtocol::ThresholdDh)?;
        let cached_result = wire
            .cached_result
            .map(DhShareWire::into_share)
            .transpose()?;
        let valid = match &wire.lifecycle {
            ThresholdDhLifecycle::Pending | ThresholdDhLifecycle::Burned { .. } => {
                cached_result.is_none()
            }
            ThresholdDhLifecycle::Completed => cached_result.as_ref().is_some_and(|result| {
                result.session_id() == context.session_id()
                    && result.request_binding() == wire.request_binding
            }),
        };
        if !valid {
            return Err(Error::malformed(
                "threshold-DH record",
                "result payload differs from lifecycle",
            ));
        }
        Ok(Self {
            context,
            derivation,
            request_binding: wire.request_binding,
            lifecycle: wire.lifecycle,
            cached_result,
            revision: wire.revision,
        })
    }
}

/// Result of atomically reserving a threshold-DH request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThresholdDhReservation {
    /// A new pending audit record was inserted.
    Inserted,
    /// The exact session already exists for retry handling.
    Existing(Box<ThresholdDhRecord>),
}

/// Compact evidence that a threshold-DH request remains terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdDhTombstone {
    /// Claimed protocol session.
    pub session_id: SigningSessionId,
    /// Claimed authorization slot.
    pub slot_id: SigningSlotId,
    /// Exact request commitment.
    pub request_binding: [u8; 32],
    /// Terminal outcome retained after detail cleanup.
    pub lifecycle: ThresholdDhLifecycle,
}

/// Durable protocol ledger shared by every registered application.
pub trait DurableProtocolStore: fmt::Debug + Send + Sync + 'static {
    /// Atomically activate a virtual MuSig2 attempt and burn an older active attempt.
    fn activate_virtual_musig_attempt(&self, context: &SigningSessionContext) -> Result<(), Error>;
    /// Atomically reserve a virtual MuSig2 record.
    fn reserve_virtual_musig(
        &self,
        record: VirtualMusigRecord,
    ) -> Result<VirtualMusigReservation, Error>;
    /// Load virtual MuSig2 state by nonce identifier.
    fn load_virtual_musig_nonce(
        &self,
        nonce_id: VirtualMusigNonceId,
    ) -> Result<Option<VirtualMusigRecord>, Error>;
    /// Load virtual MuSig2 state by session identifier.
    fn load_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<VirtualMusigRecord>, Error>;
    /// Atomically replace one exact virtual MuSig2 revision.
    fn compare_and_swap_virtual_musig(
        &self,
        expected: &VirtualMusigRecord,
        replacement: VirtualMusigRecord,
    ) -> Result<(), Error>;
    /// Return every nonterminal virtual MuSig2 record for restart recovery.
    fn nonterminal_virtual_musig_records(&self) -> Result<Vec<VirtualMusigRecord>, Error>;
    /// Compact old virtual MuSig2 detail while preserving replay tombstones.
    fn compact_virtual_musig(&self, retain_detailed: usize) -> Result<usize, Error>;

    /// Atomically activate a threshold-DH attempt and burn an older active attempt.
    fn activate_threshold_dh_attempt(&self, context: &SigningSessionContext) -> Result<(), Error>;
    /// Atomically reserve an exact threshold-DH request.
    fn reserve_threshold_dh(
        &self,
        record: ThresholdDhRecord,
    ) -> Result<ThresholdDhReservation, Error>;
    /// Load threshold-DH state by session identifier.
    fn load_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<ThresholdDhRecord>, Error>;
    /// Atomically replace one exact threshold-DH revision.
    fn compare_and_swap_threshold_dh(
        &self,
        expected: &ThresholdDhRecord,
        replacement: ThresholdDhRecord,
    ) -> Result<(), Error>;
    /// Return every nonterminal threshold-DH record for restart recovery.
    fn nonterminal_threshold_dh_records(&self) -> Result<Vec<ThresholdDhRecord>, Error>;
    /// Compact old threshold-DH detail while preserving replay tombstones.
    fn compact_threshold_dh(&self, retain_detailed: usize) -> Result<usize, Error>;
}

#[derive(Debug, Default)]
pub(crate) struct ProtocolMemoryState {
    virtual_records: BTreeMap<VirtualMusigNonceId, VirtualMusigRecord>,
    virtual_sessions: BTreeMap<SigningSessionId, VirtualMusigNonceId>,
    virtual_active_slots: BTreeMap<SigningSlotId, VirtualMusigNonceId>,
    virtual_latest_attempts: BTreeMap<SigningSlotId, (u32, SigningSessionId)>,
    virtual_tombstones: BTreeMap<VirtualMusigNonceId, VirtualMusigTombstone>,
    virtual_terminal_order: Vec<VirtualMusigNonceId>,
    dh_records: BTreeMap<SigningSessionId, ThresholdDhRecord>,
    dh_active_slots: BTreeMap<SigningSlotId, SigningSessionId>,
    dh_latest_attempts: BTreeMap<SigningSlotId, (u32, SigningSessionId)>,
    dh_tombstones: BTreeMap<SigningSessionId, ThresholdDhTombstone>,
    dh_terminal_order: Vec<SigningSessionId>,
}

impl DurableProtocolStore for MemoryNonceStore {
    fn activate_virtual_musig_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::VirtualMusig2)?;
        let mut state = self.protocol_lock()?;
        activate_virtual_attempt(&mut state, context)
    }

    fn reserve_virtual_musig(
        &self,
        record: VirtualMusigRecord,
    ) -> Result<VirtualMusigReservation, Error> {
        validate_new_virtual_record(&record)?;
        let mut state = self.protocol_lock()?;
        let session_id = record.context.session_id();
        if let Some(nonce_id) = state.virtual_sessions.get(&session_id) {
            return match state.virtual_records.get(nonce_id) {
                Some(existing) => Ok(VirtualMusigReservation::Existing(Box::new(
                    existing.clone(),
                ))),
                None => Err(Error::InvalidCommitment(
                    "virtual MuSig2 session is terminal and compacted".to_string(),
                )),
            };
        }
        if state.virtual_latest_attempts.get(&record.context.slot_id())
            != Some(&(record.context.attempt(), session_id))
            || state
                .virtual_active_slots
                .contains_key(&record.context.slot_id())
            || state.virtual_records.contains_key(&record.nonce_id())
            || state.virtual_tombstones.contains_key(&record.nonce_id())
        {
            return Err(Error::InvalidSigningSession(
                "virtual MuSig2 attempt was not exclusively activated".to_string(),
            ));
        }
        let nonce_id = record.nonce_id();
        state.virtual_sessions.insert(session_id, nonce_id);
        state
            .virtual_active_slots
            .insert(record.context.slot_id(), nonce_id);
        state.virtual_records.insert(nonce_id, record);
        Ok(VirtualMusigReservation::Inserted)
    }

    fn load_virtual_musig_nonce(
        &self,
        nonce_id: VirtualMusigNonceId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        let state = self.protocol_lock()?;
        if state.virtual_tombstones.contains_key(&nonce_id) {
            return Err(Error::InvalidCommitment(
                "virtual MuSig2 nonce is terminal and compacted".to_string(),
            ));
        }
        Ok(state.virtual_records.get(&nonce_id).cloned())
    }

    fn load_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        let state = self.protocol_lock()?;
        let Some(nonce_id) = state.virtual_sessions.get(&session_id) else {
            return Ok(None);
        };
        Ok(state.virtual_records.get(nonce_id).cloned())
    }

    fn compare_and_swap_virtual_musig(
        &self,
        expected: &VirtualMusigRecord,
        replacement: VirtualMusigRecord,
    ) -> Result<(), Error> {
        validate_virtual_transition(expected, &replacement)?;
        let mut state = self.protocol_lock()?;
        let current = state
            .virtual_records
            .get(&expected.nonce_id())
            .ok_or_else(|| {
                Error::NoncePersistence("virtual MuSig2 record disappeared".to_string())
            })?;
        if current != expected {
            return Err(Error::NoncePersistence(
                "virtual MuSig2 record changed concurrently".to_string(),
            ));
        }
        if !expected.lifecycle.is_terminal() && replacement.lifecycle.is_terminal() {
            state
                .virtual_active_slots
                .remove(&expected.context.slot_id());
            state.virtual_terminal_order.push(expected.nonce_id());
        }
        state
            .virtual_records
            .insert(expected.nonce_id(), replacement);
        Ok(())
    }

    fn nonterminal_virtual_musig_records(&self) -> Result<Vec<VirtualMusigRecord>, Error> {
        let state = self.protocol_lock()?;
        Ok(state
            .virtual_records
            .values()
            .filter(|record| !record.lifecycle.is_terminal())
            .cloned()
            .collect())
    }

    fn compact_virtual_musig(&self, retain_detailed: usize) -> Result<usize, Error> {
        let mut state = self.protocol_lock()?;
        let compact_count = state
            .virtual_terminal_order
            .len()
            .saturating_sub(retain_detailed);
        let ids = state
            .virtual_terminal_order
            .drain(..compact_count)
            .collect::<Vec<_>>();
        for nonce_id in &ids {
            let record = state.virtual_records.remove(nonce_id).ok_or_else(|| {
                Error::NoncePersistence("terminal virtual MuSig2 record is missing".to_string())
            })?;
            state.virtual_tombstones.insert(
                *nonce_id,
                VirtualMusigTombstone {
                    session_id: record.context.session_id(),
                    slot_id: record.context.slot_id(),
                    nonce_id: *nonce_id,
                    lifecycle: record.lifecycle,
                },
            );
        }
        Ok(ids.len())
    }

    fn activate_threshold_dh_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::ThresholdDh)?;
        let mut state = self.protocol_lock()?;
        activate_dh_attempt(&mut state, context)
    }

    fn reserve_threshold_dh(
        &self,
        record: ThresholdDhRecord,
    ) -> Result<ThresholdDhReservation, Error> {
        validate_new_dh_record(&record)?;
        let mut state = self.protocol_lock()?;
        let session_id = record.context.session_id();
        if let Some(existing) = state.dh_records.get(&session_id) {
            return Ok(ThresholdDhReservation::Existing(Box::new(existing.clone())));
        }
        if state.dh_tombstones.contains_key(&session_id) {
            return Err(Error::InvalidSigningSession(
                "threshold-DH session is terminal and compacted".to_string(),
            ));
        }
        if state.dh_latest_attempts.get(&record.context.slot_id())
            != Some(&(record.context.attempt(), session_id))
            || state
                .dh_active_slots
                .contains_key(&record.context.slot_id())
        {
            return Err(Error::InvalidSigningSession(
                "threshold-DH attempt was not exclusively activated".to_string(),
            ));
        }
        state
            .dh_active_slots
            .insert(record.context.slot_id(), session_id);
        state.dh_records.insert(session_id, record);
        Ok(ThresholdDhReservation::Inserted)
    }

    fn load_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<ThresholdDhRecord>, Error> {
        let state = self.protocol_lock()?;
        if state.dh_tombstones.contains_key(&session_id) {
            return Err(Error::InvalidSigningSession(
                "threshold-DH session is terminal and compacted".to_string(),
            ));
        }
        Ok(state.dh_records.get(&session_id).cloned())
    }

    fn compare_and_swap_threshold_dh(
        &self,
        expected: &ThresholdDhRecord,
        replacement: ThresholdDhRecord,
    ) -> Result<(), Error> {
        validate_dh_transition(expected, &replacement)?;
        let mut state = self.protocol_lock()?;
        let session_id = expected.context.session_id();
        let current = state.dh_records.get(&session_id).ok_or_else(|| {
            Error::NoncePersistence("threshold-DH record disappeared".to_string())
        })?;
        if current != expected {
            return Err(Error::NoncePersistence(
                "threshold-DH record changed concurrently".to_string(),
            ));
        }
        if !expected.lifecycle.is_terminal() && replacement.lifecycle.is_terminal() {
            state.dh_active_slots.remove(&expected.context.slot_id());
            state.dh_terminal_order.push(session_id);
        }
        state.dh_records.insert(session_id, replacement);
        Ok(())
    }

    fn nonterminal_threshold_dh_records(&self) -> Result<Vec<ThresholdDhRecord>, Error> {
        let state = self.protocol_lock()?;
        Ok(state
            .dh_records
            .values()
            .filter(|record| !record.lifecycle.is_terminal())
            .cloned()
            .collect())
    }

    fn compact_threshold_dh(&self, retain_detailed: usize) -> Result<usize, Error> {
        let mut state = self.protocol_lock()?;
        let compact_count = state
            .dh_terminal_order
            .len()
            .saturating_sub(retain_detailed);
        let sessions = state
            .dh_terminal_order
            .drain(..compact_count)
            .collect::<Vec<_>>();
        for session_id in &sessions {
            let record = state.dh_records.remove(session_id).ok_or_else(|| {
                Error::NoncePersistence("terminal threshold-DH record is missing".to_string())
            })?;
            state.dh_tombstones.insert(
                *session_id,
                ThresholdDhTombstone {
                    session_id: *session_id,
                    slot_id: record.context.slot_id(),
                    request_binding: record.request_binding,
                    lifecycle: record.lifecycle,
                },
            );
        }
        Ok(sessions.len())
    }
}

fn activate_virtual_attempt(
    state: &mut ProtocolMemoryState,
    context: &SigningSessionContext,
) -> Result<(), Error> {
    let slot = context.slot_id();
    check_attempt(&state.virtual_latest_attempts, context)?;
    if state
        .virtual_latest_attempts
        .get(&slot)
        .is_some_and(|(attempt, _)| *attempt == context.attempt())
    {
        return Ok(());
    }
    if let Some(nonce_id) = state.virtual_active_slots.remove(&slot) {
        let existing = state
            .virtual_records
            .get(&nonce_id)
            .cloned()
            .ok_or_else(|| {
                Error::NoncePersistence("active virtual MuSig2 record is missing".to_string())
            })?;
        let burned = existing.transition(
            VirtualMusigLifecycle::Burned {
                reason: NonceBurnReason::ReplacedAttempt,
            },
            false,
            None,
        )?;
        validate_virtual_transition(&existing, &burned)?;
        state.virtual_records.insert(nonce_id, burned);
        state.virtual_terminal_order.push(nonce_id);
    }
    state
        .virtual_latest_attempts
        .insert(slot, (context.attempt(), context.session_id()));
    Ok(())
}

fn activate_dh_attempt(
    state: &mut ProtocolMemoryState,
    context: &SigningSessionContext,
) -> Result<(), Error> {
    let slot = context.slot_id();
    check_attempt(&state.dh_latest_attempts, context)?;
    if state
        .dh_latest_attempts
        .get(&slot)
        .is_some_and(|(attempt, _)| *attempt == context.attempt())
    {
        return Ok(());
    }
    if let Some(session_id) = state.dh_active_slots.remove(&slot) {
        let existing = state.dh_records.get(&session_id).cloned().ok_or_else(|| {
            Error::NoncePersistence("active threshold-DH record is missing".to_string())
        })?;
        let burned = existing.transition(
            ThresholdDhLifecycle::Burned {
                reason: NonceBurnReason::ReplacedAttempt,
            },
            None,
        )?;
        validate_dh_transition(&existing, &burned)?;
        state.dh_records.insert(session_id, burned);
        state.dh_terminal_order.push(session_id);
    }
    state
        .dh_latest_attempts
        .insert(slot, (context.attempt(), context.session_id()));
    Ok(())
}

fn check_attempt(
    attempts: &BTreeMap<SigningSlotId, (u32, SigningSessionId)>,
    context: &SigningSessionContext,
) -> Result<(), Error> {
    match attempts.get(&context.slot_id()).copied() {
        Some((attempt, _)) if attempt > context.attempt() => Err(Error::InvalidSigningSession(
            "protocol attempt is stale".to_string(),
        )),
        Some((attempt, session_id)) if attempt == context.attempt() => {
            if session_id == context.session_id() {
                Ok(())
            } else {
                Err(Error::InvalidSigningSession(
                    "signer set changed within a protocol attempt".to_string(),
                ))
            }
        }
        _ => Ok(()),
    }
}

pub(crate) fn validate_new_virtual_record(record: &VirtualMusigRecord) -> Result<(), Error> {
    require_protocol(&record.context, CustodyProtocol::VirtualMusig2)?;
    if record.revision != 0
        || record.lifecycle != VirtualMusigLifecycle::Generated
        || record.encrypted_nonce.is_none()
        || record.cached_partial.is_some()
    {
        return Err(Error::NoncePersistence(
            "new virtual MuSig2 record is not generated revision zero".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_virtual_transition(
    expected: &VirtualMusigRecord,
    replacement: &VirtualMusigRecord,
) -> Result<(), Error> {
    if expected.context != replacement.context
        || expected.derivation != replacement.derivation
        || expected.commitment != replacement.commitment
        || replacement.revision != expected.revision.saturating_add(1)
    {
        return Err(Error::NoncePersistence(
            "invalid virtual MuSig2 compare-and-swap replacement".to_string(),
        ));
    }
    let valid = match (&expected.lifecycle, &replacement.lifecycle) {
        (VirtualMusigLifecycle::Generated, VirtualMusigLifecycle::Reserved)
        | (VirtualMusigLifecycle::Reserved, VirtualMusigLifecycle::CommitmentSent) => {
            replacement.encrypted_nonce == expected.encrypted_nonce
                && replacement.cached_partial.is_none()
        }
        (VirtualMusigLifecycle::CommitmentSent, VirtualMusigLifecycle::PartialSigning { .. }) => {
            replacement.encrypted_nonce.is_none() && replacement.cached_partial.is_none()
        }
        (
            VirtualMusigLifecycle::PartialSigning {
                transcript_binding: expected_binding,
            },
            VirtualMusigLifecycle::PartialProduced {
                transcript_binding: replacement_binding,
            },
        ) => {
            expected_binding == replacement_binding
                && replacement.encrypted_nonce.is_none()
                && replacement.cached_partial.as_ref().is_some_and(|partial| {
                    partial.transcript_binding() == *expected_binding
                        && partial.session_id() == expected.context.session_id()
                })
        }
        (
            VirtualMusigLifecycle::PartialProduced {
                transcript_binding: expected_binding,
            },
            VirtualMusigLifecycle::Consumed {
                transcript_binding: replacement_binding,
            },
        ) => {
            expected_binding == replacement_binding
                && replacement.encrypted_nonce.is_none()
                && replacement.cached_partial == expected.cached_partial
        }
        (lifecycle, VirtualMusigLifecycle::Burned { .. }) if !lifecycle.is_terminal() => {
            replacement.encrypted_nonce.is_none() && replacement.cached_partial.is_none()
        }
        _ => false,
    };
    if !valid {
        return Err(Error::NoncePersistence(
            "invalid virtual MuSig2 lifecycle transition".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_new_dh_record(record: &ThresholdDhRecord) -> Result<(), Error> {
    require_protocol(&record.context, CustodyProtocol::ThresholdDh)?;
    if record.revision != 0
        || record.lifecycle != ThresholdDhLifecycle::Pending
        || record.cached_result.is_some()
        || record.request_binding == [0; 32]
    {
        return Err(Error::NoncePersistence(
            "new threshold-DH record is not pending revision zero".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_dh_transition(
    expected: &ThresholdDhRecord,
    replacement: &ThresholdDhRecord,
) -> Result<(), Error> {
    if expected.context != replacement.context
        || expected.derivation != replacement.derivation
        || expected.request_binding != replacement.request_binding
        || replacement.revision != expected.revision.saturating_add(1)
    {
        return Err(Error::NoncePersistence(
            "invalid threshold-DH compare-and-swap replacement".to_string(),
        ));
    }
    let valid = match (&expected.lifecycle, &replacement.lifecycle) {
        (ThresholdDhLifecycle::Pending, ThresholdDhLifecycle::Completed) => {
            replacement.cached_result.as_ref().is_some_and(|result| {
                result.session_id() == expected.context.session_id()
                    && result.request_binding() == expected.request_binding
            })
        }
        (ThresholdDhLifecycle::Pending, ThresholdDhLifecycle::Burned { .. }) => {
            replacement.cached_result.is_none()
        }
        _ => false,
    };
    if !valid {
        return Err(Error::NoncePersistence(
            "invalid threshold-DH lifecycle transition".to_string(),
        ));
    }
    Ok(())
}

fn require_protocol(
    context: &SigningSessionContext,
    expected: CustodyProtocol,
) -> Result<(), Error> {
    if context.protocol() != expected {
        return Err(Error::InvalidSigningSession(
            "session uses another custody protocol namespace".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolRecordCommonWire {
    federation_id: [u8; 32],
    root_epoch: u64,
    application_id: [u8; 32],
    application_key_epoch: u64,
    derivation_version: u16,
    authorization_policy: [u8; 32],
    derivation_context: [u8; 32],
    derivation_tweaks: Vec<[u8; 32]>,
    authorization_reference: [u8; 32],
    message_commitment: [u8; 32],
    attempt: u32,
    signer_set: Vec<u16>,
}

impl ProtocolRecordCommonWire {
    fn from_parts(context: &SigningSessionContext, derivation: &SigningDerivation) -> Self {
        let registration = context.registration();
        Self {
            federation_id: context.federation_id(),
            root_epoch: context.root_epoch().get(),
            application_id: registration.id().to_bytes(),
            application_key_epoch: registration.key_epoch().get(),
            derivation_version: registration.derivation_version().get(),
            authorization_policy: registration.authorization_policy(),
            derivation_context: derivation.context(),
            derivation_tweaks: derivation.tweaks().to_vec(),
            authorization_reference: context.authorization_reference(),
            message_commitment: context.message_commitment(),
            attempt: context.attempt(),
            signer_set: context
                .signer_set()
                .iter()
                .map(|member| member.get())
                .collect(),
        }
    }

    fn into_parts(
        self,
        protocol: CustodyProtocol,
    ) -> Result<(SigningSessionContext, SigningDerivation), Error> {
        let registration = ApplicationRegistration::new(
            ApplicationId::from_bytes(self.application_id)?,
            ApplicationKeyEpoch::new(self.application_key_epoch)?,
            ApplicationDerivationVersion::new(self.derivation_version)?,
            self.authorization_policy,
        )?;
        let derivation = SigningDerivation::new(
            self.derivation_context,
            self.derivation_tweaks,
            SignatureMode::Plain,
        )?;
        let scope = SigningAuthorizationScope::new(
            self.federation_id,
            RootKeyEpoch::new(self.root_epoch)?,
            registration,
            derivation.binding(),
            self.authorization_reference,
            self.message_commitment,
        )?;
        let signer_set = self
            .signer_set
            .into_iter()
            .map(MemberId::new)
            .collect::<Result<Vec<_>, _>>()?;
        let context =
            SigningSessionContext::new_for_protocol(scope, self.attempt, signer_set, protocol)?;
        Ok((context, derivation))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualMusigRecordWire {
    version: u16,
    common: ProtocolRecordCommonWire,
    nonce_id: [u8; 32],
    participant: u16,
    public_nonce: Vec<u8>,
    lifecycle: VirtualMusigLifecycle,
    encrypted_nonce: Option<Vec<u8>>,
    cached_partial: Option<PartialShareWire>,
    revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialShareWire {
    participant: u16,
    session_id: [u8; 32],
    transcript_binding: [u8; 32],
    share: [u8; 32],
}

impl From<&VirtualMusigPartialShare> for PartialShareWire {
    fn from(share: &VirtualMusigPartialShare) -> Self {
        Self {
            participant: share.participant().get(),
            session_id: share.session_id().to_bytes(),
            transcript_binding: share.transcript_binding(),
            share: share.to_bytes(),
        }
    }
}

impl PartialShareWire {
    fn into_share(self) -> Result<VirtualMusigPartialShare, Error> {
        VirtualMusigPartialShare::from_parts(
            MemberId::new(self.participant)?,
            SigningSessionId::from_bytes(self.session_id)?,
            self.transcript_binding,
            self.share,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdDhRecordWire {
    version: u16,
    common: ProtocolRecordCommonWire,
    request_binding: [u8; 32],
    lifecycle: ThresholdDhLifecycle,
    cached_result: Option<DhShareWire>,
    revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DhShareWire {
    participant: u16,
    session_id: [u8; 32],
    request_binding: [u8; 32],
    shared_point: Vec<u8>,
    commitment_g: Vec<u8>,
    commitment_s: Vec<u8>,
    response: [u8; 32],
}

impl From<&ThresholdDhShare> for DhShareWire {
    fn from(share: &ThresholdDhShare) -> Self {
        Self {
            participant: share.participant().get(),
            session_id: share.session_id().to_bytes(),
            request_binding: share.request_binding(),
            shared_point: share.shared_point().to_vec(),
            commitment_g: share.proof().commitment_g().to_vec(),
            commitment_s: share.proof().commitment_s().to_vec(),
            response: share.proof().response(),
        }
    }
}

impl DhShareWire {
    fn into_share(self) -> Result<ThresholdDhShare, Error> {
        let shared_point = self
            .shared_point
            .try_into()
            .map_err(|_| Error::malformed("threshold-DH record", "shared point is not 33 bytes"))?;
        let commitment_g = self
            .commitment_g
            .try_into()
            .map_err(|_| Error::malformed("threshold-DH record", "G commitment is not 33 bytes"))?;
        let commitment_s = self
            .commitment_s
            .try_into()
            .map_err(|_| Error::malformed("threshold-DH record", "S commitment is not 33 bytes"))?;
        let proof = ThresholdDhProof::from_parts(commitment_g, commitment_s, self.response)?;
        ThresholdDhShare::from_parts(
            MemberId::new(self.participant)?,
            SigningSessionId::from_bytes(self.session_id)?,
            self.request_binding,
            shared_point,
            proof,
        )
    }
}

fn validate_virtual_payload(
    lifecycle: &VirtualMusigLifecycle,
    encrypted_nonce: Option<&EncryptedSecret>,
    cached_partial: Option<&VirtualMusigPartialShare>,
    commitment: &VirtualMusigNonceCommitment,
) -> Result<(), Error> {
    let valid = match lifecycle {
        VirtualMusigLifecycle::Generated
        | VirtualMusigLifecycle::Reserved
        | VirtualMusigLifecycle::CommitmentSent => {
            encrypted_nonce.is_some() && cached_partial.is_none()
        }
        VirtualMusigLifecycle::PartialSigning { .. } | VirtualMusigLifecycle::Burned { .. } => {
            encrypted_nonce.is_none() && cached_partial.is_none()
        }
        VirtualMusigLifecycle::PartialProduced { transcript_binding }
        | VirtualMusigLifecycle::Consumed { transcript_binding } => {
            cached_partial.is_some_and(|partial| {
                encrypted_nonce.is_none()
                    && partial.participant() == commitment.participant()
                    && partial.session_id() == commitment.session_id()
                    && partial.transcript_binding() == *transcript_binding
            })
        }
    };
    if !valid {
        return Err(Error::malformed(
            "virtual MuSig2 record",
            "secret or retry payload differs from lifecycle",
        ));
    }
    Ok(())
}

fn bounded_encode(kind: &'static str, value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| Error::NoncePersistence(error.to_string()))?;
    if bytes.len() > MAX_PROTOCOL_RECORD_BYTES {
        return Err(Error::OversizedInput {
            kind,
            max: MAX_PROTOCOL_RECORD_BYTES,
        });
    }
    Ok(bytes)
}

fn bounded_decode<T>(kind: &'static str, bytes: &[u8]) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de>,
{
    if bytes.len() > MAX_PROTOCOL_RECORD_BYTES {
        return Err(Error::OversizedInput {
            kind,
            max: MAX_PROTOCOL_RECORD_BYTES,
        });
    }
    serde_json::from_slice(bytes).map_err(|error| Error::malformed(kind, error))
}
