//! Private REST transport payloads for federation members.
//!
//! These types are the stable JSON surface for member-to-member operation
//! submission and catch-up. The first router checkpoint can use them before a
//! real consensus engine is wired behind the endpoints.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationAlephBftFinalizedItem, FederationApplicationDigest, FederationApplicationState,
    FederationCheckpoint, FederationCheckpointBoundary, FederationCheckpointFrontier,
    FederationCheckpointId, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationCheckpointSignature, FederationCommittedFrontier, FederationConfig,
    FederationConfigDigest, FederationConsensusRuntimeStatus, FederationDkgCeremonyId,
    FederationDkgKeysetSpec, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignature, FederationError, FederationId,
    FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationMaterializedStateDigest, FederationMaterializedStateRowFingerprint,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationPaymentObservationReportSummary, FederationWalletRecoveryStatus,
    InMemoryFederationJournal, MemberConfigPrivate, MemberId, Result,
    FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
};
use crate::nuts::nut01::BlsSecretKeyShare;
use crate::Amount;

/// Version of the private federation REST payload format.
pub const FEDERATION_REST_API_VERSION: u16 = 1;

const FEDERATION_REST_BODY_HASH_VERSION: &[u8] = b"cdk-federation-rest-body-v1";
const FEDERATION_REST_SIGNATURE_VERSION: &[u8] = b"cdk-federation-rest-signature-v1";
const FEDERATION_DKG_ACTIVATION_SIGNATURE_VERSION: &[u8] =
    b"cdk-federation-dkg-activation-signature-v1";

/// Private federation REST message kind covered by member signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRestMessageKind {
    /// Peer status request.
    Status,
    /// Request bounded scalar checkpoint metadata.
    CheckpointSummary,
    /// Fetch one stored checkpoint payload and assembled quorum by ID.
    CheckpointFetch,
    /// Request one member's stored signature for a checkpoint ID.
    CheckpointSignature,
    /// Publish one already assembled transferable checkpoint quorum.
    CheckpointQuorum,
    /// Explicitly audit local materialized state against the trusted checkpoint.
    CheckpointAudit,
    /// Request targeted redacted wallet transaction status.
    WalletTransactionStatus,
    /// Request an immediate exact-byte retry for one durable transaction.
    WalletRebroadcast,
    /// Confirm that a peer is ready for the exact configured DKG ceremony.
    DkgReadiness,
    /// Publish a pre-reveal DKG commitment hash to a peer.
    DkgCommitment,
    /// Reveal DKG public polynomial commitments to a peer.
    DkgReveal,
    /// Send private DKG secret-share contributions to the intended receiver.
    DkgSecretShare,
    /// Broadcast an application-neutral FROST DKG round-one package.
    FrostDkgRound1,
    /// Send one confidential application-neutral FROST DKG round-two package.
    FrostDkgRound2,
    /// Confirm one member independently derived the identical canonical FROST root.
    FrostDkgConfirmation,
    /// Ask one selected signer for a durable single-use nonce commitment.
    FrostSigningCommitment,
    /// Deliver an exact authorized signing package and request one signature share.
    FrostSigningPackage,
    /// Burn a session nonce after timeout, ambiguity, or coordinator failover.
    FrostSigningAbort,
    /// Notify selected signers of the independently verified aggregate result.
    FrostAggregateResult,
    /// Share a member signature over a DKG/setup transcript hash.
    DkgTranscriptSignature,
    /// Confirm that one member assembled the exact finalized DKG config.
    DkgActivation,
    /// Submit one operation envelope to a peer.
    ///
    /// The signed sequence is authenticated but does not provide replay
    /// ordering for this kind. Canonical operation IDs and durable consensus
    /// records own duplicate detection and retry safety.
    SubmitOperation,
    /// Wake a peer's idle AlephBFT creator for newly admitted consensus work.
    AlephBftActivity,
    /// Deliver one opaque AlephBFT network message to a peer.
    AlephBftNetworkMessage,
    /// Request missing journal entries from a peer.
    CatchUp,
}

impl FederationRestMessageKind {
    fn uses_sequence_replay_state(self) -> bool {
        self != Self::SubmitOperation
    }

    fn has_durable_replay_state(self) -> bool {
        matches!(
            self,
            Self::DkgCommitment
                | Self::DkgReveal
                | Self::DkgSecretShare
                | Self::FrostDkgRound1
                | Self::FrostDkgRound2
                | Self::FrostDkgConfirmation
                | Self::DkgTranscriptSignature
                | Self::DkgActivation
        )
    }

    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Status => b"status",
            Self::CheckpointSummary => b"checkpoint_summary",
            Self::CheckpointFetch => b"checkpoint_fetch",
            Self::CheckpointSignature => b"checkpoint_signature",
            Self::CheckpointQuorum => b"checkpoint_quorum",
            Self::CheckpointAudit => b"checkpoint_audit",
            Self::WalletTransactionStatus => b"wallet_transaction_status",
            Self::WalletRebroadcast => b"wallet_rebroadcast",
            Self::DkgReadiness => b"dkg_readiness",
            Self::DkgCommitment => b"dkg_commitment",
            Self::DkgReveal => b"dkg_reveal",
            Self::DkgSecretShare => b"dkg_secret_share",
            Self::FrostDkgRound1 => b"frost_dkg_round1",
            Self::FrostDkgRound2 => b"frost_dkg_round2",
            Self::FrostDkgConfirmation => b"frost_dkg_confirmation",
            Self::FrostSigningCommitment => b"frost_signing_commitment",
            Self::FrostSigningPackage => b"frost_signing_package",
            Self::FrostSigningAbort => b"frost_signing_abort",
            Self::FrostAggregateResult => b"frost_aggregate_result",
            Self::DkgTranscriptSignature => b"dkg_transcript_signature",
            Self::DkgActivation => b"dkg_activation",
            Self::SubmitOperation => b"submit_operation",
            Self::AlephBftActivity => b"aleph_bft_activity",
            Self::AlephBftNetworkMessage => b"aleph_bft_network_message",
            Self::CatchUp => b"catch_up",
        }
    }
}

/// Signed probe confirming a member is ready for one exact DKG ceremony.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgReadinessRequest {
    /// Federation whose members are preparing the ceremony.
    pub federation_id: FederationId,
    /// Exact ceremony identifier configured by every participant.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Exact aggregate keyset policy configured by every participant.
    pub keyset_spec: FederationDkgKeysetSpec,
}

impl FederationDkgReadinessRequest {
    /// Create a readiness probe for an exact ceremony and keyset policy.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        ceremony_id: FederationDkgCeremonyId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Self {
        Self {
            federation_id,
            ceremony_id,
            keyset_spec,
        }
    }

    /// Validate this probe against the receiver's configured ceremony.
    pub fn validate(
        &self,
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        keyset_spec: &FederationDkgKeysetSpec,
    ) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        self.keyset_spec.validate_for_federation(public_config)?;
        if self.ceremony_id != ceremony_id {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG readiness ceremony ID does not match local configuration".to_string(),
            });
        }
        if &self.keyset_spec != keyset_spec {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG readiness keyset policy does not match local configuration"
                    .to_string(),
            });
        }

        Ok(())
    }
}

/// Canonical SHA-256 hash of a signed federation REST request payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationRestBodyHash([u8; 32]);

impl FederationRestBodyHash {
    /// Create a body hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a body hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidRestBodyHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a body hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this body hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this body hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Compute the canonical payload hash.
    pub fn for_payload<T>(payload: &T) -> Result<Self>
    where
        T: Serialize + ?Sized,
    {
        let canonical = canonical_json_bytes(payload)?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_REST_BODY_HASH_VERSION);
        append_hash_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationRestBodyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationRestBodyHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationRestBodyHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationRestBodyHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Schnorr signature over a federation REST request transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationRestSignature([u8; 64]);

impl FederationRestSignature {
    /// Create a transport signature from raw Schnorr bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidRestSignatureSize {
                    expected: 64,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transport signature from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this signature as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    /// Return this signature as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn sign(transcript: &[u8], private_config: &MemberConfigPrivate) -> Result<Self> {
        Self::sign_with(
            transcript,
            private_config,
            FederationRestSigningMode::RandomAux,
        )
    }

    fn sign_no_aux_rand(transcript: &[u8], private_config: &MemberConfigPrivate) -> Result<Self> {
        Self::sign_with(
            transcript,
            private_config,
            FederationRestSigningMode::NoAuxRand,
        )
    }

    fn sign_with(
        transcript: &[u8],
        private_config: &MemberConfigPrivate,
        mode: FederationRestSigningMode,
    ) -> Result<Self> {
        let signature = match mode {
            FederationRestSigningMode::RandomAux => {
                private_config.identity_secret_key.sign(transcript)?
            }
            FederationRestSigningMode::NoAuxRand => private_config
                .identity_secret_key
                .sign_no_aux_rand(transcript)?,
        };
        Ok(Self(signature.serialize()))
    }

    fn as_schnorr(self) -> Result<Signature> {
        Signature::from_slice(&self.0).map_err(|_| FederationError::InvalidRestSignature)
    }
}

impl fmt::Display for FederationRestSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationRestSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationRestSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationRestSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederationRestSigningMode {
    RandomAux,
    NoAuxRand,
}

/// Signed private REST request between two federation members.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSignedRequest<T> {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this request.
    pub federation_id: FederationId,
    /// Member that signed the request.
    pub sender: MemberId,
    /// Member expected to receive the request.
    pub receiver: MemberId,
    /// Sender-local sequence covered by the request signature.
    ///
    /// Sequence replay policy is message-class specific. Effectful DKG messages
    /// use durable replay state; status, catch-up, readiness, and Aleph traffic
    /// use kind-isolated volatile windows. `SubmitOperation` authenticates this
    /// value for transport provenance but deliberately uses the canonical
    /// operation ID for restart-safe idempotency, so operation retries are not
    /// rejected by a transport watermark.
    pub sequence: u64,
    /// Request payload kind.
    pub kind: FederationRestMessageKind,
    /// Canonical hash of `payload`.
    pub body_hash: FederationRestBodyHash,
    /// JSON payload covered by `body_hash`.
    pub payload: T,
    /// Schnorr signature over the request transcript.
    pub signature: FederationRestSignature,
}

impl<T> FederationSignedRequest<T>
where
    T: Serialize,
{
    /// Sign a private REST request from the local member to `receiver`.
    pub fn new_signed(
        federation_id: FederationId,
        receiver: MemberId,
        sequence: u64,
        kind: FederationRestMessageKind,
        payload: T,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::new_signed_with_mode(
            federation_id,
            receiver,
            sequence,
            kind,
            payload,
            private_config,
            FederationRestSigningMode::RandomAux,
        )
    }

    /// Sign a private REST request without auxiliary randomness.
    pub fn new_signed_no_aux_rand(
        federation_id: FederationId,
        receiver: MemberId,
        sequence: u64,
        kind: FederationRestMessageKind,
        payload: T,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::new_signed_with_mode(
            federation_id,
            receiver,
            sequence,
            kind,
            payload,
            private_config,
            FederationRestSigningMode::NoAuxRand,
        )
    }

    fn new_signed_with_mode(
        federation_id: FederationId,
        receiver: MemberId,
        sequence: u64,
        kind: FederationRestMessageKind,
        payload: T,
        private_config: &MemberConfigPrivate,
        mode: FederationRestSigningMode,
    ) -> Result<Self> {
        let sender = private_config.member_id;
        let body_hash = FederationRestBodyHash::for_payload(&payload)?;
        let transcript =
            signing_transcript(federation_id, sender, receiver, sequence, kind, body_hash);
        let signature = match mode {
            FederationRestSigningMode::RandomAux => {
                FederationRestSignature::sign(&transcript, private_config)?
            }
            FederationRestSigningMode::NoAuxRand => {
                FederationRestSignature::sign_no_aux_rand(&transcript, private_config)?
            }
        };

        Ok(Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            sender,
            receiver,
            sequence,
            kind,
            body_hash,
            payload,
            signature,
        })
    }

    /// Verify the request envelope, payload hash, sender roster membership, and signature.
    pub fn verify(
        &self,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }

        if self.federation_id != public_config.federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }

        if self.receiver != expected_receiver {
            return Err(FederationError::RestReceiverMismatch {
                expected: expected_receiver,
                found: self.receiver,
            });
        }

        let member = public_config
            .member(self.sender)
            .ok_or(FederationError::UnknownMember {
                member_id: self.sender,
            })?;

        let found_body_hash = FederationRestBodyHash::for_payload(&self.payload)?;
        if self.body_hash != found_body_hash {
            return Err(FederationError::RestBodyHashMismatch {
                expected: self.body_hash,
                found: found_body_hash,
            });
        }

        let transcript = signing_transcript(
            self.federation_id,
            self.sender,
            self.receiver,
            self.sequence,
            self.kind,
            self.body_hash,
        );
        let signature = self.signature.as_schnorr()?;
        member
            .identity_public_key
            .verify(&transcript, &signature)
            .map_err(|_| FederationError::RestSignatureVerificationFailed {
                member_id: self.sender,
            })?;

        Ok(())
    }
}

const FEDERATION_REST_SEQUENCE_REORDER_WINDOW_NANOS: u64 = 600_000_000_000;
const FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES: usize = 4096;

/// Replay tracker for authenticated federation REST message classes that use
/// sequence ordering.
///
/// Private operation submission deliberately bypasses this tracker because its
/// canonical operation ID and durable consensus record own idempotency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FederationRestSequenceTracker {
    durable_dkg: FederationRestSequenceWindow,
    volatile_by_kind: BTreeMap<FederationRestMessageKind, FederationRestSequenceWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FederationRestSequenceWindow {
    highest_by_sender: BTreeMap<MemberId, u64>,
    recent_by_sender: BTreeMap<MemberId, BTreeSet<u64>>,
}

impl FederationRestSequenceTracker {
    /// Create an empty sequence tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a sequence tracker from persisted sender watermarks.
    #[must_use]
    pub fn from_dkg_sequences(highest_by_sender: BTreeMap<MemberId, u64>) -> Self {
        Self {
            durable_dkg: FederationRestSequenceWindow {
                highest_by_sender,
                recent_by_sender: BTreeMap::new(),
            },
            volatile_by_kind: BTreeMap::new(),
        }
    }

    /// Return durable effectful-DKG sender watermarks known to this tracker.
    #[must_use]
    pub fn dkg_sequences(&self) -> &BTreeMap<MemberId, u64> {
        &self.durable_dkg.highest_by_sender
    }

    /// Verify a request and record its sequence when its kind uses replay state.
    pub fn verify_and_record<T>(
        &mut self,
        request: &FederationSignedRequest<T>,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()>
    where
        T: Serialize,
    {
        self.verify_next(request, public_config, expected_receiver)?;
        self.record_sequence(request.kind, request.sender, request.sequence);

        Ok(())
    }

    /// Verify a request without recording its sequence.
    pub fn verify_next<T>(
        &self,
        request: &FederationSignedRequest<T>,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()>
    where
        T: Serialize,
    {
        request.verify(public_config, expected_receiver)?;

        self.verify_next_sequence(request.kind, request.sender, request.sequence)
    }

    /// Record an already-verified sender sequence when its kind uses replay state.
    pub fn record_sequence(
        &mut self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) {
        if let Some(window) = self.window_mut(kind) {
            window.record_sequence(sender, sequence);
        }
    }

    /// Verify an already-authenticated sender sequence without rechecking the request signature.
    pub fn verify_next_sequence(
        &self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) -> Result<()> {
        self.window(kind).map_or(Ok(()), |window| {
            window.ensure_sequence_advances(sender, sequence)
        })
    }

    fn window(&self, kind: FederationRestMessageKind) -> Option<&FederationRestSequenceWindow> {
        if !kind.uses_sequence_replay_state() {
            return None;
        }
        if kind.has_durable_replay_state() {
            return Some(&self.durable_dkg);
        }
        self.volatile_by_kind.get(&kind)
    }

    fn window_mut(
        &mut self,
        kind: FederationRestMessageKind,
    ) -> Option<&mut FederationRestSequenceWindow> {
        if !kind.uses_sequence_replay_state() {
            return None;
        }
        if kind.has_durable_replay_state() {
            return Some(&mut self.durable_dkg);
        }
        Some(self.volatile_by_kind.entry(kind).or_default())
    }
}

impl FederationRestSequenceWindow {
    fn record_sequence(&mut self, sender: MemberId, sequence: u64) {
        match self.highest_by_sender.get(&sender) {
            Some(last) if *last >= sequence => {}
            _ => {
                self.highest_by_sender.insert(sender, sequence);
            }
        }
        self.recent_by_sender
            .entry(sender)
            .or_default()
            .insert(sequence);
        self.prune_recent_sequences(sender);
    }

    fn ensure_sequence_advances(&self, sender: MemberId, sequence: u64) -> Result<()> {
        let Some(last) = self.highest_by_sender.get(&sender).copied() else {
            return Ok(());
        };

        if sequence > last {
            return Ok(());
        }

        let recent = self.recent_by_sender.get(&sender);
        if recent.is_some_and(|sequences| sequences.contains(&sequence)) {
            return Err(FederationError::RestStaleSequence {
                member_id: sender,
                last,
                found: sequence,
            });
        }

        if recent.is_some()
            && sequence.saturating_add(FEDERATION_REST_SEQUENCE_REORDER_WINDOW_NANOS) >= last
        {
            if recent.is_some_and(|sequences| {
                sequences.len() >= FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES
                    && sequences
                        .first()
                        .is_some_and(|oldest_retained| sequence < *oldest_retained)
            }) {
                return Err(FederationError::RestStaleSequence {
                    member_id: sender,
                    last,
                    found: sequence,
                });
            }
            return Ok(());
        }

        Err(FederationError::RestStaleSequence {
            member_id: sender,
            last,
            found: sequence,
        })
    }

    fn prune_recent_sequences(&mut self, sender: MemberId) {
        let Some(highest) = self.highest_by_sender.get(&sender).copied() else {
            return;
        };
        let Some(recent) = self.recent_by_sender.get_mut(&sender) else {
            return;
        };
        let cutoff = highest.saturating_sub(FEDERATION_REST_SEQUENCE_REORDER_WINDOW_NANOS);
        if let Some(first) = recent.first().copied() {
            if first < cutoff {
                *recent = recent.split_off(&cutoff);
            }
        }
        if recent.len() > FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES {
            let keep_from = recent
                .iter()
                .nth(recent.len() - FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES)
                .copied()
                .expect("oversized replay set has a retention boundary");
            *recent = recent.split_off(&keep_from);
        }
        if recent.is_empty() {
            self.recent_by_sender.remove(&sender);
        }
    }
}

/// Coordinator request for one signer to durably reserve a FROST nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostCommitmentRequest {
    /// Private signing payload version.
    pub version: u16,
    /// Canonical signing-session context bytes.
    pub context: Vec<u8>,
    /// Canonical additive signing-derivation bytes.
    pub derivation: Vec<u8>,
    /// Canonical typed application-authorization bytes.
    pub authorization: Vec<u8>,
}

impl FederationFrostCommitmentRequest {
    /// Construct a bounded round-one request.
    #[must_use]
    pub fn new(
        context: &cdk_frost::SigningSessionContext,
        derivation: &cdk_frost::SigningDerivation,
        authorization: &cdk_frost::AuthorizationRequest,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            context: context.encode(),
            derivation: derivation.encode(),
            authorization: authorization.encode(),
        }
    }

