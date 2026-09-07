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
/// Default accepted-operation interval between scheduled checkpoints.
pub const DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS: u64 = 10_000;
/// Default finalized-item interval between scheduled checkpoints.
pub const DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS: u64 = 10_000;
/// Default canonical finalized-history bytes between scheduled checkpoints.
pub const DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES: u64 = 32 * 1024 * 1024;
/// Protocol maximum canonical finalized-history bytes in one checkpoint batch.
pub const FEDERATION_MAX_CHECKPOINT_BATCH_CANONICAL_BYTES: u64 = 64 * 1024 * 1024;
/// Protocol maximum accepted operations in one checkpoint batch.
pub const FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS: u64 = 20_000;
/// Protocol maximum finalized consensus items in one checkpoint batch.
pub const FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS: u64 = 20_000;
/// Protocol maximum encoded bytes in one checkpoint catch-up response page.
pub const FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES: usize = 8 * 1024 * 1024;
/// Protocol maximum encoded bytes in one checkpoint catch-up request.
pub const FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES: usize = 64 * 1024;
/// Protocol maximum aggregate encoded bytes fetched for one checkpoint batch.
pub const FEDERATION_MAX_CHECKPOINT_CATCH_UP_TRANSFER_BYTES: u64 = 256 * 1024 * 1024;

/// Sum canonical finalized-history bytes for one checkpoint batch.
pub fn federation_checkpoint_batch_canonical_bytes(
    items: &[FederationAlephBftFinalizedItem],
) -> Result<u64> {
    items.iter().try_fold(0_u64, |total, item| {
        let item_bytes = u64::try_from(item.canonical_size()?)
            .map_err(|_| checkpoint_invalid("canonical finalized item size exceeds u64"))?;
        total
            .checked_add(item_bytes)
            .ok_or_else(|| checkpoint_invalid("checkpoint canonical byte count overflow"))
    })
}

fn ensure_checkpoint_batch_item_limits(
    journal_next_index: u64,
    finalized_next_item_index: u64,
    base: Option<&FederationCheckpoint>,
) -> Result<(u64, u64)> {
    let base_journal_next_index = base.map_or(0, |checkpoint| checkpoint.journal_next_index);
    let base_finalized_next_item_index =
        base.map_or(0, |checkpoint| checkpoint.finalized_next_item_index);
    let journal_progress = journal_next_index
        .checked_sub(base_journal_next_index)
        .ok_or_else(|| checkpoint_invalid("checkpoint batch journal frontier regressed"))?;
    let finalized_progress = finalized_next_item_index
        .checked_sub(base_finalized_next_item_index)
        .ok_or_else(|| checkpoint_invalid("checkpoint batch finalized frontier regressed"))?;
    if journal_progress > FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS {
        return Err(checkpoint_invalid(
            "checkpoint batch exceeds the accepted-operation maximum",
        ));
    }
    if finalized_progress > FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS {
        return Err(checkpoint_invalid(
            "checkpoint batch exceeds the finalized-item maximum",
        ));
    }
    Ok((journal_progress, finalized_progress))
}

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

/// Consensus-configured scheduled-checkpoint interval policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointPolicy {
    /// Accepted operations since the preceding scheduled checkpoint.
    pub checkpoint_interval_journal_operations: u64,
    /// Finalized consensus items since the preceding scheduled checkpoint.
    pub checkpoint_interval_finalized_items: u64,
    /// Canonical finalized-history bytes since the preceding scheduled checkpoint.
    pub checkpoint_interval_canonical_bytes: u64,
}

impl FederationCheckpointPolicy {
    /// Create and validate a scheduled-checkpoint policy.
    pub fn new(
        checkpoint_interval_journal_operations: u64,
        checkpoint_interval_finalized_items: u64,
    ) -> Result<Self> {
        let policy = Self {
            checkpoint_interval_journal_operations,
            checkpoint_interval_finalized_items,
            checkpoint_interval_canonical_bytes: DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES,
        };
        policy.validate()?;
        Ok(policy)
    }

    /// Validate that both progress dimensions have a non-zero interval.
    pub fn validate(self) -> Result<()> {
        if self.checkpoint_interval_journal_operations == 0 {
            return Err(checkpoint_invalid(
                "journal-operation checkpoint interval must be non-zero",
            ));
        }
        if self.checkpoint_interval_finalized_items == 0 {
            return Err(checkpoint_invalid(
                "finalized-item checkpoint interval must be non-zero",
            ));
        }
        if self.checkpoint_interval_journal_operations
            > FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS
        {
            return Err(checkpoint_invalid(
                "journal-operation checkpoint interval exceeds the protocol batch maximum",
            ));
        }
        if self.checkpoint_interval_finalized_items
            > FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS
        {
            return Err(checkpoint_invalid(
                "finalized-item checkpoint interval exceeds the protocol batch maximum",
            ));
        }
        if self.checkpoint_interval_canonical_bytes == 0 {
            return Err(checkpoint_invalid(
                "canonical-byte checkpoint interval must be non-zero",
            ));
        }
        if self.checkpoint_interval_canonical_bytes
            > FEDERATION_MAX_CHECKPOINT_BATCH_CANONICAL_BYTES
        {
            return Err(checkpoint_invalid(
                "canonical-byte checkpoint interval exceeds the protocol batch maximum",
            ));
        }
        Ok(())
    }
}

impl Default for FederationCheckpointPolicy {
    fn default() -> Self {
        Self {
            checkpoint_interval_journal_operations: DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS,
            checkpoint_interval_finalized_items: DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS,
            checkpoint_interval_canonical_bytes: DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES,
        }
    }
}

/// Contiguous accepted-operation journal position and cumulative order digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationJournalFrontier {
    /// First accepted-operation index not represented by the digest.
    pub next_index: u64,
    /// Cumulative accepted-operation order digest at `next_index`.
    pub order_digest: FederationJournalDigest,
}

