//! Durable, application-neutral FROST signing-session and nonce state.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::protocol::ProtocolMemoryState;
use crate::types::{
    ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch, SignatureMode,
    MAX_PARTICIPANTS,
};
use crate::{
    ApplicationRegistration, EncryptedSecret, Error, MemberId, NonceCommitment, NonceId,
    RootKeyEpoch, SignatureShare, SigningDerivation, SigningSessionId,
};

/// Maximum number of terminal records retained with full retry metadata.
pub const DEFAULT_TERMINAL_NONCE_RETENTION: usize = 1_024;
const NONCE_RECORD_VERSION: u16 = 1;
const MAX_NONCE_RECORD_BYTES: usize = 512 * 1024;

/// Stable identifier for one authorization slot, independent of signing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SigningSlotId([u8; 32]);

impl SigningSlotId {
    /// Return canonical identifier bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Cryptographic protocol owning a participant-local session namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CustodyProtocol {
    /// Ordinary two-round FROST signing.
    OrdinaryFrost,
    /// Virtual MuSig2 contribution made by a threshold FROST key.
    VirtualMusig2,
    /// Verifiable threshold Diffie-Hellman contribution.
    ThresholdDh,
}

/// Complete application-neutral context committed by a signing session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningAuthorizationScope {
    federation_id: [u8; 32],
    root_epoch: RootKeyEpoch,
    registration: ApplicationRegistration,
    derivation_binding: [u8; 32],
    authorization_reference: [u8; 32],
    message_commitment: [u8; 32],
}

impl SigningAuthorizationScope {
    /// Construct an attempt-independent application authorization scope.
    pub fn new(
        federation_id: [u8; 32],
        root_epoch: RootKeyEpoch,
        registration: ApplicationRegistration,
        derivation_binding: [u8; 32],
        authorization_reference: [u8; 32],
        message_commitment: [u8; 32],
    ) -> Result<Self, Error> {
        if federation_id == [0; 32]
            || derivation_binding == [0; 32]
            || authorization_reference == [0; 32]
            || message_commitment == [0; 32]
        {
            return Err(Error::InvalidSigningSession(
                "session commitments cannot be all zeroes".to_string(),
            ));
        }
        Ok(Self {
            federation_id,
            root_epoch,
            registration,
            derivation_binding,
            authorization_reference,
            message_commitment,
        })
    }

    /// Federation identifier committed by this authorization scope.
    pub fn federation_id(&self) -> [u8; 32] {
        self.federation_id
    }

    /// Root epoch committed by this authorization scope.
    pub fn root_epoch(&self) -> RootKeyEpoch {
        self.root_epoch
    }

    /// Registered application committed by this authorization scope.
    pub fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    /// Exact child-derivation binding committed by this scope.
    pub fn derivation_binding(&self) -> [u8; 32] {
        self.derivation_binding
    }

    /// Adapter-defined accepted-operation reference.
    pub fn authorization_reference(&self) -> [u8; 32] {
        self.authorization_reference
    }

    /// Exact message or protocol-transcript commitment.
    pub fn message_commitment(&self) -> [u8; 32] {
        self.message_commitment
    }
}

/// Complete application-neutral context committed by one signing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningSessionContext {
    scope: SigningAuthorizationScope,
    protocol: CustodyProtocol,
    attempt: u32,
    signer_set: Vec<MemberId>,
    slot_id: SigningSlotId,
    session_id: SigningSessionId,
}

impl SigningSessionContext {
    /// Construct and canonically bind every session-separation field.
    pub fn new(
        scope: SigningAuthorizationScope,
        attempt: u32,
        signer_set: impl IntoIterator<Item = MemberId>,
    ) -> Result<Self, Error> {
        Self::new_for_protocol(scope, attempt, signer_set, CustodyProtocol::OrdinaryFrost)
    }

