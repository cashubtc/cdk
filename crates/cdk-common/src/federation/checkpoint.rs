//! Quorum-signed recovery checkpoints for federated mints.
//!
//! AlephBFT remains the only ordering and finality mechanism. A checkpoint is
//! bounded, transferable evidence about an already-finalized and fully applied
//! prefix. It cannot authorize application state by itself, and opaque
//! AlephBFT backup bytes are deliberately excluded from the signed payload.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationAlephBftFinalizedItem, FederationApplicationState, FederationConfig,
    FederationConfigDigest, FederationError, FederationId, FederationJournalDigest,
    FederationMaterializedStateDigest, FederationOperationId, MemberConfigPrivate,
    MemberConfigPublic, MemberId, Result,
};

const FEDERATION_APPLICATION_DIGEST_GENESIS_DOMAIN: &[u8] =
    b"cdk-federation-application-digest-genesis-v1";
const FEDERATION_APPLICATION_DIGEST_ENTRY_DOMAIN: &[u8] =
    b"cdk-federation-application-digest-entry-v1";
const FEDERATION_FINALIZED_ITEM_DIGEST_GENESIS_DOMAIN: &[u8] =
    b"cdk-federation-finalized-item-digest-genesis-v1";
const FEDERATION_FINALIZED_ITEM_DIGEST_ENTRY_DOMAIN: &[u8] =
    b"cdk-federation-finalized-item-digest-entry-v1";
const FEDERATION_CHECKPOINT_ID_DOMAIN: &[u8] = b"cdk-federation-checkpoint-id-v1";
const FEDERATION_CHECKPOINT_SIGNATURE_DOMAIN: &[u8] = b"cdk-federation-checkpoint-signature-v1";

/// Canonical checkpoint payload version supported by this build.
pub const FEDERATION_CHECKPOINT_VERSION: u16 = 1;

/// Cumulative digest of terminal application results in accepted-operation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationApplicationDigest([u8; 32]);

impl FederationApplicationDigest {
    /// Create an application digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse an application digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidApplicationDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse an application digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this application digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this application digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Return the digest before any accepted operation reaches a terminal state.
    #[must_use]
    pub fn genesis() -> Self {
        Self(Sha256Hash::hash(FEDERATION_APPLICATION_DIGEST_GENESIS_DOMAIN).to_byte_array())
    }

    /// Append one canonical terminal application result.
    ///
    /// The durable applied cursor owns exactly-once advancement. Callers must
    /// commit that cursor, the terminal journal state, business-state changes,
    /// and the returned digest in one database transaction.
    pub fn append_terminal(
        self,
        operation_index: u64,
        operation_id: FederationOperationId,
        application: &FederationApplicationState,
    ) -> Result<Self> {
        if !application.is_terminal() {
            return Err(checkpoint_invalid(
                "application digest can include only terminal application states",
            ));
        }

        let application = canonical_json_bytes(application)?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_APPLICATION_DIGEST_ENTRY_DOMAIN);
        append_hash_bytes(&mut transcript, &self.to_bytes());
        transcript.extend_from_slice(&operation_index.to_be_bytes());
        append_hash_bytes(&mut transcript, &operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &application);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationApplicationDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationApplicationDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationApplicationDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationApplicationDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Cumulative digest of every AlephBFT finalized item and its provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationFinalizedItemDigest([u8; 32]);

impl FederationFinalizedItemDigest {
    /// Create a finalized-item digest from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a finalized-item digest from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidFinalizedItemDigestSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a finalized-item digest from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this finalized-item digest as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this finalized-item digest as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Return the digest before any AlephBFT item is finalized.
    #[must_use]
    pub fn genesis() -> Self {
        Self(Sha256Hash::hash(FEDERATION_FINALIZED_ITEM_DIGEST_GENESIS_DOMAIN).to_byte_array())
    }