impl FederationJournalFrontier {
    /// Return the empty journal frontier.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            next_index: 0,
            order_digest: FederationJournalDigest::genesis(),
        }
    }

    fn validate_shape(self) -> Result<()> {
        if self.next_index == 0 && self.order_digest != FederationJournalDigest::genesis() {
            return Err(checkpoint_invalid(
                "empty journal cursor does not use the journal genesis digest",
            ));
        }
        Ok(())
    }

    fn ensure_covers(self, current: Self) -> Result<()> {
        ensure_cursor_does_not_regress("journal next index", self.next_index, current.next_index)?;
        if self.next_index == current.next_index && self.order_digest != current.order_digest {
            return Err(checkpoint_invalid(
                "journal digest changed at an unchanged cursor",
            ));
        }
        Ok(())
    }
}

/// Contiguous AlephBFT finalized-item position and cumulative provenance digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFinalizedFrontier {
    /// First finalized-item index not represented by the digest.
    pub next_item_index: u64,
    /// Cumulative finalized-item digest at `next_item_index`.
    pub item_digest: FederationFinalizedItemDigest,
}

impl FederationFinalizedFrontier {
    /// Return the empty finalized-item frontier.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            next_item_index: 0,
            item_digest: FederationFinalizedItemDigest::genesis(),
        }
    }

    fn validate_shape(self) -> Result<()> {
        if self.next_item_index == 0 && self.item_digest != FederationFinalizedItemDigest::genesis()
        {
            return Err(checkpoint_invalid(
                "empty finalized-item cursor does not use the finalized-item genesis digest",
            ));
        }
        Ok(())
    }

    fn ensure_covers(self, current: Self) -> Result<()> {
        ensure_cursor_does_not_regress(
            "finalized next item index",
            self.next_item_index,
            current.next_item_index,
        )?;
        if self.next_item_index == current.next_item_index
            && self.item_digest != current.item_digest
        {
            return Err(checkpoint_invalid(
                "finalized-item digest changed at an unchanged cursor",
            ));
        }
        Ok(())
    }
}

/// Contiguous terminal-application position and cumulative result digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationApplicationFrontier {
    /// First accepted-operation index without a committed terminal result.
    pub next_index: u64,
    /// Cumulative terminal-application digest at `next_index`.
    pub digest: FederationApplicationDigest,
}

impl FederationApplicationFrontier {
    /// Return the empty terminal-application frontier.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            next_index: 0,
            digest: FederationApplicationDigest::genesis(),
        }
    }

    fn validate_shape(self) -> Result<()> {
        if self.next_index == 0 && self.digest != FederationApplicationDigest::genesis() {
            return Err(checkpoint_invalid(
                "empty application cursor does not use the application genesis digest",
            ));
        }
        Ok(())
    }

    fn ensure_covers(self, current: Self) -> Result<()> {
        ensure_cursor_does_not_regress(
            "application next index",
            self.next_index,
            current.next_index,
        )?;
        if self.next_index == current.next_index && self.digest != current.digest {
            return Err(checkpoint_invalid(
                "application digest changed at an unchanged cursor",
            ));
        }
        Ok(())
    }
}

/// AlephBFT session and complete-unit recovery position committed by a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAlephBftRecoveryCursor {
    /// Session containing the committed complete-unit boundary.
    pub session: u64,
    /// Next finalized-unit index expected in `session`.
    pub next_unit_finalization_index: u64,
    /// Next unit index expected from every configured member in `session`.
    pub next_unit_index_by_member: BTreeMap<MemberId, u64>,
}

impl FederationAlephBftRecoveryCursor {
    fn validate_roster(&self, config: &FederationConfig) -> Result<()> {
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

    fn ensure_covers(&self, current: &Self) -> Result<()> {
        if self.session < current.session {
            return Err(checkpoint_invalid("AlephBFT session regressed"));
        }
        if self.session != current.session {
            return Ok(());
        }
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
            ensure_cursor_does_not_regress("member unit index", *candidate_index, *current_index)?;
        }
        Ok(())
    }
}

/// Canonical committed federation recovery frontier.
///
/// Existing checkpoint, boundary, durable, status, and transport records keep
/// their explicit serialized fields and project them into this type. This keeps
/// checkpoint IDs and database encodings stable while centralizing equality,
/// coverage, roster, complete-unit, and terminality rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCommittedFrontier {
    /// Accepted-operation journal position.
    pub journal: FederationJournalFrontier,
    /// AlephBFT finalized-item position.
    pub finalized: FederationFinalizedFrontier,
    /// Terminal-application position.
    pub application: FederationApplicationFrontier,
    /// AlephBFT complete-unit recovery position.
    pub aleph_bft: FederationAlephBftRecoveryCursor,
}

impl FederationCommittedFrontier {
    /// Validate cursor relationships and genesis digest bindings.
    pub fn validate_shape(&self) -> Result<()> {
        self.journal.validate_shape()?;
        self.finalized.validate_shape()?;
        self.application.validate_shape()?;
        if self.application.next_index > self.journal.next_index {
            return Err(checkpoint_invalid(
                "application cursor is ahead of the journal cursor",
            ));
        }
        if self.journal.next_index > self.finalized.next_item_index {
            return Err(checkpoint_invalid(
                "accepted-operation cursor is ahead of the finalized-item cursor",
            ));
        }
        Ok(())
    }

    /// Return whether every accepted operation has a terminal application result.
    #[must_use]
    pub fn is_fully_terminal(&self) -> bool {
        self.application.next_index == self.journal.next_index
    }

    /// Validate this as a terminal complete-unit frontier for the exact roster.
    pub fn validate_complete_unit(&self, config: &FederationConfig) -> Result<()> {
        config.validate()?;
        self.validate_complete_unit_for_validated_config(config)
    }

    /// Validate this as a terminal complete-unit frontier against an already-validated config.
    pub fn validate_complete_unit_for_validated_config(
        &self,
        config: &FederationConfig,
    ) -> Result<()> {
        self.validate_complete_unit_shape()?;
        self.aleph_bft.validate_roster(config)
    }