    /// Construct an attempt in a protocol-separated participant namespace.
    pub fn new_for_protocol(
        scope: SigningAuthorizationScope,
        attempt: u32,
        signer_set: impl IntoIterator<Item = MemberId>,
        protocol: CustodyProtocol,
    ) -> Result<Self, Error> {
        let signer_set = signer_set.into_iter().collect::<Vec<_>>();
        if signer_set.is_empty() || signer_set.len() > MAX_PARTICIPANTS {
            return Err(Error::InvalidSigningSession(
                "signer set is empty or exceeds the participant bound".to_string(),
            ));
        }
        if signer_set.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::InvalidSigningSession(
                "signer set must be strictly ordered and unique".to_string(),
            ));
        }

        let (slot_domain, session_domain) = match protocol {
            CustodyProtocol::OrdinaryFrost => (
                b"cdk/frost/signing-slot/v1".as_slice(),
                b"cdk/frost/signing-session/v1".as_slice(),
            ),
            CustodyProtocol::VirtualMusig2 => (
                b"cdk/frost/virtual-musig2-slot/v1".as_slice(),
                b"cdk/frost/virtual-musig2-session/v1".as_slice(),
            ),
            CustodyProtocol::ThresholdDh => (
                b"cdk/frost/threshold-dh-slot/v1".as_slice(),
                b"cdk/frost/threshold-dh-session/v1".as_slice(),
            ),
        };
        let slot_id = SigningSlotId(session_hash(slot_domain, &scope, None, &[]));
        let session_id = SigningSessionId::from_bytes(session_hash(
            session_domain,
            &scope,
            Some(attempt),
            &signer_set,
        ))?;
        Ok(Self {
            scope,
            protocol,
            attempt,
            signer_set,
            slot_id,
            session_id,
        })
    }

    /// Protocol namespace committed by this session.
    pub fn protocol(&self) -> CustodyProtocol {
        self.protocol
    }

    /// Federation identifier committed by this session.
    pub fn federation_id(&self) -> [u8; 32] {
        self.scope.federation_id
    }

    /// Root key epoch committed by this session.
    pub fn root_epoch(&self) -> RootKeyEpoch {
        self.scope.root_epoch
    }

    /// Registered application committed by this session.
    pub fn registration(&self) -> ApplicationRegistration {
        self.scope.registration
    }

    /// Exact signing-derivation binding.
    pub fn derivation_binding(&self) -> [u8; 32] {
        self.scope.derivation_binding
    }

    /// Adapter-defined authoritative authorization reference.
    pub fn authorization_reference(&self) -> [u8; 32] {
        self.scope.authorization_reference
    }

    /// Commitment to the exact message or transcript to sign.
    pub fn message_commitment(&self) -> [u8; 32] {
        self.scope.message_commitment
    }

    /// Monotonic attempt selected for this authorization slot.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Strictly ordered selected signer set.
    pub fn signer_set(&self) -> &[MemberId] {
        &self.signer_set
    }

    /// Attempt-independent authorization slot.
    pub fn slot_id(&self) -> SigningSlotId {
        self.slot_id
    }

    /// Fully bound signing session identifier.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }
}

/// Reason a nonce became permanently unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonceBurnReason {
    /// Caller explicitly aborted the attempt.
    Aborted,
    /// The response status became ambiguous.
    AmbiguousResponse,
    /// The attempt exceeded its deadline.
    Timeout,
    /// Coordinator ownership was lost.
    CoordinatorLost,
    /// A signer set changed.
    SignerSetChanged,
    /// The round-two package was invalid.
    InvalidPackage,
    /// A process restarted with a nonterminal record.
    RestartRecovery,
    /// A newer explicit attempt replaced this attempt.
    ReplacedAttempt,
    /// A persistence or cryptographic failure made use uncertain.
    InternalFailure,
}

