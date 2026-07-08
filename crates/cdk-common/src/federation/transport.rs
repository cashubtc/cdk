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
    FederationAlephBftFinalizedItem, FederationAlephBftUnitBodyHash, FederationAlephBftUnitPayload,
    FederationConfig, FederationConsensusBackendKind, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignature, FederationError, FederationId,
    FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationStateDigest, InMemoryFederationJournal, MemberConfigPrivate, MemberConfigPublic,
    MemberId, Result, FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
};
use crate::nuts::nut01::BlsSecretKeyShare;
use crate::Amount;

/// Version of the private federation REST payload format.
pub const FEDERATION_REST_API_VERSION: u16 = 1;
/// Version of signed federation session outcome payloads.
pub const FEDERATION_SESSION_OUTCOME_VERSION: u16 = 1;

const FEDERATION_REST_BODY_HASH_VERSION: &[u8] = b"cdk-federation-rest-body-v1";
const FEDERATION_REST_SIGNATURE_VERSION: &[u8] = b"cdk-federation-rest-signature-v1";
const FEDERATION_SESSION_OUTCOME_SIGNATURE_VERSION: &[u8] =
    b"cdk-federation-session-outcome-signature-v1";

/// Private federation REST message kind covered by member signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRestMessageKind {
    /// Peer status request.
    Status,
    /// Publish a pre-reveal DKG commitment hash to a peer.
    DkgCommitment,
    /// Reveal DKG public polynomial commitments to a peer.
    DkgReveal,
    /// Send private DKG secret-share contributions to the intended receiver.
    DkgSecretShare,
    /// Share a member signature over a DKG/setup transcript hash.
    DkgTranscriptSignature,
    /// Submit one operation envelope to a peer.
    SubmitOperation,
    /// Deliver one opaque AlephBFT network message to a peer.
    AlephBftNetworkMessage,
    /// Deliver one AlephBFT unit that has already been finalized.
    AlephBftFinalizedUnit,
    /// Request missing journal entries from a peer.
    CatchUp,
}

impl FederationRestMessageKind {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Status => b"status",
            Self::DkgCommitment => b"dkg_commitment",
            Self::DkgReveal => b"dkg_reveal",
            Self::DkgSecretShare => b"dkg_secret_share",
            Self::DkgTranscriptSignature => b"dkg_transcript_signature",
            Self::SubmitOperation => b"submit_operation",
            Self::AlephBftNetworkMessage => b"aleph_bft_network_message",
            Self::AlephBftFinalizedUnit => b"aleph_bft_finalized_unit",
            Self::CatchUp => b"catch_up",
        }
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
pub struct FederationSignedRequest<T> {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this request.
    pub federation_id: FederationId,
    /// Member that signed the request.
    pub sender: MemberId,
    /// Member expected to receive the request.
    pub receiver: MemberId,
    /// Monotonic sender-local sequence used to reject simple replays.
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

/// In-memory replay tracker for authenticated federation REST requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FederationRestSequenceTracker {
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
    pub fn from_sequences(highest_by_sender: BTreeMap<MemberId, u64>) -> Self {
        Self {
            highest_by_sender,
            recent_by_sender: BTreeMap::new(),
        }
    }

    /// Return persisted sender watermarks known to this tracker.
    #[must_use]
    pub fn sequences(&self) -> &BTreeMap<MemberId, u64> {
        &self.highest_by_sender
    }

    /// Verify a request and record its sequence if it advances the sender stream.
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
        self.record_sequence(request.sender, request.sequence);

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

        self.ensure_sequence_advances(request.sender, request.sequence)
    }

    /// Record an already-verified sender sequence.
    pub fn record_sequence(&mut self, sender: MemberId, sequence: u64) {
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

    /// Verify an already-authenticated sender sequence without rechecking the request signature.
    pub fn verify_next_sequence(&self, sender: MemberId, sequence: u64) -> Result<()> {
        self.ensure_sequence_advances(sender, sequence)
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
        if recent.is_empty() {
            self.recent_by_sender.remove(&sender);
        }
    }
}

/// Signed payload used to request a peer status snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPeerStatusRequest {
    /// Include per-row materialized state fingerprints for operator diffing.
    #[serde(default, skip_serializing_if = "is_false")]
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

fn is_false(value: &bool) -> bool {
    !*value
}