    fn validate_complete_unit_shape(&self) -> Result<()> {
        self.validate_shape()?;
        if !self.is_fully_terminal() {
            return Err(checkpoint_invalid(
                "checkpoint boundary has non-terminal accepted operations",
            ));
        }
        if self.finalized.next_item_index == 0 || self.aleph_bft.next_unit_finalization_index == 0 {
            return Err(checkpoint_invalid(
                "checkpoint boundary does not contain a complete finalized unit",
            ));
        }
        if self.aleph_bft.next_unit_index_by_member.is_empty() {
            return Err(checkpoint_invalid(
                "checkpoint boundary has no member unit watermarks",
            ));
        }
        Ok(())
    }

    /// Validate that this frontier monotonically covers `current`.
    pub fn ensure_covers(&self, current: &Self) -> Result<()> {
        self.validate_shape()?;
        current.validate_shape()?;
        self.journal.ensure_covers(current.journal)?;
        self.finalized.ensure_covers(current.finalized)?;
        self.application.ensure_covers(current.application)?;
        self.aleph_bft.ensure_covers(&current.aleph_bft)
    }

    /// Return whether all four committed progress dimensions match exactly.
    #[must_use]
    pub fn exactly_matches(&self, other: &Self) -> bool {
        self == other
    }
}

/// Scalar durable checkpoint metadata used by bounded startup validation.
///
/// The three rolling frontiers advance only through contiguous persisted rows.
/// `application_next_index` may lag the journal while accepted operations are
/// being applied, but it can never advance beyond `journal_next_index`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

    /// Project the durable accepted-operation journal position.
    #[must_use]
    pub const fn journal_frontier(&self) -> FederationJournalFrontier {
        FederationJournalFrontier {
            next_index: self.journal_next_index,
            order_digest: self.journal_order_digest,
        }
    }

    /// Project the durable finalized-item position.
    #[must_use]
    pub const fn finalized_frontier(&self) -> FederationFinalizedFrontier {
        FederationFinalizedFrontier {
            next_item_index: self.finalized_next_item_index,
            item_digest: self.finalized_item_digest,
        }
    }

    /// Project the durable terminal-application position.
    #[must_use]
    pub const fn application_frontier(&self) -> FederationApplicationFrontier {
        FederationApplicationFrontier {
            next_index: self.application_next_index,
            digest: self.application_digest,
        }
    }

    /// Project durable history with the separately persisted AlephBFT recovery cursor.
    pub fn committed_frontier(
        &self,
        aleph_bft: FederationAlephBftRecoveryCursor,
    ) -> Result<FederationCommittedFrontier> {
        let frontier = FederationCommittedFrontier {
            journal: self.journal_frontier(),
            finalized: self.finalized_frontier(),
            application: self.application_frontier(),
            aleph_bft,
        };
        frontier.validate_shape()?;
        Ok(frontier)
    }

    /// Validate durable cursor relationships and genesis digest bindings.
    pub fn validate_shape(&self) -> Result<()> {
        self.journal_frontier().validate_shape()?;
        self.finalized_frontier().validate_shape()?;
        self.application_frontier().validate_shape()?;
        if self.application_next_index > self.journal_next_index {
            return Err(checkpoint_invalid(
                "application cursor is ahead of the journal cursor",
            ));
        }
        if self.journal_next_index > self.finalized_next_item_index {
            return Err(checkpoint_invalid(
                "accepted-operation cursor is ahead of the finalized-item cursor",
            ));
        }
        Ok(())
    }

    /// Return whether the durable application cursor has reached the journal cursor.
    #[must_use]
    pub fn is_fully_terminal(&self) -> bool {
        self.application_next_index == self.journal_next_index
    }

    /// Validate that durable history monotonically covers a checkpoint target.
    pub fn ensure_covers_checkpoint(&self, checkpoint: &FederationCheckpoint) -> Result<()> {
        self.validate_shape()?;
        let target = checkpoint.committed_frontier();
        self.journal_frontier().ensure_covers(target.journal)?;
        self.finalized_frontier().ensure_covers(target.finalized)?;
        self.application_frontier()
            .ensure_covers(target.application)
    }

    /// Validate only that durable cursors cover a checkpoint position.
    ///
    /// Database promotion uses this before loading the historical digests at
    /// the checkpoint's earlier cursor. It does not establish digest trust.
    pub fn ensure_covers_checkpoint_position(
        &self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<()> {
        self.validate_shape()?;
        if checkpoint.journal_next_index > self.journal_next_index
            || checkpoint.journal_next_index > self.application_next_index
            || checkpoint.finalized_next_item_index > self.finalized_next_item_index
        {
            return Err(checkpoint_invalid(
                "checkpoint is ahead of durable local frontiers",
            ));
        }
        Ok(())
    }

    /// Return whether durable cursors are exactly at a checkpoint position.
    #[must_use]
    pub fn is_at_checkpoint_position(&self, checkpoint: &FederationCheckpoint) -> bool {
        self.journal_next_index == checkpoint.journal_next_index
            && self.application_next_index == checkpoint.journal_next_index
            && self.finalized_next_item_index == checkpoint.finalized_next_item_index
    }

    /// Return whether all durable history dimensions equal a checkpoint target exactly.
    #[must_use]
    pub fn exactly_matches_checkpoint(&self, checkpoint: &FederationCheckpoint) -> bool {
        let target = checkpoint.committed_frontier();
        self.journal_frontier() == target.journal
            && self.finalized_frontier() == target.finalized
            && self.application_frontier() == target.application
    }

    /// Return whether durable ordered history equals another durable frontier.
    #[must_use]
    pub fn ordered_history_matches(&self, other: &Self) -> bool {
        self.journal_frontier() == other.journal_frontier()
            && self.finalized_frontier() == other.finalized_frontier()
            && self.trusted_checkpoint_id == other.trusted_checkpoint_id
    }
}