/// Durable one-time nonce lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum NonceLifecycle {
    /// Secret material has been generated and sealed.
    Generated,
    /// The authorization slot durably owns this nonce.
    Reserved,
    /// The public commitment may have left the participant.
    CommitmentSent,
    /// Round two is durably locked; secret material cannot be loaded again.
    Signing {
        /// Hash of the exact validated signing package.
        package_hash: [u8; 32],
    },
    /// A share is durably cached but has not yet been marked delivered.
    ShareProduced {
        /// Hash of the exact package used for the cached share.
        package_hash: [u8; 32],
    },
    /// Terminal success; exact retries may return the cached share.
    Consumed {
        /// Hash required for an exact idempotent retry.
        package_hash: [u8; 32],
    },
    /// Terminal failure; this nonce can never be used again.
    Burned {
        /// Terminal reason the nonce became unusable.
        reason: NonceBurnReason,
    },
}

impl NonceLifecycle {
    /// Whether this state permanently prevents use of secret nonce material.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Consumed { .. } | Self::Burned { .. })
    }
}

/// Participant-local durable nonce record.
#[derive(Clone, PartialEq, Eq)]
pub struct NonceRecord {
    context: SigningSessionContext,
    nonce_id: NonceId,
    derivation: SigningDerivation,
    commitment: NonceCommitment,
    lifecycle: NonceLifecycle,
    encrypted_nonces: Option<EncryptedSecret>,
    cached_share: Option<SignatureShare>,
    revision: u64,
}

impl fmt::Debug for NonceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NonceRecord")
            .field("session_id", &self.context.session_id())
            .field("slot_id", &self.context.slot_id())
            .field("nonce_id", &self.nonce_id)
            .field("lifecycle", &self.lifecycle)
            .field(
                "secret",
                &self.encrypted_nonces.as_ref().map(|_| "<sealed>"),
            )
            .field(
                "cached_share",
                &self.cached_share.as_ref().map(|_| "<redacted>"),
            )
            .field("revision", &self.revision)
            .finish()
    }
}

impl NonceRecord {
    pub(crate) fn generated(
        context: SigningSessionContext,
        nonce_id: NonceId,
        derivation: SigningDerivation,
        commitment: NonceCommitment,
        encrypted_nonces: EncryptedSecret,
    ) -> Self {
        Self {
            context,
            nonce_id,
            derivation,
            commitment,
            lifecycle: NonceLifecycle::Generated,
            encrypted_nonces: Some(encrypted_nonces),
            cached_share: None,
            revision: 0,
        }
    }

    /// Fully bound session context.
    pub fn context(&self) -> &SigningSessionContext {
        &self.context
    }

    /// Single-use nonce identifier.
    pub fn nonce_id(&self) -> NonceId {
        self.nonce_id
    }

    /// Public round-one commitment.
    pub fn commitment(&self) -> &NonceCommitment {
        &self.commitment
    }

    /// Current durable lifecycle.
    pub fn lifecycle(&self) -> &NonceLifecycle {
        &self.lifecycle
    }