/// Signed request payload carrying one member's pre-reveal DKG commitment hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Signed request payload revealing one member's DKG public commitments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Signed request payload carrying one participant's private DKG contributions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationDkgSecretShareRequest {
    /// Federation whose DKG round this contribution belongs to.
    pub federation_id: FederationId,
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
        sender: MemberId,
        receiver: MemberId,
        secret_shares: BTreeMap<Amount, BlsSecretKeyShare>,
    ) -> Self {
        Self {
            federation_id,
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
}

/// Signed request payload carrying one member's DKG/setup transcript signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Response acknowledging a validated DKG setup payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Peer status response for federation health and catch-up checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPeerStatus {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member ID of this peer.
    pub member_id: MemberId,
    /// Consensus backend producing this peer's accepted operation order.
    pub consensus_backend: FederationConsensusBackendKind,
    /// Next AlephBFT finalized-item index required by this peer, when using AlephBFT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aleph_bft_next_item_index: Option<u64>,
    /// Number of accepted operations in the local journal.
    pub operation_count: u64,
    /// Highest accepted operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
    /// Digest of consensus-visible state.
    #[serde(default = "FederationStateDigest::genesis")]
    pub state_digest: FederationStateDigest,
}

impl FederationPeerStatus {
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
            consensus_backend: FederationConsensusBackendKind::InMemory,
            aleph_bft_next_item_index: None,
            operation_count,
            latest_index,
            latest_order_digest,
            state_digest: FederationStateDigest::from_order_digest(latest_order_digest),
        }
    }

    /// Create a peer status snapshot with an explicit state digest.
    #[must_use]
    pub fn new_with_state_digest(
        federation_id: FederationId,
        member_id: MemberId,
        operation_count: u64,
        latest_index: Option<u64>,
        latest_order_digest: FederationJournalDigest,
        state_digest: FederationStateDigest,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            member_id,
            consensus_backend: FederationConsensusBackendKind::InMemory,
            aleph_bft_next_item_index: None,
            operation_count,
            latest_index,
            latest_order_digest,
            state_digest,
        }
    }

    /// Return this status with an explicit consensus backend.
    #[must_use]
    pub fn with_consensus_backend(
        mut self,
        consensus_backend: FederationConsensusBackendKind,
    ) -> Self {
        self.consensus_backend = consensus_backend;
        self
    }

    /// Return this status with an explicit AlephBFT finalized-item cursor.
    #[must_use]
    pub fn with_aleph_bft_next_item_index(mut self, next_item_index: u64) -> Self {
        self.aleph_bft_next_item_index = Some(next_item_index);
        self
    }

    /// Create a peer status snapshot from the in-memory journal.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        member_id: MemberId,
        journal: &InMemoryFederationJournal,
    ) -> Self {
        Self::from_journal_with_backend(
            federation_id,
            member_id,
            journal,
            FederationConsensusBackendKind::InMemory,
        )
    }

    /// Create a peer status snapshot from a journal and explicit backend.
    #[must_use]
    pub fn from_journal_with_backend(
        federation_id: FederationId,
        member_id: MemberId,
        journal: &InMemoryFederationJournal,
        consensus_backend: FederationConsensusBackendKind,
    ) -> Self {
        let latest_index = journal.entries().last().map(|entry| entry.index);
        Self::new_with_state_digest(
            federation_id,
            member_id,
            journal.len() as u64,
            latest_index,
            journal.latest_order_digest(),
            journal.state_digest(),
        )
        .with_consensus_backend(consensus_backend)
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
        match self.outcome.entry().status {
            FederationOperationStatus::Accepted
            | FederationOperationStatus::Applying
            | FederationOperationStatus::Applied => Ok(()),
            FederationOperationStatus::Rejected | FederationOperationStatus::Failed => {
                Err(FederationError::RejectedOperation {
                    operation_id: self.operation_id,
                    status: self.outcome.entry().status,
                })
            }
        }
    }
}

/// Request to deliver one already-finalized AlephBFT unit to a federation peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAlephBftFinalizedUnitRequest {
    /// Unit payload produced by the AlephBFT ordering layer.
    pub unit: FederationAlephBftUnitPayload,
}

impl FederationAlephBftFinalizedUnitRequest {
    /// Create a finalized-unit delivery request.
    #[must_use]
    pub fn new(unit: FederationAlephBftUnitPayload) -> Self {
        Self { unit }
    }

    /// Reject this request if the unit payload is malformed or too large.
    pub fn ensure_admissible_size(&self) -> super::Result<usize> {
        self.unit
            .ensure_within_size_limit(FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES)
    }
}

/// Request to deliver one opaque AlephBFT network message to a federation peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Persisted outbound AlephBFT network message retry material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationAlephBftNetworkRetryRecord {
    /// Deterministic retry identifier derived from the request and target peers.
    pub retry_id: String,
    /// AlephBFT session this retry belongs to.
    pub session_id: u64,
    /// Opaque AlephBFT network-message request to redeliver.
    pub request: FederationAlephBftNetworkMessageRequest,
    /// Federation peers that still need this message.
    pub peer_member_ids: Vec<MemberId>,
}