/// Exact durable frontier captured after one complete finalized unit is terminally applied.
///
/// Callers construct this while holding the federation operation-application
/// lock. Construction rejects a partial application frontier; the explicit
/// unit cursors make the complete-unit boundary part of the derived checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCheckpointBoundary {
    /// First accepted-operation index after this boundary.
    pub journal_next_index: u64,
    /// Cumulative accepted-operation order digest at this boundary.
    pub journal_order_digest: FederationJournalDigest,
    /// First finalized-item index after this boundary.
    pub finalized_next_item_index: u64,
    /// Cumulative finalized-item digest at this boundary.
    pub finalized_item_digest: FederationFinalizedItemDigest,
    /// Cumulative terminal-application digest at this boundary.
    pub application_digest: FederationApplicationDigest,
    /// AlephBFT session containing the completed unit.
    pub aleph_bft_session: u64,
    /// Next finalized-unit index expected in the active session.
    pub next_unit_finalization_index: u64,
    /// Next unit index expected from each member observed in the active session.
    pub next_unit_index_by_member: BTreeMap<MemberId, u64>,
}

impl FederationCheckpointBoundary {
    /// Project this boundary into the canonical committed frontier.
    #[must_use]
    pub fn committed_frontier(&self) -> FederationCommittedFrontier {
        FederationCommittedFrontier {
            journal: FederationJournalFrontier {
                next_index: self.journal_next_index,
                order_digest: self.journal_order_digest,
            },
            finalized: FederationFinalizedFrontier {
                next_item_index: self.finalized_next_item_index,
                item_digest: self.finalized_item_digest,
            },
            application: FederationApplicationFrontier {
                next_index: self.journal_next_index,
                digest: self.application_digest,
            },
            aleph_bft: FederationAlephBftRecoveryCursor {
                session: self.aleph_bft_session,
                next_unit_finalization_index: self.next_unit_finalization_index,
                next_unit_index_by_member: self.next_unit_index_by_member.clone(),
            },
        }
    }

    /// Capture a terminal durable frontier at an explicitly completed unit boundary.
    pub fn from_complete_unit(
        frontier: &FederationCheckpointFrontier,
        aleph_bft_session: u64,
        next_unit_finalization_index: u64,
        next_unit_index_by_member: BTreeMap<MemberId, u64>,
    ) -> Result<Self> {
        if frontier.materialized_state_audit_mismatch {
            return Err(checkpoint_invalid(
                "materialized-state audit mismatch is latched",
            ));
        }
        frontier.validate_shape()?;
        if !frontier.is_fully_terminal() {
            return Err(checkpoint_invalid(
                "checkpoint boundary has non-terminal accepted operations",
            ));
        }
        let boundary = Self {
            journal_next_index: frontier.journal_next_index,
            journal_order_digest: frontier.journal_order_digest,
            finalized_next_item_index: frontier.finalized_next_item_index,
            finalized_item_digest: frontier.finalized_item_digest,
            application_digest: frontier.application_digest,
            aleph_bft_session,
            next_unit_finalization_index,
            next_unit_index_by_member,
        };
        boundary
            .committed_frontier()
            .validate_complete_unit_shape()?;
        Ok(boundary)
    }

    /// Validate a transported complete-unit boundary against the configured roster.
    pub fn validate_against_config(&self, config: &FederationConfig) -> Result<()> {
        self.committed_frontier().validate_complete_unit(config)
    }

    /// Validate a transported complete-unit boundary against an already-validated config.
    pub fn validate_against_validated_config(&self, config: &FederationConfig) -> Result<()> {
        self.committed_frontier()
            .validate_complete_unit_for_validated_config(config)
    }

    /// Return whether this complete boundary has crossed either interval after the anchor.
    pub fn is_scheduled_eligible(
        &self,
        policy: FederationCheckpointPolicy,
        latest_scheduled: Option<&FederationCheckpoint>,
        batch_canonical_bytes: u64,
    ) -> Result<bool> {
        policy.validate()?;
        if let Some(checkpoint) = latest_scheduled {
            if checkpoint.kind != FederationCheckpointKind::Scheduled {
                return Err(checkpoint_invalid(
                    "scheduled eligibility anchor is not a scheduled checkpoint",
                ));
            }
        }
        let (journal_progress, finalized_progress) = ensure_checkpoint_batch_item_limits(
            self.journal_next_index,
            self.finalized_next_item_index,
            latest_scheduled,
        )?;

        Ok(
            journal_progress >= policy.checkpoint_interval_journal_operations
                || finalized_progress >= policy.checkpoint_interval_finalized_items
                || batch_canonical_bytes >= policy.checkpoint_interval_canonical_bytes,
        )
    }

    /// Build the next scheduled checkpoint at this eligible complete boundary.
    pub fn scheduled_checkpoint(
        &self,
        config: &FederationConfig,
        latest_scheduled: Option<&FederationCheckpoint>,
        materialized_state_digest: FederationMaterializedStateDigest,
        batch_canonical_bytes: u64,
    ) -> Result<FederationCheckpoint> {
        config.validate()?;
        self.scheduled_checkpoint_for_validated_config(
            config,
            latest_scheduled,
            materialized_state_digest,
            batch_canonical_bytes,
        )
    }

    /// Build the next scheduled checkpoint using an already-validated config.
    pub fn scheduled_checkpoint_for_validated_config(
        &self,
        config: &FederationConfig,
        latest_scheduled: Option<&FederationCheckpoint>,
        materialized_state_digest: FederationMaterializedStateDigest,
        batch_canonical_bytes: u64,
    ) -> Result<FederationCheckpoint> {
        if !self.is_scheduled_eligible(
            config.checkpoint_policy,
            latest_scheduled,
            batch_canonical_bytes,
        )? {
            return Err(checkpoint_invalid(
                "complete unit boundary has not crossed a scheduled interval",
            ));
        }
        let (scheduled_checkpoint_sequence, scheduled_anchor_id) = match latest_scheduled {
            Some(checkpoint) => (
                checkpoint
                    .scheduled_checkpoint_sequence
                    .checked_add(1)
                    .ok_or_else(|| checkpoint_invalid("scheduled checkpoint sequence overflow"))?,
                Some(checkpoint.id()?),
            ),
            None => (1, None),
        };
        let checkpoint = self.build_checkpoint_for_validated_config(
            config,
            FederationCheckpointKind::Scheduled,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            materialized_state_digest,
        )?;
        checkpoint.validate_promotion_from(latest_scheduled)?;
        Ok(checkpoint)
    }

