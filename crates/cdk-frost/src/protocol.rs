//! Durable participant state for non-FROST custody protocols.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::nonce::CustodyProtocol;
use crate::session::{Failure, MemorySessionTable, MemoryStoreTable, SessionRecord};
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
    virtual_musig: MemorySessionTable<VirtualMusigRecord>,
    threshold_dh: MemorySessionTable<ThresholdDhRecord>,
}

const PROTOCOL_LOCK_POISONED: &str = "protocol store lock poisoned";

impl MemoryNonceStore {
    fn virtual_musig_table(&self) -> MemoryStoreTable<'_, ProtocolMemoryState, VirtualMusigRecord> {
        MemoryStoreTable::new(&self.protocol_state, PROTOCOL_LOCK_POISONED, |state| {
            &mut state.virtual_musig
        })
    }

    fn threshold_dh_table(&self) -> MemoryStoreTable<'_, ProtocolMemoryState, ThresholdDhRecord> {
        MemoryStoreTable::new(&self.protocol_state, PROTOCOL_LOCK_POISONED, |state| {
            &mut state.threshold_dh
        })
    }
}

impl DurableProtocolStore for MemoryNonceStore {
    fn activate_virtual_musig_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.virtual_musig_table().activate_attempt(context)
    }

    fn reserve_virtual_musig(
        &self,
        record: VirtualMusigRecord,
    ) -> Result<VirtualMusigReservation, Error> {
        self.virtual_musig_table().reserve(record)
    }

    fn load_virtual_musig_nonce(
        &self,
        nonce_id: VirtualMusigNonceId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        self.virtual_musig_table().load_key(nonce_id)
    }

    fn load_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        self.virtual_musig_table().load_session(session_id)
    }

    fn compare_and_swap_virtual_musig(
        &self,
        expected: &VirtualMusigRecord,
        replacement: VirtualMusigRecord,
    ) -> Result<(), Error> {
        self.virtual_musig_table()
            .compare_and_swap(expected, replacement)
    }

    fn nonterminal_virtual_musig_records(&self) -> Result<Vec<VirtualMusigRecord>, Error> {
        self.virtual_musig_table().nonterminal_records()
    }

    fn compact_virtual_musig(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.virtual_musig_table().compact_terminal(retain_detailed)
    }

    fn activate_threshold_dh_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.threshold_dh_table().activate_attempt(context)
    }

    fn reserve_threshold_dh(
        &self,
        record: ThresholdDhRecord,
    ) -> Result<ThresholdDhReservation, Error> {
        self.threshold_dh_table().reserve(record)
    }

    fn load_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<ThresholdDhRecord>, Error> {
        self.threshold_dh_table().load_session(session_id)
    }

    fn compare_and_swap_threshold_dh(
        &self,
        expected: &ThresholdDhRecord,
        replacement: ThresholdDhRecord,
    ) -> Result<(), Error> {
        self.threshold_dh_table()
            .compare_and_swap(expected, replacement)
    }

    fn nonterminal_threshold_dh_records(&self) -> Result<Vec<ThresholdDhRecord>, Error> {
        self.threshold_dh_table().nonterminal_records()
    }

    fn compact_threshold_dh(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.threshold_dh_table().compact_terminal(retain_detailed)
    }
}

impl SessionRecord for VirtualMusigRecord {
    type Key = VirtualMusigNonceId;
    type Tombstone = VirtualMusigTombstone;
    type Reservation = VirtualMusigReservation;

    const KEYED_BY_SESSION: bool = false;

    fn key(&self) -> VirtualMusigNonceId {
        self.nonce_id()
    }

    fn key_bytes(key: VirtualMusigNonceId) -> [u8; 32] {
        key.to_bytes()
    }

    fn context(&self) -> &SigningSessionContext {
        &self.context
    }

    fn is_terminal(&self) -> bool {
        self.lifecycle.is_terminal()
    }

    fn tombstone(&self) -> VirtualMusigTombstone {
        VirtualMusigTombstone {
            session_id: self.context.session_id(),
            slot_id: self.context.slot_id(),
            nonce_id: self.nonce_id(),
            lifecycle: self.lifecycle.clone(),
        }
    }

    fn burned_for_replacement(&self) -> Result<Self, Error> {
        self.transition(
            VirtualMusigLifecycle::Burned {
                reason: NonceBurnReason::ReplacedAttempt,
            },
            false,
            None,
        )
    }