/// Response to an opaque AlephBFT network-message delivery request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Response to a finalized AlephBFT unit delivery request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAlephBftFinalizedUnitResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this response.
    pub federation_id: FederationId,
    /// Body hash of the finalized unit accepted by the peer.
    pub unit_body_hash: FederationAlephBftUnitBodyHash,
    /// Operation IDs derived from the accepted unit body.
    pub operation_ids: Vec<FederationOperationId>,
    /// Submit responses produced for operation items in the accepted unit.
    pub responses: Vec<FederationSubmitOperationResponse>,
}

impl FederationAlephBftFinalizedUnitResponse {
    /// Create a response from an accepted finalized unit and its operation responses.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        unit: &FederationAlephBftUnitPayload,
        responses: Vec<FederationSubmitOperationResponse>,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            unit_body_hash: unit.body_hash,
            operation_ids: unit.operation_ids.clone(),
            responses,
        }
    }

    /// Verify this response belongs to the expected federation and unit body.
    pub fn validate(
        &self,
        expected_federation_id: FederationId,
        unit: &FederationAlephBftUnitPayload,
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
        if self.unit_body_hash != unit.body_hash {
            return Err(FederationError::AlephBftUnitBodyHashMismatch {
                expected: unit.body_hash.to_hex(),
                found: self.unit_body_hash.to_hex(),
            });
        }
        if self.operation_ids != unit.operation_ids {
            return Err(FederationError::AlephBftUnitOperationIdsMismatch);
        }
        let response_operation_ids = self
            .responses
            .iter()
            .map(|response| response.operation_id)
            .collect::<Vec<_>>();
        if response_operation_ids != self.operation_ids {
            return Err(FederationError::AlephBftUnitOperationIdsMismatch);
        }
        for (expected_operation_id, response) in self.operation_ids.iter().zip(&self.responses) {
            response.validate(*expected_operation_id)?;
        }

        Ok(())
    }
}

/// Request journal entries starting at the next missing index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationJournalCatchUpRequest {
    /// First missing zero-based journal index.
    pub next_index: u64,
    /// Optional maximum entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    /// Optional digest the requester has after `next_index - 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_order_digest: Option<FederationJournalDigest>,
    /// Next missing AlephBFT finalized-item index for AlephBFT catch-up.
    pub aleph_bft_next_item_index: u64,
}

impl FederationJournalCatchUpRequest {
    /// Create a catch-up request from a next missing index.
    #[must_use]
    pub fn new(next_index: u64) -> Self {
        Self {
            next_index,
            limit: None,
            known_order_digest: None,
            aleph_bft_next_item_index: 0,
        }
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

    /// Return the bounded limit to apply to a response.
    #[must_use]
    pub fn effective_limit(&self, default_limit: u16, max_limit: u16) -> usize {
        let requested = self.limit.unwrap_or(default_limit).min(max_limit);
        usize::from(requested)
    }
}

/// Signed summary of a contiguous accepted-operation range served during catch-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSessionOutcome {
    /// Session outcome payload version.
    pub version: u16,
    /// Federation whose accepted range is summarized.
    pub federation_id: FederationId,
    /// Member that signed this outcome.
    pub signer: MemberId,
    /// First returned journal index.
    pub from_index: u64,
    /// Number of returned entries.
    pub operation_count: u64,
    /// Last returned journal index.
    pub latest_index: u64,
    /// Digest before the first returned entry.
    pub previous_order_digest: FederationJournalDigest,
    /// Digest after the returned range.
    pub range_order_digest: FederationJournalDigest,
    /// Latest accepted-order digest known to the signer.
    pub latest_order_digest: FederationJournalDigest,
    /// Latest consensus-visible state digest known to the signer.
    pub state_digest: FederationStateDigest,
    /// Operation IDs covered by this range, in order.
    pub operation_ids: Vec<FederationOperationId>,
    /// Member identity signature over the session outcome transcript.
    pub signature: FederationRestSignature,
}

/// Verified quorum of signed session outcomes over one catch-up range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationSessionOutcomeQuorum {
    /// Distinct valid member signatures required by federation consensus params.
    pub required: u16,
    /// Distinct federation members whose signatures verified for the range.
    pub signers: BTreeSet<MemberId>,
    /// First journal index covered by the quorum.
    pub from_index: u64,
    /// Last journal index covered by the quorum.
    pub latest_index: u64,
    /// Number of operations covered by the quorum.
    pub operation_count: u64,
    /// Order digest after the covered range.
    pub range_order_digest: FederationJournalDigest,
}