    /// Build an on-demand recovery checkpoint without advancing scheduled lineage.
    pub fn recovery_checkpoint(
        &self,
        config: &FederationConfig,
        latest_scheduled: Option<&FederationCheckpoint>,
        materialized_state_digest: FederationMaterializedStateDigest,
    ) -> Result<FederationCheckpoint> {
        config.validate()?;
        self.recovery_checkpoint_for_validated_config(
            config,
            latest_scheduled,
            materialized_state_digest,
        )
    }

    /// Build an on-demand recovery checkpoint using an already-validated config.
    pub fn recovery_checkpoint_for_validated_config(
        &self,
        config: &FederationConfig,
        latest_scheduled: Option<&FederationCheckpoint>,
        materialized_state_digest: FederationMaterializedStateDigest,
    ) -> Result<FederationCheckpoint> {
        if latest_scheduled
            .is_some_and(|checkpoint| checkpoint.kind != FederationCheckpointKind::Scheduled)
        {
            return Err(checkpoint_invalid(
                "recovery anchor is not a scheduled checkpoint",
            ));
        }
        ensure_checkpoint_batch_item_limits(
            self.journal_next_index,
            self.finalized_next_item_index,
            latest_scheduled,
        )?;
        let (scheduled_checkpoint_sequence, scheduled_anchor_id) = match latest_scheduled {
            Some(checkpoint) => (
                checkpoint.scheduled_checkpoint_sequence,
                Some(checkpoint.id()?),
            ),
            None => (0, None),
        };
        let checkpoint = self.build_checkpoint_for_validated_config(
            config,
            FederationCheckpointKind::Recovery,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            materialized_state_digest,
        )?;
        checkpoint.validate_promotion_from(latest_scheduled)?;
        Ok(checkpoint)
    }

    fn build_checkpoint_for_validated_config(
        &self,
        config: &FederationConfig,
        kind: FederationCheckpointKind,
        scheduled_checkpoint_sequence: u64,
        scheduled_anchor_id: Option<FederationCheckpointId>,
        materialized_state_digest: FederationMaterializedStateDigest,
    ) -> Result<FederationCheckpoint> {
        if self
            .next_unit_index_by_member
            .keys()
            .any(|member_id| !config.members.contains_key(member_id))
        {
            return Err(checkpoint_invalid(
                "checkpoint boundary contains an unknown member watermark",
            ));
        }
        let next_unit_index_by_member = config
            .members
            .keys()
            .map(|member_id| {
                (
                    *member_id,
                    self.next_unit_index_by_member
                        .get(member_id)
                        .copied()
                        .unwrap_or(0),
                )
            })
            .collect();
        let checkpoint = FederationCheckpoint {
            version: FEDERATION_CHECKPOINT_VERSION,
            federation_id: config.federation_id,
            federation_config_digest: config.config_digest_for_validated_config()?,
            kind,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            journal_next_index: self.journal_next_index,
            journal_order_digest: self.journal_order_digest,
            finalized_next_item_index: self.finalized_next_item_index,
            finalized_item_digest: self.finalized_item_digest,
            application_digest: self.application_digest,
            aleph_bft_session: self.aleph_bft_session,
            next_unit_finalization_index: self.next_unit_finalization_index,
            next_unit_index_by_member,
            materialized_state_digest,
        };
        checkpoint.validate_against_validated_config(config)?;
        Ok(checkpoint)
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
    /// Project the signed payload into the canonical committed frontier.
    #[must_use]
    pub fn committed_frontier(&self) -> FederationCommittedFrontier {
        FederationCommittedFrontier {
            journal: FederationJournalFrontier {
                next_index: self.journal_next_index,
                order_digest: self.journal_order_digest,
            },
            finalized: FederationFinalizedFrontier {
                next_item_index: self.finalized_next_item_index,
                item_digest: self.finalized_item_digest,
            },
            application: FederationApplicationFrontier {
                next_index: self.journal_next_index,
                digest: self.application_digest,
            },
            aleph_bft: FederationAlephBftRecoveryCursor {
                session: self.aleph_bft_session,
                next_unit_finalization_index: self.next_unit_finalization_index,
                next_unit_index_by_member: self.next_unit_index_by_member.clone(),
            },
        }
    }

    /// Return whether this checkpoint makes the candidate proof redundant.
    #[must_use]
    pub fn covers(&self, candidate: &Self) -> bool {
        self.scheduled_checkpoint_sequence >= candidate.scheduled_checkpoint_sequence
            && self
                .committed_frontier()
                .ensure_covers(&candidate.committed_frontier())
                .is_ok()
    }

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
        config.validate()?;
        self.validate_against_validated_config(config)
    }