    fn validate_activation(context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::VirtualMusig2)
    }

    fn validate_new(&self) -> Result<(), Error> {
        validate_new_virtual_record(self)
    }

    fn validate_transition(expected: &Self, replacement: &Self) -> Result<(), Error> {
        validate_virtual_transition(expected, replacement)
    }

    fn inserted() -> VirtualMusigReservation {
        VirtualMusigReservation::Inserted
    }

    fn existing(record: Self) -> VirtualMusigReservation {
        VirtualMusigReservation::Existing(Box::new(record))
    }

    fn error(failure: Failure) -> Error {
        let session = |message: &str| Error::InvalidSigningSession(message.to_string());
        let commitment = |message: &str| Error::InvalidCommitment(message.to_string());
        let persistence = |message: &str| Error::NoncePersistence(message.to_string());
        match failure {
            Failure::StaleAttempt => session("virtual MuSig2 attempt is stale"),
            Failure::SignerSetChanged => {
                session("signer set changed within a virtual MuSig2 attempt")
            }
            Failure::ActiveRecordMissing => persistence("active virtual MuSig2 record is missing"),
            Failure::SessionCompacted => {
                commitment("virtual MuSig2 session is terminal and compacted")
            }
            Failure::NotActivated => session("virtual MuSig2 attempt was not durably activated"),
            Failure::SlotClaimed | Failure::KeyClaimed => {
                session("virtual MuSig2 slot or nonce is already claimed")
            }
            Failure::KeyCompacted => commitment("virtual MuSig2 nonce is terminal and compacted"),
            Failure::RecordDisappeared => persistence("virtual MuSig2 record disappeared"),
            Failure::ChangedConcurrently => {
                persistence("virtual MuSig2 record changed concurrently")
            }
            Failure::TerminalRecordMissing => {
                persistence("terminal virtual MuSig2 record is missing")
            }
            Failure::TerminalOrderNonterminal => {
                persistence("virtual MuSig2 terminal order references active state")
            }
        }
    }
}

impl SessionRecord for ThresholdDhRecord {
    type Key = SigningSessionId;
    type Tombstone = ThresholdDhTombstone;
    type Reservation = ThresholdDhReservation;

    const KEYED_BY_SESSION: bool = true;

    fn key(&self) -> SigningSessionId {
        self.context.session_id()
    }

    fn key_bytes(key: SigningSessionId) -> [u8; 32] {
        key.to_bytes()
    }

    fn context(&self) -> &SigningSessionContext {
        &self.context
    }

    fn is_terminal(&self) -> bool {
        self.lifecycle.is_terminal()
    }

    fn tombstone(&self) -> ThresholdDhTombstone {
        ThresholdDhTombstone {
            session_id: self.context.session_id(),
            slot_id: self.context.slot_id(),
            request_binding: self.request_binding,
            lifecycle: self.lifecycle.clone(),
        }
    }

    fn burned_for_replacement(&self) -> Result<Self, Error> {
        self.transition(
            ThresholdDhLifecycle::Burned {
                reason: NonceBurnReason::ReplacedAttempt,
            },
            None,
        )
    }

    fn validate_activation(context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::ThresholdDh)
    }

    fn validate_new(&self) -> Result<(), Error> {
        validate_new_dh_record(self)
    }

    fn validate_transition(expected: &Self, replacement: &Self) -> Result<(), Error> {
        validate_dh_transition(expected, replacement)
    }

    fn inserted() -> ThresholdDhReservation {
        ThresholdDhReservation::Inserted
    }

    fn existing(record: Self) -> ThresholdDhReservation {
        ThresholdDhReservation::Existing(Box::new(record))
    }

    fn error(failure: Failure) -> Error {
        let session = |message: &str| Error::InvalidSigningSession(message.to_string());
        let persistence = |message: &str| Error::NoncePersistence(message.to_string());
        match failure {
            Failure::StaleAttempt => session("threshold-DH attempt is stale"),
            Failure::SignerSetChanged => {
                session("signer set changed within a threshold-DH attempt")
            }
            Failure::ActiveRecordMissing => persistence("active threshold-DH record is missing"),
            Failure::SessionCompacted | Failure::KeyCompacted => {
                session("threshold-DH session is terminal and compacted")
            }
            Failure::NotActivated => session("threshold-DH attempt was not durably activated"),
            // Records are keyed by session, so a claimed key is always reported
            // as an existing or compacted session before the slot check.
            Failure::SlotClaimed | Failure::KeyClaimed => {
                session("threshold-DH slot is already claimed")
            }
            Failure::RecordDisappeared => persistence("threshold-DH record disappeared"),
            Failure::ChangedConcurrently => persistence("threshold-DH record changed concurrently"),
            Failure::TerminalRecordMissing => {
                persistence("terminal threshold-DH record is missing")
            }
            Failure::TerminalOrderNonterminal => {
                persistence("threshold-DH terminal order references active state")
            }
        }
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

pub(crate) fn require_protocol(
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