impl FederationSessionOutcome {
    /// Sign a contiguous range of accepted journal entries.
    pub fn new_signed(
        federation_id: FederationId,
        entries: &[FederationJournalEntry],
        latest_order_digest: FederationJournalDigest,
        state_digest: FederationStateDigest,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        let first = entries
            .first()
            .ok_or_else(|| session_outcome_mismatch("cannot sign an empty entry range"))?;
        let last = entries
            .last()
            .ok_or_else(|| session_outcome_mismatch("cannot sign an empty entry range"))?;
        let operation_ids = entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        let transcript = session_outcome_transcript(
            federation_id,
            private_config.member_id,
            first.index,
            entries.len() as u64,
            last.index,
            first.previous_order_digest,
            last.order_digest,
            latest_order_digest,
            state_digest,
            &operation_ids,
        );
        let signature = FederationRestSignature::sign(&transcript, private_config)?;

        Ok(Self {
            version: FEDERATION_SESSION_OUTCOME_VERSION,
            federation_id,
            signer: private_config.member_id,
            from_index: first.index,
            operation_count: entries.len() as u64,
            latest_index: last.index,
            previous_order_digest: first.previous_order_digest,
            range_order_digest: last.order_digest,
            latest_order_digest,
            state_digest,
            operation_ids,
            signature,
        })
    }

    /// Verify this outcome against returned catch-up entries and a roster member.
    pub fn verify(
        &self,
        member: &MemberConfigPublic,
        federation_id: FederationId,
        from_index: u64,
        entries: &[FederationJournalEntry],
        latest_order_digest: FederationJournalDigest,
        state_digest: FederationStateDigest,
    ) -> Result<()> {
        if self.version != FEDERATION_SESSION_OUTCOME_VERSION {
            return Err(FederationError::SessionOutcomeVersionMismatch {
                expected: FEDERATION_SESSION_OUTCOME_VERSION,
                found: self.version,
            });
        }

        if self.federation_id != federation_id {
            return Err(session_outcome_mismatch("federation id mismatch"));
        }

        if self.signer != member.member_id {
            return Err(session_outcome_mismatch("signer member mismatch"));
        }

        let first = entries
            .first()
            .ok_or_else(|| session_outcome_mismatch("missing entries for signed outcome"))?;
        let last = entries
            .last()
            .ok_or_else(|| session_outcome_mismatch("missing entries for signed outcome"))?;
        let operation_ids = entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();

        if self.from_index != from_index || first.index != from_index {
            return Err(session_outcome_mismatch("range start mismatch"));
        }

        if self.operation_count != entries.len() as u64
            || self.latest_index != last.index
            || self.previous_order_digest != first.previous_order_digest
            || self.range_order_digest != last.order_digest
            || self.latest_order_digest != latest_order_digest
            || self.state_digest != state_digest
            || self.operation_ids != operation_ids
        {
            return Err(session_outcome_mismatch("range content mismatch"));
        }

        let transcript = session_outcome_transcript(
            self.federation_id,
            self.signer,
            self.from_index,
            self.operation_count,
            self.latest_index,
            self.previous_order_digest,
            self.range_order_digest,
            self.latest_order_digest,
            self.state_digest,
            &self.operation_ids,
        );
        let signature = self.signature.as_schnorr()?;
        member
            .identity_public_key
            .verify(&transcript, &signature)
            .map_err(
                |_| FederationError::SessionOutcomeSignatureVerificationFailed {
                    member_id: self.signer,
                },
            )?;

        Ok(())
    }
}

/// Catch-up response containing accepted journal entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationJournalCatchUpResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// First requested index.
    pub from_index: u64,
    /// Journal entries returned in ascending order.
    pub entries: Vec<FederationJournalEntry>,
    /// Highest local operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest local accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
    /// Latest local consensus-visible state digest.
    #[serde(default = "FederationStateDigest::genesis")]
    pub state_digest: FederationStateDigest,
    /// Signed summary of the returned accepted-operation range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_outcome: Option<FederationSessionOutcome>,
    /// AlephBFT finalized items returned for exact adapter replay.
    pub aleph_bft_finalized_items: Vec<FederationAlephBftFinalizedItem>,
    /// Whether all requested AlephBFT finalized items were returned.
    pub aleph_bft_finalized_items_complete: bool,
    /// Whether the peer returned all entries currently available from `from_index`.
    pub complete: bool,
}