    /// Validate payload structure and bindings against an already-validated config.
    pub fn validate_against_validated_config(&self, config: &FederationConfig) -> Result<()> {
        self.validate_shape()?;
        if self.federation_id != config.federation_id {
            return Err(checkpoint_invalid("federation id mismatch"));
        }
        if self.federation_config_digest != config.config_digest_for_validated_config()? {
            return Err(checkpoint_invalid("federation config digest mismatch"));
        }

        self.committed_frontier().aleph_bft.validate_roster(config)
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

        let candidate_frontier = self.committed_frontier();
        let current_frontier = current.committed_frontier();
        candidate_frontier.ensure_covers(&current_frontier)?;
        if candidate_frontier.journal == current_frontier.journal
            && candidate_frontier.finalized == current_frontier.finalized
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

    /// Validate that one checkpoint-bounded transfer stays within item limits.
    pub fn validate_catch_up_batch_from(&self, current: Option<&Self>) -> Result<()> {
        self.validate_promotion_from(current)?;
        ensure_checkpoint_batch_item_limits(
            self.journal_next_index,
            self.finalized_next_item_index,
            current,
        )?;
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
        self.committed_frontier().validate_complete_unit_shape()
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

    /// Verify the bounded proof and return authority that cannot be constructed unchecked.
    pub fn verify(&self, config: &FederationConfig) -> Result<VerifiedFederationCheckpointQuorum> {
        config.validate()?;
        self.verify_against_validated_config(config)
    }

    /// Verify the bounded proof against an already-validated federation config.
    pub fn verify_against_validated_config(
        &self,
        config: &FederationConfig,
    ) -> Result<VerifiedFederationCheckpointQuorum> {
        // This method skips keyset and setup-transcript validation, but it still
        // establishes every config invariant that grants checkpoint authority.
        // In particular, malformed thresholds must never manufacture an empty
        // `VerifiedFederationCheckpointQuorum`.
        let expected_federation_id = FederationConfig::derive_federation_id(
            config.setup_authorization,
            config.threshold_params,
            &config.members,
        )?;
        if expected_federation_id != config.federation_id {
            return Err(FederationError::FederationIdMismatch {
                expected: expected_federation_id,
                found: config.federation_id,
            });
        }
        self.checkpoint.validate_against_validated_config(config)?;
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

        Ok(VerifiedFederationCheckpointQuorum {
            quorum: self.clone(),
            signers,
        })
    }
}

/// Cryptographically verified authority to trust or promote one checkpoint quorum.
///
/// The raw [`FederationCheckpointQuorum`] remains the serialized transport and
/// storage type. This wrapper has private fields, does not implement
/// deserialization, and is created only by [`FederationCheckpointQuorum::verify`].
///
/// ```compile_fail
/// use std::collections::BTreeSet;
/// use cdk_common::federation::{
///     FederationCheckpointQuorum, VerifiedFederationCheckpointQuorum,
/// };
///
/// fn bypass(raw: FederationCheckpointQuorum) -> VerifiedFederationCheckpointQuorum {
///     VerifiedFederationCheckpointQuorum {
///         quorum: raw,
///         signers: BTreeSet::new(),
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFederationCheckpointQuorum {
    quorum: FederationCheckpointQuorum,
    signers: BTreeSet<MemberId>,
}

impl VerifiedFederationCheckpointQuorum {
    /// Return the verified raw proof for persistence or transport.
    #[must_use]
    pub const fn as_quorum(&self) -> &FederationCheckpointQuorum {
        &self.quorum
    }

    /// Return the verified checkpoint payload.
    #[must_use]
    pub const fn checkpoint(&self) -> &FederationCheckpoint {
        &self.quorum.checkpoint
    }

    /// Return the distinct configured signers that formed the verified threshold.
    #[must_use]
    pub const fn signers(&self) -> &BTreeSet<MemberId> {
        &self.signers
    }

    /// Consume the authority wrapper and return its raw proof.
    #[must_use]
    pub fn into_quorum(self) -> FederationCheckpointQuorum {
        self.quorum
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
        FederationConsensusItem, FederationConsensusModuleFact, ThresholdParams,
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
        let config = FederationConfig::new(
            crate::federation::FederationSetupAuthorizationHash::from_bytes([1; 32]),
            params,
            members,
            Vec::new(),
        )
        .expect("config");
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

    fn complete_boundary(
        config: &FederationConfig,
        journal_next_index: u64,
        finalized_next_item_index: u64,
    ) -> FederationCheckpointBoundary {
        let frontier = FederationCheckpointFrontier {
            journal_next_index,
            journal_order_digest: FederationJournalDigest::from_bytes([11; 32]),
            finalized_next_item_index,
            finalized_item_digest: FederationFinalizedItemDigest::from_bytes([12; 32]),
            application_next_index: journal_next_index,
            application_digest: FederationApplicationDigest::from_bytes([13; 32]),
            trusted_checkpoint_id: None,
            latest_audited_checkpoint_id: None,
            latest_materialized_state_digest: None,
            materialized_state_audit_mismatch: false,
        };
        FederationCheckpointBoundary::from_complete_unit(
            &frontier,
            2,
            4,
            config
                .members
                .keys()
                .copied()
                .map(|member_id| (member_id, u64::from(member_id.get())))
                .collect(),
        )
        .expect("complete checkpoint boundary")
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
            item: FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
                "test",
                "checkpoint-test",
                json!({"b": 2, "a": 1}),
            )),
        }
    }