    /// Append one canonical finalized item, including status and unit provenance.
    pub fn append(self, item: &FederationAlephBftFinalizedItem) -> Result<Self> {
        item.item.ensure_supported_version()?;
        if item.item.operation_id()? != item.operation_id {
            return Err(checkpoint_invalid(
                "finalized item operation id does not match its canonical body",
            ));
        }

        let item = canonical_json_bytes(item)?;
        let mut transcript = Vec::new();
        append_hash_bytes(
            &mut transcript,
            FEDERATION_FINALIZED_ITEM_DIGEST_ENTRY_DOMAIN,
        );
        append_hash_bytes(&mut transcript, &self.to_bytes());
        append_hash_bytes(&mut transcript, &item);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationFinalizedItemDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationFinalizedItemDigest {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationFinalizedItemDigest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationFinalizedItemDigest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Checkpoint creation and retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationCheckpointKind {
    /// Deterministic interval checkpoint retained as a historical batch boundary.
    Scheduled,
    /// On-demand checkpoint at an exact converged stable tip.
    Recovery,
}

/// Scalar durable checkpoint metadata used by bounded startup validation.
///
/// The three rolling frontiers advance only through contiguous persisted rows.
/// `application_next_index` may lag the journal while accepted operations are
/// being applied, but it can never advance beyond `journal_next_index`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationCheckpointFrontier {
    /// First accepted-operation index not yet durably journaled.
    pub journal_next_index: u64,
    /// Cumulative accepted-operation order digest at `journal_next_index`.
    pub journal_order_digest: FederationJournalDigest,
    /// First finalized-item index not yet durably recorded.
    pub finalized_next_item_index: u64,
    /// Cumulative finalized-item digest at `finalized_next_item_index`.
    pub finalized_item_digest: FederationFinalizedItemDigest,
    /// First accepted-operation index without a terminal durable application result.
    pub application_next_index: u64,
    /// Cumulative terminal-application digest at `application_next_index`.
    pub application_digest: FederationApplicationDigest,
    /// Latest locally trusted quorum checkpoint, when one has been promoted.
    pub trusted_checkpoint_id: Option<FederationCheckpointId>,
    /// Latest checkpoint candidate whose full local state projection was audited.
    pub latest_audited_checkpoint_id: Option<FederationCheckpointId>,
    /// Full materialized-state digest captured with the latest local audit.
    pub latest_materialized_state_digest: Option<FederationMaterializedStateDigest>,
    /// Durable fail-closed latch set by a failed explicit or checkpoint-time audit.
    pub materialized_state_audit_mismatch: bool,
}

impl FederationCheckpointFrontier {
    /// Return the empty-history durable frontier.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            journal_next_index: 0,
            journal_order_digest: FederationJournalDigest::genesis(),
            finalized_next_item_index: 0,
            finalized_item_digest: FederationFinalizedItemDigest::genesis(),
            application_next_index: 0,
            application_digest: FederationApplicationDigest::genesis(),
            trusted_checkpoint_id: None,
            latest_audited_checkpoint_id: None,
            latest_materialized_state_digest: None,
            materialized_state_audit_mismatch: false,
        }
    }
}

/// Canonical quorum-signed federation recovery frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpoint {
    /// Canonical checkpoint payload version.
    pub version: u16,
    /// Federation whose finalized state is committed.
    pub federation_id: FederationId,
    /// Digest of the exact public config and identity-key epoch.
    pub federation_config_digest: FederationConfigDigest,
    /// Scheduled or on-demand recovery creation policy.
    pub kind: FederationCheckpointKind,
    /// Latest scheduled checkpoint sequence represented by this payload.
    pub scheduled_checkpoint_sequence: u64,
    /// Previous scheduled checkpoint for scheduled payloads, or latest scheduled checkpoint for
    /// recovery payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_anchor_id: Option<FederationCheckpointId>,
    /// First accepted-operation index after this checkpoint.
    pub journal_next_index: u64,
    /// Cumulative accepted-operation order digest at `journal_next_index`.
    pub journal_order_digest: FederationJournalDigest,
    /// First finalized-item index after this checkpoint.
    pub finalized_next_item_index: u64,
    /// Cumulative finalized-item digest at `finalized_next_item_index`.
    pub finalized_item_digest: FederationFinalizedItemDigest,
    /// Cumulative terminal-application digest at `journal_next_index`.
    pub application_digest: FederationApplicationDigest,
    /// Active AlephBFT session containing the checkpoint boundary.
    pub aleph_bft_session: u64,
    /// Next finalized-unit index expected in the active session.
    pub next_unit_finalization_index: u64,
    /// Next unit index expected from each configured member in the active session.
    pub next_unit_index_by_member: BTreeMap<MemberId, u64>,
    /// Full audit digest of consensus-authoritative materialized state.
    pub materialized_state_digest: FederationMaterializedStateDigest,
}