    /// Decode and cross-check every round-one session binding.
    pub fn decode(
        &self,
        federation_id: FederationId,
        signer: MemberId,
    ) -> Result<(
        cdk_frost::SigningSessionContext,
        cdk_frost::SigningDerivation,
        cdk_frost::AuthorizationRequest,
    )> {
        ensure_frost_signing_version(self.version)?;
        let context =
            cdk_frost::SigningSessionContext::decode(&self.context).map_err(frost_signing_error)?;
        let derivation =
            cdk_frost::SigningDerivation::decode(&self.derivation).map_err(frost_signing_error)?;
        let authorization = cdk_frost::AuthorizationRequest::decode(&self.authorization)
            .map_err(frost_signing_error)?;
        if context.federation_id() != federation_id.to_bytes()
            || !context
                .signer_set()
                .iter()
                .any(|member| member.get() == signer.get())
            || context.derivation_binding() != derivation.binding()
            || authorization.session_id() != context.session_id()
            || authorization.application_id() != context.registration().id()
            || authorization.application_key_epoch() != context.registration().key_epoch()
            || authorization.derivation_version() != context.registration().derivation_version()
        {
            return Err(frost_signing_invalid(
                "commitment request bindings are inconsistent",
            ));
        }
        Ok((context, derivation, authorization))
    }
}

/// Signed signer response containing one durable public nonce commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostCommitmentResponse {
    /// Private signing payload version.
    pub version: u16,
    /// Fully bound signing session identifier.
    pub session_id: [u8; 32],
    /// Canonical public nonce commitment bytes.
    pub commitment: Vec<u8>,
}

impl FederationFrostCommitmentResponse {
    /// Construct a response from one key-manager commitment.
    #[must_use]
    pub fn new(commitment: &cdk_frost::NonceCommitment) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            session_id: commitment.session_id().to_bytes(),
            commitment: commitment.encode(),
        }
    }

    /// Decode and verify signer ownership and session identity.
    pub fn decode(
        &self,
        signer: MemberId,
        expected_session: cdk_frost::SigningSessionId,
    ) -> Result<cdk_frost::NonceCommitment> {
        ensure_frost_signing_version(self.version)?;
        let commitment =
            cdk_frost::NonceCommitment::decode(&self.commitment).map_err(frost_signing_error)?;
        if self.session_id != expected_session.to_bytes()
            || commitment.session_id() != expected_session
            || commitment.participant().get() != signer.get()
        {
            return Err(frost_signing_invalid(
                "commitment response has the wrong signer or session",
            ));
        }
        Ok(commitment)
    }
}

/// Coordinator request delivering one exact authorized FROST signing package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostSigningPackageRequest {
    /// Private signing payload version.
    pub version: u16,
    /// Canonical signing-session context bytes.
    pub context: Vec<u8>,
    /// Local nonce selected by this signer in round one.
    pub nonce_id: [u8; 32],
    /// Canonical typed application-authorization bytes.
    pub authorization: Vec<u8>,
    /// Canonically ordered signer commitment set.
    pub commitment_set: Vec<u8>,
    /// Canonical hash of the exact session, message, and commitments.
    pub package_hash: [u8; 32],
}

impl FederationFrostSigningPackageRequest {
    /// Construct a complete round-two request.
    pub fn new(
        context: &cdk_frost::SigningSessionContext,
        nonce_id: cdk_frost::NonceId,
        authorization: &cdk_frost::AuthorizationRequest,
        commitments: &cdk_frost::NonceCommitmentSet,
    ) -> Result<Self> {
        let package_hash = cdk_frost::signing_package_hash(
            context.session_id(),
            &context.message_commitment(),
            commitments,
        )
        .map_err(frost_signing_error)?;
        Ok(Self {
            version: FEDERATION_REST_API_VERSION,
            context: context.encode(),
            nonce_id: nonce_id.to_bytes(),
            authorization: authorization.encode(),
            commitment_set: commitments.encode(),
            package_hash,
        })
    }

    /// Decode and cross-check the complete round-two package.
    pub fn decode(
        &self,
        federation_id: FederationId,
        signer: MemberId,
    ) -> Result<(
        cdk_frost::SigningSessionContext,
        cdk_frost::NonceId,
        cdk_frost::AuthorizationRequest,
        cdk_frost::NonceCommitmentSet,
    )> {
        ensure_frost_signing_version(self.version)?;
        let context =
            cdk_frost::SigningSessionContext::decode(&self.context).map_err(frost_signing_error)?;
        let nonce_id =
            cdk_frost::NonceId::from_bytes(self.nonce_id).map_err(frost_signing_error)?;
        let authorization = cdk_frost::AuthorizationRequest::decode(&self.authorization)
            .map_err(frost_signing_error)?;
        let commitments = cdk_frost::NonceCommitmentSet::decode(&self.commitment_set)
            .map_err(frost_signing_error)?;
        let signer = cdk_frost::MemberId::new(signer.get()).map_err(frost_signing_error)?;
        let own = commitments.commitments().get(&signer);
        let expected_hash = cdk_frost::signing_package_hash(
            context.session_id(),
            &context.message_commitment(),
            &commitments,
        )
        .map_err(frost_signing_error)?;
        if context.federation_id() != federation_id.to_bytes()
            || authorization.session_id() != context.session_id()
            || authorization.application_id() != context.registration().id()
            || authorization.application_key_epoch() != context.registration().key_epoch()
            || authorization.derivation_version() != context.registration().derivation_version()
            || own.map(cdk_frost::NonceCommitment::nonce_id) != Some(nonce_id)
            || self.package_hash != expected_hash
        {
            return Err(frost_signing_invalid(
                "signing package bindings are inconsistent",
            ));
        }
        Ok((context, nonce_id, authorization, commitments))
    }
}

/// Signed signer response containing one application-authorized signature share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostSignatureShareResponse {
    /// Private signing payload version.
    pub version: u16,
    /// Canonical signing package hash used by this share.
    pub package_hash: [u8; 32],
    /// Canonical signature-share bytes.
    pub share: Vec<u8>,
}

/// Coordinator request to permanently burn one exact signing-session nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostSigningAbortRequest {
    /// Private signing payload version.
    pub version: u16,
    /// Canonical signing-session context bytes.
    pub context: Vec<u8>,
    /// Canonical typed application authorization bytes.
    pub authorization: Vec<u8>,
    /// Terminal reason for burning any local nonce.
    pub reason: cdk_frost::NonceBurnReason,
}

impl FederationFrostSigningAbortRequest {
    /// Construct a session-bound abort request.
    #[must_use]
    pub fn new(
        context: &cdk_frost::SigningSessionContext,
        authorization: &cdk_frost::AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            context: context.encode(),
            authorization: authorization.encode(),
            reason,
        }
    }

    /// Decode and validate session, application, federation, and signer bindings.
    pub fn decode(
        &self,
        federation_id: FederationId,
        signer: MemberId,
    ) -> Result<(
        cdk_frost::SigningSessionContext,
        cdk_frost::AuthorizationRequest,
        cdk_frost::NonceBurnReason,
    )> {
        ensure_frost_signing_version(self.version)?;
        let context =
            cdk_frost::SigningSessionContext::decode(&self.context).map_err(frost_signing_error)?;
        let authorization = cdk_frost::AuthorizationRequest::decode(&self.authorization)
            .map_err(frost_signing_error)?;
        if context.federation_id() != federation_id.to_bytes()
            || authorization.session_id() != context.session_id()
            || authorization.application_id() != context.registration().id()
            || !context
                .signer_set()
                .iter()
                .any(|member| member.get() == signer.get())
        {
            return Err(frost_signing_invalid(
                "signing abort bindings are inconsistent",
            ));
        }
        Ok((context, authorization, self.reason))
    }
}

impl FederationFrostSignatureShareResponse {
    /// Construct a share response after durable nonce consumption.
    #[must_use]
    pub fn new(package_hash: [u8; 32], share: &cdk_frost::SignatureShare) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            package_hash,
            share: share.encode(),
        }
    }

    /// Decode and verify signer, session, and package ownership.
    pub fn decode(
        &self,
        signer: MemberId,
        expected_session: cdk_frost::SigningSessionId,
        expected_package_hash: [u8; 32],
    ) -> Result<cdk_frost::SignatureShare> {
        ensure_frost_signing_version(self.version)?;
        let share = cdk_frost::SignatureShare::decode(&self.share).map_err(frost_signing_error)?;
        if self.package_hash != expected_package_hash
            || share.session_id() != expected_session
            || share.participant().get() != signer.get()
        {
            return Err(frost_signing_invalid(
                "signature share has the wrong signer, session, or package hash",
            ));
        }
        Ok(share)
    }
}

/// Coordinator notification of an independently verified aggregate signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostAggregateResultNotification {
    /// Private signing payload version.
    pub version: u16,
    /// Canonical signing-session context bytes.
    pub context: Vec<u8>,
    /// Canonical typed application-authorization bytes.
    pub authorization: Vec<u8>,
    /// Exact signing package hash that produced the aggregate.
    pub package_hash: [u8; 32],
    /// Compact BIP340 aggregate signature.
    pub signature: Vec<u8>,
}

impl FederationFrostAggregateResultNotification {
    /// Construct a successful aggregate-result notification.
    #[must_use]
    pub fn new(
        context: &cdk_frost::SigningSessionContext,
        authorization: &cdk_frost::AuthorizationRequest,
        package_hash: [u8; 32],
        signature: cdk_frost::AggregateSignature,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            context: context.encode(),
            authorization: authorization.encode(),
            package_hash,
            signature: signature.to_bytes().to_vec(),
        }
    }

    /// Decode the bounded aggregate signature and session identifier.
    pub fn decode(
        &self,
    ) -> Result<(
        cdk_frost::SigningSessionContext,
        cdk_frost::AuthorizationRequest,
        [u8; 32],
        cdk_frost::AggregateSignature,
    )> {
        ensure_frost_signing_version(self.version)?;
        let context =
            cdk_frost::SigningSessionContext::decode(&self.context).map_err(frost_signing_error)?;
        let authorization = cdk_frost::AuthorizationRequest::decode(&self.authorization)
            .map_err(frost_signing_error)?;
        if authorization.session_id() != context.session_id()
            || authorization.application_id() != context.registration().id()
        {
            return Err(frost_signing_invalid(
                "aggregate result has inconsistent application or session bindings",
            ));
        }
        let signature: [u8; 64] = self.signature.clone().try_into().map_err(|_| {
            frost_signing_invalid("aggregate signature must contain exactly 64 bytes")
        })?;
        let signature =
            cdk_frost::AggregateSignature::from_bytes(signature).map_err(frost_signing_error)?;
        Ok((context, authorization, self.package_hash, signature))
    }
}

/// Signed acknowledgement for an aggregate-result notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostSigningAck {
    /// Private signing payload version.
    pub version: u16,
    /// Accepted session identifier.
    pub session_id: [u8; 32],
    /// Accepted request kind.
    pub kind: FederationRestMessageKind,
}

impl FederationFrostSigningAck {
    /// Construct an acknowledgement for one signing message.
    #[must_use]
    pub fn new(session_id: cdk_frost::SigningSessionId, kind: FederationRestMessageKind) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            session_id: session_id.to_bytes(),
            kind,
        }
    }
}

fn ensure_frost_signing_version(version: u16) -> Result<()> {
    if version != FEDERATION_REST_API_VERSION {
        return Err(FederationError::RestVersionMismatch {
            expected: FEDERATION_REST_API_VERSION,
            found: version,
        });
    }
    Ok(())
}

fn frost_signing_error(error: cdk_frost::Error) -> FederationError {
    frost_signing_invalid(error.to_string())
}

fn frost_signing_invalid(reason: impl ToString) -> FederationError {
    FederationError::FrostSigningInvalid {
        reason: reason.to_string(),
    }
}

/// Signed payload used to request a peer status snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPeerStatusRequest {
    /// Include per-row materialized state fingerprints for operator diffing.
    pub include_materialized_rows: bool,
}

impl FederationPeerStatusRequest {
    /// Request materialized-state row fingerprints in addition to status digests.
    #[must_use]
    pub const fn with_materialized_rows() -> Self {
        Self {
            include_materialized_rows: true,
        }
    }
}

/// Signed request for bounded scalar checkpoint metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSummaryRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Ask reachable members to form one recovery checkpoint at a stable converged tip.
    pub request_recovery_checkpoint: bool,
    /// Return the first stored scheduled quorum after this sequence, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_scheduled_quorum_after: Option<u64>,
}

impl FederationCheckpointSummaryRequest {
    /// Construct a checkpoint-summary request.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            request_recovery_checkpoint: false,
            next_scheduled_quorum_after: None,
        }
    }

    /// Request on-demand recovery-checkpoint formation.
    #[must_use]
    pub const fn with_recovery_checkpoint(mut self) -> Self {
        self.request_recovery_checkpoint = true;
        self
    }

    /// Request the first stored scheduled quorum after `scheduled_checkpoint_sequence`.
    #[must_use]
    pub const fn with_next_scheduled_quorum_after(
        mut self,
        scheduled_checkpoint_sequence: u64,
    ) -> Self {
        self.next_scheduled_quorum_after = Some(scheduled_checkpoint_sequence);
        self
    }

    /// Validate the private REST payload version.
    pub fn validate(&self) -> Result<()> {
        ensure_federation_rest_version(self.version)
    }
}

impl Default for FederationCheckpointSummaryRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded identity and canonical recovery frontier for one stored checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSummary {
    /// Canonical checkpoint ID.
    pub checkpoint_id: FederationCheckpointId,
    /// Scheduled or recovery checkpoint policy.
    pub kind: FederationCheckpointKind,
    /// Latest scheduled sequence represented by the checkpoint.
    pub scheduled_checkpoint_sequence: u64,
    /// Exact committed recovery frontier represented by the checkpoint.
    pub frontier: FederationCommittedFrontier,
}

impl FederationCheckpointSummary {
    /// Build scalar metadata from a validated checkpoint payload.
    pub fn from_checkpoint(checkpoint: &FederationCheckpoint) -> Result<Self> {
        Ok(Self {
            checkpoint_id: checkpoint.id()?,
            kind: checkpoint.kind,
            scheduled_checkpoint_sequence: checkpoint.scheduled_checkpoint_sequence,
            frontier: checkpoint.committed_frontier(),
        })
    }

    /// Confirm that this summary describes the exact supplied checkpoint.
    pub fn validate_checkpoint(&self, checkpoint: &FederationCheckpoint) -> Result<()> {
        if *self != Self::from_checkpoint(checkpoint)? {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint summary does not match the checkpoint payload".to_string(),
            });
        }
        Ok(())
    }

    /// Return whether this summary makes the candidate proof redundant.
    #[must_use]
    pub fn covers(&self, candidate: &Self) -> bool {
        self.checkpoint_id == candidate.checkpoint_id
            || self.scheduled_checkpoint_sequence > candidate.scheduled_checkpoint_sequence
            || (self.scheduled_checkpoint_sequence == candidate.scheduled_checkpoint_sequence
                && self.frontier.ensure_covers(&candidate.frontier).is_ok())
    }
}

/// Bounded scalar checkpoint metadata returned during ordinary peer polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSummaryResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member serving this response.
    pub member_id: MemberId,
    /// Exact current terminal complete-unit frontier, without a state-audit digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_tip: Option<FederationCheckpointBoundary>,
    /// Newest locally stored scheduled candidate, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_scheduled_candidate: Option<FederationCheckpointSummary>,
    /// Newest locally stored recovery candidate, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_recovery_candidate: Option<FederationCheckpointSummary>,
    /// Newest locally stored scheduled quorum, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_scheduled_quorum: Option<FederationCheckpointSummary>,
    /// Newest locally stored recovery quorum, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_recovery_quorum: Option<FederationCheckpointSummary>,
    /// First locally stored scheduled quorum after the requested sequence, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_scheduled_quorum: Option<FederationCheckpointSummary>,
}