    #[test]
    fn committed_frontier_centralizes_exact_coverage_and_complete_unit_rules() {
        let (config, _) = configs();
        let checkpoint = checkpoint(&config);
        let base = checkpoint.committed_frontier();
        assert!(base.exactly_matches(&base));
        base.ensure_covers(&base).expect("reflexive coverage");
        base.validate_complete_unit(&config)
            .expect("checkpoint complete-unit frontier");

        let boundary = FederationCheckpointBoundary {
            journal_next_index: checkpoint.journal_next_index,
            journal_order_digest: checkpoint.journal_order_digest,
            finalized_next_item_index: checkpoint.finalized_next_item_index,
            finalized_item_digest: checkpoint.finalized_item_digest,
            application_digest: checkpoint.application_digest,
            aleph_bft_session: checkpoint.aleph_bft_session,
            next_unit_finalization_index: checkpoint.next_unit_finalization_index,
            next_unit_index_by_member: checkpoint.next_unit_index_by_member.clone(),
        };
        assert!(base.exactly_matches(&boundary.committed_frontier()));

        let mut advanced = base.clone();
        advanced.journal.next_index += 1;
        advanced.journal.order_digest = FederationJournalDigest::from_bytes([21; 32]);
        advanced.finalized.next_item_index += 1;
        advanced.finalized.item_digest = FederationFinalizedItemDigest::from_bytes([22; 32]);
        advanced.application.next_index += 1;
        advanced.application.digest = FederationApplicationDigest::from_bytes([23; 32]);
        advanced.aleph_bft.next_unit_finalization_index += 1;
        for unit_index in advanced.aleph_bft.next_unit_index_by_member.values_mut() {
            *unit_index += 1;
        }
        advanced.ensure_covers(&base).expect("monotonic coverage");
        assert!(base.ensure_covers(&advanced).is_err());

        for changed in [
            {
                let mut changed = base.clone();
                changed.journal.order_digest = FederationJournalDigest::from_bytes([31; 32]);
                changed
            },
            {
                let mut changed = base.clone();
                changed.finalized.item_digest = FederationFinalizedItemDigest::from_bytes([32; 32]);
                changed
            },
            {
                let mut changed = base.clone();
                changed.application.digest = FederationApplicationDigest::from_bytes([33; 32]);
                changed
            },
        ] {
            assert!(changed.ensure_covers(&base).is_err());
        }

        let mut application_lag = base.clone();
        application_lag.application.next_index -= 1;
        assert!(!application_lag.is_fully_terminal());
        assert!(application_lag.validate_complete_unit(&config).is_err());

        let mut incomplete_roster = base;
        incomplete_roster
            .aleph_bft
            .next_unit_index_by_member
            .remove(&member_id(1));
        assert!(incomplete_roster.validate_complete_unit(&config).is_err());
    }