impl FederationJournalCatchUpResponse {
    /// Create a catch-up response from an in-memory journal.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        journal: &InMemoryFederationJournal,
        request: &FederationJournalCatchUpRequest,
        default_limit: u16,
        max_limit: u16,
    ) -> Self {
        let mut entries = journal.entries_from(request.next_index);
        let limit = request.effective_limit(default_limit, max_limit);
        let complete = entries.len() <= limit;
        entries.truncate(limit);
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            from_index: request.next_index,
            entries,
            latest_index: journal.entries().last().map(|entry| entry.index),
            latest_order_digest: journal.latest_order_digest(),
            state_digest: journal.state_digest(),
            session_outcome: None,
            aleph_bft_finalized_items: Vec::new(),
            aleph_bft_finalized_items_complete: true,
            complete,
        }
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

    /// Attach a member-signed session outcome for the returned entry range.
    pub fn with_signed_session_outcome(
        mut self,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        if !self.entries.is_empty() {
            self.session_outcome = Some(FederationSessionOutcome::new_signed(
                self.federation_id,
                &self.entries,
                self.latest_order_digest,
                self.state_digest,
                private_config,
            )?);
        }

        Ok(self)
    }

    /// Verify the signed session outcome for this response.
    pub fn verify_session_outcome(&self, member: &MemberConfigPublic) -> Result<()> {
        match (self.entries.is_empty(), &self.session_outcome) {
            (true, None) => Ok(()),
            (true, Some(_)) => Err(session_outcome_mismatch(
                "empty catch-up response carried an outcome",
            )),
            (false, None) => Err(session_outcome_mismatch(
                "non-empty catch-up response missing outcome",
            )),
            (false, Some(outcome)) => outcome.verify(
                member,
                self.federation_id,
                self.from_index,
                &self.entries,
                self.latest_order_digest,
                self.state_digest,
            ),
        }
    }

    /// Verify distinct member signatures over this exact catch-up range.
    pub fn verify_session_outcome_quorum(
        &self,
        config: &FederationConfig,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<FederationSessionOutcomeQuorum> {
        let required = config.threshold_params.consensus_threshold;
        if self.entries.is_empty() {
            return Ok(FederationSessionOutcomeQuorum {
                required,
                signers: BTreeSet::new(),
                from_index: self.from_index,
                latest_index: self.from_index.saturating_sub(1),
                operation_count: 0,
                range_order_digest: self.latest_order_digest,
            });
        }

        let mut signers = BTreeSet::new();
        for outcome in self
            .session_outcome
            .iter()
            .chain(additional_outcomes.iter())
        {
            let member = config
                .member(outcome.signer)
                .ok_or(FederationError::UnknownMember {
                    member_id: outcome.signer,
                })?;
            outcome.verify(
                member,
                self.federation_id,
                self.from_index,
                &self.entries,
                self.latest_order_digest,
                self.state_digest,
            )?;
            signers.insert(outcome.signer);
        }

        if signers.len() < usize::from(required) {
            return Err(FederationError::SessionOutcomeQuorumNotReached {
                required,
                found: signers.len() as u16,
            });
        }

        let latest_entry = self
            .entries
            .last()
            .ok_or_else(|| session_outcome_mismatch("missing entries for quorum"))?;
        Ok(FederationSessionOutcomeQuorum {
            required,
            signers,
            from_index: self.from_index,
            latest_index: latest_entry.index,
            operation_count: self.entries.len() as u64,
            range_order_digest: latest_entry.order_digest,
        })
    }
}

fn session_outcome_transcript(
    federation_id: FederationId,
    signer: MemberId,
    from_index: u64,
    operation_count: u64,
    latest_index: u64,
    previous_order_digest: FederationJournalDigest,
    range_order_digest: FederationJournalDigest,
    latest_order_digest: FederationJournalDigest,
    state_digest: FederationStateDigest,
    operation_ids: &[FederationOperationId],
) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_hash_bytes(
        &mut transcript,
        FEDERATION_SESSION_OUTCOME_SIGNATURE_VERSION,
    );
    transcript.extend_from_slice(&FEDERATION_SESSION_OUTCOME_VERSION.to_be_bytes());
    append_hash_bytes(&mut transcript, &federation_id.to_bytes());
    transcript.extend_from_slice(&signer.get().to_be_bytes());
    transcript.extend_from_slice(&from_index.to_be_bytes());
    transcript.extend_from_slice(&operation_count.to_be_bytes());
    transcript.extend_from_slice(&latest_index.to_be_bytes());
    append_hash_bytes(&mut transcript, &previous_order_digest.to_bytes());
    append_hash_bytes(&mut transcript, &range_order_digest.to_bytes());
    append_hash_bytes(&mut transcript, &latest_order_digest.to_bytes());
    append_hash_bytes(&mut transcript, &state_digest.to_bytes());
    transcript.extend_from_slice(&(operation_ids.len() as u64).to_be_bytes());
    for operation_id in operation_ids {
        append_hash_bytes(&mut transcript, &operation_id.to_bytes());
    }
    transcript
}