/// Explicit operator request to audit local state against the trusted checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointAuditRequest {
    /// Private REST payload version.
    pub version: u16,
}

impl FederationCheckpointAuditRequest {
    /// Construct a checkpoint audit request.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
        }
    }

    /// Validate the private REST payload version.
    pub fn validate(&self) -> Result<()> {
        ensure_federation_rest_version(self.version)
    }
}

impl Default for FederationCheckpointAuditRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// Successful explicit operator audit of one exact trusted checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointAuditResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this member.
    pub federation_id: FederationId,
    /// Member whose local state was audited.
    pub member_id: MemberId,
    /// Trusted checkpoint audited at the current exact frontier.
    pub checkpoint: FederationCheckpointSummary,
    /// Full materialized-state digest recomputed by the operator request.
    pub materialized_state_digest: FederationMaterializedStateDigest,
}

impl FederationCheckpointSummaryResponse {
    /// Validate response bindings and summary kinds.
    pub fn validate(&self, config: &FederationConfig, expected_member_id: MemberId) -> Result<()> {
        ensure_federation_rest_version(self.version)?;
        if self.federation_id != config.federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: config.federation_id,
                found: self.federation_id,
            });
        }
        if self.member_id != expected_member_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint summary response member mismatch".to_string(),
            });
        }
        if let Some(stable_tip) = self.stable_tip.as_ref() {
            stable_tip.validate_against_config(config)?;
        }
        for (summary, expected_kind) in [
            (
                self.latest_scheduled_candidate.as_ref(),
                FederationCheckpointKind::Scheduled,
            ),
            (
                self.latest_recovery_candidate.as_ref(),
                FederationCheckpointKind::Recovery,
            ),
            (
                self.latest_scheduled_quorum.as_ref(),
                FederationCheckpointKind::Scheduled,
            ),
            (
                self.latest_recovery_quorum.as_ref(),
                FederationCheckpointKind::Recovery,
            ),
            (
                self.next_scheduled_quorum.as_ref(),
                FederationCheckpointKind::Scheduled,
            ),
        ] {
            if let Some(summary) = summary {
                if summary.kind != expected_kind {
                    return Err(FederationError::CheckpointInvalid {
                        reason: "checkpoint summary field contains the wrong checkpoint kind"
                            .to_string(),
                    });
                }
                summary.frontier.validate_complete_unit(config)?;
            }
        }
        Ok(())
    }
}

/// Signed request to fetch one stored checkpoint payload and quorum by ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointFetchRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Exact checkpoint to fetch.
    pub checkpoint_id: FederationCheckpointId,
}

impl FederationCheckpointFetchRequest {
    /// Construct a checkpoint fetch request.
    #[must_use]
    pub const fn new(checkpoint_id: FederationCheckpointId) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            checkpoint_id,
        }
    }

    /// Validate the private REST payload version.
    pub fn validate(&self) -> Result<()> {
        ensure_federation_rest_version(self.version)
    }
}

/// Stored checkpoint material returned without recomputation or new signing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointFetchResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member serving this response.
    pub member_id: MemberId,
    /// Stored checkpoint payload, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<FederationCheckpoint>,
    /// Stored assembled quorum, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quorum: Option<FederationCheckpointQuorum>,
}

impl FederationCheckpointFetchResponse {
    /// Validate response bindings and all returned checkpoint material.
    pub fn validate(
        &self,
        config: &FederationConfig,
        expected_member_id: MemberId,
        expected_checkpoint_id: FederationCheckpointId,
    ) -> Result<()> {
        ensure_federation_rest_version(self.version)?;
        if self.federation_id != config.federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: config.federation_id,
                found: self.federation_id,
            });
        }
        if self.member_id != expected_member_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint fetch response member mismatch".to_string(),
            });
        }
        if let Some(checkpoint) = self.checkpoint.as_ref() {
            checkpoint.validate_against_config(config)?;
            if checkpoint.id()? != expected_checkpoint_id {
                return Err(FederationError::CheckpointInvalid {
                    reason: "checkpoint fetch returned a different checkpoint payload".to_string(),
                });
            }
        }
        if let Some(quorum) = self.quorum.as_ref() {
            quorum.verify(config)?;
            if quorum.checkpoint.id()? != expected_checkpoint_id {
                return Err(FederationError::CheckpointInvalid {
                    reason: "checkpoint fetch returned a quorum for a different checkpoint"
                        .to_string(),
                });
            }
            if self
                .checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint != &quorum.checkpoint)
            {
                return Err(FederationError::CheckpointInvalid {
                    reason: "checkpoint fetch payload and quorum disagree".to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Signed request for one member's signature over an exact checkpoint ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSignatureRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Exact locally derived checkpoint whose stored signature is requested.
    pub checkpoint_id: FederationCheckpointId,
}

impl FederationCheckpointSignatureRequest {
    /// Construct a checkpoint-signature request.
    #[must_use]
    pub const fn new(checkpoint_id: FederationCheckpointId) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            checkpoint_id,
        }
    }

    /// Validate the private REST payload version.
    pub fn validate(&self) -> Result<()> {
        ensure_federation_rest_version(self.version)
    }
}

/// One already persisted member checkpoint signature, when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSignatureResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member serving this response.
    pub member_id: MemberId,
    /// Stored local signature, if this member independently derived the checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<FederationCheckpointSignature>,
}

impl FederationCheckpointSignatureResponse {
    /// Validate response bindings and the optional signature.
    pub fn validate(
        &self,
        config: &FederationConfig,
        expected_member_id: MemberId,
        checkpoint: &FederationCheckpoint,
    ) -> Result<()> {
        ensure_federation_rest_version(self.version)?;
        if self.federation_id != config.federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: config.federation_id,
                found: self.federation_id,
            });
        }
        if self.member_id != expected_member_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint signature response member mismatch".to_string(),
            });
        }
        if let Some(signature) = self.signature.as_ref() {
            let member =
                config
                    .member(expected_member_id)
                    .ok_or(FederationError::UnknownMember {
                        member_id: expected_member_id,
                    })?;
            signature.verify(checkpoint, member)?;
        }
        Ok(())
    }
}

/// Signed publication of one already assembled transferable checkpoint quorum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointQuorumRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Bounded quorum proof to verify and persist.
    pub quorum: FederationCheckpointQuorum,
}

impl FederationCheckpointQuorumRequest {
    /// Construct a checkpoint quorum publication.
    #[must_use]
    pub fn new(quorum: FederationCheckpointQuorum) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            quorum,
        }
    }

    /// Validate the payload version and bounded quorum proof.
    pub fn validate(&self, config: &FederationConfig) -> Result<()> {
        ensure_federation_rest_version(self.version)?;
        self.quorum.verify(config)?;
        Ok(())
    }
}

/// Acknowledgement that a checkpoint quorum was verified and persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointQuorumResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member serving this response.
    pub member_id: MemberId,
    /// Checkpoint whose quorum was persisted.
    pub checkpoint_id: FederationCheckpointId,
}

impl FederationCheckpointQuorumResponse {
    /// Validate the acknowledgement bindings.
    pub fn validate(
        &self,
        expected_federation_id: FederationId,
        expected_member_id: MemberId,
        expected_checkpoint_id: FederationCheckpointId,
    ) -> Result<()> {
        ensure_federation_rest_version(self.version)?;
        if self.federation_id != expected_federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: expected_federation_id,
                found: self.federation_id,
            });
        }
        if self.member_id != expected_member_id || self.checkpoint_id != expected_checkpoint_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint quorum acknowledgement mismatch".to_string(),
            });
        }
        Ok(())
    }
}

fn ensure_federation_rest_version(version: u16) -> Result<()> {
    if version != FEDERATION_REST_API_VERSION {
        return Err(FederationError::RestVersionMismatch {
            expected: FEDERATION_REST_API_VERSION,
            found: version,
        });
    }
    Ok(())
}

/// Signed targeted operator request for one durable wallet transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletTransactionRequest {
    /// Private REST payload version.
    pub version: u16,
    /// Canonical accepted transaction proposal identity.
    pub proposal_id: super::FederationTransactionProposalId,
}

impl FederationWalletTransactionRequest {
    /// Construct one versioned targeted operator request.
    #[must_use]
    pub fn new(proposal_id: super::FederationTransactionProposalId) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            proposal_id,
        }
    }

    /// Reject requests from incompatible private API versions.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        Ok(())
    }
}

/// Result of a guarded immediate exact-byte rebroadcast attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletRebroadcastResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Whether this request submitted a durable consensus transition or observation.
    pub advanced: bool,
    /// Redacted transaction state after the attempt.
    pub status: super::FederationWalletTransactionStatus,
}

/// Signed request payload carrying one member's pre-reveal DKG commitment hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgCommitmentRequest {
    /// DKG commitment being shared by the signed sender.
    pub commitment: FederationDkgParticipantCommitment,
}

impl FederationDkgCommitmentRequest {
    /// Create a DKG commitment transport request.
    #[must_use]
    pub fn new(commitment: FederationDkgParticipantCommitment) -> Self {
        Self { commitment }
    }

    /// Validate the commitment and ensure the signed sender owns the payload.
    pub fn validate_sender(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        self.commitment.validate_against_config(public_config)?;
        ensure_dkg_payload_sender(self.commitment.member_id, sender)
    }

    /// Validate the signed owner and bind this commitment to the active ceremony.
    pub fn validate_sender_for_ceremony(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        ceremony_id: FederationDkgCeremonyId,
    ) -> Result<()> {
        ensure_dkg_payload_sender(self.commitment.member_id, sender)?;
        if self.commitment.ceremony_id != ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.commitment.member_id,
                reason: "commitment belongs to another ceremony".to_string(),
            });
        }
        self.commitment.validate_against_config(public_config)
    }
}

/// Signed request payload revealing one member's DKG public commitments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgRevealRequest {
    /// DKG reveal being shared by the signed sender.
    pub reveal: FederationDkgParticipantCommitmentReveal,
}

impl FederationDkgRevealRequest {
    /// Create a DKG reveal transport request.
    #[must_use]
    pub fn new(reveal: FederationDkgParticipantCommitmentReveal) -> Self {
        Self { reveal }
    }

    /// Validate the reveal and ensure the signed sender owns the payload.
    pub fn validate_sender(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        self.reveal.validate_against_config(public_config)?;
        ensure_dkg_payload_sender(self.reveal.member_id, sender)
    }

    /// Validate the signed owner and exact active ceremony policy before ingestion.
    pub fn validate_sender_for_ceremony(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        ceremony_id: FederationDkgCeremonyId,
        keyset_spec: &FederationDkgKeysetSpec,
    ) -> Result<()> {
        ensure_dkg_payload_sender(self.reveal.member_id, sender)?;
        if self.reveal.ceremony_id != ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.reveal.member_id,
                reason: "reveal belongs to another ceremony".to_string(),
            });
        }
        if &self.reveal.keyset_spec != keyset_spec {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.reveal.member_id,
                reason: "reveal keyset policy does not match the active ceremony".to_string(),
            });
        }
        if self.reveal.coefficient_commitments.len() != keyset_spec.amounts.len()
            || !self
                .reveal
                .coefficient_commitments
                .keys()
                .eq(keyset_spec.amounts.iter())
        {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.reveal.member_id,
                reason: "reveal amount set does not match the active ceremony".to_string(),
            });
        }
        self.reveal.validate_against_config(public_config)
    }
}

/// Signed request payload carrying one participant's private DKG contributions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgSecretShareRequest {
    /// Federation whose DKG round this contribution belongs to.
    pub federation_id: FederationId,
    /// Unique DKG ceremony this private contribution belongs to.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Participant that generated the secret-share contributions.
    pub sender: MemberId,
    /// Member that must receive and store these local secret contributions.
    pub receiver: MemberId,
    /// Secret contribution per amount, evaluated for `receiver`.
    pub secret_shares: BTreeMap<Amount, BlsSecretKeyShare>,
}

impl FederationDkgSecretShareRequest {
    /// Create a DKG secret-share contribution transport request.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        ceremony_id: FederationDkgCeremonyId,
        sender: MemberId,
        receiver: MemberId,
        secret_shares: BTreeMap<Amount, BlsSecretKeyShare>,
    ) -> Self {
        Self {
            federation_id,
            ceremony_id,
            sender,
            receiver,
            secret_shares,
        }
    }

    /// Validate the contribution and bind it to the signed sender and receiver.
    pub fn validate_sender_receiver(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        receiver: MemberId,
    ) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        public_config
            .member(self.sender)
            .ok_or(FederationError::MissingMember {
                member_id: self.sender,
            })?;
        public_config
            .member(self.receiver)
            .ok_or(FederationError::MissingMember {
                member_id: self.receiver,
            })?;
        ensure_dkg_payload_sender(self.sender, sender)?;
        if self.receiver != receiver {
            return Err(FederationError::RestReceiverMismatch {
                expected: receiver,
                found: self.receiver,
            });
        }
        if self.secret_shares.is_empty() {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.sender,
                reason: "empty local secret contributions".to_string(),
            });
        }

        let signer_id = self.receiver.to_bls_signer_id();
        for (amount, secret_share) in &self.secret_shares {
            if secret_share.signer_id() != signer_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.sender,
                    reason: format!("secret contribution signer mismatch for amount {amount}"),
                });
            }
        }

        Ok(())
    }

    /// Validate the signed endpoints and exact active ceremony contribution shape.
    pub fn validate_sender_receiver_for_ceremony(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
        receiver: MemberId,
        ceremony_id: FederationDkgCeremonyId,
        keyset_spec: &FederationDkgKeysetSpec,
    ) -> Result<()> {
        ensure_dkg_payload_sender(self.sender, sender)?;
        if self.receiver != receiver {
            return Err(FederationError::RestReceiverMismatch {
                expected: receiver,
                found: self.receiver,
            });
        }
        if self.ceremony_id != ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.sender,
                reason: "secret contribution belongs to another ceremony".to_string(),
            });
        }
        if self.secret_shares.len() != keyset_spec.amounts.len()
            || !self.secret_shares.keys().eq(keyset_spec.amounts.iter())
        {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.sender,
                reason: "secret contribution amount set does not match the active ceremony"
                    .to_string(),
            });
        }
        self.validate_sender_receiver(public_config, sender, receiver)
    }
}

/// Signed request payload carrying one member's DKG/setup transcript signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgTranscriptSignatureRequest {
    /// Federation whose transcript is being signed.
    pub federation_id: FederationId,
    /// Member that produced the transcript signature.
    pub signer: MemberId,
    /// Public setup transcript hash being signed.
    pub transcript_hash: FederationDkgTranscriptHash,
    /// Member identity signature over the transcript hash.
    pub signature: FederationDkgTranscriptSignature,
}

impl FederationDkgTranscriptSignatureRequest {
    /// Create a DKG transcript signature transport request.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        signer: MemberId,
        transcript_hash: FederationDkgTranscriptHash,
        signature: FederationDkgTranscriptSignature,
    ) -> Self {
        Self {
            federation_id,
            signer,
            transcript_hash,
            signature,
        }
    }

    /// Validate the transcript signature and ensure the signed sender owns it.
    pub fn validate_sender(
        &self,
        public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        ensure_dkg_payload_sender(self.signer, sender)?;
        let member = public_config
            .member(self.signer)
            .ok_or(FederationError::MissingMember {
                member_id: self.signer,
            })?;
        self.signature.verify(self.transcript_hash, member)
    }
}

/// Signed confirmation that one member assembled the exact finalized DKG config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgActivationRequest {
    /// Federation whose finalized configuration is being confirmed.
    pub federation_id: FederationId,
    /// Dealer-free ceremony that produced the finalized keyset.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Member confirming readiness to activate.
    pub signer: MemberId,
    /// Complete public setup transcript hash confirmed by this member.
    pub transcript_hash: FederationDkgTranscriptHash,
    /// Digest of the complete finalized public federation config.
    pub final_config_digest: FederationConfigDigest,
    /// Identity signature over the domain-separated activation binding.
    pub signature: FederationDkgTranscriptSignature,
}

impl FederationDkgActivationRequest {
    /// Sign an activation confirmation after complete final config assembly.
    pub fn sign(
        ceremony_id: FederationDkgCeremonyId,
        final_public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        final_public_config.validate()?;
        Self::sign_for_validated_config(ceremony_id, final_public_config, private_config)
    }