    #[test]
    fn committed_frontier_serialization_and_checkpoint_projection_round_trip() {
        let (config, _) = configs();
        let checkpoint = checkpoint(&config);
        let checkpoint_id = checkpoint.id().expect("checkpoint id");
        let frontier = checkpoint.committed_frontier();
        let encoded = serde_json::to_vec(&frontier).expect("serialize committed frontier");
        let decoded: FederationCommittedFrontier =
            serde_json::from_slice(&encoded).expect("deserialize committed frontier");
        assert_eq!(decoded, frontier);

        let checkpoint_encoded = serde_json::to_vec(&checkpoint).expect("serialize checkpoint");
        let checkpoint_decoded: FederationCheckpoint =
            serde_json::from_slice(&checkpoint_encoded).expect("deserialize checkpoint");
        assert_eq!(
            checkpoint_decoded.id().expect("decoded checkpoint id"),
            checkpoint_id
        );
        assert_eq!(checkpoint_decoded.committed_frontier(), frontier);

        let summary = crate::federation::FederationCheckpointSummary::from_checkpoint(&checkpoint)
            .expect("checkpoint summary");
        let summary_encoded = serde_json::to_vec(&summary).expect("serialize checkpoint summary");
        let summary_decoded: crate::federation::FederationCheckpointSummary =
            serde_json::from_slice(&summary_encoded).expect("deserialize checkpoint summary");
        assert_eq!(summary_decoded, summary);
        assert!(summary.covers(&summary_decoded));

        let mut conflicting_summary = summary_decoded;
        conflicting_summary.checkpoint_id = FederationCheckpointId::from_bytes([99; 32]);
        conflicting_summary.frontier.journal.order_digest =
            FederationJournalDigest::from_bytes([98; 32]);
        assert!(!summary.covers(&conflicting_summary));
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
                "a4c7c40f068a7f7e3eb431237c494e59ebb329167ab6a5a5123c8c70d8115628",
                "6f98c1caffcb0b8c7970d6b697c821e0f46949fce43c74c4e349a10e38ccfb3d",
                "c36c288970f8857e797be007431d5ad82755a9e01b980b4a2b7c4740d322a6c9",
            )
        );
    }

    #[test]
    fn scheduled_eligibility_requires_terminal_complete_unit_progress() {
        assert_eq!(
            FederationCheckpointPolicy::default(),
            FederationCheckpointPolicy {
                checkpoint_interval_journal_operations: 10_000,
                checkpoint_interval_finalized_items: 10_000,
                checkpoint_interval_canonical_bytes: DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES,
            }
        );
        assert!(FederationCheckpointPolicy::new(0, 10).is_err());
        assert!(FederationCheckpointPolicy::new(10, 0).is_err());
        assert!(FederationCheckpointPolicy::new(
            FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS.saturating_add(1),
            10,
        )
        .is_err());
        let invalid_byte_policy = FederationCheckpointPolicy {
            checkpoint_interval_canonical_bytes: FEDERATION_MAX_CHECKPOINT_BATCH_CANONICAL_BYTES
                .saturating_add(1),
            ..Default::default()
        };
        assert!(invalid_byte_policy.validate().is_err());

        let (mut config, _) = configs();
        config.checkpoint_policy = FederationCheckpointPolicy::new(5, 10).expect("policy");
        let mut frontier = FederationCheckpointFrontier::genesis();
        frontier.journal_next_index = 5;
        frontier.journal_order_digest = FederationJournalDigest::from_bytes([11; 32]);
        frontier.finalized_next_item_index = 9;
        frontier.finalized_item_digest = FederationFinalizedItemDigest::from_bytes([12; 32]);
        frontier.application_next_index = 4;
        frontier.application_digest = FederationApplicationDigest::from_bytes([13; 32]);
        let watermarks = config
            .members
            .keys()
            .copied()
            .map(|member_id| (member_id, 0))
            .collect();

        assert!(
            FederationCheckpointBoundary::from_complete_unit(&frontier, 0, 1, watermarks).is_err()
        );

        let before = complete_boundary(&config, 4, 9);
        assert!(!before
            .is_scheduled_eligible(config.checkpoint_policy, None, 0)
            .expect("eligibility before interval"));
        let journal_eligible = complete_boundary(&config, 6, 9);
        assert!(journal_eligible
            .is_scheduled_eligible(config.checkpoint_policy, None, 0)
            .expect("journal eligibility"));
        let finalized_eligible = complete_boundary(&config, 4, 12);
        assert!(finalized_eligible
            .is_scheduled_eligible(config.checkpoint_policy, None, 0)
            .expect("finalized eligibility"));
        assert!(before
            .is_scheduled_eligible(
                config.checkpoint_policy,
                None,
                DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES,
            )
            .expect("canonical byte eligibility"));
        assert!(!before
            .is_scheduled_eligible(
                config.checkpoint_policy,
                None,
                DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES - 1,
            )
            .expect("canonical bytes immediately before eligibility"));

        let mut oversized = checkpoint(&config);
        oversized.journal_next_index =
            FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS.saturating_add(1);
        oversized.finalized_next_item_index = oversized.journal_next_index;
        assert!(matches!(
            oversized.validate_catch_up_batch_from(None),
            Err(FederationError::CheckpointInvalid { .. })
        ));
    }

    #[test]
    fn transported_complete_boundary_requires_the_exact_roster() {
        let (config, _) = configs();
        let boundary = complete_boundary(&config, 4, 9);
        boundary
            .validate_against_config(&config)
            .expect("boundary validates");
        let encoded = serde_json::to_vec(&boundary).expect("serialize boundary");
        let decoded: FederationCheckpointBoundary =
            serde_json::from_slice(&encoded).expect("deserialize boundary");
        assert_eq!(decoded, boundary);

        let mut missing_member = boundary;
        let member_id = *config.members.keys().next().expect("roster member");
        missing_member.next_unit_index_by_member.remove(&member_id);
        assert!(matches!(
            missing_member.validate_against_config(&config),
            Err(FederationError::CheckpointInvalid { .. })
        ));
    }

    #[test]
    fn boundary_builders_keep_recovery_out_of_scheduled_lineage() {
        let (mut config, _) = configs();
        config.checkpoint_policy = FederationCheckpointPolicy::new(5, 10).expect("policy");
        let first_boundary = complete_boundary(&config, 6, 12);
        let audit = FederationMaterializedStateDigest::from_bytes([14; 32]);
        let first = first_boundary
            .scheduled_checkpoint(&config, None, audit, 0)
            .expect("first scheduled checkpoint");
        let recovery_boundary = complete_boundary(&config, 8, 14);
        let recovery = recovery_boundary
            .recovery_checkpoint(&config, Some(&first), audit)
            .expect("recovery checkpoint");
        let second_boundary = complete_boundary(&config, 12, 22);
        let second = second_boundary
            .scheduled_checkpoint(&config, Some(&first), audit, 0)
            .expect("second scheduled checkpoint");

        assert_eq!(first.scheduled_checkpoint_sequence, 1);
        assert_eq!(recovery.scheduled_checkpoint_sequence, 1);
        assert_eq!(
            recovery.scheduled_anchor_id,
            Some(first.id().expect("first id"))
        );
        assert_eq!(second.scheduled_checkpoint_sequence, 2);
        assert_eq!(
            second.scheduled_anchor_id,
            Some(first.id().expect("first id"))
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
        reordered_payload.item = FederationConsensusItem::ModuleFact(
            FederationConsensusModuleFact::new("test", "checkpoint-test", json!({"a": 1, "b": 2})),
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
        assert_eq!(quorum.verify(&config).expect("quorum").signers().len(), 3);

        let insufficient =
            FederationCheckpointQuorum::new(checkpoint.clone(), signatures[..2].to_vec());
        assert!(matches!(
            insufficient.verify_against_validated_config(&config),
            Err(FederationError::CheckpointQuorumNotReached {
                required: 3,
                found: 2
            })
        ));

        let mut zero_threshold = config.clone();
        zero_threshold.threshold_params.consensus_threshold = 0;
        let empty = FederationCheckpointQuorum::new(checkpoint.clone(), Vec::new());
        assert!(matches!(
            empty.verify_against_validated_config(&zero_threshold),
            Err(FederationError::InvalidThresholdParams { .. })
        ));

        let duplicate = FederationCheckpointQuorum::new(
            checkpoint.clone(),
            vec![signatures[0], signatures[0], signatures[1]],
        );
        assert!(matches!(
            duplicate.verify_against_validated_config(&config),
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
            corrupted.verify_against_validated_config(&config),
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
            unknown.verify_against_validated_config(&config),
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
            other_config.setup_authorization,
            other_config.threshold_params,
            &other_config.members,
        )
        .expect("other federation id");
        assert!(checkpoint(&config)
            .validate_against_config(&other_config)
            .is_err());
    }

    #[test]
    fn validated_config_checkpoint_validation_rejects_tampered_bindings_and_roster() {
        let (config, _) = configs();
        let original = checkpoint(&config);

        let mut wrong_federation = original.clone();
        wrong_federation.federation_id = FederationId::from_bytes([91; 32]);
        assert!(matches!(
            wrong_federation.validate_against_validated_config(&config),
            Err(FederationError::CheckpointInvalid { reason })
                if reason == "federation id mismatch"
        ));

        let mut wrong_digest = original.clone();
        wrong_digest.federation_config_digest = FederationConfigDigest::from_bytes([92; 32]);
        assert!(matches!(
            wrong_digest.validate_against_validated_config(&config),
            Err(FederationError::CheckpointInvalid { reason })
                if reason == "federation config digest mismatch"
        ));

        let mut missing_member = original.clone();
        missing_member
            .next_unit_index_by_member
            .remove(&member_id(4));
        assert!(matches!(
            missing_member.validate_against_validated_config(&config),
            Err(FederationError::CheckpointInvalid { reason })
                if reason == "next-unit watermarks do not match the configured federation roster"
        ));

        let mut unknown_member = original;
        unknown_member
            .next_unit_index_by_member
            .insert(member_id(9), 0);
        assert!(matches!(
            unknown_member.validate_against_validated_config(&config),
            Err(FederationError::CheckpointInvalid { reason })
                if reason == "next-unit watermarks do not match the configured federation roster"
        ));
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