impl FederationCheckpoint {
    /// Return canonical JSON bytes for the signed payload.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate_shape()?;
        canonical_json_bytes(self)
    }

    /// Derive this checkpoint's domain-separated identifier.
    pub fn id(&self) -> Result<FederationCheckpointId> {
        let canonical = self.canonical_bytes()?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_CHECKPOINT_ID_DOMAIN);
        append_hash_bytes(&mut transcript, &canonical);
        Ok(FederationCheckpointId::from_bytes(
            Sha256Hash::hash(&transcript).to_byte_array(),
        ))
    }

    /// Validate payload structure, federation binding, and roster watermarks.
    pub fn validate_against_config(&self, config: &FederationConfig) -> Result<()> {
        self.validate_shape()?;
        config.validate()?;
        if self.federation_id != config.federation_id {
            return Err(checkpoint_invalid("federation id mismatch"));
        }
        if self.federation_config_digest != config.config_digest_for_validated_config()? {
            return Err(checkpoint_invalid("federation config digest mismatch"));
        }

        let expected_members = config.members.keys().copied().collect::<BTreeSet<_>>();
        let found_members = self
            .next_unit_index_by_member
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if found_members != expected_members {
            return Err(checkpoint_invalid(
                "next-unit watermarks do not match the configured federation roster",
            ));
        }

        Ok(())
    }

    /// Validate monotonic promotion from an already trusted checkpoint.
    ///
    /// Digest continuation for an advancing cursor is validated by the catch-up
    /// batch. This method rejects rollback, lineage forks, and conflicting
    /// digests at an unchanged cursor using only bounded checkpoint metadata.
    pub fn validate_promotion_from(&self, current: Option<&Self>) -> Result<()> {
        self.validate_shape()?;
        let Some(current) = current else {
            return Ok(());
        };
        current.validate_shape()?;

        if self.federation_id != current.federation_id
            || self.federation_config_digest != current.federation_config_digest
        {
            return Err(checkpoint_invalid(
                "trusted checkpoint federation or config lineage changed",
            ));
        }
        if self.id()? == current.id()? {
            return Ok(());
        }

        ensure_cursor_does_not_regress(
            "journal next index",
            self.journal_next_index,
            current.journal_next_index,
        )?;
        ensure_cursor_does_not_regress(
            "finalized next item index",
            self.finalized_next_item_index,
            current.finalized_next_item_index,
        )?;
        if self.aleph_bft_session < current.aleph_bft_session {
            return Err(checkpoint_invalid("AlephBFT session regressed"));
        }
        if self.aleph_bft_session == current.aleph_bft_session {
            ensure_cursor_does_not_regress(
                "unit finalization index",
                self.next_unit_finalization_index,
                current.next_unit_finalization_index,
            )?;
            for (member_id, current_index) in &current.next_unit_index_by_member {
                let Some(candidate_index) = self.next_unit_index_by_member.get(member_id) else {
                    return Err(checkpoint_invalid(
                        "candidate checkpoint omitted a member unit watermark",
                    ));
                };
                ensure_cursor_does_not_regress(
                    "member unit index",
                    *candidate_index,
                    *current_index,
                )?;
            }
        }

        if self.journal_next_index == current.journal_next_index {
            if self.journal_order_digest != current.journal_order_digest {
                return Err(checkpoint_invalid(
                    "journal digest changed at an unchanged cursor",
                ));
            }
            if self.application_digest != current.application_digest {
                return Err(checkpoint_invalid(
                    "application digest changed at an unchanged cursor",
                ));
            }
        }
        if self.finalized_next_item_index == current.finalized_next_item_index
            && self.finalized_item_digest != current.finalized_item_digest
        {
            return Err(checkpoint_invalid(
                "finalized-item digest changed at an unchanged cursor",
            ));
        }
        if self.journal_next_index == current.journal_next_index
            && self.finalized_next_item_index == current.finalized_next_item_index
            && self.materialized_state_digest != current.materialized_state_digest
        {
            return Err(checkpoint_invalid(
                "materialized-state digest changed at an unchanged frontier",
            ));
        }

        let (scheduled_sequence, latest_scheduled_id) = match current.kind {
            FederationCheckpointKind::Scheduled => {
                (current.scheduled_checkpoint_sequence, Some(current.id()?))
            }
            FederationCheckpointKind::Recovery => (
                current.scheduled_checkpoint_sequence,
                current.scheduled_anchor_id,
            ),
        };
        match self.kind {
            FederationCheckpointKind::Scheduled => {
                let expected_sequence = scheduled_sequence
                    .checked_add(1)
                    .ok_or_else(|| checkpoint_invalid("scheduled checkpoint sequence overflow"))?;
                if self.scheduled_checkpoint_sequence != expected_sequence
                    || self.scheduled_anchor_id != latest_scheduled_id
                {
                    return Err(checkpoint_invalid(
                        "scheduled checkpoint sequence or anchor does not continue the chain",
                    ));
                }
            }
            FederationCheckpointKind::Recovery => {
                if self.scheduled_checkpoint_sequence != scheduled_sequence
                    || self.scheduled_anchor_id != latest_scheduled_id
                {
                    return Err(checkpoint_invalid(
                        "recovery checkpoint does not retain the latest scheduled anchor",
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_shape(&self) -> Result<()> {
        if self.version != FEDERATION_CHECKPOINT_VERSION {
            return Err(FederationError::CheckpointVersionMismatch {
                expected: FEDERATION_CHECKPOINT_VERSION,
                found: self.version,
            });
        }
        match self.kind {
            FederationCheckpointKind::Scheduled => {
                if self.scheduled_checkpoint_sequence == 0 {
                    return Err(checkpoint_invalid(
                        "scheduled checkpoint sequences start at one",
                    ));
                }
                if (self.scheduled_checkpoint_sequence == 1) != self.scheduled_anchor_id.is_none() {
                    return Err(checkpoint_invalid(
                        "only the first scheduled checkpoint has no scheduled anchor",
                    ));
                }
            }
            FederationCheckpointKind::Recovery => {
                if (self.scheduled_checkpoint_sequence == 0) != self.scheduled_anchor_id.is_none() {
                    return Err(checkpoint_invalid(
                        "recovery checkpoint sequence and scheduled anchor disagree",
                    ));
                }
            }
        }
        if self.journal_next_index > self.finalized_next_item_index {
            return Err(checkpoint_invalid(
                "accepted-operation cursor is ahead of the finalized-item cursor",
            ));
        }
        if self.journal_next_index == 0 {
            if self.journal_order_digest != FederationJournalDigest::genesis() {
                return Err(checkpoint_invalid(
                    "empty journal cursor does not use the journal genesis digest",
                ));
            }
            if self.application_digest != FederationApplicationDigest::genesis() {
                return Err(checkpoint_invalid(
                    "empty application cursor does not use the application genesis digest",
                ));
            }
        }
        if self.finalized_next_item_index == 0
            && self.finalized_item_digest != FederationFinalizedItemDigest::genesis()
        {
            return Err(checkpoint_invalid(
                "empty finalized-item cursor does not use the finalized-item genesis digest",
            ));
        }
        if self.next_unit_index_by_member.is_empty() {
            return Err(checkpoint_invalid(
                "checkpoint must include member unit watermarks",
            ));
        }

        Ok(())
    }
}

/// Domain-separated hash of one canonical checkpoint payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationCheckpointId([u8; 32]);

impl FederationCheckpointId {
    /// Create a checkpoint ID from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a checkpoint ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidCheckpointIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a checkpoint ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this checkpoint ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this checkpoint ID as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationCheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationCheckpointId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationCheckpointId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationCheckpointId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// One member identity signature over a checkpoint ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointSignature {
    /// Exact checkpoint ID signed by the member.
    pub checkpoint_id: FederationCheckpointId,
    /// Configured member that produced the signature.
    pub signer: MemberId,
    /// Raw BIP-340 Schnorr signature bytes.
    #[serde(with = "checkpoint_signature_hex")]
    signature: [u8; 64],
}

impl FederationCheckpointSignature {
    /// Sign a locally derived checkpoint with a member identity key.
    pub fn new_signed(
        checkpoint: &FederationCheckpoint,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::new_signed_with(checkpoint, private_config, true)
    }

    /// Sign a checkpoint without auxiliary randomness for reproducible fixtures.
    pub fn new_signed_no_aux_rand(
        checkpoint: &FederationCheckpoint,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::new_signed_with(checkpoint, private_config, false)
    }

    /// Parse a checkpoint signature from its committed metadata and raw bytes.
    pub fn from_slice(
        checkpoint_id: FederationCheckpointId,
        signer: MemberId,
        signature: &[u8],
    ) -> Result<Self> {
        let signature: [u8; 64] =
            signature
                .try_into()
                .map_err(|_| FederationError::InvalidCheckpointSignatureSize {
                    expected: 64,
                    found: signature.len(),
                })?;
        Ok(Self {
            checkpoint_id,
            signer,
            signature,
        })
    }

    /// Return the raw Schnorr signature bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 64] {
        self.signature
    }

    /// Verify this signature against an exact checkpoint and roster member.
    pub fn verify(
        &self,
        checkpoint: &FederationCheckpoint,
        member: &MemberConfigPublic,
    ) -> Result<()> {
        let checkpoint_id = checkpoint.id()?;
        if self.checkpoint_id != checkpoint_id {
            return Err(checkpoint_invalid(
                "checkpoint signature references a different checkpoint id",
            ));
        }
        if self.signer != member.member_id {
            return Err(checkpoint_invalid(
                "checkpoint signature signer does not match the roster member",
            ));
        }
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| FederationError::InvalidCheckpointSignature)?;
        member
            .identity_public_key
            .verify(&checkpoint_signature_transcript(checkpoint_id), &signature)
            .map_err(|_| FederationError::CheckpointSignatureVerificationFailed {
                member_id: self.signer,
            })
    }

    fn new_signed_with(
        checkpoint: &FederationCheckpoint,
        private_config: &MemberConfigPrivate,
        use_aux_rand: bool,
    ) -> Result<Self> {
        if private_config
            .federation_id
            .is_some_and(|federation_id| federation_id != checkpoint.federation_id)
        {
            return Err(checkpoint_invalid(
                "private config federation does not match the checkpoint",
            ));
        }
        let checkpoint_id = checkpoint.id()?;
        let transcript = checkpoint_signature_transcript(checkpoint_id);
        let signature = if use_aux_rand {
            private_config.identity_secret_key.sign(&transcript)?
        } else {
            private_config
                .identity_secret_key
                .sign_no_aux_rand(&transcript)?
        };
        Ok(Self {
            checkpoint_id,
            signer: private_config.member_id,
            signature: signature.serialize(),
        })
    }
}

/// Transferable set of member signatures over one exact checkpoint payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointQuorum {
    /// Canonical checkpoint payload shared by every signer.
    pub checkpoint: FederationCheckpoint,
    /// Individual member signatures; quorum verification rejects duplicates.
    pub signatures: Vec<FederationCheckpointSignature>,
}

impl FederationCheckpointQuorum {
    /// Create a checkpoint proof from a payload and collected signatures.
    #[must_use]
    pub fn new(
        checkpoint: FederationCheckpoint,
        signatures: Vec<FederationCheckpointSignature>,
    ) -> Self {
        Self {
            checkpoint,
            signatures,
        }
    }

    /// Verify the bounded proof against the configured consensus threshold.
    pub fn verify(&self, config: &FederationConfig) -> Result<BTreeSet<MemberId>> {
        self.checkpoint.validate_against_config(config)?;
        if self.signatures.len() > config.members.len() {
            return Err(checkpoint_invalid(
                "checkpoint proof contains more signatures than configured members",
            ));
        }

        let mut signers = BTreeSet::new();
        for signature in &self.signatures {
            if !signers.insert(signature.signer) {
                return Err(FederationError::DuplicateCheckpointSigner {
                    member_id: signature.signer,
                });
            }
            let member = config
                .member(signature.signer)
                .ok_or(FederationError::UnknownMember {
                    member_id: signature.signer,
                })?;
            signature.verify(&self.checkpoint, member)?;
        }

        let found = u16::try_from(signers.len()).unwrap_or(u16::MAX);
        let required = config.threshold_params.consensus_threshold;
        if found < required {
            return Err(FederationError::CheckpointQuorumNotReached { required, found });
        }

        Ok(signers)
    }
}

fn checkpoint_signature_transcript(checkpoint_id: FederationCheckpointId) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_hash_bytes(&mut transcript, FEDERATION_CHECKPOINT_SIGNATURE_DOMAIN);
    transcript.extend_from_slice(&FEDERATION_CHECKPOINT_VERSION.to_be_bytes());
    append_hash_bytes(&mut transcript, &checkpoint_id.to_bytes());
    transcript
}

fn ensure_cursor_does_not_regress(name: &str, candidate: u64, current: u64) -> Result<()> {
    if candidate < current {
        return Err(checkpoint_invalid(&format!(
            "{name} regressed from {current} to {candidate}"
        )));
    }
    Ok(())
}

fn checkpoint_invalid(reason: &str) -> FederationError {
    FederationError::CheckpointInvalid {
        reason: reason.to_string(),
    }
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

mod checkpoint_signature_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(
        signature: &[u8; 64],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(signature))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> std::result::Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            serde::de::Error::custom(format!(
                "invalid checkpoint signature length: expected 64, found {}",
                bytes.len()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use serde_json::json;

    use super::*;
    use crate::error::{ErrorCode, ErrorResponse};
    use crate::federation::{
        FederationAlephBftFinalizedItemStatus, FederationAlephBftUnitBodyHash,
        FederationConsensusDefaultItem, FederationConsensusItem, ThresholdParams,
    };
    use crate::{mint_url::MintUrl, SecretKey};

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(id: u16) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = id as u8;
        SecretKey::from_slice(&bytes).expect("valid identity secret")
    }

    fn configs() -> (FederationConfig, BTreeMap<MemberId, MemberConfigPrivate>) {
        let params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
        let members = (1..=4)
            .map(|id| {
                let member_id = member_id(id);
                (
                    member_id,
                    MemberConfigPublic {
                        member_id,
                        public_mint_url: MintUrl::from_str(&format!(
                            "https://mint{id}.example.invalid"
                        ))
                        .expect("public mint url"),
                        federation_api_url: MintUrl::from_str(&format!(
                            "https://federation{id}.example.invalid"
                        ))
                        .expect("federation api url"),
                        identity_public_key: identity_secret(id).public_key(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let config = FederationConfig::new(params, members, Vec::new()).expect("config");
        let private_configs = (1..=4)
            .map(|id| {
                let member_id = member_id(id);
                (
                    member_id,
                    MemberConfigPrivate {
                        federation_id: Some(config.federation_id),
                        member_id,
                        identity_secret_key: identity_secret(id),
                        keyset_secret_shares: BTreeMap::new(),
                        setup_transcript_hash: None,
                    },
                )
            })
            .collect();
        (config, private_configs)
    }

    fn checkpoint(config: &FederationConfig) -> FederationCheckpoint {
        FederationCheckpoint {
            version: FEDERATION_CHECKPOINT_VERSION,
            federation_id: config.federation_id,
            federation_config_digest: config.config_digest().expect("config digest"),
            kind: FederationCheckpointKind::Scheduled,
            scheduled_checkpoint_sequence: 1,
            scheduled_anchor_id: None,
            journal_next_index: 5,
            journal_order_digest: FederationJournalDigest::from_bytes([1; 32]),
            finalized_next_item_index: 8,
            finalized_item_digest: FederationFinalizedItemDigest::from_bytes([2; 32]),
            application_digest: FederationApplicationDigest::from_bytes([3; 32]),
            aleph_bft_session: 2,
            next_unit_finalization_index: 4,
            next_unit_index_by_member: config
                .members
                .keys()
                .copied()
                .map(|member_id| (member_id, u64::from(member_id.get()) + 10))
                .collect(),
            materialized_state_digest: FederationMaterializedStateDigest::from_bytes([4; 32]),
        }
    }

    fn advanced_recovery(
        current: &FederationCheckpoint,
        latest_scheduled: &FederationCheckpoint,
        advance: u8,
    ) -> FederationCheckpoint {
        let mut recovery = current.clone();
        recovery.kind = FederationCheckpointKind::Recovery;
        recovery.scheduled_checkpoint_sequence = latest_scheduled.scheduled_checkpoint_sequence;
        recovery.scheduled_anchor_id = Some(latest_scheduled.id().expect("scheduled id"));
        recovery.journal_next_index += u64::from(advance);
        recovery.journal_order_digest = FederationJournalDigest::from_bytes([10 + advance; 32]);
        recovery.finalized_next_item_index += u64::from(advance);
        recovery.finalized_item_digest =
            FederationFinalizedItemDigest::from_bytes([20 + advance; 32]);
        recovery.application_digest = FederationApplicationDigest::from_bytes([30 + advance; 32]);
        recovery.next_unit_finalization_index += u64::from(advance);
        for unit_index in recovery.next_unit_index_by_member.values_mut() {
            *unit_index += u64::from(advance);
        }
        recovery.materialized_state_digest =
            FederationMaterializedStateDigest::from_bytes([40 + advance; 32]);
        recovery
    }

    fn finalized_item(
        status: FederationAlephBftFinalizedItemStatus,
    ) -> FederationAlephBftFinalizedItem {
        FederationAlephBftFinalizedItem {
            accepted_item_index: 7,
            session: 2,
            unit_finalization_index: 3,
            unit_submitting_member: member_id(1),
            unit_index: 5,
            item_index_in_unit: 0,
            operation_id: None,
            status,
            unit_body_hash: FederationAlephBftUnitBodyHash::from_bytes([8; 32]),
            item: FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
                "checkpoint-test",
                json!({"b": 2, "a": 1}),
            )),
        }
    }

    #[test]
    fn checkpoint_id_and_rolling_digests_have_fixed_vectors() {
        let (config, _) = configs();
        let checkpoint = checkpoint(&config);
        let checkpoint_id = checkpoint.id().expect("checkpoint id").to_hex();

        let application_digest = FederationApplicationDigest::genesis()
            .append_terminal(
                0,
                FederationOperationId::from_bytes([9; 32]),
                &FederationApplicationState::Applied {},
            )
            .expect("application digest");

        let finalized_digest = FederationFinalizedItemDigest::genesis()
            .append(&finalized_item(
                FederationAlephBftFinalizedItemStatus::Accepted,
            ))
            .expect("finalized digest");
        let application_digest = application_digest.to_hex();
        let finalized_digest = finalized_digest.to_hex();
        assert_eq!(
            (
                checkpoint_id.as_str(),
                application_digest.as_str(),
                finalized_digest.as_str(),
            ),
            (
                "e7abf8c59a987296a5537a25f7bd4a04ac430987a07256f2263f9793dc9a6a2a",
                "6f98c1caffcb0b8c7970d6b697c821e0f46949fce43c74c4e349a10e38ccfb3d",
                "d741b5d8863d623672627c1d7fd5ccea7a0420e78b9b68ac89b231ae2be5daa2",
            )
        );
    }

    #[test]
    fn application_digest_accepts_only_terminal_results_and_is_deterministic() {
        let operation_id = FederationOperationId::from_bytes([11; 32]);
        let states = [
            FederationApplicationState::Applied {},
            FederationApplicationState::ConsensusRejected {},
            FederationApplicationState::Rejected {
                response: ErrorResponse::new(
                    ErrorCode::Unknown(43_210),
                    "deterministic rejection".to_string(),
                ),
            },
        ];

        let mut distinct = BTreeSet::new();
        for state in states {
            let first = FederationApplicationDigest::genesis()
                .append_terminal(0, operation_id, &state)
                .expect("terminal state");
            let retry = FederationApplicationDigest::genesis()
                .append_terminal(0, operation_id, &state)
                .expect("deterministic retry");
            assert_eq!(first, retry);
            distinct.insert(first);
        }
        assert_eq!(distinct.len(), 3);

        for state in [
            FederationApplicationState::Accepted {},
            FederationApplicationState::Applying {},
            FederationApplicationState::Failed {},
        ] {
            assert!(FederationApplicationDigest::genesis()
                .append_terminal(0, operation_id, &state)
                .is_err());
        }
    }

    #[test]
    fn finalized_digest_commits_status_and_unit_provenance() {
        let accepted = finalized_item(FederationAlephBftFinalizedItemStatus::Accepted);
        let duplicate = finalized_item(FederationAlephBftFinalizedItemStatus::DuplicateOperation);
        let rejected =
            finalized_item(FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation);
        let digest = |item: &FederationAlephBftFinalizedItem| {
            FederationFinalizedItemDigest::genesis()
                .append(item)
                .expect("digest")
        };
        assert_ne!(digest(&accepted), digest(&duplicate));
        assert_ne!(digest(&accepted), digest(&rejected));

        let mut other_unit = accepted.clone();
        other_unit.unit_finalization_index += 1;
        assert_ne!(digest(&accepted), digest(&other_unit));

        let mut reordered_payload = accepted.clone();
        reordered_payload.item = FederationConsensusItem::Default(
            FederationConsensusDefaultItem::new("checkpoint-test", json!({"a": 1, "b": 2})),
        );
        assert_eq!(digest(&accepted), digest(&reordered_payload));
    }

    #[test]
    fn quorum_verification_uses_distinct_configured_consensus_members() {
        let (config, private_configs) = configs();
        let checkpoint = checkpoint(&config);
        let signatures = private_configs
            .values()
            .take(3)
            .map(|private| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&checkpoint, private)
                    .expect("signature")
            })
            .collect::<Vec<_>>();
        let quorum = FederationCheckpointQuorum::new(checkpoint.clone(), signatures.clone());
        assert_eq!(quorum.verify(&config).expect("quorum").len(), 3);

        let insufficient =
            FederationCheckpointQuorum::new(checkpoint.clone(), signatures[..2].to_vec());
        assert!(matches!(
            insufficient.verify(&config),
            Err(FederationError::CheckpointQuorumNotReached {
                required: 3,
                found: 2
            })
        ));

        let duplicate = FederationCheckpointQuorum::new(
            checkpoint.clone(),
            vec![signatures[0], signatures[0], signatures[1]],
        );
        assert!(matches!(
            duplicate.verify(&config),
            Err(FederationError::DuplicateCheckpointSigner { .. })
        ));

        let mut corrupted_bytes = signatures[0].to_bytes();
        corrupted_bytes[0] ^= 1;
        let corrupted_signature = FederationCheckpointSignature::from_slice(
            signatures[0].checkpoint_id,
            signatures[0].signer,
            &corrupted_bytes,
        )
        .expect("corrupted signature bytes");
        let corrupted = FederationCheckpointQuorum::new(
            checkpoint.clone(),
            vec![corrupted_signature, signatures[1], signatures[2]],
        );
        assert!(matches!(
            corrupted.verify(&config),
            Err(FederationError::CheckpointSignatureVerificationFailed { .. })
                | Err(FederationError::InvalidCheckpointSignature)
        ));

        let unknown_private = MemberConfigPrivate {
            federation_id: Some(config.federation_id),
            member_id: member_id(9),
            identity_secret_key: identity_secret(9),
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        };
        let unknown_signature =
            FederationCheckpointSignature::new_signed_no_aux_rand(&checkpoint, &unknown_private)
                .expect("unknown signature");
        let unknown = FederationCheckpointQuorum::new(
            checkpoint,
            vec![signatures[0], signatures[1], unknown_signature],
        );
        assert!(matches!(
            unknown.verify(&config),
            Err(FederationError::UnknownMember { member_id: found }) if found == member_id(9)
        ));
    }

    #[test]
    fn every_checkpoint_field_is_bound_by_the_signature() {
        let (config, private_configs) = configs();
        let original = checkpoint(&config);
        let signer = member_id(1);
        let signature = FederationCheckpointSignature::new_signed_no_aux_rand(
            &original,
            private_configs.get(&signer).expect("private config"),
        )
        .expect("signature");
        let member = config.member(signer).expect("member");

        let mut mutations = Vec::new();

        let mut checkpoint = original.clone();
        checkpoint.version += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.federation_id = FederationId::from_bytes([51; 32]);
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.federation_config_digest = FederationConfigDigest::from_bytes([52; 32]);
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.kind = FederationCheckpointKind::Recovery;
        checkpoint.scheduled_checkpoint_sequence = 0;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.scheduled_checkpoint_sequence = 2;
        checkpoint.scheduled_anchor_id = Some(FederationCheckpointId::from_bytes([53; 32]));
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.journal_next_index += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.journal_order_digest = FederationJournalDigest::from_bytes([54; 32]);
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.finalized_next_item_index += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.finalized_item_digest = FederationFinalizedItemDigest::from_bytes([55; 32]);
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.application_digest = FederationApplicationDigest::from_bytes([56; 32]);
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.aleph_bft_session += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        checkpoint.next_unit_finalization_index += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original.clone();
        *checkpoint
            .next_unit_index_by_member
            .get_mut(&member_id(1))
            .expect("member watermark") += 1;
        mutations.push(checkpoint);

        let mut checkpoint = original;
        checkpoint.materialized_state_digest =
            FederationMaterializedStateDigest::from_bytes([57; 32]);
        mutations.push(checkpoint);

        for mutation in mutations {
            assert!(signature.verify(&mutation, member).is_err());
        }
    }

    #[test]
    fn scheduled_and_recovery_promotion_share_one_monotonic_lineage() {
        let (config, _) = configs();
        let scheduled = checkpoint(&config);
        let recovery_one = advanced_recovery(&scheduled, &scheduled, 1);
        recovery_one
            .validate_promotion_from(Some(&scheduled))
            .expect("first recovery");

        let recovery_two = advanced_recovery(&recovery_one, &scheduled, 2);
        recovery_two
            .validate_promotion_from(Some(&recovery_one))
            .expect("second recovery");

        let mut stale_recovery = recovery_one.clone();
        stale_recovery.materialized_state_digest =
            FederationMaterializedStateDigest::from_bytes([99; 32]);
        assert!(stale_recovery
            .validate_promotion_from(Some(&recovery_two))
            .is_err());

        let mut next_scheduled = recovery_two.clone();
        next_scheduled.kind = FederationCheckpointKind::Scheduled;
        next_scheduled.scheduled_checkpoint_sequence = 2;
        next_scheduled.scheduled_anchor_id = Some(scheduled.id().expect("scheduled id"));
        next_scheduled
            .validate_promotion_from(Some(&recovery_two))
            .expect("next scheduled after recovery");
        next_scheduled
            .validate_promotion_from(Some(&scheduled))
            .expect("same next scheduled without recovery");

        let mut broken_anchor = next_scheduled;
        broken_anchor.scheduled_anchor_id = Some(FederationCheckpointId::from_bytes([77; 32]));
        assert!(broken_anchor
            .validate_promotion_from(Some(&recovery_two))
            .is_err());
    }

    #[test]
    fn checkpoint_rejects_wrong_config_and_incomplete_roster_watermarks() {
        let (config, _) = configs();
        let mut candidate = checkpoint(&config);
        candidate
            .validate_against_config(&config)
            .expect("valid checkpoint");

        candidate.next_unit_index_by_member.remove(&member_id(4));
        assert!(candidate.validate_against_config(&config).is_err());

        let (other_config, _) = configs();
        let mut other_config = other_config;
        other_config
            .members
            .get_mut(&member_id(1))
            .expect("member")
            .identity_public_key = identity_secret(8).public_key();
        other_config.federation_id = FederationConfig::derive_federation_id(
            other_config.threshold_params,
            &other_config.members,
        )
        .expect("other federation id");
        assert!(checkpoint(&config)
            .validate_against_config(&other_config)
            .is_err());
    }

    #[test]
    fn checkpoint_quorum_round_trips_as_a_transferable_proof() {
        let (config, private_configs) = configs();
        let checkpoint = checkpoint(&config);
        let signatures = private_configs
            .values()
            .take(3)
            .map(|private| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&checkpoint, private)
                    .expect("signature")
            })
            .collect();
        let proof = FederationCheckpointQuorum::new(checkpoint, signatures);
        let encoded = serde_json::to_vec(&proof).expect("serialize proof");
        let decoded: FederationCheckpointQuorum =
            serde_json::from_slice(&encoded).expect("deserialize proof");
        assert_eq!(proof, decoded);
        decoded.verify(&config).expect("transferred quorum");
    }
}