    /// Sign after the caller has fully validated the finalized config.
    ///
    /// This avoids repeating threshold-key and transcript validation in DKG
    /// planner paths that have just assembled and validated the same immutable
    /// finalized config.
    pub fn sign_for_validated_config(
        ceremony_id: FederationDkgCeremonyId,
        final_public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        let mut setup_public_config = final_public_config.clone();
        setup_public_config.keysets.clear();
        setup_public_config.setup_transcript = None;
        setup_public_config.frost_wallet = None;
        let mut identity_config = private_config.clone();
        identity_config.keyset_secret_shares.clear();
        identity_config.setup_transcript_hash = None;
        identity_config.validate_against_public(&setup_public_config)?;
        let transcript = final_public_config
            .setup_transcript
            .as_ref()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "DKG activation requires a complete setup transcript".to_string(),
            })?;
        if !transcript
            .payload
            .ceremonies
            .iter()
            .any(|ceremony| ceremony.ceremony_id() == ceremony_id)
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG activation ceremony {ceremony_id} is absent from the final transcript"
                ),
            });
        }
        let signer = private_config.member_id;
        let transcript_hash = transcript.transcript_hash;
        let final_config_digest = final_public_config.config_digest_for_validated_config()?;
        let signing_hash = dkg_activation_signing_hash(
            final_public_config.federation_id,
            ceremony_id,
            signer,
            transcript_hash,
            final_config_digest,
        );
        let signature =
            FederationDkgTranscriptSignature::sign_no_aux_rand(signing_hash, private_config)?;
        Ok(Self {
            federation_id: final_public_config.federation_id,
            ceremony_id,
            signer,
            transcript_hash,
            final_config_digest,
            signature,
        })
    }

    /// Validate an activation confirmation against setup and finalized configs.
    pub fn validate_sender(
        &self,
        setup_public_config: &FederationConfig,
        final_public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        setup_public_config.validate()?;
        final_public_config.validate()?;
        self.validate_sender_against_validated_configs(
            setup_public_config,
            final_public_config,
            sender,
        )
    }

    /// Validate after the caller has fully validated both federation configs.
    ///
    /// All activation binding and identity-signature checks remain mandatory;
    /// only repeated validation of the immutable configs is skipped.
    pub fn validate_sender_against_validated_configs(
        &self,
        setup_public_config: &FederationConfig,
        final_public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        self.validate_signature_sender_against_validated_config(setup_public_config, sender)?;
        if self.federation_id != setup_public_config.federation_id
            || final_public_config.federation_id != setup_public_config.federation_id
        {
            return Err(FederationError::FederationIdMismatch {
                expected: setup_public_config.federation_id,
                found: self.federation_id,
            });
        }

        let mut expected_setup = final_public_config.clone();
        expected_setup.keysets.clear();
        expected_setup.setup_transcript = None;
        expected_setup.frost_wallet = None;
        if &expected_setup != setup_public_config {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG activation final config does not extend the setup config".to_string(),
            });
        }
        let transcript = final_public_config
            .setup_transcript
            .as_ref()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "DKG activation requires a complete setup transcript".to_string(),
            })?;
        if !transcript
            .payload
            .ceremonies
            .iter()
            .any(|ceremony| ceremony.ceremony_id() == self.ceremony_id)
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG activation ceremony {} is absent from the final transcript",
                    self.ceremony_id
                ),
            });
        }
        if self.transcript_hash != transcript.transcript_hash {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG activation transcript hash mismatch".to_string(),
            });
        }
        let expected_digest = final_public_config.config_digest_for_validated_config()?;
        if self.final_config_digest != expected_digest {
            return Err(FederationError::FederationConfigDigestMismatch {
                member_id: self.signer,
                expected: expected_digest,
                found: self.final_config_digest,
            });
        }
        Ok(())
    }

    /// Validate the signed activation payload before final-config comparison.
    pub fn validate_signature_sender(
        &self,
        setup_public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        setup_public_config.validate()?;
        self.validate_signature_sender_against_validated_config(setup_public_config, sender)
    }

    fn validate_signature_sender_against_validated_config(
        &self,
        setup_public_config: &FederationConfig,
        sender: MemberId,
    ) -> Result<()> {
        ensure_dkg_payload_sender(self.signer, sender)?;
        if self.federation_id != setup_public_config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: setup_public_config.federation_id,
                found: self.federation_id,
            });
        }
        let member =
            setup_public_config
                .member(self.signer)
                .ok_or(FederationError::MissingMember {
                    member_id: self.signer,
                })?;
        self.signature.verify(
            dkg_activation_signing_hash(
                self.federation_id,
                self.ceremony_id,
                self.signer,
                self.transcript_hash,
                self.final_config_digest,
            ),
            member,
        )
    }
}

fn dkg_activation_signing_hash(
    federation_id: FederationId,
    ceremony_id: FederationDkgCeremonyId,
    signer: MemberId,
    transcript_hash: FederationDkgTranscriptHash,
    final_config_digest: FederationConfigDigest,
) -> FederationDkgTranscriptHash {
    let mut transcript = Vec::with_capacity(
        FEDERATION_DKG_ACTIVATION_SIGNATURE_VERSION.len() + 32 + 32 + 2 + 32 + 32,
    );
    transcript.extend_from_slice(FEDERATION_DKG_ACTIVATION_SIGNATURE_VERSION);
    transcript.extend_from_slice(&federation_id.to_bytes());
    transcript.extend_from_slice(&ceremony_id.to_bytes());
    transcript.extend_from_slice(&signer.get().to_be_bytes());
    transcript.extend_from_slice(&transcript_hash.to_bytes());
    transcript.extend_from_slice(&final_config_digest.to_bytes());
    FederationDkgTranscriptHash::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
}

/// Response acknowledging a validated DKG setup payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgSetupResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation that accepted the setup payload.
    pub federation_id: FederationId,
    /// Member that accepted the setup payload.
    pub receiver: MemberId,
    /// Request kind that was accepted.
    pub kind: FederationRestMessageKind,
    /// Member whose setup payload was accepted.
    pub accepted_member: MemberId,
}

impl FederationDkgSetupResponse {
    /// Create a DKG setup acknowledgement.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        receiver: MemberId,
        kind: FederationRestMessageKind,
        accepted_member: MemberId,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            receiver,
            kind,
            accepted_member,
        }
    }

    /// Validate this acknowledgement against the request that produced it.
    pub fn validate(
        &self,
        federation_id: FederationId,
        receiver: MemberId,
        kind: FederationRestMessageKind,
        accepted_member: MemberId,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        if self.federation_id != federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: federation_id,
                found: self.federation_id,
            });
        }
        if self.receiver != receiver {
            return Err(FederationError::RestReceiverMismatch {
                expected: receiver,
                found: self.receiver,
            });
        }
        if self.kind != kind {
            return Err(FederationError::RestMessageKindMismatch {
                expected: kind,
                found: self.kind,
            });
        }
        if self.accepted_member != accepted_member {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: accepted_member,
                reason: "acknowledged member mismatch".to_string(),
            });
        }

        Ok(())
    }
}

/// Aggregated invalid-operation alert counters exposed to federation operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationInvalidOperationAlertSummary {
    /// Redacted source category, such as `auth_token` or `client_ip`.
    pub source: String,
    /// Number of source buckets currently above the alert threshold.
    pub alerting_buckets: u64,
    /// Highest invalid-operation count among alerting buckets.
    pub max_count: u64,
    /// Count required for one bucket to alert.
    pub threshold: u64,
    /// Alert aggregation window in seconds.
    pub window_secs: u64,
}

/// Redacted local status for one active federated on-chain wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainWalletStatus {
    /// Non-sensitive source implementation name.
    pub source: String,
    /// Operator-declared source independence (`independent` or `shared`).
    pub source_scope: String,
    /// Stable opaque source identity; never an endpoint or credential.
    pub source_id: String,
    /// Whether this source is intentionally correlated with other members.
    pub correlated_source_warning: bool,
    /// Stable health name (`never_synced`, `healthy`, or `degraded`).
    pub health: String,
    /// Latest locally persisted BDK checkpoint height.
    pub local_tip_height: u32,
    /// Latest locally persisted BDK checkpoint hash.
    pub local_tip_hash: String,
    /// Unix time of the most recent successful synchronization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_unix: Option<u64>,
    /// Saturating consecutive failure count.
    pub consecutive_failures: u32,
}

/// Detailed private federation member status used by operator tooling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationLocalStatusResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Consensus-order and materialized-state status for this member.
    pub peer_status: FederationPeerStatus,
    /// Stable local synchronization state name.
    pub sync_state: String,
    /// Whether public federation operations may currently be served.
    pub sync_ready: bool,
    /// Redacted operator-facing synchronization detail.
    pub sync_message: String,
    /// Peer involved in the current synchronization state, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_peer: Option<MemberId>,
    /// Local operation count for a lagging state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_local_operation_count: Option<u64>,
    /// Peer operation count for a lagging state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_peer_operation_count: Option<u64>,
    /// Operation count being verified after catch-up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_operation_count: Option<u64>,
    /// Operations waiting in the local AlephBFT mempool.
    pub pending_mempool_items: u64,
    /// Public requests waiting for an operation result.
    pub pending_response_waiters: u64,
    /// Cached completed operation responses.
    pub response_cache_items: u64,
    /// Total response-cache lookups.
    pub response_cache_lookups: u64,
    /// Total response-cache hits.
    pub response_cache_hits: u64,
    /// Response-cache hit rate in basis points.
    pub response_cache_hit_rate_bps: u64,
    /// Duplicate operation submissions observed locally.
    pub duplicate_submissions: u64,
    /// Duplicate submissions joined to an existing pending waiter.
    pub duplicate_pending_waiter_joins: u64,
    /// Bounded checkpoint, quorum, application-digest, and audit status.
    pub checkpoint: FederationCheckpointStatus,
    /// Payment-observation health summaries.
    pub payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
    /// Local AlephBFT runtime counters, when the runtime is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aleph_bft_status: Option<FederationConsensusRuntimeStatus>,
    /// Whether the local AlephBFT live session is active.
    pub aleph_bft_live_session_active: bool,
    /// Whether the AlephBFT network inbound queue is active.
    pub aleph_bft_network_inbound_active: bool,
    /// Optional redacted materialized-state row fingerprints.
    pub materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
    /// Aggregated invalid-operation alerts.
    pub invalid_operation_alerts: Vec<FederationInvalidOperationAlertSummary>,
    /// Active local federated on-chain wallet health records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub onchain_wallets: Vec<FederationOnchainWalletStatus>,
    /// Redacted state-to-action recovery classification for the joint wallet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_recovery: Option<FederationWalletRecoveryStatus>,
    /// Redacted proposer and signing-coordinator liveness state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_liveness: Option<super::FederationWalletLivenessStatus>,
    /// Bounded accepted-state wallet diagnosis without addresses or transaction contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_operator: Option<super::FederationWalletOperatorStatus>,
}

/// Scalar metadata for one stored, previously verified checkpoint quorum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointQuorumSummary {
    /// Checkpoint identity and recovery cursors.
    pub checkpoint: FederationCheckpointSummary,
    /// Configured member IDs whose stored signatures form the proof.
    pub signers: Vec<MemberId>,
}

impl FederationCheckpointQuorumSummary {
    /// Build bounded status metadata from a quorum verified before persistence.
    pub fn from_stored_quorum(quorum: &FederationCheckpointQuorum) -> Result<Self> {
        let signers = quorum
            .signatures
            .iter()
            .map(|signature| signature.signer)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Self {
            checkpoint: FederationCheckpointSummary::from_checkpoint(&quorum.checkpoint)?,
            signers,
        })
    }
}

/// Bounded checkpoint status used by ordinary operator polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointStatus {
    /// Current durable rolling digests and local audit latch.
    pub frontier: FederationCheckpointFrontier,
    /// Newest locally captured scheduled checkpoint candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_scheduled_checkpoint: Option<FederationCheckpointSummary>,
    /// Newest stored scheduled checkpoint quorum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_scheduled_quorum: Option<FederationCheckpointQuorumSummary>,
    /// Newest stored recovery checkpoint quorum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_recovery_quorum: Option<FederationCheckpointQuorumSummary>,
    /// Locally trusted checkpoint used as the recovery base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_checkpoint: Option<FederationCheckpointQuorumSummary>,
    /// Accepted operations after the locally trusted checkpoint.
    pub lag_from_trusted_checkpoint: u64,
}

impl Default for FederationCheckpointStatus {
    fn default() -> Self {
        Self {
            frontier: FederationCheckpointFrontier::genesis(),
            latest_scheduled_checkpoint: None,
            latest_scheduled_quorum: None,
            latest_recovery_quorum: None,
            trusted_checkpoint: None,
            lag_from_trusted_checkpoint: 0,
        }
    }
}

/// Peer status response for federation health and catch-up checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPeerStatus {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member ID of this peer.
    pub member_id: MemberId,
    /// Next AlephBFT finalized-item index required by this peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aleph_bft_next_item_index: Option<u64>,
    /// AlephBFT session represented by this peer status.
    pub aleph_bft_session: u64,
    /// Number of accepted operations in the local journal.
    pub operation_count: u64,
    /// Highest accepted operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
    /// Cumulative terminal-application digest at `operation_count`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_digest: Option<FederationApplicationDigest>,
    /// Checkpoint that owns `materialized_state_digest`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_audited_checkpoint_id: Option<FederationCheckpointId>,
    /// Digest of deterministic materialized database state, when computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub materialized_state_digest: Option<FederationMaterializedStateDigest>,
}

impl FederationPeerStatus {
    /// Return whether both members report the same committed and audited state.
    #[must_use]
    pub fn has_same_committed_state(&self, other: &Self) -> bool {
        self.aleph_bft_session == other.aleph_bft_session
            && self.has_same_committed_history_and_audit(other)
    }

    /// Return whether committed history and audit state match, ignoring active session.
    #[must_use]
    pub fn has_same_committed_history_and_audit(&self, other: &Self) -> bool {
        self.federation_id == other.federation_id
            && self.aleph_bft_next_item_index == other.aleph_bft_next_item_index
            && self.operation_count == other.operation_count
            && self.latest_index == other.latest_index
            && self.latest_order_digest == other.latest_order_digest
            && self.application_digest == other.application_digest
            && self.latest_audited_checkpoint_id == other.latest_audited_checkpoint_id
            && self.materialized_state_digest == other.materialized_state_digest
    }

    /// Create a peer status snapshot.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        member_id: MemberId,
        operation_count: u64,
        latest_index: Option<u64>,
        latest_order_digest: FederationJournalDigest,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            member_id,
            aleph_bft_next_item_index: Some(0),
            aleph_bft_session: 0,
            operation_count,
            latest_index,
            latest_order_digest,
            application_digest: None,
            latest_audited_checkpoint_id: None,
            materialized_state_digest: None,
        }
    }

    /// Create a peer status snapshot with an explicit materialized-state digest.
    #[must_use]
    pub fn new_with_materialized_state_digest(
        federation_id: FederationId,
        member_id: MemberId,
        operation_count: u64,
        latest_index: Option<u64>,
        latest_order_digest: FederationJournalDigest,
        materialized_state_digest: FederationMaterializedStateDigest,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            member_id,
            aleph_bft_next_item_index: Some(0),
            aleph_bft_session: 0,
            operation_count,
            latest_index,
            latest_order_digest,
            application_digest: None,
            latest_audited_checkpoint_id: None,
            materialized_state_digest: Some(materialized_state_digest),
        }
    }

    /// Return this status with an explicit AlephBFT finalized-item cursor.
    #[must_use]
    pub fn with_aleph_bft_next_item_index(mut self, next_item_index: u64) -> Self {
        self.aleph_bft_next_item_index = Some(next_item_index);
        self
    }

    /// Return this status with an explicit AlephBFT session.
    #[must_use]
    pub fn with_aleph_bft_session(mut self, session: u64) -> Self {
        self.aleph_bft_session = session;
        self
    }

    /// Create an order-only peer status snapshot from the in-memory journal.
    ///
    /// The materialized-state digest is absent and must be supplied by an owner
    /// that can read the durable mint state before convergence comparison.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        member_id: MemberId,
        journal: &InMemoryFederationJournal,
    ) -> Self {
        let latest_index = journal.next_index().checked_sub(1);
        Self::new(
            federation_id,
            member_id,
            journal.operation_count(),
            latest_index,
            journal.latest_order_digest(),
        )
    }
}

fn ensure_dkg_payload_sender(payload_member: MemberId, sender: MemberId) -> Result<()> {
    if payload_member != sender {
        return Err(FederationError::DkgCommitmentMismatch {
            member_id: payload_member,
            reason: "request sender mismatch".to_string(),
        });
    }

    Ok(())
}

/// Request to submit an operation envelope to a federation peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSubmitOperationRequest {
    /// Operation envelope proposed for consensus.
    pub envelope: FederationOperationEnvelope,
}

impl FederationSubmitOperationRequest {
    /// Create an operation submit request.
    #[must_use]
    pub fn new(envelope: FederationOperationEnvelope) -> Self {
        Self { envelope }
    }

    /// Compute the submitted operation ID.
    pub fn operation_id(&self) -> super::Result<FederationOperationId> {
        self.envelope.operation_id()
    }

    /// Reject this submit request if its operation envelope exceeds the default consensus limit.
    pub fn ensure_admissible_size(&self) -> super::Result<usize> {
        self.envelope.ensure_admissible_size()
    }
}

/// Response to an operation submit request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSubmitOperationResponse {
    /// Submitted operation ID.
    pub operation_id: FederationOperationId,
    /// Journal append result.
    pub outcome: FederationJournalAppendOutcome,
}

impl FederationSubmitOperationResponse {
    /// Create a submit response from a journal append outcome.
    #[must_use]
    pub fn from_outcome(outcome: FederationJournalAppendOutcome) -> Self {
        Self {
            operation_id: outcome.entry().operation_id,
            outcome,
        }
    }