    /// Monotonic compare-and-swap revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Encode the complete encrypted durable record in a bounded versioned form.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let signature_mode = match self.derivation.signature_mode() {
            SignatureMode::Plain => SignatureModeWire::Plain,
            SignatureMode::TaprootKeyPath { merkle_root } => {
                SignatureModeWire::TaprootKeyPath { merkle_root }
            }
        };
        let registration = self.context.registration();
        let wire = NonceRecordWire {
            version: NONCE_RECORD_VERSION,
            federation_id: self.context.federation_id(),
            root_epoch: self.context.root_epoch().get(),
            application_id: registration.id().to_bytes(),
            application_key_epoch: registration.key_epoch().get(),
            derivation_version: registration.derivation_version().get(),
            authorization_policy: registration.authorization_policy(),
            derivation_context: self.derivation.context(),
            derivation_tweaks: self.derivation.tweaks().to_vec(),
            signature_mode,
            authorization_reference: self.context.authorization_reference(),
            message_commitment: self.context.message_commitment(),
            attempt: self.context.attempt(),
            signer_set: self
                .context
                .signer_set()
                .iter()
                .map(|member| member.get())
                .collect(),
            commitment: self.commitment.encode(),
            lifecycle: self.lifecycle.clone(),
            encrypted_nonces: self
                .encrypted_nonces
                .as_ref()
                .map(EncryptedSecret::to_bytes),
            cached_share: self.cached_share.as_ref().map(SignatureShare::encode),
            revision: self.revision,
        };
        let bytes = serde_json::to_vec(&wire)
            .map_err(|error| Error::NoncePersistence(error.to_string()))?;
        if bytes.len() > MAX_NONCE_RECORD_BYTES {
            return Err(Error::OversizedInput {
                kind: "durable nonce record",
                max: MAX_NONCE_RECORD_BYTES,
            });
        }
        Ok(bytes)
    }

    /// Decode and fully validate one encrypted durable record.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_NONCE_RECORD_BYTES {
            return Err(Error::OversizedInput {
                kind: "durable nonce record",
                max: MAX_NONCE_RECORD_BYTES,
            });
        }
        let wire: NonceRecordWire = serde_json::from_slice(bytes)
            .map_err(|error| Error::malformed("durable nonce record", error))?;
        if wire.version != NONCE_RECORD_VERSION {
            return Err(Error::malformed(
                "durable nonce record",
                "unsupported record version",
            ));
        }
        let registration = ApplicationRegistration::new(
            ApplicationId::from_bytes(wire.application_id)?,
            ApplicationKeyEpoch::new(wire.application_key_epoch)?,
            ApplicationDerivationVersion::new(wire.derivation_version)?,
            wire.authorization_policy,
        )?;
        let signature_mode = match wire.signature_mode {
            SignatureModeWire::Plain => SignatureMode::Plain,
            SignatureModeWire::TaprootKeyPath { merkle_root } => {
                SignatureMode::TaprootKeyPath { merkle_root }
            }
        };
        let derivation = SigningDerivation::new(
            wire.derivation_context,
            wire.derivation_tweaks,
            signature_mode,
        )?;
        let signer_set = wire
            .signer_set
            .into_iter()
            .map(MemberId::new)
            .collect::<Result<Vec<_>, _>>()?;
        let scope = SigningAuthorizationScope::new(
            wire.federation_id,
            RootKeyEpoch::new(wire.root_epoch)?,
            registration,
            derivation.binding(),
            wire.authorization_reference,
            wire.message_commitment,
        )?;
        let context = SigningSessionContext::new(scope, wire.attempt, signer_set)?;
        let commitment = NonceCommitment::decode(&wire.commitment)?;
        if commitment.root_epoch() != context.root_epoch()
            || commitment.registration() != context.registration()
            || commitment.session_id() != context.session_id()
            || commitment.derivation_binding() != derivation.binding()
            || !context.signer_set().contains(&commitment.participant())
        {
            return Err(Error::malformed(
                "durable nonce record",
                "commitment differs from its session context",
            ));
        }
        let encrypted_nonces = wire
            .encrypted_nonces
            .map(EncryptedSecret::from_bytes)
            .transpose()?;
        let cached_share = wire
            .cached_share
            .map(|share| SignatureShare::decode(&share))
            .transpose()?;
        validate_record_payload(
            &wire.lifecycle,
            encrypted_nonces.as_ref(),
            cached_share.as_ref(),
            &commitment,
        )?;
        Ok(Self {
            context,
            nonce_id: commitment.nonce_id(),
            derivation,
            commitment,
            lifecycle: wire.lifecycle,
            encrypted_nonces,
            cached_share,
            revision: wire.revision,
        })
    }

    pub(crate) fn derivation(&self) -> &SigningDerivation {
        &self.derivation
    }

    pub(crate) fn encrypted_nonces(&self) -> Option<&EncryptedSecret> {
        self.encrypted_nonces.as_ref()
    }

    pub(crate) fn cached_share(&self) -> Option<&SignatureShare> {
        self.cached_share.as_ref()
    }

    pub(crate) fn transition(
        &self,
        lifecycle: NonceLifecycle,
        keep_secret: bool,
        cached_share: Option<SignatureShare>,
    ) -> Result<Self, Error> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::NoncePersistence("nonce revision overflow".to_string()))?;
        Ok(Self {
            context: self.context.clone(),
            nonce_id: self.nonce_id,
            derivation: self.derivation.clone(),
            commitment: self.commitment.clone(),
            lifecycle,
            encrypted_nonces: keep_secret.then(|| self.encrypted_nonces.clone()).flatten(),
            cached_share,
            revision,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SignatureModeWire {
    Plain,
    TaprootKeyPath { merkle_root: Option<[u8; 32]> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceRecordWire {
    version: u16,
    federation_id: [u8; 32],
    root_epoch: u64,
    application_id: [u8; 32],
    application_key_epoch: u64,
    derivation_version: u16,
    authorization_policy: [u8; 32],
    derivation_context: [u8; 32],
    derivation_tweaks: Vec<[u8; 32]>,
    signature_mode: SignatureModeWire,
    authorization_reference: [u8; 32],
    message_commitment: [u8; 32],
    attempt: u32,
    signer_set: Vec<u16>,
    commitment: Vec<u8>,
    lifecycle: NonceLifecycle,
    encrypted_nonces: Option<Vec<u8>>,
    cached_share: Option<Vec<u8>>,
    revision: u64,
}

fn validate_record_payload(
    lifecycle: &NonceLifecycle,
    encrypted_nonces: Option<&EncryptedSecret>,
    cached_share: Option<&SignatureShare>,
    commitment: &NonceCommitment,
) -> Result<(), Error> {
    let payload_shape_valid = match lifecycle {
        NonceLifecycle::Generated | NonceLifecycle::Reserved | NonceLifecycle::CommitmentSent => {
            encrypted_nonces.is_some() && cached_share.is_none()
        }
        NonceLifecycle::Signing { .. } | NonceLifecycle::Burned { .. } => {
            encrypted_nonces.is_none() && cached_share.is_none()
        }
        NonceLifecycle::ShareProduced { .. } | NonceLifecycle::Consumed { .. } => {
            encrypted_nonces.is_none() && cached_share.is_some()
        }
    };
    if !payload_shape_valid {
        return Err(Error::malformed(
            "durable nonce record",
            "secret or retry payload differs from lifecycle",
        ));
    }
    if let Some(share) = cached_share {
        if share.root_epoch() != commitment.root_epoch()
            || share.registration() != commitment.registration()
            || share.session_id() != commitment.session_id()
            || share.derivation_binding() != commitment.derivation_binding()
            || share.participant() != commitment.participant()
        {
            return Err(Error::malformed(
                "durable nonce record",
                "cached share differs from its commitment",
            ));
        }
    }
    Ok(())
}

/// Compact permanent evidence that a session and nonce cannot be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonceTombstone {
    /// Signing session that remains permanently claimed.
    pub session_id: SigningSessionId,
    /// Authorization slot that remains permanently claimed for this attempt.
    pub slot_id: SigningSlotId,
    /// Nonce identifier that remains permanently unusable.
    pub nonce_id: NonceId,
    /// Terminal outcome retained after detail cleanup.
    pub lifecycle: NonceLifecycle,
}

/// Result of atomically reserving a signing session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceReservation {
    /// A new generated record was inserted.
    Inserted,
    /// The exact session already exists and is returned for retry handling.
    Existing(Box<NonceRecord>),
}

/// Durable compare-and-swap boundary owned by one participant key manager.
///
/// Implementations must not report success until the mutation is durable. All
/// session, slot, and nonce uniqueness checks must occur in the same atomic
/// transaction as insertion or replacement.
pub trait DurableNonceStore: fmt::Debug + Send + Sync + 'static {
    /// Atomically advance the durable attempt high-water mark for one slot.
    ///
    /// A higher attempt burns an active older nonce in the same transaction.
    /// Equal retries must name the same fully bound session; lower attempts are
    /// permanently stale.
    fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error>;

    /// Atomically reserve a new record, or return the exact existing session.
    fn reserve(&self, record: NonceRecord) -> Result<NonceReservation, Error>;

    /// Load a detailed record by nonce identifier.
    fn load_nonce(&self, nonce_id: NonceId) -> Result<Option<NonceRecord>, Error>;

    /// Load a detailed record by fully bound signing session identifier.
    fn load_session(&self, session_id: SigningSessionId) -> Result<Option<NonceRecord>, Error>;

    /// Atomically replace one exact record revision.
    fn compare_and_swap(
        &self,
        expected: &NonceRecord,
        replacement: NonceRecord,
    ) -> Result<(), Error>;

    /// Return every nonterminal detailed record for restart recovery.
    fn nonterminal_records(&self) -> Result<Vec<NonceRecord>, Error>;

    /// Compact old terminal detail while retaining permanent replay evidence.
    fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error>;
}

#[derive(Debug, Default)]
struct MemoryNonceState {
    records: BTreeMap<NonceId, NonceRecord>,
    sessions: BTreeMap<SigningSessionId, NonceId>,
    active_slots: BTreeMap<SigningSlotId, NonceId>,
    latest_attempts: BTreeMap<SigningSlotId, (u32, SigningSessionId)>,
    tombstones: BTreeMap<NonceId, NonceTombstone>,
    terminal_order: Vec<NonceId>,
}

/// Cloneable in-memory CAS store for tests and embedding prototypes.
///
/// Production integrations must provide a store backed by durable local
/// storage; this implementation intentionally makes no durability claim.
#[derive(Debug, Clone, Default)]
pub struct MemoryNonceStore {
    state: Arc<Mutex<MemoryNonceState>>,
    pub(crate) protocol_state: Arc<Mutex<ProtocolMemoryState>>,
}

impl MemoryNonceStore {
    /// Inspect a retained tombstone in tests or operator tooling.
    pub fn tombstone(&self, nonce_id: NonceId) -> Result<Option<NonceTombstone>, Error> {
        let state = self.lock()?;
        Ok(state.tombstones.get(&nonce_id).cloned())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, MemoryNonceState>, Error> {
        self.state
            .lock()
            .map_err(|_| Error::NoncePersistence("nonce store lock poisoned".to_string()))
    }

    pub(crate) fn protocol_lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ProtocolMemoryState>, Error> {
        self.protocol_state
            .lock()
            .map_err(|_| Error::NoncePersistence("protocol store lock poisoned".to_string()))
    }
}

impl DurableNonceStore for MemoryNonceStore {
    fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        let mut state = self.lock()?;
        let slot_id = context.slot_id();
        match state.latest_attempts.get(&slot_id).copied() {
            Some((attempt, _)) if attempt > context.attempt() => {
                return Err(Error::InvalidSigningSession(
                    "signing attempt is stale".to_string(),
                ));
            }
            Some((attempt, session_id)) if attempt == context.attempt() => {
                if session_id != context.session_id() {
                    return Err(Error::InvalidSigningSession(
                        "signer set changed within an active attempt".to_string(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }

        if let Some(nonce_id) = state.active_slots.get(&slot_id).copied() {
            let existing = state.records.get(&nonce_id).cloned().ok_or_else(|| {
                Error::NoncePersistence("active signing slot has no nonce record".to_string())
            })?;
            let burned = existing.transition(
                NonceLifecycle::Burned {
                    reason: NonceBurnReason::ReplacedAttempt,
                },
                false,
                None,
            )?;
            validate_nonce_transition(&existing, &burned)?;
            state.records.insert(nonce_id, burned);
            state.active_slots.remove(&slot_id);
            state.terminal_order.push(nonce_id);
        }
        state
            .latest_attempts
            .insert(slot_id, (context.attempt(), context.session_id()));
        Ok(())
    }

    fn reserve(&self, record: NonceRecord) -> Result<NonceReservation, Error> {
        let mut state = self.lock()?;
        let session_id = record.context.session_id();
        if let Some(nonce_id) = state.sessions.get(&session_id) {
            if let Some(existing) = state.records.get(nonce_id) {
                return Ok(NonceReservation::Existing(Box::new(existing.clone())));
            }
            return Err(Error::InvalidCommitment(
                "signing session was already compacted and cannot be retried".to_string(),
            ));
        }
        if state.latest_attempts.get(&record.context.slot_id())
            != Some(&(record.context.attempt(), session_id))
        {
            return Err(Error::InvalidSigningSession(
                "signing attempt was not durably activated".to_string(),
            ));
        }
        if state.active_slots.contains_key(&record.context.slot_id()) {
            return Err(Error::InvalidSigningSession(
                "authorization slot already has an active signing attempt".to_string(),
            ));
        }
        validate_new_nonce_record(&record)?;
        if state.records.contains_key(&record.nonce_id)
            || state.tombstones.contains_key(&record.nonce_id)
        {
            return Err(Error::InvalidCommitment(
                "nonce identifier was already reserved".to_string(),
            ));
        }
        state.sessions.insert(session_id, record.nonce_id);
        state
            .active_slots
            .insert(record.context.slot_id(), record.nonce_id);
        state.records.insert(record.nonce_id, record);
        Ok(NonceReservation::Inserted)
    }

    fn load_nonce(&self, nonce_id: NonceId) -> Result<Option<NonceRecord>, Error> {
        let state = self.lock()?;
        if state.tombstones.contains_key(&nonce_id) {
            return Err(Error::InvalidCommitment(
                "nonce is terminal and its retry metadata was compacted".to_string(),
            ));
        }
        Ok(state.records.get(&nonce_id).cloned())
    }

    fn load_session(&self, session_id: SigningSessionId) -> Result<Option<NonceRecord>, Error> {
        let state = self.lock()?;
        let Some(nonce_id) = state.sessions.get(&session_id) else {
            return Ok(None);
        };
        Ok(state.records.get(nonce_id).cloned())
    }

    fn compare_and_swap(
        &self,
        expected: &NonceRecord,
        replacement: NonceRecord,
    ) -> Result<(), Error> {
        validate_nonce_transition(expected, &replacement)?;
        let mut state = self.lock()?;
        let current = state.records.get(&expected.nonce_id).ok_or_else(|| {
            Error::NoncePersistence("nonce record disappeared during update".to_string())
        })?;
        if current != expected {
            return Err(Error::NoncePersistence(
                "nonce record changed concurrently".to_string(),
            ));
        }
        if !expected.lifecycle.is_terminal() && replacement.lifecycle.is_terminal() {
            state.active_slots.remove(&expected.context.slot_id());
            state.terminal_order.push(expected.nonce_id);
        }
        state.records.insert(expected.nonce_id, replacement);
        Ok(())
    }

    fn nonterminal_records(&self) -> Result<Vec<NonceRecord>, Error> {
        let state = self.lock()?;
        Ok(state
            .records
            .values()
            .filter(|record| !record.lifecycle.is_terminal())
            .cloned()
            .collect())
    }

    fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        let mut state = self.lock()?;
        let compact_count = state.terminal_order.len().saturating_sub(retain_detailed);
        let compact_ids = state
            .terminal_order
            .drain(..compact_count)
            .collect::<Vec<_>>();
        for nonce_id in &compact_ids {
            let record = state.records.remove(nonce_id).ok_or_else(|| {
                Error::NoncePersistence("terminal nonce record is missing".to_string())
            })?;
            state.tombstones.insert(
                *nonce_id,
                NonceTombstone {
                    session_id: record.context.session_id(),
                    slot_id: record.context.slot_id(),
                    nonce_id: *nonce_id,
                    lifecycle: record.lifecycle,
                },
            );
        }
        Ok(compact_ids.len())
    }
}

pub(crate) fn validate_new_nonce_record(record: &NonceRecord) -> Result<(), Error> {
    if record.context.protocol() != CustodyProtocol::OrdinaryFrost
        || record.revision != 0
        || record.lifecycle != NonceLifecycle::Generated
        || record.encrypted_nonces.is_none()
        || record.cached_share.is_some()
    {
        return Err(Error::NoncePersistence(
            "new nonce reservation is not a generated revision-zero record".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_nonce_transition(
    expected: &NonceRecord,
    replacement: &NonceRecord,
) -> Result<(), Error> {
    if expected.context.protocol() != CustodyProtocol::OrdinaryFrost
        || replacement.context.protocol() != CustodyProtocol::OrdinaryFrost
        || expected.nonce_id != replacement.nonce_id
        || expected.context != replacement.context
        || expected.derivation != replacement.derivation
        || expected.commitment != replacement.commitment
        || replacement.revision != expected.revision.saturating_add(1)
    {
        return Err(Error::NoncePersistence(
            "invalid nonce compare-and-swap replacement".to_string(),
        ));
    }
    let valid = match (&expected.lifecycle, &replacement.lifecycle) {
        (NonceLifecycle::Generated, NonceLifecycle::Reserved)
        | (NonceLifecycle::Reserved, NonceLifecycle::CommitmentSent) => {
            replacement.encrypted_nonces == expected.encrypted_nonces
                && replacement.cached_share.is_none()
        }
        (NonceLifecycle::CommitmentSent, NonceLifecycle::Signing { .. }) => {
            replacement.encrypted_nonces.is_none() && replacement.cached_share.is_none()
        }
        (
            NonceLifecycle::Signing {
                package_hash: expected_hash,
            },
            NonceLifecycle::ShareProduced {
                package_hash: replacement_hash,
            },
        ) => {
            expected_hash == replacement_hash
                && replacement.encrypted_nonces.is_none()
                && replacement.cached_share.is_some()
        }
        (
            NonceLifecycle::ShareProduced {
                package_hash: expected_hash,
            },
            NonceLifecycle::Consumed {
                package_hash: replacement_hash,
            },
        ) => {
            expected_hash == replacement_hash
                && replacement.encrypted_nonces.is_none()
                && replacement.cached_share == expected.cached_share
        }
        (lifecycle, NonceLifecycle::Burned { .. }) if !lifecycle.is_terminal() => {
            replacement.encrypted_nonces.is_none() && replacement.cached_share.is_none()
        }
        _ => false,
    };
    if !valid {
        return Err(Error::NoncePersistence(
            "invalid durable nonce lifecycle transition".to_string(),
        ));
    }
    Ok(())
}

fn session_hash(
    domain: &[u8],
    scope: &SigningAuthorizationScope,
    attempt: Option<u32>,
    signer_set: &[MemberId],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(scope.federation_id);
    hasher.update(scope.root_epoch.get().to_be_bytes());
    let registration = scope.registration;
    hasher.update(registration.id().to_bytes());
    hasher.update(registration.key_epoch().get().to_be_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    hasher.update(scope.derivation_binding);
    hasher.update(scope.authorization_reference);
    hasher.update(scope.message_commitment);
    if let Some(attempt) = attempt {
        hasher.update(attempt.to_be_bytes());
        hasher.update((signer_set.len() as u16).to_be_bytes());
        for signer in signer_set {
            hasher.update(signer.get().to_be_bytes());
        }
    }
    hasher.finalize().into()
}