fn session_outcome_mismatch(reason: &str) -> FederationError {
    FederationError::SessionOutcomeMismatch {
        reason: reason.to_string(),
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
        FederationConfig, FederationConsensusItem, FederationError, MemberConfigPrivate,
        MemberConfigPublic, ThresholdParams,
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
            member_id,
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

    fn set_submit_response_status(
        response: &mut FederationSubmitOperationResponse,
        status: FederationOperationStatus,
    ) {
        match &mut response.outcome {
            FederationJournalAppendOutcome::Appended(entry)
            | FederationJournalAppendOutcome::Duplicate(entry) => {
                entry.status = status;
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
        assert_eq!(
            decoded.consensus_backend,
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(decoded.operation_count, 1);
        assert_eq!(decoded.latest_order_digest, entry.order_digest);
        assert_eq!(decoded.state_digest, journal.state_digest());
    }

    #[test]
    fn peer_status_rejects_missing_consensus_backend() {
        let status = FederationPeerStatus::new(
            federation_id(),
            member_id(),
            0,
            None,
            FederationJournalDigest::genesis(),
        );
        let mut value = serde_json::to_value(&status).expect("status value");
        value
            .as_object_mut()
            .expect("status object")
            .remove("consensus_backend");

        let err = serde_json::from_value::<FederationPeerStatus>(value)
            .expect_err("status without consensus backend must fail");
        assert!(err.to_string().contains("consensus_backend"));
    }

    #[test]
    fn peer_status_round_trips_aleph_bft_backend_and_cursor() {
        let status = FederationPeerStatus::new(
            federation_id(),
            member_id(),
            0,
            None,
            FederationJournalDigest::genesis(),
        )
        .with_consensus_backend(FederationConsensusBackendKind::AlephBft)
        .with_aleph_bft_next_item_index(7);

        let json = serde_json::to_string(&status).expect("status json");
        let decoded: FederationPeerStatus = serde_json::from_str(&json).expect("status");

        assert_eq!(
            decoded.consensus_backend,
            FederationConsensusBackendKind::AlephBft
        );
        assert_eq!(decoded.aleph_bft_next_item_index, Some(7));
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
        assert_eq!(response.state_digest, journal.state_digest());
        assert!(response.aleph_bft_finalized_items.is_empty());
        assert!(response.aleph_bft_finalized_items_complete);
    }

    #[test]
    fn catch_up_request_and_response_carry_aleph_bft_replay_fields() {
        let request = FederationJournalCatchUpRequest::new(3).with_aleph_bft_next_item_index(7);
        assert_eq!(request.aleph_bft_next_item_index, 7);
        let encoded_request = serde_json::to_value(&request).expect("request json");
        assert_eq!(encoded_request["aleph_bft_next_item_index"], 7);

        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_aleph_bft_finalized_items(Vec::new(), false);
        let encoded_response = serde_json::to_value(&response).expect("response json");
        assert!(encoded_response["aleph_bft_finalized_items"]
            .as_array()
            .expect("finalized items array")
            .is_empty());
        assert_eq!(
            encoded_response["aleph_bft_finalized_items_complete"],
            false
        );
    }

    #[test]
    fn catch_up_messages_reject_missing_aleph_bft_replay_fields() {
        let mut request_value =
            serde_json::to_value(FederationJournalCatchUpRequest::new(3)).expect("request json");
        request_value
            .as_object_mut()
            .expect("request object")
            .remove("aleph_bft_next_item_index");
        assert!(serde_json::from_value::<FederationJournalCatchUpRequest>(request_value).is_err());

        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
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
    fn catch_up_response_signs_and_verifies_session_outcome() {
        let (config, _, second_private) = transport_config();
        let member = config
            .member(second_private.member_id)
            .expect("signing member");
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        let request = FederationJournalCatchUpRequest::new(0);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&second_private)
        .expect("signed response");

        let outcome = response.session_outcome.as_ref().expect("session outcome");
        assert_eq!(outcome.signer, second_private.member_id);
        assert_eq!(outcome.operation_count, 2);
        assert_eq!(outcome.operation_ids.len(), 2);
        response
            .verify_session_outcome(member)
            .expect("verified outcome");
    }

    #[test]
    fn catch_up_response_rejects_tampered_session_outcome() {
        let (config, _, second_private) = transport_config();
        let member = config
            .member(second_private.member_id)
            .expect("signing member");
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let request = FederationJournalCatchUpRequest::new(0);
        let mut response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&second_private)
        .expect("signed response");
        response.entries[0].operation_id = FederationOperationId::from_bytes([42; 32]);

        assert!(matches!(
            response.verify_session_outcome(member),
            Err(FederationError::SessionOutcomeMismatch { .. })
        ));
    }

    #[test]
    fn catch_up_response_rejects_truncated_signed_session_outcome_range() {
        let (config, _, second_private) = transport_config();
        let member = config
            .member(second_private.member_id)
            .expect("signing member");
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        let request = FederationJournalCatchUpRequest::new(0);
        let mut response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&second_private)
        .expect("signed response");
        response.entries.pop();

        assert!(matches!(
            response.verify_session_outcome(member),
            Err(FederationError::SessionOutcomeMismatch { .. })
        ));
    }

    #[test]
    fn catch_up_response_rejects_bad_session_outcome_signature() {
        let (config, _, second_private) = transport_config();
        let member = config
            .member(second_private.member_id)
            .expect("signing member");
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let request = FederationJournalCatchUpRequest::new(0);
        let mut response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&second_private)
        .expect("signed response");
        let outcome = response.session_outcome.as_mut().expect("session outcome");
        let mut signature_bytes = outcome.signature.to_bytes();
        signature_bytes[0] ^= 0x01;
        outcome.signature =
            FederationRestSignature::from_slice(&signature_bytes).expect("signature bytes");

        assert!(matches!(
            response.verify_session_outcome(member),
            Err(FederationError::SessionOutcomeSignatureVerificationFailed { member_id })
                if member_id == second_private.member_id
        ));
    }

    #[test]
    fn catch_up_response_verifies_session_outcome_quorum() {
        let (config, private_members) = transport_config_three();
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        let request = FederationJournalCatchUpRequest::new(0);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[0])
        .expect("signed response");
        let second_outcome = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[1])
        .expect("second signed response")
        .session_outcome
        .expect("second session outcome");

        let quorum = response
            .verify_session_outcome_quorum(&config, &[second_outcome])
            .expect("quorum");

        assert_eq!(quorum.required, 2);
        assert_eq!(quorum.from_index, 0);
        assert_eq!(quorum.operation_count, 2);
        assert_eq!(quorum.signers.len(), 2);
        assert!(quorum.signers.contains(&private_members[0].member_id));
        assert!(quorum.signers.contains(&private_members[1].member_id));
    }

    #[test]
    fn catch_up_response_rejects_insufficient_session_outcome_quorum() {
        let (config, private_members) = transport_config_three();
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let request = FederationJournalCatchUpRequest::new(0);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[0])
        .expect("signed response");

        assert!(matches!(
            response.verify_session_outcome_quorum(&config, &[]),
            Err(FederationError::SessionOutcomeQuorumNotReached {
                required: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn catch_up_response_does_not_count_duplicate_session_outcome_signers() {
        let (config, private_members) = transport_config_three();
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        let request = FederationJournalCatchUpRequest::new(0);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[0])
        .expect("signed response");
        let duplicate_outcome = response
            .session_outcome
            .as_ref()
            .expect("session outcome")
            .clone();

        assert!(matches!(
            response.verify_session_outcome_quorum(&config, &[duplicate_outcome]),
            Err(FederationError::SessionOutcomeQuorumNotReached {
                required: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn catch_up_response_rejects_mismatched_session_outcome_quorum_member() {
        let (config, private_members) = transport_config_three();
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[0])
        .expect("signed response");
        let mismatched_outcome = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(1),
            50,
            100,
        )
        .with_signed_session_outcome(&private_members[1])
        .expect("mismatched signed response")
        .session_outcome
        .expect("mismatched session outcome");

        assert!(matches!(
            response.verify_session_outcome_quorum(&config, &[mismatched_outcome]),
            Err(FederationError::SessionOutcomeMismatch { .. })
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
    fn signed_aleph_bft_finalized_unit_request_verifies_and_response_validates() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let envelope = mint_quote_envelope();
        let operation_id = envelope.operation_id().expect("operation id");
        let unit = FederationAlephBftUnitPayload::new(
            config.federation_id,
            first_private.member_id,
            0,
            0,
            vec![FederationConsensusItem::operation(envelope.clone())],
        )
        .expect("AlephBFT unit");
        let payload = FederationAlephBftFinalizedUnitRequest::new(unit.clone());
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            7,
            FederationRestMessageKind::AlephBftFinalizedUnit,
            payload,
            &first_private,
        )
        .expect("signed AlephBFT finalized-unit request");

        request.verify(&config, receiver).expect("verified request");
        request
            .payload
            .ensure_admissible_size()
            .expect("admissible finalized unit");
        let decoded: FederationSignedRequest<FederationAlephBftFinalizedUnitRequest> =
            serde_json::from_str(
                &serde_json::to_string(&request).expect("AlephBFT finalized-unit json"),
            )
            .expect("decoded request");
        assert_eq!(
            decoded.kind,
            FederationRestMessageKind::AlephBftFinalizedUnit
        );
        decoded.verify(&config, receiver).expect("decoded verifies");

        let mut journal = InMemoryFederationJournal::new();
        let response = FederationAlephBftFinalizedUnitResponse::new(
            config.federation_id,
            &unit,
            vec![FederationSubmitOperationResponse::from_outcome(
                journal.append(envelope).expect("journal append"),
            )],
        );
        response
            .validate(config.federation_id, &unit)
            .expect("response validates");
        assert_eq!(response.operation_ids, vec![operation_id]);
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
    fn finalized_unit_response_validation_rejects_mismatched_outcome_operation_id() {
        let (config, first_private, _) = transport_config();
        let expected = mint_quote_envelope();
        let expected_operation_id = expected.operation_id().expect("expected operation id");
        let unit = FederationAlephBftUnitPayload::new(
            config.federation_id,
            first_private.member_id,
            0,
            0,
            vec![FederationConsensusItem::operation(expected)],
        )
        .expect("AlephBFT unit");
        let other = mint_envelope(
            "quote-2",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut journal = InMemoryFederationJournal::new();
        let mut submit_response = FederationSubmitOperationResponse::from_outcome(
            journal.append(other).expect("journal append"),
        );
        let found_operation_id = submit_response.operation_id;
        assert_ne!(expected_operation_id, found_operation_id);

        submit_response.operation_id = expected_operation_id;
        let response = FederationAlephBftFinalizedUnitResponse::new(
            config.federation_id,
            &unit,
            vec![submit_response],
        );

        assert!(matches!(
            response.validate(config.federation_id, &unit),
            Err(FederationError::SubmitOperationResponseMismatch { expected, found })
                if expected == expected_operation_id && found == found_operation_id
        ));
    }

    #[test]
    fn peer_status_request_materialized_rows_are_opt_in() {
        let default_json =
            serde_json::to_string(&FederationPeerStatusRequest::default()).expect("default json");
        assert_eq!(default_json, "{}");

        let opt_in_json =
            serde_json::to_string(&FederationPeerStatusRequest::with_materialized_rows())
                .expect("opt-in json");
        assert_eq!(opt_in_json, r#"{"include_materialized_rows":true}"#);

        let legacy: FederationPeerStatusRequest =
            serde_json::from_str("{}").expect("legacy status request");
        assert!(!legacy.include_materialized_rows);
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
            .validate_sender(&config, request.sender)
            .expect("payload owner");
        let json = serde_json::to_string(&request).expect("dkg commitment json");
        let decoded: FederationSignedRequest<FederationDkgCommitmentRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgCommitment);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender(&config, decoded.sender)
            .expect("decoded payload owner");
    }

    #[test]
    fn signed_dkg_reveal_request_verifies_sender_and_payload_owner() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload =
            FederationDkgRevealRequest::new(dkg_reveal(&config, first_private.member_id, 11));
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
            .validate_sender(&config, request.sender)
            .expect("payload owner");
        let json = serde_json::to_string(&request).expect("dkg reveal json");
        let decoded: FederationSignedRequest<FederationDkgRevealRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgReveal);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender(&config, decoded.sender)
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
            .validate_sender_receiver(&config, request.sender, receiver)
            .expect("payload owner and receiver");
        let json = serde_json::to_string(&request).expect("dkg secret share json");
        let decoded: FederationSignedRequest<FederationDkgSecretShareRequest> =
            serde_json::from_str(&json).expect("decoded request");
        assert_eq!(decoded.kind, FederationRestMessageKind::DkgSecretShare);
        decoded.verify(&config, receiver).expect("decoded verifies");
        decoded
            .payload
            .validate_sender_receiver(&config, decoded.sender, receiver)
            .expect("decoded payload owner and receiver");
    }

    #[test]
    fn signed_dkg_secret_share_request_rejects_sender_receiver_or_signer_mismatch() {
        let (config, first_private, second_private) = transport_config();
        let receiver = roster_member_id(2);
        let sender_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
            second_private.member_id,
            receiver,
            BTreeMap::from([(Amount::from(1), dkg_secret_share(receiver, 11))]),
        );
        let receiver_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
            first_private.member_id,
            first_private.member_id,
            BTreeMap::from([(
                Amount::from(1),
                dkg_secret_share(first_private.member_id, 11),
            )]),
        );
        let signer_mismatch = FederationDkgSecretShareRequest::new(
            config.federation_id,
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

        tracker.record_sequence(first_member, 5);
        tracker
            .verify_next_sequence(first_member, 6)
            .expect("higher authenticated sequence accepted");
        assert!(matches!(
            tracker.verify_next_sequence(first_member, 5),
            Err(FederationError::RestStaleSequence {
                member_id,
                last: 5,
                found: 5,
            }) if member_id == first_member
        ));
        tracker
            .verify_next_sequence(second_member, 1)
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
    fn sequence_tracker_rejects_persisted_stale_request_without_recent_window() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let last_sequence = 1_000_000_000_100;
        let stale_sequence = last_sequence - 50;
        let stale = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            stale_sequence,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("stale request");
        let mut persisted = BTreeMap::new();
        persisted.insert(first_private.member_id, last_sequence);
        let mut tracker = FederationRestSequenceTracker::from_sequences(persisted);

        assert!(matches!(
            tracker.verify_and_record(&stale, &config, receiver),
            Err(FederationError::RestStaleSequence {
                member_id,
                last,
                found
            }) if member_id == roster_member_id(1)
                && last == last_sequence
                && found == stale_sequence
        ));
    }
}