    /// Verify this response matches the operation that was submitted.
    pub fn validate(&self, expected_operation_id: FederationOperationId) -> Result<()> {
        if self.operation_id != expected_operation_id {
            return Err(FederationError::SubmitOperationResponseMismatch {
                expected: expected_operation_id,
                found: self.operation_id,
            });
        }

        let outcome_operation_id = self.outcome.entry().operation_id;
        if outcome_operation_id != self.operation_id {
            return Err(FederationError::SubmitOperationResponseMismatch {
                expected: self.operation_id,
                found: outcome_operation_id,
            });
        }

        let outcome_envelope_operation_id = self.outcome.entry().envelope.operation_id()?;
        if outcome_envelope_operation_id != outcome_operation_id {
            return Err(FederationError::SubmitOperationResponseMismatch {
                expected: outcome_operation_id,
                found: outcome_envelope_operation_id,
            });
        }

        Ok(())
    }

    /// Reject terminal non-accepted consensus outcomes before callers read applied state.
    pub fn ensure_accepted(&self) -> Result<()> {
        match self.outcome.entry().status() {
            FederationOperationStatus::Accepted
            | FederationOperationStatus::Applying
            | FederationOperationStatus::Applied => Ok(()),
            FederationOperationStatus::Rejected | FederationOperationStatus::Failed => {
                Err(FederationError::RejectedOperation {
                    operation_id: self.operation_id,
                    status: self.outcome.entry().status(),
                })
            }
        }
    }
}

/// Signed control request that wakes one idle AlephBFT live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftActivityRequest {
    /// AlephBFT session containing the newly admitted operation.
    pub session_id: u64,
    /// Operation whose admission requires the federation to resume unit creation.
    pub operation_id: FederationOperationId,
}

impl FederationAlephBftActivityRequest {
    /// Create an operation-bound activity request.
    #[must_use]
    pub const fn new(session_id: u64, operation_id: FederationOperationId) -> Self {
        Self {
            session_id,
            operation_id,
        }
    }
}

/// Response acknowledging one authenticated AlephBFT activity request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftActivityResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this response.
    pub federation_id: FederationId,
    /// AlephBFT session that was woken.
    pub session_id: u64,
    /// Operation bound to the accepted activity request.
    pub operation_id: FederationOperationId,
}

impl FederationAlephBftActivityResponse {
    /// Create an accepted activity response.
    #[must_use]
    pub const fn new(
        federation_id: FederationId,
        session_id: u64,
        operation_id: FederationOperationId,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            session_id,
            operation_id,
        }
    }

    /// Verify this response belongs to the expected activity request.
    pub fn validate(
        &self,
        expected_federation_id: FederationId,
        expected_session_id: u64,
        expected_operation_id: FederationOperationId,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        if self.federation_id != expected_federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: expected_federation_id,
                found: self.federation_id,
            });
        }
        if self.session_id != expected_session_id {
            return Err(FederationError::AlephBftUnitSessionMismatch {
                expected: expected_session_id,
                found: self.session_id,
            });
        }
        if self.operation_id != expected_operation_id {
            return Err(FederationError::AlephBftActivityOperationMismatch {
                expected: expected_operation_id,
                found: self.operation_id,
            });
        }

        Ok(())
    }
}

/// Request to deliver one opaque AlephBFT network message to a federation peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftNetworkMessageRequest {
    /// AlephBFT session this message belongs to.
    pub session_id: u64,
    /// SCALE-encoded upstream AlephBFT network message as hex.
    pub encoded_message: String,
}

impl FederationAlephBftNetworkMessageRequest {
    /// Create an AlephBFT network-message request.
    #[must_use]
    pub fn new(session_id: u64, encoded_message: Vec<u8>) -> Self {
        Self {
            session_id,
            encoded_message: crate::util::hex::encode(encoded_message),
        }
    }

    /// Decode the SCALE-encoded AlephBFT network message bytes.
    pub fn message_bytes(&self) -> Result<Vec<u8>> {
        Ok(crate::util::hex::decode(&self.encoded_message)?)
    }

    /// Reject this request if the encoded message is malformed or too large.
    pub fn ensure_admissible_size(&self) -> Result<usize> {
        let bytes = self.message_bytes()?;
        let actual = bytes.len();
        if actual > FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES {
            return Err(FederationError::ConsensusItemTooLarge {
                actual,
                max: FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
            });
        }

        Ok(actual)
    }
}

/// Response to an opaque AlephBFT network-message delivery request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftNetworkMessageResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this response.
    pub federation_id: FederationId,
    /// AlephBFT session that accepted the message.
    pub session_id: u64,
}

impl FederationAlephBftNetworkMessageResponse {
    /// Create a response for an accepted AlephBFT network message.
    #[must_use]
    pub fn new(federation_id: FederationId, session_id: u64) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            session_id,
        }
    }

    /// Verify this response belongs to the expected federation and session.
    pub fn validate(
        &self,
        expected_federation_id: FederationId,
        expected_session_id: u64,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }
        if self.federation_id != expected_federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: expected_federation_id,
                found: self.federation_id,
            });
        }
        if self.session_id != expected_session_id {
            return Err(FederationError::AlephBftUnitSessionMismatch {
                expected: expected_session_id,
                found: self.session_id,
            });
        }

        Ok(())
    }
}

/// Request journal entries starting at the next missing index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationJournalCatchUpRequest {
    /// First missing zero-based journal index.
    pub next_index: u64,
    /// Stored quorum checkpoint that bounds this catch-up page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_checkpoint_id: Option<FederationCheckpointId>,
    /// Optional maximum entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    /// Optional digest the requester has after `next_index - 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_order_digest: Option<FederationJournalDigest>,
    /// Next missing AlephBFT finalized-item index for AlephBFT catch-up.
    pub aleph_bft_next_item_index: u64,
    /// Optional bounded AlephBFT backup-page request for live-session recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aleph_bft_backup: Option<FederationAlephBftBackupRequest>,
}

impl FederationJournalCatchUpRequest {
    /// Create a catch-up request from a next missing index.
    #[must_use]
    pub fn new(next_index: u64) -> Self {
        Self {
            next_index,
            target_checkpoint_id: None,
            limit: None,
            known_order_digest: None,
            aleph_bft_next_item_index: 0,
            aleph_bft_backup: None,
        }
    }

    /// Bound the returned bodies to one already fetched quorum checkpoint.
    #[must_use]
    pub fn with_target_checkpoint(mut self, checkpoint_id: FederationCheckpointId) -> Self {
        self.target_checkpoint_id = Some(checkpoint_id);
        self
    }

    /// Set an optional entry limit.
    #[must_use]
    pub fn with_limit(mut self, limit: u16) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the requester's known order digest.
    #[must_use]
    pub fn with_known_order_digest(mut self, digest: FederationJournalDigest) -> Self {
        self.known_order_digest = Some(digest);
        self
    }

    /// Set the requester's next missing AlephBFT finalized-item index.
    #[must_use]
    pub fn with_aleph_bft_next_item_index(mut self, next_item_index: u64) -> Self {
        self.aleph_bft_next_item_index = next_item_index;
        self
    }

    /// Request AlephBFT backup chunks starting at `next_chunk` for one session.
    #[must_use]
    pub fn with_aleph_bft_backup(mut self, session: u64, next_chunk: u64) -> Self {
        self.aleph_bft_backup = Some(FederationAlephBftBackupRequest {
            session,
            next_chunk,
        });
        self
    }

    /// Return the bounded limit to apply to a response.
    #[must_use]
    pub fn effective_limit(&self, default_limit: u16, max_limit: u16) -> usize {
        let requested = self.limit.unwrap_or(default_limit).min(max_limit);
        usize::from(requested)
    }
}

/// One page request for an append-only AlephBFT session backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftBackupRequest {
    /// AlephBFT session whose signed DAG backup is requested.
    pub session: u64,
    /// First missing zero-based backup chunk.
    pub next_chunk: u64,
}

/// One bounded page of an append-only AlephBFT session backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftBackupResponse {
    /// AlephBFT session represented by these chunks.
    pub session: u64,
    /// Zero-based index of the first returned chunk.
    pub from_chunk: u64,
    /// Consecutive opaque upstream AlephBFT backup chunks.
    pub chunks: Vec<Vec<u8>>,
    /// Whether this page reaches the peer's current append-only backup end.
    pub complete: bool,
}

/// Compact operation commitment and post-consensus application metadata for catch-up.
///
/// The finalized AlephBFT items in a catch-up response are the authoritative
/// replayable history. This projection deliberately does not duplicate the
/// operation envelope, conflict keys, or consensus evidence that replay derives
/// from those finalized items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCatchUpOperation {
    /// Zero-based accepted-operation index.
    pub index: u64,
    /// Stable operation ID committed by the accepted-order digest chain.
    pub operation_id: FederationOperationId,
    /// Digest before this operation was appended.
    pub previous_order_digest: FederationJournalDigest,
    /// Digest after this operation was appended.
    pub order_digest: FederationJournalDigest,
    /// Local post-consensus application state reported by the serving member.
    pub application: FederationApplicationState,
}

impl From<&FederationJournalEntry> for FederationCatchUpOperation {
    fn from(entry: &FederationJournalEntry) -> Self {
        Self {
            index: entry.index,
            operation_id: entry.operation_id,
            previous_order_digest: entry.previous_order_digest,
            order_digest: entry.order_digest,
            application: entry.application.clone(),
        }
    }
}

/// Catch-up response containing finalized history and compact application metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationJournalCatchUpResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// First requested index.
    pub from_index: u64,
    /// Exact quorum checkpoint that bounded this page, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_checkpoint_id: Option<FederationCheckpointId>,
    /// Compact operation commitments returned in ascending order.
    pub entries: Vec<FederationCatchUpOperation>,
    /// Highest local operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest local accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
    /// Latest local materialized-state digest.
    pub materialized_state_digest: FederationMaterializedStateDigest,
    /// First requested AlephBFT finalized-item index.
    pub aleph_bft_from_item_index: u64,
    /// AlephBFT finalized items returned for exact adapter replay.
    pub aleph_bft_finalized_items: Vec<FederationAlephBftFinalizedItem>,
    /// Whether all requested AlephBFT finalized items were returned.
    pub aleph_bft_finalized_items_complete: bool,
    /// Optional bounded AlephBFT backup page requested for session recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aleph_bft_backup: Option<FederationAlephBftBackupResponse>,
    /// Whether the peer returned all entries currently available from `from_index`.
    pub complete: bool,
}

impl FederationJournalCatchUpResponse {
    /// Validate the contiguous operation commitments carried by this page.
    pub fn validate_entries(&self) -> Result<()> {
        let mut expected_index = self.from_index;
        let mut expected_previous = self
            .entries
            .first()
            .map_or(self.latest_order_digest, |entry| {
                entry.previous_order_digest
            });
        for entry in &self.entries {
            if entry.index != expected_index {
                return Err(FederationError::CatchUpIndexMismatch {
                    expected: expected_index,
                    found: entry.index,
                });
            }
            if entry.previous_order_digest != expected_previous {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: entry.index,
                    expected: expected_previous,
                    found: entry.previous_order_digest,
                });
            }
            let expected_order_digest = entry
                .previous_order_digest
                .append(entry.index, entry.operation_id);
            if entry.order_digest != expected_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: entry.index.saturating_add(1),
                    expected: expected_order_digest,
                    found: entry.order_digest,
                });
            }
            expected_previous = entry.order_digest;
            expected_index = expected_index.saturating_add(1);
        }
        Ok(())
    }

    /// Create a compact catch-up response from an in-memory journal.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        journal: &InMemoryFederationJournal,
        materialized_state_digest: FederationMaterializedStateDigest,
        request: &FederationJournalCatchUpRequest,
        default_limit: u16,
        max_limit: u16,
    ) -> Self {
        let mut entries = journal
            .entries_from(request.next_index)
            .iter()
            .map(FederationCatchUpOperation::from)
            .collect::<Vec<_>>();
        let limit = request.effective_limit(default_limit, max_limit);
        let complete = entries.len() <= limit;
        entries.truncate(limit);
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            from_index: request.next_index,
            target_checkpoint_id: request.target_checkpoint_id,
            entries,
            latest_index: journal.next_index().checked_sub(1),
            latest_order_digest: journal.latest_order_digest(),
            materialized_state_digest,
            aleph_bft_from_item_index: request.aleph_bft_next_item_index,
            aleph_bft_finalized_items: Vec::new(),
            aleph_bft_finalized_items_complete: true,
            aleph_bft_backup: None,
            complete,
        }
    }

    /// Bind this response to an exact checkpoint and discard journal bodies beyond it.
    pub fn bound_to_checkpoint(mut self, checkpoint: &FederationCheckpoint) -> Result<Self> {
        let checkpoint_id = checkpoint.id()?;
        if self.target_checkpoint_id != Some(checkpoint_id) {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up target checkpoint binding mismatch".to_string(),
            });
        }
        let remaining = checkpoint
            .journal_next_index
            .checked_sub(self.from_index)
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "catch-up journal cursor is ahead of target checkpoint".to_string(),
            })?;
        let remaining =
            usize::try_from(remaining).map_err(|_| FederationError::CheckpointInvalid {
                reason: "catch-up checkpoint range does not fit in memory".to_string(),
            })?;
        self.entries.truncate(remaining);
        self.latest_index = checkpoint.journal_next_index.checked_sub(1);
        self.latest_order_digest = checkpoint.journal_order_digest;
        self.materialized_state_digest = checkpoint.materialized_state_digest;
        self.complete = self.entries.len() == remaining;
        Ok(self)
    }

    /// Validate that this page is bounded by the supplied quorum checkpoint.
    pub fn validate_checkpoint_binding(&self, checkpoint: &FederationCheckpoint) -> Result<()> {
        self.validate_entries()?;
        let checkpoint_id = checkpoint.id()?;
        if self.target_checkpoint_id != Some(checkpoint_id)
            || self.federation_id != checkpoint.federation_id
            || self.latest_index != checkpoint.journal_next_index.checked_sub(1)
            || self.latest_order_digest != checkpoint.journal_order_digest
            || self.materialized_state_digest != checkpoint.materialized_state_digest
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up response does not match its target checkpoint".to_string(),
            });
        }
        let returned_next_index = self
            .from_index
            .checked_add(self.entries.len() as u64)
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "catch-up journal cursor overflow".to_string(),
            })?;
        if returned_next_index > checkpoint.journal_next_index
            || self.complete != (returned_next_index == checkpoint.journal_next_index)
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up response crosses or misstates its checkpoint boundary"
                    .to_string(),
            });
        }
        let mut expected_finalized_index = self.aleph_bft_from_item_index;
        for item in &self.aleph_bft_finalized_items {
            if item.accepted_item_index != expected_finalized_index {
                return Err(FederationError::CheckpointInvalid {
                    reason: "catch-up finalized items are not contiguous".to_string(),
                });
            }
            expected_finalized_index =
                expected_finalized_index.checked_add(1).ok_or_else(|| {
                    FederationError::CheckpointInvalid {
                        reason: "catch-up finalized-item cursor overflow".to_string(),
                    }
                })?;
        }
        if expected_finalized_index > checkpoint.finalized_next_item_index
            || self.aleph_bft_finalized_items_complete
                != (expected_finalized_index == checkpoint.finalized_next_item_index)
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up finalized page crosses or misstates its checkpoint boundary"
                    .to_string(),
            });
        }
        Ok(())
    }

    /// Attach AlephBFT finalized items for adapter replay catch-up.
    #[must_use]
    pub fn with_aleph_bft_finalized_items(
        mut self,
        finalized_items: Vec<FederationAlephBftFinalizedItem>,
        complete: bool,
    ) -> Self {
        self.aleph_bft_finalized_items = finalized_items;
        self.aleph_bft_finalized_items_complete = complete;
        self
    }

    /// Attach one bounded AlephBFT backup page for live-session recovery.
    #[must_use]
    pub fn with_aleph_bft_backup(mut self, backup: FederationAlephBftBackupResponse) -> Self {
        self.aleph_bft_backup = Some(backup);
        self
    }
}

fn signing_transcript(
    federation_id: FederationId,
    sender: MemberId,
    receiver: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    body_hash: FederationRestBodyHash,
) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_hash_bytes(&mut transcript, FEDERATION_REST_SIGNATURE_VERSION);
    transcript.extend_from_slice(&FEDERATION_REST_API_VERSION.to_be_bytes());
    append_hash_bytes(&mut transcript, &federation_id.to_bytes());
    transcript.extend_from_slice(&sender.get().to_be_bytes());
    transcript.extend_from_slice(&receiver.get().to_be_bytes());
    transcript.extend_from_slice(&sequence.to_be_bytes());
    append_hash_bytes(&mut transcript, kind.as_bytes());
    append_hash_bytes(&mut transcript, &body_hash.to_bytes());
    transcript
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use super::super::{
        FederationApplicationDigest, FederationConfig, FederationDkgKeysetSpec, FederationError,
        FederationFinalizedItemDigest, MemberConfigPrivate, MemberConfigPublic, ThresholdParams,
        FEDERATION_MAX_DKG_KEYSET_AMOUNTS, FEDERATION_MAX_DKG_MEMBERS,
        FEDERATION_MAX_DKG_REQUEST_BODY_BYTES,
    };
    use super::*;
    use crate::{
        mint_url::MintUrl, Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request,
        MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey,
        SecretKey,
    };
    use cashu::nuts::nut01::{evaluate_secret_key_coefficients, BlsSecretKey, BlsSecretKeyShare};

    fn federation_id() -> FederationId {
        FederationId::from_bytes([11; 32])
    }

    fn dkg_ceremony_id() -> FederationDkgCeremonyId {
        FederationDkgCeremonyId::from_bytes([12; 32])
    }

    fn member_id() -> MemberId {
        MemberId::new(1).expect("member id")
    }

    fn roster_member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(id: u8) -> SecretKey {
        SecretKey::from_slice(&[id; 32]).expect("identity secret")
    }

    fn member_config(id: u16, identity_secret: &SecretKey) -> MemberConfigPublic {
        MemberConfigPublic {
            member_id: roster_member_id(id),
            public_mint_url: MintUrl::from_str(&format!("https://mint{}.example.com", id))
                .expect("public mint url"),
            federation_api_url: MintUrl::from_str(&format!("https://fed{}.example.com", id))
                .expect("federation api url"),
            identity_public_key: identity_secret.public_key(),
        }
    }

    fn private_member(id: u16, identity_secret: SecretKey) -> MemberConfigPrivate {
        MemberConfigPrivate {
            federation_id: None,
            member_id: roster_member_id(id),
            identity_secret_key: identity_secret,
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        }
    }

    fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
        let first_secret = identity_secret(1);
        let second_secret = identity_secret(2);
        let members = BTreeMap::from([
            (roster_member_id(1), member_config(1, &first_secret)),
            (roster_member_id(2), member_config(2, &second_secret)),
        ]);
        let threshold_params = ThresholdParams::new(2, 2, 2).expect("threshold params");
        let config = FederationConfig::new(threshold_params, members, Vec::new())
            .expect("federation config");
        let first_private = private_member(1, first_secret);
        let second_private = private_member(2, second_secret);
        (config, first_private, second_private)
    }

    fn transport_config_three() -> (FederationConfig, Vec<MemberConfigPrivate>) {
        let first_secret = identity_secret(1);
        let second_secret = identity_secret(2);
        let third_secret = identity_secret(3);
        let members = BTreeMap::from([
            (roster_member_id(1), member_config(1, &first_secret)),
            (roster_member_id(2), member_config(2, &second_secret)),
            (roster_member_id(3), member_config(3, &third_secret)),
        ]);
        let threshold_params = ThresholdParams::new(3, 2, 2).expect("threshold params");
        let config = FederationConfig::new(threshold_params, members, Vec::new())
            .expect("federation config");
        let private_members = vec![
            private_member(1, first_secret),
            private_member(2, second_secret),
            private_member(3, third_secret),
        ];
        (config, private_members)
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

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn dkg_reveal(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> FederationDkgParticipantCommitmentReveal {
        FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            ceremony_id: dkg_ceremony_id(),
            member_id,
            keyset_spec: FederationDkgKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                [Amount::from(1)].into_iter().collect(),
            )
            .expect("DKG keyset spec"),
            coefficient_commitments: BTreeMap::from([(
                Amount::from(1),
                vec![
                    bls_key(seed).public_key_g2(),
                    bls_key(seed + 10).public_key_g2(),
                ],
            )]),
        }
    }

    fn dkg_secret_share(receiver: MemberId, seed: u8) -> BlsSecretKeyShare {
        evaluate_secret_key_coefficients(
            receiver.to_bls_signer_id(),
            &[bls_key(seed), bls_key(seed + 10)],
        )
        .expect("dkg secret share")
    }

    fn mint_envelope(quote: &str, hex: &str) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::Mint(super::super::FederationMintOperation {
                request: super::super::FederationMintRequest::Single(MintRequest {
                    quote: quote.to_string(),
                    outputs: vec![output(hex)],
                    signature: None,
                }),
            }),
        )
    }

    fn mint_quote_envelope() -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MintQuote(
                super::super::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request_lookup_id: crate::payment::PaymentIdentifier::CustomId(
                        "quote-1".to_string(),
                    ),
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
                        method: crate::PaymentMethod::BOLT11,
                        amount_paid: Amount::ZERO,
                        amount_issued: Amount::ZERO,
                        updated_at: 0,
                        state: crate::MintQuoteState::Unpaid,
                        expiry: None,
                        pubkey: None,
                    }),
                },
            ),
        )
    }

    fn set_submit_response_status(
        response: &mut FederationSubmitOperationResponse,
        status: FederationOperationStatus,
    ) {
        match &mut response.outcome {
            FederationJournalAppendOutcome::Appended(entry)
            | FederationJournalAppendOutcome::Duplicate(entry) => {
                entry.application = FederationApplicationState::from_status(status);
            }
        }
    }

    #[test]
    fn peer_status_serializes_with_journal_digest() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope())
            .expect("append")
            .entry()
            .clone();
        let status = FederationPeerStatus::from_journal(federation_id(), member_id(), &journal);
        let json = serde_json::to_string(&status).expect("status json");
        let decoded: FederationPeerStatus = serde_json::from_str(&json).expect("status");

        assert_eq!(decoded.version, FEDERATION_REST_API_VERSION);
        assert_eq!(decoded.latest_index, Some(entry.index));
        assert_eq!(decoded.operation_count, 1);
        assert_eq!(decoded.latest_order_digest, entry.order_digest);
        assert_eq!(decoded.materialized_state_digest, None);
    }

    #[test]
    fn peer_status_round_trips_aleph_bft_cursor() {
        let status = FederationPeerStatus::new(
            federation_id(),
            member_id(),
            0,
            None,
            FederationJournalDigest::genesis(),
        )
        .with_aleph_bft_next_item_index(7);

        let json = serde_json::to_string(&status).expect("status json");
        let decoded: FederationPeerStatus = serde_json::from_str(&json).expect("status");

        assert_eq!(decoded.aleph_bft_next_item_index, Some(7));
    }

    #[test]
    fn peer_status_round_trips_aleph_bft_session() {
        let status = FederationPeerStatus::new(
            federation_id(),
            member_id(),
            0,
            None,
            FederationJournalDigest::genesis(),
        )
        .with_aleph_bft_session(3);

        let json = serde_json::to_string(&status).expect("status json");
        let decoded: FederationPeerStatus = serde_json::from_str(&json).expect("status");

        assert_eq!(decoded.aleph_bft_session, 3);
    }

    #[test]
    fn local_status_wire_fixture_round_trips_every_current_field() {
        let member = member_id();
        let status = FederationLocalStatusResponse {
            version: FEDERATION_REST_API_VERSION,
            peer_status: FederationPeerStatus::new(
                federation_id(),
                member,
                4,
                Some(3),
                FederationJournalDigest::genesis(),
            ),
            sync_state: "catching_up".to_string(),
            sync_ready: false,
            sync_message: "catching up from federation member 1".to_string(),
            sync_peer: Some(member),
            sync_local_operation_count: Some(2),
            sync_peer_operation_count: Some(4),
            sync_operation_count: Some(4),
            pending_mempool_items: 1,
            pending_response_waiters: 2,
            response_cache_items: 3,
            response_cache_lookups: 5,
            response_cache_hits: 4,
            response_cache_hit_rate_bps: 8_000,
            duplicate_submissions: 6,
            duplicate_pending_waiter_joins: 7,
            checkpoint: FederationCheckpointStatus::default(),
            payment_observation_reports: Vec::new(),
            aleph_bft_status: Some(FederationConsensusRuntimeStatus {
                federation_id: federation_id(),
                local_member: member,
                session: 1,
                next_unit_index: 2,
                next_unit_finalization_index: 3,
                next_item_index: 4,
                queued_item_count: 5,
                pending_finalization_item_count: 6,
                finalized_item_count: 7,
                accepted_operation_count: 4,
                latest_accepted_operation_index: Some(3),
                allowed_member_count: Some(3),
            }),
            aleph_bft_live_session_active: true,
            aleph_bft_network_inbound_active: true,
            materialized_state_rows: vec![FederationMaterializedStateRowFingerprint {
                table: "mint_quotes".to_string(),
                key_digest: FederationMaterializedStateDigest::genesis(),
                row_digest: FederationMaterializedStateDigest::genesis(),
            }],
            invalid_operation_alerts: vec![FederationInvalidOperationAlertSummary {
                source: "auth_token".to_string(),
                alerting_buckets: 1,
                max_count: 20,
                threshold: 20,
                window_secs: 60,
            }],
            onchain_wallets: vec![FederationOnchainWalletStatus {
                source: "bitcoin_rpc".to_string(),
                source_scope: "shared".to_string(),
                source_id: "shared-regtest-source".to_string(),
                correlated_source_warning: true,
                health: "healthy".to_string(),
                local_tip_height: 101,
                local_tip_hash: "00".repeat(32),
                last_success_unix: Some(1_700_000_000),
                consecutive_failures: 0,
            }],
            wallet_recovery: Some(
                FederationWalletRecoveryStatus::from_states(
                    &crate::federation::FederationAddressAllocationState::new(),
                    &crate::federation::FederationChainObservationState::new(),
                    &crate::federation::FederationTransactionState::new(),
                )
                .expect("wallet recovery fixture"),
            ),
            wallet_liveness: Some(crate::federation::FederationWalletLivenessStatus::default()),
            wallet_operator: None,
        };

        let value = serde_json::to_value(&status).expect("local status json");
        let fields = value
            .as_object()
            .expect("local status object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            fields,
            BTreeSet::from([
                "aleph_bft_live_session_active",
                "aleph_bft_network_inbound_active",
                "aleph_bft_status",
                "duplicate_pending_waiter_joins",
                "duplicate_submissions",
                "invalid_operation_alerts",
                "materialized_state_rows",
                "onchain_wallets",
                "payment_observation_reports",
                "peer_status",
                "pending_mempool_items",
                "pending_response_waiters",
                "response_cache_hit_rate_bps",
                "response_cache_hits",
                "response_cache_items",
                "response_cache_lookups",
                "sync_local_operation_count",
                "sync_message",
                "sync_operation_count",
                "sync_peer",
                "sync_peer_operation_count",
                "sync_ready",
                "sync_state",
                "version",
                "wallet_recovery",
                "wallet_liveness",
            ])
        );
        let wallet_status = value["onchain_wallets"][0]
            .as_object()
            .expect("on-chain wallet status");
        assert_eq!(wallet_status["source_scope"], "shared");
        assert_eq!(wallet_status["source_id"], "shared-regtest-source");
        assert_eq!(wallet_status["correlated_source_warning"], true);
        let decoded: FederationLocalStatusResponse =
            serde_json::from_value(value).expect("local status round trip");
        assert_eq!(decoded, status);
    }

    #[test]
    fn submit_response_uses_append_outcome_operation_id() {
        let mut journal = InMemoryFederationJournal::new();
        let outcome = journal.append(mint_quote_envelope()).expect("append");
        let response = FederationSubmitOperationResponse::from_outcome(outcome.clone());

        assert_eq!(response.operation_id, outcome.entry().operation_id);
        serde_json::to_string(&response).expect("submit response json");
    }

    #[test]
    fn submit_response_acceptance_allows_success_statuses() {
        let mut journal = InMemoryFederationJournal::new();
        let outcome = journal.append(mint_quote_envelope()).expect("append");

        for status in [
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Applying,
            FederationOperationStatus::Applied,
        ] {
            let mut response = FederationSubmitOperationResponse::from_outcome(outcome.clone());
            set_submit_response_status(&mut response, status);

            response.ensure_accepted().expect("status accepted");
        }
    }

    #[test]
    fn submit_response_acceptance_rejects_terminal_failure_statuses() {
        let mut journal = InMemoryFederationJournal::new();
        let outcome = journal.append(mint_quote_envelope()).expect("append");

        for status in [
            FederationOperationStatus::Rejected,
            FederationOperationStatus::Failed,
        ] {
            let mut response = FederationSubmitOperationResponse::from_outcome(outcome.clone());
            set_submit_response_status(&mut response, status);
            let operation_id = response.operation_id;

            assert!(matches!(
                response.ensure_accepted(),
                Err(FederationError::RejectedOperation {
                    operation_id: found_operation_id,
                    status: found_status,
                }) if found_operation_id == operation_id && found_status == status
            ));
        }
    }

    #[test]
    fn catch_up_response_applies_limits_and_reports_completeness() {
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        journal
            .append(mint_envelope(
                "quote-2",
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            ))
            .expect("append second mint");

        let request = FederationJournalCatchUpRequest::new(1)
            .with_limit(1)
            .with_known_order_digest(journal.entries()[0].order_digest);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &request,
            50,
            100,
        );

        assert_eq!(request.effective_limit(50, 100), 1);
        assert_eq!(response.from_index, 1);
        assert_eq!(response.entries.len(), 1);
        assert!(!response.complete);
        assert_eq!(response.latest_index, Some(2));
        assert_eq!(response.latest_order_digest, journal.latest_order_digest());
        assert_eq!(
            response.materialized_state_digest,
            FederationMaterializedStateDigest::genesis()
        );
        assert!(response.aleph_bft_finalized_items.is_empty());
        assert!(response.aleph_bft_finalized_items_complete);
    }

    #[test]
    fn catch_up_response_is_bounded_by_exact_checkpoint() {
        let (config, _, _) = transport_config();
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        let target_order_digest = journal.latest_order_digest();
        journal
            .append(mint_envelope(
                "quote-2",
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            ))
            .expect("append later mint");
        let boundary = FederationCheckpointBoundary {
            journal_next_index: 2,
            journal_order_digest: target_order_digest,
            finalized_next_item_index: 2,
            finalized_item_digest: FederationFinalizedItemDigest::genesis(),
            application_digest: FederationApplicationDigest::genesis(),
            aleph_bft_session: 0,
            next_unit_finalization_index: 2,
            next_unit_index_by_member: config
                .members
                .keys()
                .copied()
                .map(|member_id| (member_id, 1))
                .collect(),
        };
        let checkpoint = boundary
            .recovery_checkpoint(
                &config,
                None,
                FederationMaterializedStateDigest::from_bytes([42; 32]),
            )
            .expect("recovery checkpoint");
        let checkpoint_id = checkpoint.id().expect("checkpoint id");
        let request = FederationJournalCatchUpRequest::new(0)
            .with_target_checkpoint(checkpoint_id)
            .with_aleph_bft_next_item_index(2);
        let response = FederationJournalCatchUpResponse::from_journal(
            config.federation_id,
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &request,
            50,
            100,
        )
        .bound_to_checkpoint(&checkpoint)
        .expect("bounded checkpoint response");

        assert_eq!(response.target_checkpoint_id, Some(checkpoint_id));
        assert_eq!(response.entries.len(), 2);
        assert!(response.complete);
        assert_eq!(response.latest_index, Some(1));
        assert_eq!(response.latest_order_digest, target_order_digest);
        assert_eq!(
            response.materialized_state_digest,
            checkpoint.materialized_state_digest
        );
        response
            .validate_checkpoint_binding(&checkpoint)
            .expect("checkpoint binding validates");

        let partial_request = FederationJournalCatchUpRequest::new(0)
            .with_target_checkpoint(checkpoint_id)
            .with_aleph_bft_next_item_index(2)
            .with_limit(1);
        let partial = FederationJournalCatchUpResponse::from_journal(
            config.federation_id,
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &partial_request,
            50,
            100,
        )
        .bound_to_checkpoint(&checkpoint)
        .expect("partial checkpoint response");
        assert_eq!(partial.entries.len(), 1);
        assert!(!partial.complete);
        partial
            .validate_checkpoint_binding(&checkpoint)
            .expect("partial checkpoint binding validates");
    }

    #[test]
    fn catch_up_request_and_response_carry_aleph_bft_replay_fields() {
        let request = FederationJournalCatchUpRequest::new(3)
            .with_aleph_bft_next_item_index(7)
            .with_aleph_bft_backup(2, 11);
        assert_eq!(request.aleph_bft_next_item_index, 7);
        assert_eq!(
            request.aleph_bft_backup,
            Some(FederationAlephBftBackupRequest {
                session: 2,
                next_chunk: 11,
            })
        );
        let encoded_request = serde_json::to_value(&request).expect("request json");
        assert_eq!(encoded_request["aleph_bft_next_item_index"], 7);
        assert_eq!(encoded_request["aleph_bft_backup"]["session"], 2);
        assert_eq!(encoded_request["aleph_bft_backup"]["next_chunk"], 11);

        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_aleph_bft_finalized_items(Vec::new(), false)
        .with_aleph_bft_backup(FederationAlephBftBackupResponse {
            session: 2,
            from_chunk: 11,
            chunks: vec![b"backup".to_vec()],
            complete: true,
        });
        let encoded_response = serde_json::to_value(&response).expect("response json");
        let operation = encoded_response["entries"][0]
            .as_object()
            .expect("compact catch-up operation");
        assert!(operation.contains_key("operation_id"));
        assert!(operation.contains_key("application"));
        assert!(!operation.contains_key("status"));
        assert!(!operation.contains_key("rejection"));
        assert!(!operation.contains_key("envelope"));
        assert!(!operation.contains_key("conflict_keys"));
        assert!(!operation.contains_key("consensus_evidence"));
        let mut obsolete_response = encoded_response.clone();
        obsolete_response["entries"][0]["envelope"] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<FederationJournalCatchUpResponse>(obsolete_response).is_err()
        );
        let mut obsolete_response = encoded_response.clone();
        obsolete_response["session_outcome"] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<FederationJournalCatchUpResponse>(obsolete_response).is_err()
        );
        assert!(encoded_response["aleph_bft_finalized_items"]
            .as_array()
            .expect("finalized items array")
            .is_empty());
        assert_eq!(
            encoded_response["aleph_bft_finalized_items_complete"],
            false
        );
        assert_eq!(encoded_response["aleph_bft_backup"]["session"], 2);
        assert_eq!(encoded_response["aleph_bft_backup"]["from_chunk"], 11);
        assert_eq!(encoded_response["aleph_bft_backup"]["complete"], true);
    }

    #[test]
    fn catch_up_messages_require_replay_fields_and_order_status_omits_materialized_digest() {
        let mut request_value =
            serde_json::to_value(FederationJournalCatchUpRequest::new(3)).expect("request json");
        request_value
            .as_object_mut()
            .expect("request object")
            .remove("aleph_bft_next_item_index");
        assert!(serde_json::from_value::<FederationJournalCatchUpRequest>(request_value).is_err());

        let status_value = serde_json::to_value(FederationPeerStatus::new(
            federation_id(),
            member_id(),
            0,
            None,
            FederationJournalDigest::genesis(),
        ))
        .expect("status json");
        assert!(status_value.get("materialized_state_digest").is_none());
        assert_eq!(
            serde_json::from_value::<FederationPeerStatus>(status_value)
                .expect("order-only status")
                .materialized_state_digest,
            None
        );

        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        );
        let mut response_without_items =
            serde_json::to_value(&response).expect("response json without items");
        response_without_items
            .as_object_mut()
            .expect("response object")
            .remove("aleph_bft_finalized_items");
        assert!(
            serde_json::from_value::<FederationJournalCatchUpResponse>(response_without_items)
                .is_err()
        );

        let mut response_without_state_digest =
            serde_json::to_value(&response).expect("response json without state digest");
        response_without_state_digest
            .as_object_mut()
            .expect("response object")
            .remove("materialized_state_digest");
        assert!(serde_json::from_value::<FederationJournalCatchUpResponse>(
            response_without_state_digest
        )
        .is_err());

        let mut response_without_complete =
            serde_json::to_value(&response).expect("response json without completeness");
        response_without_complete
            .as_object_mut()
            .expect("response object")
            .remove("aleph_bft_finalized_items_complete");
        assert!(serde_json::from_value::<FederationJournalCatchUpResponse>(
            response_without_complete
        )
        .is_err());
    }

    #[test]
    fn catch_up_response_rejects_tampered_operation_commitment() {
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let request = FederationJournalCatchUpRequest::new(0);
        let mut response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &request,
            50,
            100,
        );
        response.entries[0].operation_id = FederationOperationId::from_bytes([42; 32]);

        assert!(matches!(
            response.validate_entries(),
            Err(FederationError::CatchUpDigestMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_verifies_sender_and_payload_hash() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationPeerStatusRequest::default();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            1,
            FederationRestMessageKind::Status,
            payload,
            &first_private,
        )
        .expect("signed request");

        request.verify(&config, receiver).expect("verified request");
        let json = serde_json::to_string(&request).expect("signed request json");
        let decoded: FederationSignedRequest<FederationPeerStatusRequest> =
            serde_json::from_str(&json).expect("decoded request");
        decoded.verify(&config, receiver).expect("decoded verifies");
    }

    #[test]
    fn checkpoint_transport_messages_are_bound_and_bounded() {
        let (config, first_private, second_private) = transport_config();
        let receiver = roster_member_id(2);
        let checkpoint_id = FederationCheckpointId::from_bytes([41; 32]);

        let summary_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            1,
            FederationRestMessageKind::CheckpointSummary,
            FederationCheckpointSummaryRequest::new(),
            &first_private,
        )
        .expect("signed checkpoint summary request");
        summary_request
            .verify(&config, receiver)
            .expect("checkpoint summary request verifies");
        summary_request
            .payload
            .validate()
            .expect("checkpoint summary payload validates");
        assert!(!summary_request.payload.request_recovery_checkpoint);
        let recovery_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            4,
            FederationRestMessageKind::CheckpointSummary,
            FederationCheckpointSummaryRequest::new().with_recovery_checkpoint(),
            &first_private,
        )
        .expect("signed recovery checkpoint request");
        recovery_request
            .verify(&config, receiver)
            .expect("recovery checkpoint request verifies");
        assert!(recovery_request.payload.request_recovery_checkpoint);
        let next_request =
            FederationCheckpointSummaryRequest::new().with_next_scheduled_quorum_after(7);
        assert_eq!(next_request.next_scheduled_quorum_after, Some(7));

        let fetch_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            2,
            FederationRestMessageKind::CheckpointFetch,
            FederationCheckpointFetchRequest::new(checkpoint_id),
            &first_private,
        )
        .expect("signed checkpoint fetch request");
        fetch_request
            .verify(&config, receiver)
            .expect("checkpoint fetch request verifies");

        let signature_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            3,
            FederationRestMessageKind::CheckpointSignature,
            FederationCheckpointSignatureRequest::new(checkpoint_id),
            &first_private,
        )
        .expect("signed checkpoint signature request");
        signature_request
            .verify(&config, receiver)
            .expect("checkpoint signature request verifies");

        let summary_response = FederationCheckpointSummaryResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: config.federation_id,
            member_id: receiver,
            stable_tip: None,
            latest_scheduled_candidate: None,
            latest_recovery_candidate: None,
            latest_scheduled_quorum: None,
            latest_recovery_quorum: None,
            next_scheduled_quorum: None,
        };
        summary_response
            .validate(&config, receiver)
            .expect("checkpoint summary response validates");
        let signed_summary_response = FederationSignedRequest::new_signed(
            config.federation_id,
            first_private.member_id,
            summary_request.sequence,
            FederationRestMessageKind::CheckpointSummary,
            summary_response.clone(),
            &second_private,
        )
        .expect("signed checkpoint summary response");
        signed_summary_response
            .verify(&config, first_private.member_id)
            .expect("checkpoint summary response signature verifies");
        let encoded = serde_json::to_value(&summary_response).expect("checkpoint summary json");
        assert!(encoded.get("latest_scheduled_candidate").is_none());
        assert!(encoded.get("latest_recovery_candidate").is_none());
        assert!(encoded.get("latest_scheduled_quorum").is_none());
        assert!(encoded.get("latest_recovery_quorum").is_none());

        FederationCheckpointFetchResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: config.federation_id,
            member_id: receiver,
            checkpoint: None,
            quorum: None,
        }
        .validate(&config, receiver, checkpoint_id)
        .expect("empty checkpoint fetch response validates");
    }

    #[test]
    fn signed_aleph_bft_activity_request_verifies_and_response_is_bound() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let operation_id = mint_quote_envelope()
            .operation_id()
            .expect("activity operation id");
        let payload = FederationAlephBftActivityRequest::new(3, operation_id);
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            6,
            FederationRestMessageKind::AlephBftActivity,
            payload,
            &first_private,
        )
        .expect("signed AlephBFT activity request");

        request.verify(&config, receiver).expect("verified request");
        let decoded: FederationSignedRequest<FederationAlephBftActivityRequest> =
            serde_json::from_str(
                &serde_json::to_string(&request).expect("AlephBFT activity request json"),
            )
            .expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::AlephBftActivity);
        decoded.verify(&config, receiver).expect("decoded verifies");

        let response = FederationAlephBftActivityResponse::new(
            config.federation_id,
            payload.session_id,
            operation_id,
        );
        response
            .validate(config.federation_id, payload.session_id, operation_id)
            .expect("response validates");
        assert!(matches!(
            response.validate(
                config.federation_id,
                payload.session_id,
                FederationOperationId::from_bytes([42; 32]),
            ),
            Err(FederationError::AlephBftActivityOperationMismatch { expected, found })
                if expected == FederationOperationId::from_bytes([42; 32])
                    && found == operation_id
        ));
        assert!(matches!(
            response.validate(config.federation_id, payload.session_id + 1, operation_id),
            Err(FederationError::AlephBftUnitSessionMismatch {
                expected: 4,
                found: 3
            })
        ));
    }

    #[test]
    fn signed_aleph_bft_network_message_request_verifies_and_response_validates() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationAlephBftNetworkMessageRequest::new(3, vec![1, 2, 3, 4]);
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            8,
            FederationRestMessageKind::AlephBftNetworkMessage,
            payload,
            &first_private,
        )
        .expect("signed AlephBFT network-message request");

        request.verify(&config, receiver).expect("verified request");
        assert_eq!(
            request.payload.message_bytes().expect("message bytes"),
            vec![1, 2, 3, 4]
        );
        request
            .payload
            .ensure_admissible_size()
            .expect("admissible network message");
        let decoded: FederationSignedRequest<FederationAlephBftNetworkMessageRequest> =
            serde_json::from_str(
                &serde_json::to_string(&request).expect("AlephBFT network-message json"),
            )
            .expect("decoded request");
        assert_eq!(
            decoded.kind,
            FederationRestMessageKind::AlephBftNetworkMessage
        );
        decoded.verify(&config, receiver).expect("decoded verifies");

        let response = FederationAlephBftNetworkMessageResponse::new(config.federation_id, 3);
        response
            .validate(config.federation_id, 3)
            .expect("response validates");
        assert!(matches!(
            response.validate(config.federation_id, 4),
            Err(FederationError::AlephBftUnitSessionMismatch {
                expected: 4,
                found: 3
            })
        ));
    }

    #[test]
    fn submit_operation_response_validation_rejects_mismatched_outcome_operation_id() {
        let expected = mint_quote_envelope();
        let expected_operation_id = expected.operation_id().expect("expected operation id");
        let other = mint_envelope(
            "quote-2",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut journal = InMemoryFederationJournal::new();
        let mut response = FederationSubmitOperationResponse::from_outcome(
            journal.append(other).expect("journal append"),
        );
        let found_operation_id = response.operation_id;
        assert_ne!(expected_operation_id, found_operation_id);

        response.operation_id = expected_operation_id;

        assert!(matches!(
            response.validate(expected_operation_id),
            Err(FederationError::SubmitOperationResponseMismatch { expected, found })
                if expected == expected_operation_id && found == found_operation_id
        ));
    }

    #[test]
    fn submit_operation_response_validation_rejects_tampered_embedded_envelope() {
        let expected = mint_quote_envelope();
        let expected_operation_id = expected.operation_id().expect("expected operation id");
        let tampered = mint_envelope(
            "quote-2",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let found_operation_id = tampered.operation_id().expect("tampered operation id");
        assert_ne!(expected_operation_id, found_operation_id);
        let mut journal = InMemoryFederationJournal::new();
        let mut response = FederationSubmitOperationResponse::from_outcome(
            journal.append(expected).expect("journal append"),
        );
        match &mut response.outcome {
            FederationJournalAppendOutcome::Appended(entry)
            | FederationJournalAppendOutcome::Duplicate(entry) => {
                entry.envelope = tampered;
            }
        }

        assert!(matches!(
            response.validate(expected_operation_id),
            Err(FederationError::SubmitOperationResponseMismatch { expected, found })
                if expected == expected_operation_id && found == found_operation_id
        ));
    }

    #[test]
    fn peer_status_request_materialized_rows_are_opt_in() {
        let default_json =
            serde_json::to_string(&FederationPeerStatusRequest::default()).expect("default json");
        assert_eq!(default_json, r#"{"include_materialized_rows":false}"#);

        let opt_in_json =
            serde_json::to_string(&FederationPeerStatusRequest::with_materialized_rows())
                .expect("opt-in json");
        assert_eq!(opt_in_json, r#"{"include_materialized_rows":true}"#);

        assert!(serde_json::from_str::<FederationPeerStatusRequest>("{}").is_err());
    }

    #[test]
    fn signed_request_rejects_tampered_payload() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationJournalCatchUpRequest::new(1);
        let mut request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            1,
            FederationRestMessageKind::CatchUp,
            payload,
            &first_private,
        )
        .expect("signed request");

        request.payload.next_index = 2;
        assert!(matches!(
            request.verify(&config, receiver),
            Err(FederationError::RestBodyHashMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_wrong_federation() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let request = FederationSignedRequest::new_signed(
            FederationId::from_bytes([99; 32]),
            receiver,
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, receiver),
            Err(FederationError::RestFederationMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_wrong_receiver() {
        let (config, first_private, _) = transport_config();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(2),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, roster_member_id(1)),
            Err(FederationError::RestReceiverMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_unknown_sender() {
        let (config, _, _) = transport_config();
        let unknown_private = private_member(3, identity_secret(3));
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(2),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &unknown_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, roster_member_id(2)),
            Err(FederationError::UnknownMember { member_id }) if member_id == roster_member_id(3)
        ));
    }

    #[test]
    fn signed_request_rejects_signature_for_different_sender() {
        let (config, _, second_private) = transport_config();
        let mut request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(1),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &second_private,
        )
        .expect("signed request");
        request.sender = roster_member_id(1);

        assert!(matches!(
            request.verify(&config, roster_member_id(1)),
            Err(FederationError::RestSignatureVerificationFailed { member_id })
                if member_id == roster_member_id(1)
        ));
    }

    #[test]
    fn signed_dkg_commitment_request_verifies_sender_and_payload_owner() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let reveal = dkg_reveal(&config, first_private.member_id, 11);
        let commitment =
            FederationDkgParticipantCommitment::from_reveal(&reveal).expect("commitment");
        let payload = FederationDkgCommitmentRequest::new(commitment);
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            7,
            FederationRestMessageKind::DkgCommitment,
            payload,
            &first_private,
        )
        .expect("signed dkg commitment");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .validate_sender_for_ceremony(&config, request.sender, dkg_ceremony_id())
            .expect("payload owner");
        assert!(request
            .payload
            .validate_sender_for_ceremony(
                &config,
                request.sender,
                FederationDkgCeremonyId::from_bytes([99; 32]),
            )
            .is_err());
        let json = serde_json::to_string(&request).expect("dkg commitment json");
        let decoded: FederationSignedRequest<FederationDkgCommitmentRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgCommitment);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender_for_ceremony(&config, decoded.sender, dkg_ceremony_id())
            .expect("decoded payload owner");
    }

    #[test]
    fn signed_dkg_readiness_request_binds_exact_ceremony_policy() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let keyset_spec = dkg_reveal(&config, first_private.member_id, 11).keyset_spec;
        let payload = FederationDkgReadinessRequest::new(
            config.federation_id,
            dkg_ceremony_id(),
            keyset_spec.clone(),
        );
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            7,
            FederationRestMessageKind::DkgReadiness,
            payload,
            &first_private,
        )
        .expect("signed DKG readiness request");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .validate(&config, dkg_ceremony_id(), &keyset_spec)
            .expect("matching readiness request");
        assert!(request
            .payload
            .validate(
                &config,
                FederationDkgCeremonyId::from_bytes([99; 32]),
                &keyset_spec,
            )
            .is_err());
        let mut changed_spec = keyset_spec.clone();
        changed_spec.input_fee_ppk = 1;
        assert!(request
            .payload
            .validate(&config, dkg_ceremony_id(), &changed_spec)
            .is_err());

        let json = serde_json::to_string(&request).expect("DKG readiness JSON");
        let decoded: FederationSignedRequest<FederationDkgReadinessRequest> =
            serde_json::from_str(&json).expect("decoded DKG readiness request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgReadiness);
        decoded.verify(&config, receiver).expect("decoded verifies");
    }

    #[test]
    fn signed_dkg_reveal_request_verifies_sender_and_payload_owner() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload =
            FederationDkgRevealRequest::new(dkg_reveal(&config, first_private.member_id, 11));
        let keyset_spec = payload.reveal.keyset_spec.clone();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            8,
            FederationRestMessageKind::DkgReveal,
            payload,
            &first_private,
        )
        .expect("signed dkg reveal");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .validate_sender_for_ceremony(&config, request.sender, dkg_ceremony_id(), &keyset_spec)
            .expect("payload owner");
        let mut changed_spec = keyset_spec.clone();
        changed_spec.input_fee_ppk = 1;
        assert!(
            request
                .payload
                .validate_sender_for_ceremony(
                    &config,
                    request.sender,
                    dkg_ceremony_id(),
                    &changed_spec,
                )
                .is_err()
        );
        let json = serde_json::to_string(&request).expect("dkg reveal json");
        let decoded: FederationSignedRequest<FederationDkgRevealRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgReveal);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender_for_ceremony(&config, decoded.sender, dkg_ceremony_id(), &keyset_spec)
            .expect("decoded payload owner");
    }

    #[test]
    fn signed_dkg_transcript_signature_request_verifies_sender_and_signature() {
        let (config, first_private, second_private) = transport_config();
        let receiver = roster_member_id(2);
        let transcript_hash = FederationDkgTranscriptHash::from_bytes([42; 32]);
        let signature = FederationDkgTranscriptSignature::sign(transcript_hash, &first_private)
            .expect("transcript signature");
        let payload = FederationDkgTranscriptSignatureRequest::new(
            config.federation_id,
            first_private.member_id,
            transcript_hash,
            signature,
        );
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            9,
            FederationRestMessageKind::DkgTranscriptSignature,
            payload,
            &first_private,
        )
        .expect("signed dkg transcript signature");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .validate_sender(&config, request.sender)
            .expect("payload owner");
        let json = serde_json::to_string(&request).expect("dkg transcript signature json");
        let decoded: FederationSignedRequest<FederationDkgTranscriptSignatureRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(
            decoded.kind,
            FederationRestMessageKind::DkgTranscriptSignature
        );
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender(&config, decoded.sender)
            .expect("decoded payload owner");

        let bad_signature =
            FederationDkgTranscriptSignature::sign(transcript_hash, &second_private)
                .expect("bad transcript signature");
        let bad_payload = FederationDkgTranscriptSignatureRequest::new(
            config.federation_id,
            first_private.member_id,
            transcript_hash,
            bad_signature,
        );
        assert!(matches!(
            bad_payload.validate_sender(&config, first_private.member_id),
            Err(FederationError::DkgTranscriptSignatureVerificationFailed { member_id })
                if member_id == first_private.member_id
        ));
    }

    #[test]
    fn signed_dkg_secret_share_request_verifies_sender_receiver_and_payload() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationDkgSecretShareRequest::new(
            config.federation_id,
            dkg_ceremony_id(),
            first_private.member_id,
            receiver,
            BTreeMap::from([(Amount::from(1), dkg_secret_share(receiver, 11))]),
        );
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            10,
            FederationRestMessageKind::DkgSecretShare,
            payload,
            &first_private,
        )
        .expect("signed dkg secret share");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .validate_sender_receiver_for_ceremony(
                &config,
                request.sender,
                receiver,
                dkg_ceremony_id(),
                &FederationDkgKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    [Amount::from(1)].into_iter().collect(),
                )
                .expect("keyset spec"),
            )
            .expect("payload owner and receiver");
        let json = serde_json::to_string(&request).expect("dkg secret share json");
        let decoded: FederationSignedRequest<FederationDkgSecretShareRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgSecretShare);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender_receiver_for_ceremony(
                &config,
                decoded.sender,
                receiver,
                dkg_ceremony_id(),
                &FederationDkgKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    [Amount::from(1)].into_iter().collect(),
                )
                .expect("keyset spec"),
            )
            .expect("decoded payload owner and receiver");
    }

    #[test]
    fn maximum_supported_dkg_reveal_fits_private_rest_body_limit() {
        let (config, first_private, _) = transport_config();
        let amounts = (1..=FEDERATION_MAX_DKG_KEYSET_AMOUNTS)
            .map(|amount| Amount::from(amount as u64))
            .collect::<std::collections::BTreeSet<_>>();
        let keyset_spec =
            FederationDkgKeysetSpec::new(CurrencyUnit::Sat, true, 0, None, amounts.clone())
                .expect("maximum keyset spec");
        let commitment = bls_key(11).public_key_g2();
        let coefficient_commitments = amounts
            .into_iter()
            .map(|amount| (amount, vec![commitment; FEDERATION_MAX_DKG_MEMBERS]))
            .collect();
        let payload = FederationDkgRevealRequest::new(FederationDkgParticipantCommitmentReveal {
            federation_id: config.federation_id,
            ceremony_id: dkg_ceremony_id(),
            member_id: first_private.member_id,
            keyset_spec,
            coefficient_commitments,
        });
        let request = FederationSignedRequest::new_signed_no_aux_rand(
            config.federation_id,
            roster_member_id(2),
            1,
            FederationRestMessageKind::DkgReveal,
            payload,
            &first_private,
        )
        .expect("maximum signed DKG reveal");
        let encoded = serde_json::to_vec(&request).expect("maximum DKG reveal JSON");

        assert!(
            encoded.len() <= FEDERATION_MAX_DKG_REQUEST_BODY_BYTES,
            "maximum supported DKG reveal is {} bytes, limit is {}",
            encoded.len(),
            FEDERATION_MAX_DKG_REQUEST_BODY_BYTES
        );
    }

    #[test]
    fn signed_dkg_secret_share_request_rejects_sender_receiver_or_signer_mismatch() {
        let (config, first_private, second_private) = transport_config();
        let receiver = roster_member_id(2);
        let sender_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
            dkg_ceremony_id(),
            second_private.member_id,
            receiver,
            BTreeMap::from([(Amount::from(1), dkg_secret_share(receiver, 11))]),
        );
        let receiver_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
            dkg_ceremony_id(),
            first_private.member_id,
            first_private.member_id,
            BTreeMap::from([(
                Amount::from(1),
                dkg_secret_share(first_private.member_id, 11),
            )]),
        );
        let signer_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
            dkg_ceremony_id(),
            first_private.member_id,
            receiver,
            BTreeMap::from([(
                Amount::from(1),
                dkg_secret_share(first_private.member_id, 11),
            )]),
        );

        assert!(matches!(
            sender_mismatch.validate_sender_receiver(&config, first_private.member_id, receiver),
            Err(FederationError::DkgCommitmentMismatch { member_id, reason })
                if member_id == second_private.member_id && reason == "request sender mismatch"
        ));
        assert!(matches!(
            receiver_mismatch.validate_sender_receiver(&config, first_private.member_id, receiver),
            Err(FederationError::RestReceiverMismatch { expected, found })
                if expected == receiver && found == first_private.member_id
        ));
        assert!(matches!(
            signer_mismatch.validate_sender_receiver(&config, first_private.member_id, receiver),
            Err(FederationError::DkgCommitmentMismatch { member_id, reason })
                if member_id == first_private.member_id
                    && reason.contains("secret contribution signer mismatch")
        ));
    }

    #[test]
    fn signed_dkg_setup_requests_reject_sender_body_mismatch() {
        let (config, first_private, second_private) = transport_config();
        let receiver = roster_member_id(2);
        let reveal = dkg_reveal(&config, second_private.member_id, 21);
        let commitment =
            FederationDkgParticipantCommitment::from_reveal(&reveal).expect("commitment");
        let commitment_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            9,
            FederationRestMessageKind::DkgCommitment,
            FederationDkgCommitmentRequest::new(commitment),
            &first_private,
        )
        .expect("signed mismatched commitment");
        let reveal_request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            10,
            FederationRestMessageKind::DkgReveal,
            FederationDkgRevealRequest::new(reveal),
            &first_private,
        )
        .expect("signed mismatched reveal");

        commitment_request
            .verify(&config, receiver)
            .expect("signature verifies");
        reveal_request
            .verify(&config, receiver)
            .expect("signature verifies");
        assert!(matches!(
            commitment_request
                .payload
                .validate_sender(&config, commitment_request.sender),
            Err(FederationError::DkgCommitmentMismatch { member_id, reason })
                if member_id == second_private.member_id && reason == "request sender mismatch"
        ));
        assert!(matches!(
            reveal_request
                .payload
                .validate_sender(&config, reveal_request.sender),
            Err(FederationError::DkgCommitmentMismatch { member_id, reason })
                if member_id == second_private.member_id && reason == "request sender mismatch"
        ));
    }

    #[test]
    fn signed_dkg_reveal_request_rejects_invalid_threshold_shape() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let mut reveal = dkg_reveal(&config, first_private.member_id, 11);
        reveal
            .coefficient_commitments
            .get_mut(&Amount::from(1))
            .expect("amount commitments")
            .pop();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            11,
            FederationRestMessageKind::DkgReveal,
            FederationDkgRevealRequest::new(reveal),
            &first_private,
        )
        .expect("signed invalid reveal");

        request
            .verify(&config, receiver)
            .expect("signature verifies");
        assert!(matches!(
            request.payload.validate_sender(&config, request.sender),
            Err(FederationError::DkgCommitmentMismatch { member_id, reason })
                if member_id == first_private.member_id
                    && reason.contains("coefficient commitment count mismatch")
        ));
    }

    #[test]
    fn sequence_tracker_rejects_replayed_or_stale_request() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let first = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            5,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("first request");
        let replay = first.clone();
        let second = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            6,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("second request");

        let mut tracker = FederationRestSequenceTracker::new();
        tracker
            .verify_and_record(&first, &config, receiver)
            .expect("first accepted");
        assert!(matches!(
            tracker.verify_and_record(&replay, &config, receiver),
            Err(FederationError::RestStaleSequence {
                member_id,
                last: 5,
                found: 5,
            }) if member_id == roster_member_id(1)
        ));
        tracker
            .verify_and_record(&second, &config, receiver)
            .expect("higher sequence accepted");
    }

    #[test]
    fn sequence_tracker_verifies_authenticated_sequence_without_rechecking_request() {
        let first_member = roster_member_id(1);
        let second_member = roster_member_id(2);
        let mut tracker = FederationRestSequenceTracker::new();

        tracker.record_sequence(FederationRestMessageKind::Status, first_member, 5);
        tracker
            .verify_next_sequence(FederationRestMessageKind::Status, first_member, 6)
            .expect("higher authenticated sequence accepted");
        assert!(matches!(
            tracker.verify_next_sequence(FederationRestMessageKind::Status, first_member, 5),
            Err(FederationError::RestStaleSequence {
                member_id,
                last: 5,
                found: 5,
            }) if member_id == first_member
        ));
        tracker
            .verify_next_sequence(FederationRestMessageKind::Status, second_member, 1)
            .expect("independent sender sequence accepted");
    }

    #[test]
    fn sequence_tracker_accepts_out_of_order_request_once() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let later_sequence = 1_000_000_000_100;
        let earlier_sequence = later_sequence - 50;
        let later = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            later_sequence,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("later request");
        let earlier = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            earlier_sequence,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("earlier request");

        let mut tracker = FederationRestSequenceTracker::new();
        tracker
            .verify_and_record(&later, &config, receiver)
            .expect("later request arrived first");
        tracker
            .verify_and_record(&earlier, &config, receiver)
            .expect("earlier in-flight request still accepted");
        assert!(matches!(
            tracker.verify_and_record(&earlier, &config, receiver),
            Err(FederationError::RestStaleSequence {
                member_id,
                last,
                found
            }) if member_id == roster_member_id(1)
                && last == later_sequence
                && found == earlier_sequence
        ));
    }

    #[test]
    fn sequence_tracker_restores_only_durable_dkg_replay_state() {
        let (_, first_private, _) = transport_config();
        let last_sequence = 1_000_000_000_100;
        let stale_sequence = last_sequence - 50;
        let mut persisted = BTreeMap::new();
        persisted.insert(first_private.member_id, last_sequence);
        let mut tracker = FederationRestSequenceTracker::from_dkg_sequences(persisted);

        assert!(matches!(
            tracker.verify_next_sequence(
                FederationRestMessageKind::DkgActivation,
                first_private.member_id,
                stale_sequence,
            ),
            Err(FederationError::RestStaleSequence {
                member_id,
                last,
                found
            }) if member_id == roster_member_id(1)
                && last == last_sequence
                && found == stale_sequence
        ));
        for kind in [
            FederationRestMessageKind::Status,
            FederationRestMessageKind::CheckpointSummary,
            FederationRestMessageKind::CheckpointFetch,
            FederationRestMessageKind::CheckpointSignature,
            FederationRestMessageKind::CheckpointQuorum,
            FederationRestMessageKind::CheckpointAudit,
            FederationRestMessageKind::CatchUp,
            FederationRestMessageKind::DkgReadiness,
            FederationRestMessageKind::SubmitOperation,
            FederationRestMessageKind::AlephBftActivity,
            FederationRestMessageKind::AlephBftNetworkMessage,
        ] {
            tracker
                .verify_next_sequence(kind, first_private.member_id, 1)
                .expect("volatile replay state starts empty after restart");
        }
        tracker.record_sequence(
            FederationRestMessageKind::Status,
            first_private.member_id,
            last_sequence + 1,
        );
        assert_eq!(
            tracker.dkg_sequences().get(&first_private.member_id),
            Some(&last_sequence),
            "volatile traffic must not alter the durable DKG watermark"
        );
    }

    #[test]
    fn sequence_tracker_isolates_volatile_kinds_and_shares_dkg_state() {
        let member = roster_member_id(1);
        let mut tracker = FederationRestSequenceTracker::new();

        tracker.record_sequence(FederationRestMessageKind::Status, member, 10_000);
        tracker
            .verify_next_sequence(FederationRestMessageKind::CatchUp, member, 1)
            .expect("status traffic must not advance catch-up replay state");
        tracker
            .verify_next_sequence(FederationRestMessageKind::SubmitOperation, member, 1)
            .expect("status traffic must not advance operation replay state");
        tracker
            .verify_next_sequence(FederationRestMessageKind::DkgCommitment, member, 1)
            .expect("status traffic must not advance durable DKG replay state");

        tracker.record_sequence(FederationRestMessageKind::DkgCommitment, member, 1);
        assert!(matches!(
            tracker.verify_next_sequence(FederationRestMessageKind::DkgReveal, member, 1),
            Err(FederationError::RestStaleSequence {
                member_id,
                last: 1,
                found: 1,
            }) if member_id == member
        ));
        tracker
            .verify_next_sequence(FederationRestMessageKind::DkgSecretShare, member, 2)
            .expect("effectful DKG kinds share one advancing durable stream");
        assert_eq!(tracker.dkg_sequences().get(&member), Some(&1));
    }

    #[test]
    fn sequence_tracker_keeps_submit_operation_sequence_informational() {
        let member = roster_member_id(1);
        let mut tracker = FederationRestSequenceTracker::new();

        tracker.record_sequence(FederationRestMessageKind::SubmitOperation, member, 5);
        tracker
            .verify_next_sequence(FederationRestMessageKind::SubmitOperation, member, 5)
            .expect("operation replay is governed by its canonical operation ID");
        assert!(
            tracker
                .window(FederationRestMessageKind::SubmitOperation)
                .is_none(),
            "operation submissions must never allocate transport replay state"
        );
    }

    #[test]
    fn sequence_tracker_bounds_high_rate_volatile_history() {
        let member = roster_member_id(1);
        let first = 1_000_000_000_000;
        let count = FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES + 100;
        let mut tracker = FederationRestSequenceTracker::new();

        for offset in 0..count {
            tracker.record_sequence(
                FederationRestMessageKind::AlephBftNetworkMessage,
                member,
                first + u64::try_from(offset).expect("test offset fits u64"),
            );
        }

        let window = tracker
            .window(FederationRestMessageKind::AlephBftNetworkMessage)
            .expect("network-message replay window");
        assert_eq!(
            window
                .recent_by_sender
                .get(&member)
                .expect("member replay history")
                .len(),
            FEDERATION_REST_SEQUENCE_REORDER_MAX_ENTRIES
        );
        assert!(matches!(
            tracker.verify_next_sequence(
                FederationRestMessageKind::AlephBftNetworkMessage,
                member,
                first,
            ),
            Err(FederationError::RestStaleSequence { member_id, .. }) if member_id == member
        ));
        tracker
            .verify_next_sequence(FederationRestMessageKind::AlephBftActivity, member, first)
            .expect("high-rate network gossip must not advance activity replay state");
    }

    #[test]
    fn only_effectful_dkg_messages_have_durable_replay_state() {
        for kind in [
            FederationRestMessageKind::Status,
            FederationRestMessageKind::CheckpointSummary,
            FederationRestMessageKind::CheckpointFetch,
            FederationRestMessageKind::CheckpointSignature,
            FederationRestMessageKind::CheckpointQuorum,
            FederationRestMessageKind::CheckpointAudit,
            FederationRestMessageKind::CatchUp,
            FederationRestMessageKind::DkgReadiness,
            FederationRestMessageKind::SubmitOperation,
            FederationRestMessageKind::AlephBftActivity,
            FederationRestMessageKind::AlephBftNetworkMessage,
            FederationRestMessageKind::FrostSigningCommitment,
            FederationRestMessageKind::FrostSigningPackage,
            FederationRestMessageKind::FrostSigningAbort,
            FederationRestMessageKind::FrostAggregateResult,
        ] {
            assert!(!kind.has_durable_replay_state(), "unexpected durable kind");
        }
        for kind in [
            FederationRestMessageKind::DkgCommitment,
            FederationRestMessageKind::DkgReveal,
            FederationRestMessageKind::DkgSecretShare,
            FederationRestMessageKind::DkgTranscriptSignature,
            FederationRestMessageKind::DkgActivation,
        ] {
            assert!(
                kind.has_durable_replay_state(),
                "DKG mutation must be durable"
            );
        }
    }

    #[test]
    fn frost_commitment_transport_binds_federation_signer_application_attempt_and_derivation() {
        let registration =
            cdk_frost_bip32::application_registration().expect("application registration");
        let derivation = cdk_frost::SigningDerivation::application_key();
        let scope = cdk_frost::SigningAuthorizationScope::new(
            federation_id().to_bytes(),
            cdk_frost::RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            derivation.binding(),
            [31; 32],
            [32; 32],
        )
        .expect("authorization scope");
        let context = cdk_frost::SigningSessionContext::new(
            scope,
            7,
            [
                cdk_frost::MemberId::new(1).expect("first signer"),
                cdk_frost::MemberId::new(2).expect("second signer"),
            ],
        )
        .expect("signing context");
        let authorization = cdk_frost::AuthorizationRequest::new(
            registration.id(),
            registration.key_epoch(),
            registration.derivation_version(),
            context.session_id(),
            b"typed authorization".to_vec(),
        )
        .expect("authorization request");
        let request = FederationFrostCommitmentRequest::new(&context, &derivation, &authorization);
        request
            .decode(federation_id(), roster_member_id(1))
            .expect("valid request");
        assert!(request
            .decode(FederationId::from_bytes([99; 32]), roster_member_id(1))
            .is_err());
        assert!(request
            .decode(federation_id(), roster_member_id(3))
            .is_err());

        let wrong_context = cdk_frost::SigningSessionContext::new(
            cdk_frost::SigningAuthorizationScope::new(
                federation_id().to_bytes(),
                cdk_frost::RootKeyEpoch::new(1).expect("root epoch"),
                registration,
                derivation.binding(),
                [31; 32],
                [32; 32],
            )
            .expect("authorization scope"),
            8,
            [
                cdk_frost::MemberId::new(1).expect("first signer"),
                cdk_frost::MemberId::new(2).expect("second signer"),
            ],
        )
        .expect("wrong attempt context");
        let mut cross_attempt = request;
        cross_attempt.context = wrong_context.encode();
        assert!(cross_attempt
            .decode(federation_id(), roster_member_id(1))
            .is_err());
    }
}
