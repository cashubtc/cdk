//! Dealer-free DKG ceremony, participant-state, and transcript types.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::config::{
    FederatedKeyset, FederationConfig, FederationId, MemberConfigPrivate, MemberConfigPublic,
    MemberId, ThresholdParams,
};
use super::transport::{
    FederationDkgActivationRequest, FederationDkgCommitmentRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgTranscriptSignatureRequest,
};
use super::{
    append_bytes, FederationError, Result, FEDERATION_DKG_COMMITMENT_HASH_VERSION,
    FEDERATION_DKG_PARTICIPANT_STATE_VERSION, FEDERATION_DKG_PUBLIC_CEREMONY_VERSION,
    FEDERATION_DKG_TRANSCRIPT_HASH_VERSION, FEDERATION_DKG_TRANSCRIPT_SIGNATURE_VERSION,
    FEDERATION_MAX_DKG_KEYSET_AMOUNTS, FEDERATION_MAX_DKG_MEMBERS,
};
use crate::nuts::nut01::{
    aggregate_public_key_commitments, aggregate_secret_key_shares, evaluate_public_key_commitments,
    evaluate_secret_key_coefficients, BlsAggregatePublicKey, BlsPublicKeyShare, BlsSecretKey,
    BlsSecretKeyShare,
};
use crate::{Amount, CurrencyUnit, Id, PublicKey};

/// Validate a public configuration as the immutable base of one DKG ceremony.
///
/// An empty keyset/transcript pair denotes bootstrap. A configuration with
/// validated keysets denotes an explicit rotation ceremony: its roster,
/// threshold, identities, payment policy, and wallet metadata remain bound to
/// every DKG record, while the newly derived BLS keyset is only a candidate
/// result until a separate activation path accepts it.
pub fn validate_federation_dkg_base_config(public_config: &FederationConfig) -> Result<()> {
    public_config.validate()?;
    if public_config.keysets.is_empty() {
        if public_config.setup_transcript.is_some() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG bootstrap config has a transcript without keysets".to_string(),
            });
        }
        return Ok(());
    }
    if !public_config.keysets.iter().any(|keyset| keyset.active) {
        return Err(FederationError::DkgTranscriptMismatch {
            reason: "DKG rotation requires an active validated base keyset".to_string(),
        });
    }
    Ok(())
}

/// Public DKG-derived keyset plus one member's local secret BLS shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgMemberKeysetShare {
    /// Federation roster this completed DKG keyset belongs to.
    pub federation_id: FederationId,
    /// Local federation member that owns the secret shares.
    pub member_id: MemberId,
    /// Public aggregate keyset derived from all revealed DKG commitments.
    pub keyset: FederatedKeyset,
    /// Complete public ceremony that produced this keyset.
    pub public_ceremony: FederationDkgPublicCeremony,
    /// Final local secret BLS share per amount for one federation member.
    pub secret_shares: BTreeMap<Amount, BlsSecretKeyShare>,
}

impl FederationDkgMemberKeysetShare {
    /// Hash locally derived durable DKG output without reevaluating public polynomials.
    ///
    /// This is the hot-path cache boundary used while collecting transcript
    /// signatures. Callers must supply records produced locally by
    /// [`FederationDkgCommitmentRound::derive_member_keyset_share`]. Final
    /// startup activation and config export still use
    /// [`FederationDkgFinalizedMemberConfig::from_records`] to independently
    /// rederive and verify all public shares.
    pub fn transcript_hash_for_locally_derived_records(
        setup_public_config: &FederationConfig,
        keyset_shares: &[Self],
    ) -> Result<FederationDkgTranscriptHash> {
        validate_federation_dkg_base_config(setup_public_config)?;
        let local_member = keyset_shares
            .first()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "missing local DKG keyset share before transcript signature".to_string(),
            })?
            .member_id;
        let mut keysets_by_id = BTreeMap::new();
        let mut ceremonies_by_keyset_id = BTreeMap::new();
        for share in keyset_shares {
            if share.federation_id != setup_public_config.federation_id {
                return Err(FederationError::FederationIdMismatch {
                    expected: setup_public_config.federation_id,
                    found: share.federation_id,
                });
            }
            if share.member_id != local_member {
                return Err(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                });
            }
            share
                .public_ceremony
                .validate_against_config(setup_public_config)?;
            share
                .keyset
                .validate_metadata(&setup_public_config.members)?;
            let keyset_spec = &share
                .public_ceremony
                .reveals
                .values()
                .next()
                .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                    reason: "locally derived DKG ceremony has no reveals".to_string(),
                })?
                .keyset_spec;
            let keyset_amounts = share
                .keyset
                .aggregate_keys
                .keys()
                .copied()
                .collect::<BTreeSet<_>>();
            if share.keyset.unit != keyset_spec.unit
                || share.keyset.active != keyset_spec.active
                || share.keyset.input_fee_ppk != keyset_spec.input_fee_ppk
                || share.keyset.final_expiry != keyset_spec.final_expiry
                || keyset_amounts != keyset_spec.amounts
            {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "locally derived DKG keyset policy mismatch for {}",
                        share.keyset.id
                    ),
                });
            }
            match keysets_by_id.insert(share.keyset.id, share.keyset.clone()) {
                Some(existing) if existing != share.keyset => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("conflicting local DKG keyset {}", share.keyset.id),
                    });
                }
                Some(_) | None => {}
            }
            match ceremonies_by_keyset_id.insert(share.keyset.id, share.public_ceremony.clone()) {
                Some(existing) if existing != share.public_ceremony => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting local public DKG ceremony for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let mut final_public_config = setup_public_config.clone();
        final_public_config.keysets = keysets_by_id.into_values().collect();
        final_public_config.setup_transcript = None;
        final_public_config.validate_wallet_runtime_metadata()?;
        let public_ceremonies = ceremonies_by_keyset_id.into_values().collect::<Vec<_>>();
        let payload = FederationDkgTranscript::payload_from_canonical_ceremonies(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
            public_ceremonies,
        );
        FederationDkgTranscriptHash::for_payload(&payload)
    }
}
/// Unique identifier that domain-separates one dealer-free DKG ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgCeremonyId([u8; 32]);

impl FederationDkgCeremonyId {
    /// Create a ceremony ID from 32 bytes of operator-provided entropy.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a ceremony ID from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgCeremonyIdSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a ceremony ID from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return the ceremony ID as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return the ceremony ID as lowercase hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationDkgCeremonyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgCeremonyId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgCeremonyId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgCeremonyId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Public keyset policy agreed before a dealer-free DKG ceremony starts.
///
/// Every participant commits to this specification together with its public
/// polynomial commitments. A federation therefore cannot complete a ceremony
/// when members disagree about the unit, fees, expiry, activation state, or
/// supported denominations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgKeysetSpec {
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether the completed keyset should sign new outputs.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    pub final_expiry: Option<u64>,
    /// Denominations for which every participant generates a polynomial.
    pub amounts: BTreeSet<Amount>,
}

impl FederationDkgKeysetSpec {
    /// Create and validate a dealer-free DKG keyset specification.
    pub fn new(
        unit: CurrencyUnit,
        active: bool,
        input_fee_ppk: u64,
        final_expiry: Option<u64>,
        amounts: BTreeSet<Amount>,
    ) -> Result<Self> {
        let spec = Self {
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            amounts,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Validate that this ceremony creates a bounded, non-empty denomination set.
    pub fn validate(&self) -> Result<()> {
        if self.amounts.is_empty() {
            return Err(FederationError::EmptyDkgKeyset {
                unit: self.unit.clone(),
            });
        }
        if self.amounts.len() > FEDERATION_MAX_DKG_KEYSET_AMOUNTS {
            return Err(FederationError::DkgKeysetAmountLimitExceeded {
                found: self.amounts.len(),
                max: FEDERATION_MAX_DKG_KEYSET_AMOUNTS,
            });
        }
        Ok(())
    }

    /// Validate this keyset together with the bounded dealer-free DKG roster.
    pub fn validate_for_federation(&self, public_config: &FederationConfig) -> Result<()> {
        self.validate()?;
        if public_config.members.len() > FEDERATION_MAX_DKG_MEMBERS {
            return Err(FederationError::DkgMemberLimitExceeded {
                found: public_config.members.len(),
                max: FEDERATION_MAX_DKG_MEMBERS,
            });
        }
        Ok(())
    }
}

/// Durable lifecycle state for one dealer-free DKG ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationDkgCeremonyState {
    /// The ceremony may accept setup messages and advance its driver.
    Active,
    /// The ceremony was explicitly replaced before completion.
    Aborted,
    /// Every configured member confirmed the finalized configuration.
    Complete,
}

impl FederationDkgCeremonyState {
    /// Return the stable operator-facing state label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Aborted => "aborted",
            Self::Complete => "complete",
        }
    }
}

/// Durable operator and driver record for one dealer-free DKG ceremony.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgCeremonyRecord {
    /// Federation roster and threshold this ceremony belongs to.
    pub federation_id: FederationId,
    /// Unique ceremony identifier.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Local member that owns this database record.
    pub member_id: MemberId,
    /// Exact keyset policy committed by the ceremony.
    pub keyset_spec: FederationDkgKeysetSpec,
    /// Current durable lifecycle state.
    pub state: FederationDkgCeremonyState,
    /// Explicit predecessor replaced by this ceremony, when any.
    pub replaces_ceremony_id: Option<FederationDkgCeremonyId>,
    /// Explicit successor that caused this ceremony to abort, when any.
    pub replacement_ceremony_id: Option<FederationDkgCeremonyId>,
}

impl FederationDkgCeremonyRecord {
    /// Create a new active ceremony record.
    #[must_use]
    pub fn active(
        federation_id: FederationId,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
        replaces_ceremony_id: Option<FederationDkgCeremonyId>,
    ) -> Self {
        Self {
            federation_id,
            ceremony_id,
            member_id,
            keyset_spec,
            state: FederationDkgCeremonyState::Active,
            replaces_ceremony_id,
            replacement_ceremony_id: None,
        }
    }

    /// Return the exact aborted successor state for an active record.
    pub fn aborted_by(&self, replacement_ceremony_id: FederationDkgCeremonyId) -> Result<Self> {
        if self.state != FederationDkgCeremonyState::Active
            || self.replacement_ceremony_id.is_some()
            || replacement_ceremony_id == self.ceremony_id
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG ceremony {} cannot transition from {} to aborted by {}",
                    self.ceremony_id,
                    self.state.as_str(),
                    replacement_ceremony_id
                ),
            });
        }
        let mut aborted = self.clone();
        aborted.state = FederationDkgCeremonyState::Aborted;
        aborted.replacement_ceremony_id = Some(replacement_ceremony_id);
        Ok(aborted)
    }

    /// Return the exact completed successor state for an active record.
    pub fn completed(&self) -> Result<Self> {
        if self.state != FederationDkgCeremonyState::Active
            || self.replacement_ceremony_id.is_some()
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "DKG ceremony {} cannot transition from {} to complete",
                    self.ceremony_id,
                    self.state.as_str()
                ),
            });
        }
        let mut completed = self.clone();
        completed.state = FederationDkgCeremonyState::Complete;
        Ok(completed)
    }
}

/// Stable hash of one member's revealed DKG polynomial commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgCommitmentHash([u8; 32]);

impl FederationDkgCommitmentHash {
    /// Create a commitment hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a commitment hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgCommitmentHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a commitment hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this commitment hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this commitment hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn for_reveal(reveal: &FederationDkgParticipantCommitmentReveal) -> Result<Self> {
        let canonical = serde_json::to_vec(reveal)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_DKG_COMMITMENT_HASH_VERSION);
        append_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationDkgCommitmentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgCommitmentHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgCommitmentHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgCommitmentHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// A member's pre-reveal DKG commitment hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgParticipantCommitment {
    /// Federation roster hash this commitment belongs to.
    pub federation_id: FederationId,
    /// Unique DKG ceremony this commitment belongs to.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Member that committed to later reveal material.
    pub member_id: MemberId,
    /// Hash of the member's later public commitment reveal.
    pub commitment_hash: FederationDkgCommitmentHash,
}

impl FederationDkgParticipantCommitment {
    /// Build a commitment from a participant reveal.
    pub fn from_reveal(reveal: &FederationDkgParticipantCommitmentReveal) -> Result<Self> {
        Ok(Self {
            federation_id: reveal.federation_id,
            ceremony_id: reveal.ceremony_id,
            member_id: reveal.member_id,
            commitment_hash: reveal.commitment_hash()?,
        })
    }

    /// Validate this commitment against the federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "federation id mismatch".to_string(),
            });
        }
        let member =
            public_config
                .member(self.member_id)
                .ok_or(FederationError::UnknownMember {
                    member_id: self.member_id,
                })?;
        if member.member_id != self.member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: self.member_id,
                found: member.member_id,
            });
        }

        Ok(())
    }

    /// Validate that a reveal matches this earlier commitment.
    pub fn validate_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        public_config: &FederationConfig,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        reveal.validate_against_config(public_config)?;
        if reveal.member_id != self.member_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "reveal member id mismatch".to_string(),
            });
        }
        if reveal.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "ceremony id mismatch".to_string(),
            });
        }
        let reveal_hash = reveal.commitment_hash()?;
        if reveal_hash != self.commitment_hash {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "reveal hash mismatch".to_string(),
            });
        }

        Ok(())
    }
}

/// Revealed public DKG polynomial commitments for one participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgParticipantCommitmentReveal {
    /// Federation roster hash this reveal belongs to.
    pub federation_id: FederationId,
    /// Unique DKG ceremony this reveal belongs to.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Member that reveals public polynomial commitments.
    pub member_id: MemberId,
    /// Keyset policy and denomination set committed by this participant.
    pub keyset_spec: FederationDkgKeysetSpec,
    /// Per-amount public polynomial commitments in G2.
    ///
    /// The vector length for each amount must equal the signature threshold.
    /// Entry 0 is the participant's public constant-term commitment.
    pub coefficient_commitments: BTreeMap<Amount, Vec<BlsAggregatePublicKey>>,
}

impl FederationDkgParticipantCommitmentReveal {
    /// Return the commitment hash that should have been announced before reveal.
    pub fn commitment_hash(&self) -> Result<FederationDkgCommitmentHash> {
        FederationDkgCommitmentHash::for_reveal(self)
    }

    /// Validate this reveal against the federation roster and threshold.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "federation id mismatch".to_string(),
            });
        }
        if !public_config.members.contains_key(&self.member_id) {
            return Err(FederationError::UnknownMember {
                member_id: self.member_id,
            });
        }
        self.keyset_spec.validate_for_federation(public_config)?;
        if self.coefficient_commitments.is_empty() {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "missing coefficient commitments".to_string(),
            });
        }

        let committed_amounts = self
            .coefficient_commitments
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if committed_amounts != self.keyset_spec.amounts {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "keyset specification amount set mismatch".to_string(),
            });
        }

        let expected_coefficients = usize::from(public_config.threshold_params.signature_threshold);
        for (amount, commitments) in &self.coefficient_commitments {
            if commitments.len() != expected_coefficients {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "coefficient commitment count mismatch for amount {amount}: expected={expected_coefficients}, found={}",
                        commitments.len()
                    ),
                });
            }
        }

        Ok(())
    }

    /// Verify one private contribution against this participant's revealed polynomial.
    pub fn validate_secret_share_request(
        &self,
        public_config: &FederationConfig,
        request: &FederationDkgSecretShareRequest,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        request.validate_sender_receiver(public_config, self.member_id, request.receiver)?;
        if request.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!(
                    "secret contribution ceremony id mismatch for receiver {}",
                    request.receiver
                ),
            });
        }
        let request_amounts = request
            .secret_shares
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if request_amounts != self.keyset_spec.amounts {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!(
                    "secret contribution amount set mismatch for receiver {}",
                    request.receiver
                ),
            });
        }
        for (amount, secret_share) in &request.secret_shares {
            let commitments = self.coefficient_commitments.get(amount).ok_or(
                FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!("missing public commitments for amount {amount}"),
                },
            )?;
            let expected_share =
                evaluate_public_key_commitments(request.receiver.to_bls_signer_id(), commitments)?;
            if secret_share.public_key_share() != expected_share {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "secret contribution does not match public commitments for receiver {} amount {amount}",
                        request.receiver
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Private, restartable state owned by one dealer-free DKG participant.
///
/// Generation creates one random polynomial per denomination and immediately
/// evaluates it for every configured receiver. Only the public reveal and the
/// already-evaluated per-recipient contributions remain in this state; the
/// polynomial coefficients are dropped and zeroized before generation returns.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgParticipantState {
    version: u16,
    federation_id: FederationId,
    ceremony_id: FederationDkgCeremonyId,
    member_id: MemberId,
    pub(super) reveal: FederationDkgParticipantCommitmentReveal,
    pub(super) secret_share_requests: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
}

impl fmt::Debug for FederationDkgParticipantState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FederationDkgParticipantState")
            .field("version", &self.version)
            .field("federation_id", &self.federation_id)
            .field("ceremony_id", &self.ceremony_id)
            .field("member_id", &self.member_id)
            .field("keyset_spec", &self.reveal.keyset_spec)
            .field("receiver_count", &self.secret_share_requests.len())
            .finish_non_exhaustive()
    }
}

impl FederationDkgParticipantState {
    /// Generate private state for one cooperative dealer-free DKG participant.
    pub fn generate(
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Result<Self> {
        let state =
            Self::generate_by_construction(public_config, ceremony_id, member_id, keyset_spec)?;
        state.validate(public_config)?;
        Ok(state)
    }

    pub(super) fn generate_by_construction(
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Result<Self> {
        public_config.validate()?;
        if !public_config.members.contains_key(&member_id) {
            return Err(FederationError::MissingMember { member_id });
        }
        keyset_spec.validate_for_federation(public_config)?;

        let coefficient_count = usize::from(public_config.threshold_params.signature_threshold);
        let coefficients = keyset_spec
            .amounts
            .iter()
            .map(|amount| {
                let coefficients = (0..coefficient_count)
                    .map(|_| BlsSecretKey::generate())
                    .collect::<Vec<_>>();
                (*amount, coefficients)
            })
            .collect::<BTreeMap<_, _>>();
        let coefficient_commitments = coefficients
            .iter()
            .map(|(amount, coefficients)| {
                (
                    *amount,
                    coefficients
                        .iter()
                        .map(BlsSecretKey::public_key_g2)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let reveal = FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            ceremony_id,
            member_id,
            keyset_spec,
            coefficient_commitments,
        };

        let mut secret_share_requests = BTreeMap::new();
        for receiver in public_config.members.keys().copied() {
            let secret_shares = coefficients
                .iter()
                .map(|(amount, coefficients)| {
                    Ok((
                        *amount,
                        evaluate_secret_key_coefficients(
                            receiver.to_bls_signer_id(),
                            coefficients,
                        )?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            secret_share_requests.insert(
                receiver,
                FederationDkgSecretShareRequest::new(
                    public_config.federation_id,
                    ceremony_id,
                    member_id,
                    receiver,
                    secret_shares,
                ),
            );
        }

        Ok(Self {
            version: FEDERATION_DKG_PARTICIPANT_STATE_VERSION,
            federation_id: public_config.federation_id,
            ceremony_id,
            member_id,
            reveal,
            secret_share_requests,
        })
    }

    /// Validate private participant state before resuming a ceremony.
    pub fn validate(&self, public_config: &FederationConfig) -> Result<()> {
        public_config.validate()?;
        if self.version != FEDERATION_DKG_PARTICIPANT_STATE_VERSION {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: format!("unsupported participant state version {}", self.version),
            });
        }
        if self.federation_id != public_config.federation_id
            || self.reveal.federation_id != self.federation_id
        {
            return Err(FederationError::FederationIdMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }
        if self.reveal.ceremony_id != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "participant state ceremony id mismatch".to_string(),
            });
        }
        if self.reveal.member_id != self.member_id {
            return Err(FederationError::MemberIdMismatch {
                expected: self.member_id,
                found: self.reveal.member_id,
            });
        }
        self.reveal.validate_against_config(public_config)?;

        let expected_receivers = public_config
            .members
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let receivers = self
            .secret_share_requests
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if receivers != expected_receivers {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: self.member_id,
                reason: "participant receiver set mismatch".to_string(),
            });
        }

        for (receiver, request) in &self.secret_share_requests {
            if request.receiver != *receiver {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: self.member_id,
                    reason: format!(
                        "secret contribution receiver mismatch: expected={receiver}, found={}",
                        request.receiver
                    ),
                });
            }
            self.reveal
                .validate_secret_share_request(public_config, request)?;
        }

        Ok(())
    }

    /// Return this participant's member ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Return the ceremony ID bound to every participant message.
    #[must_use]
    pub fn ceremony_id(&self) -> FederationDkgCeremonyId {
        self.ceremony_id
    }

    /// Return the keyset specification committed by this participant.
    #[must_use]
    pub fn keyset_spec(&self) -> &FederationDkgKeysetSpec {
        &self.reveal.keyset_spec
    }

    /// Build this participant's first-round commitment request.
    pub fn commitment_request(&self) -> Result<FederationDkgCommitmentRequest> {
        Ok(FederationDkgCommitmentRequest::new(
            FederationDkgParticipantCommitment::from_reveal(&self.reveal)?,
        ))
    }

    /// Build this participant's public commitment-reveal request.
    #[must_use]
    pub fn reveal_request(&self) -> FederationDkgRevealRequest {
        FederationDkgRevealRequest::new(self.reveal.clone())
    }

    /// Return the private contribution addressed to one configured receiver.
    pub fn secret_share_request(
        &self,
        receiver: MemberId,
    ) -> Result<FederationDkgSecretShareRequest> {
        self.secret_share_requests
            .get(&receiver)
            .cloned()
            .ok_or(FederationError::MissingMember {
                member_id: receiver,
            })
    }
}

/// DKG commitment round collected before participants reveal public commitments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgCommitmentRound {
    /// Federation roster hash for this setup round.
    pub federation_id: FederationId,
    /// Unique DKG ceremony represented by this round.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Deterministic participant order.
    pub participant_order: Vec<MemberId>,
    /// Pre-reveal commitment hashes by member.
    pub commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
}

impl FederationDkgCommitmentRound {
    /// Create and validate a commitment round for a public federation config.
    pub fn new(
        public_config: &FederationConfig,
        commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
    ) -> Result<Self> {
        let ceremony_id = commitments
            .values()
            .next()
            .ok_or(FederationError::DkgCommitmentRoundMismatch {
                reason: "missing commitments".to_string(),
            })?
            .ceremony_id;
        let round = Self {
            federation_id: public_config.federation_id,
            ceremony_id,
            participant_order: public_config.members.keys().copied().collect(),
            commitments,
        };
        round.validate_against_config(public_config)?;
        Ok(round)
    }

    /// Validate this commitment round against the federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "federation id mismatch".to_string(),
            });
        }
        let expected_order = public_config.members.keys().copied().collect::<Vec<_>>();
        if self.participant_order != expected_order {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "participant order mismatch".to_string(),
            });
        }
        for member_id in self.commitments.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }
        if self.commitments.len() != public_config.members.len() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "commitment count mismatch".to_string(),
            });
        }
        for member_id in public_config.members.keys() {
            let commitment = self.commitments.get(member_id).ok_or(
                FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing commitment for member {member_id}"),
                },
            )?;
            if commitment.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: commitment.member_id,
                });
            }
            if commitment.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "commitment ceremony id mismatch".to_string(),
                });
            }
            commitment.validate_against_config(public_config)?;
        }

        Ok(())
    }

    /// Validate revealed participant commitments against this earlier commitment round.
    pub fn validate_reveals(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<()> {
        self.validate_against_config(public_config)?;
        for member_id in reveals.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }
        if reveals.len() != public_config.members.len() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "reveal count mismatch".to_string(),
            });
        }
        for member_id in public_config.members.keys() {
            let reveal =
                reveals
                    .get(member_id)
                    .ok_or(FederationError::DkgCommitmentRoundMismatch {
                        reason: format!("missing reveal for member {member_id}"),
                    })?;
            let commitment = self.commitments.get(member_id).ok_or(
                FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing commitment for member {member_id}"),
                },
            )?;
            commitment.validate_reveal(reveal, public_config)?;
            if reveal.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "reveal ceremony id mismatch".to_string(),
                });
            }
        }

        let first_member =
            self.participant_order
                .first()
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: "empty participant order".to_string(),
                })?;
        let expected_spec = &reveals
            .get(first_member)
            .ok_or(FederationError::DkgCommitmentRoundMismatch {
                reason: format!("missing reveal for member {first_member}"),
            })?
            .keyset_spec;
        for (member_id, reveal) in reveals {
            if &reveal.keyset_spec != expected_spec {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "keyset specification mismatch".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Derive a federated aggregate keyset from a fully revealed DKG round.
    ///
    /// Every participant contributes a public polynomial commitment for each
    /// amount. The final aggregate mint key for an amount is the sum of all
    /// constant-term commitments, and each member's public share is the sum of
    /// every participant polynomial evaluated at that member ID.
    pub fn derive_federated_keyset(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<FederatedKeyset> {
        self.validate_reveals(public_config, reveals)?;
        let first_member_id =
            self.participant_order
                .first()
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: "empty participant order".to_string(),
                })?;
        let first_reveal =
            reveals
                .get(first_member_id)
                .ok_or(FederationError::DkgCommitmentRoundMismatch {
                    reason: format!("missing reveal for member {first_member_id}"),
                })?;
        let keyset_spec = &first_reveal.keyset_spec;

        let mut aggregate_keys = BTreeMap::new();
        let mut public_shares = public_config
            .members
            .keys()
            .map(|member_id| (*member_id, BTreeMap::new()))
            .collect::<BTreeMap<_, _>>();
        for amount in &keyset_spec.amounts {
            let coefficient_count = usize::from(public_config.threshold_params.signature_threshold);
            let aggregate_coefficients = (0..coefficient_count)
                .map(|coefficient_index| {
                    let participant_coefficients = self
                        .participant_order
                        .iter()
                        .map(|participant_id| {
                            let commitments = dkg_reveal_amount_commitments(
                                reveals,
                                *participant_id,
                                *amount,
                            )?;
                            commitments.get(coefficient_index).copied().ok_or(
                                FederationError::DkgCommitmentMismatch {
                                    member_id: *participant_id,
                                    reason: format!(
                                        "missing coefficient commitment {coefficient_index} for amount {amount}"
                                    ),
                                },
                            )
                        })
                        .collect::<Result<Vec<_>>>()?;
                    aggregate_public_key_commitments(&participant_coefficients)
                        .map_err(Into::into)
                })
                .collect::<Result<Vec<_>>>()?;
            aggregate_keys.insert(
                *amount,
                *aggregate_coefficients.first().ok_or(
                    FederationError::DkgCommitmentRoundMismatch {
                        reason: format!("empty aggregate polynomial for amount {amount}"),
                    },
                )?,
            );

            for member_id in public_config.members.keys() {
                let signer_id = member_id.to_bls_signer_id();
                let public_share =
                    evaluate_public_key_commitments(signer_id, &aggregate_coefficients)?;
                public_shares
                    .get_mut(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?
                    .insert(*amount, public_share);
            }
        }

        let keyset = FederatedKeyset::new(
            keyset_spec.unit.clone(),
            keyset_spec.active,
            keyset_spec.input_fee_ppk,
            keyset_spec.final_expiry,
            aggregate_keys,
            public_shares,
        );
        keyset.validate(public_config.threshold_params, &public_config.members)?;
        Ok(keyset)
    }

    /// Derive one member's final local secret shares for a DKG keyset.
    ///
    /// `participant_secret_shares` contains the private same-signer scalar
    /// contribution received from each DKG participant. Each contribution is
    /// checked against that participant's revealed public polynomial
    /// commitments before the final local share is aggregated.
    pub fn derive_member_keyset_share(
        &self,
        public_config: &FederationConfig,
        reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
        member_id: MemberId,
        participant_secret_shares: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    ) -> Result<FederationDkgMemberKeysetShare> {
        if !public_config.members.contains_key(&member_id) {
            return Err(FederationError::MissingMember { member_id });
        }
        for (participant_id, request) in participant_secret_shares {
            request.validate_sender_receiver(public_config, *participant_id, member_id)?;
            if request.ceremony_id != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *participant_id,
                    reason: "secret contribution belongs to another ceremony".to_string(),
                });
            }
        }
        let keyset = self.derive_federated_keyset(public_config, reveals)?;
        let signer_id = member_id.to_bls_signer_id();
        let public_member_shares =
            keyset
                .public_shares
                .get(&member_id)
                .ok_or(FederationError::MissingPublicShares {
                    keyset_id: keyset.id,
                    member_id,
                })?;

        let mut secret_shares = BTreeMap::new();
        for amount in keyset.aggregate_keys.keys() {
            let mut participant_shares = Vec::with_capacity(self.participant_order.len());
            for participant_id in &self.participant_order {
                let secret_share = dkg_participant_secret_share(
                    participant_secret_shares,
                    *participant_id,
                    *amount,
                )?;
                if secret_share.signer_id() != signer_id {
                    return Err(FederationError::SecretShareSignerMismatch {
                        keyset_id: keyset.id,
                        member_id,
                        signer_id: secret_share.signer_id().get(),
                    });
                }

                let commitments = dkg_reveal_amount_commitments(reveals, *participant_id, *amount)?;
                let expected_public_share =
                    evaluate_public_key_commitments(signer_id, commitments)?;
                if secret_share.public_key_share() != expected_public_share {
                    return Err(FederationError::DkgCommitmentMismatch {
                        member_id: *participant_id,
                        reason: format!("local secret contribution mismatch for amount {amount}"),
                    });
                }
                participant_shares.push(secret_share.clone());
            }

            let secret_share = aggregate_secret_key_shares(signer_id, &participant_shares)?;
            let public_share = public_member_shares.get(amount).ok_or(
                FederationError::MissingPublicShareAmount {
                    keyset_id: keyset.id,
                    member_id,
                    amount: *amount,
                },
            )?;
            if secret_share.public_key_share() != *public_share {
                return Err(FederationError::PrivateShareMismatch {
                    keyset_id: keyset.id,
                    member_id,
                    amount: *amount,
                });
            }
            secret_shares.insert(*amount, secret_share);
        }

        let public_ceremony =
            FederationDkgPublicCeremony::new(public_config, self.clone(), reveals.clone())?;
        Ok(FederationDkgMemberKeysetShare {
            federation_id: public_config.federation_id,
            member_id,
            keyset,
            public_ceremony,
            secret_shares,
        })
    }
}

/// Complete public record of one dealer-free DKG ceremony.
///
/// This deliberately excludes private scalar contributions. The commitment
/// round and every reveal are sufficient to audit the commit-before-reveal
/// exchange and deterministically rederive the aggregate keyset and all public
/// member shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgPublicCeremony {
    /// Public ceremony record format version.
    pub version: u16,
    /// Pre-reveal commitment round for every federation member.
    pub commitment_round: FederationDkgCommitmentRound,
    /// Revealed public polynomial commitments for every federation member.
    pub reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
}

impl FederationDkgPublicCeremony {
    /// Build and validate a complete public DKG ceremony record.
    pub fn new(
        public_config: &FederationConfig,
        commitment_round: FederationDkgCommitmentRound,
        reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    ) -> Result<Self> {
        let ceremony = Self {
            version: FEDERATION_DKG_PUBLIC_CEREMONY_VERSION,
            commitment_round,
            reveals,
        };
        ceremony.validate_against_config(public_config)?;
        Ok(ceremony)
    }

    /// Return the unique ID that domain-separates this ceremony.
    #[must_use]
    pub fn ceremony_id(&self) -> FederationDkgCeremonyId {
        self.commitment_round.ceremony_id
    }

    /// Validate the full commitment/reveal exchange against a federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.version != FEDERATION_DKG_PUBLIC_CEREMONY_VERSION {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "public ceremony version mismatch: expected={}, found={}",
                    FEDERATION_DKG_PUBLIC_CEREMONY_VERSION, self.version
                ),
            });
        }
        self.commitment_round
            .validate_reveals(public_config, &self.reveals)
    }

    /// Rederive the public aggregate keyset committed by this ceremony.
    pub fn derive_federated_keyset(
        &self,
        public_config: &FederationConfig,
    ) -> Result<FederatedKeyset> {
        self.validate_against_config(public_config)?;
        self.commitment_round
            .derive_federated_keyset(public_config, &self.reveals)
    }
}

fn dkg_reveal_amount_commitments(
    reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    member_id: MemberId,
    amount: Amount,
) -> Result<&[BlsAggregatePublicKey]> {
    let reveal = reveals
        .get(&member_id)
        .ok_or(FederationError::DkgCommitmentRoundMismatch {
            reason: format!("missing reveal for member {member_id}"),
        })?;
    let commitments = reveal.coefficient_commitments.get(&amount).ok_or(
        FederationError::DkgCommitmentMismatch {
            member_id,
            reason: format!("missing coefficient commitments for amount {amount}"),
        },
    )?;
    Ok(commitments)
}

fn dkg_participant_secret_share(
    participant_secret_shares: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    member_id: MemberId,
    amount: Amount,
) -> Result<&BlsSecretKeyShare> {
    let contribution = participant_secret_shares.get(&member_id).ok_or(
        FederationError::DkgCommitmentMismatch {
            member_id,
            reason: "missing local secret contributions".to_string(),
        },
    )?;
    let secret_share =
        contribution
            .secret_shares
            .get(&amount)
            .ok_or(FederationError::DkgCommitmentMismatch {
                member_id,
                reason: format!("missing local secret contribution for amount {amount}"),
            })?;
    Ok(secret_share)
}

/// In-memory DKG setup state collected from authenticated member setup messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgSetupState {
    /// Federation roster hash for this setup state.
    pub federation_id: FederationId,
    /// Ceremony accepted by the first commitment, if setup has started.
    pub ceremony_id: Option<FederationDkgCeremonyId>,
    /// Deterministic participant order.
    pub participant_order: Vec<MemberId>,
    /// Pre-reveal commitment hashes collected by member.
    pub commitments: BTreeMap<MemberId, FederationDkgParticipantCommitment>,
    /// Revealed public polynomial commitments collected by member.
    pub reveals: BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
}

impl FederationDkgSetupState {
    /// Create an empty DKG setup state for a public federation config.
    #[must_use]
    pub fn new(public_config: &FederationConfig) -> Self {
        Self {
            federation_id: public_config.federation_id,
            ceremony_id: None,
            participant_order: public_config.members.keys().copied().collect(),
            commitments: BTreeMap::new(),
            reveals: BTreeMap::new(),
        }
    }

    /// Validate this setup state against the federation roster.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        if self.federation_id != public_config.federation_id {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "federation id mismatch".to_string(),
            });
        }
        let expected_order = public_config.members.keys().copied().collect::<Vec<_>>();
        if self.participant_order != expected_order {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "participant order mismatch".to_string(),
            });
        }
        if self.commitments.is_empty() && self.ceremony_id.is_some() {
            return Err(FederationError::DkgCommitmentRoundMismatch {
                reason: "setup state has a ceremony id without commitments".to_string(),
            });
        }
        for (member_id, commitment) in &self.commitments {
            if commitment.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: commitment.member_id,
                });
            }
            if Some(commitment.ceremony_id) != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "setup-state commitment ceremony id mismatch".to_string(),
                });
            }
            commitment.validate_against_config(public_config)?;
        }
        for (member_id, reveal) in &self.reveals {
            if reveal.member_id != *member_id {
                return Err(FederationError::MemberIdMismatch {
                    expected: *member_id,
                    found: reveal.member_id,
                });
            }
            if Some(reveal.ceremony_id) != self.ceremony_id {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: *member_id,
                    reason: "setup-state reveal ceremony id mismatch".to_string(),
                });
            }
            reveal.validate_against_config(public_config)?;
            let commitment =
                self.commitments
                    .get(member_id)
                    .ok_or(FederationError::DkgCommitmentMismatch {
                        member_id: *member_id,
                        reason: "missing prior commitment".to_string(),
                    })?;
            commitment.validate_reveal(reveal, public_config)?;
        }

        Ok(())
    }

    /// Insert a member commitment.
    ///
    /// Returns `true` when the state changed and `false` for an idempotent duplicate.
    pub fn insert_commitment(
        &mut self,
        public_config: &FederationConfig,
        commitment: FederationDkgParticipantCommitment,
    ) -> Result<bool> {
        self.validate_against_config(public_config)?;
        commitment.validate_against_config(public_config)?;
        if let Some(existing) = self.commitments.get(&commitment.member_id) {
            if existing == &commitment {
                return Ok(false);
            }
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: commitment.member_id,
                reason: "conflicting commitment".to_string(),
            });
        }
        match self.ceremony_id {
            Some(ceremony_id) if ceremony_id != commitment.ceremony_id => {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: commitment.member_id,
                    reason: "commitment belongs to another ceremony".to_string(),
                });
            }
            Some(_) => {}
            None => self.ceremony_id = Some(commitment.ceremony_id),
        }
        self.commitments.insert(commitment.member_id, commitment);
        Ok(true)
    }

    /// Insert a member reveal after its matching commitment is known.
    ///
    /// Returns `true` when the state changed and `false` for an idempotent duplicate.
    pub fn insert_reveal(
        &mut self,
        public_config: &FederationConfig,
        reveal: FederationDkgParticipantCommitmentReveal,
    ) -> Result<bool> {
        self.validate_against_config(public_config)?;
        reveal.validate_against_config(public_config)?;
        if let Some(existing) = self.reveals.get(&reveal.member_id) {
            if existing == &reveal {
                return Ok(false);
            }
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "conflicting reveal".to_string(),
            });
        }
        let commitment = self.commitments.get(&reveal.member_id).ok_or(
            FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "missing prior commitment".to_string(),
            },
        )?;
        if Some(reveal.ceremony_id) != self.ceremony_id {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: reveal.member_id,
                reason: "reveal belongs to another ceremony".to_string(),
            });
        }
        commitment.validate_reveal(&reveal, public_config)?;
        self.reveals.insert(reveal.member_id, reveal);
        Ok(true)
    }

    /// Return whether all member commitments have been collected.
    #[must_use]
    pub fn commitments_complete(&self, public_config: &FederationConfig) -> bool {
        self.commitments.len() == public_config.members.len()
    }

    /// Return whether all member reveals have been collected.
    #[must_use]
    pub fn reveals_complete(&self, public_config: &FederationConfig) -> bool {
        self.reveals.len() == public_config.members.len()
    }

    /// Return the fully validated round once all commitments and reveals are present.
    pub fn complete_round(
        &self,
        public_config: &FederationConfig,
    ) -> Result<Option<FederationDkgCommitmentRound>> {
        self.validate_against_config(public_config)?;
        if !self.commitments_complete(public_config) || !self.reveals_complete(public_config) {
            return Ok(None);
        }
        let round = FederationDkgCommitmentRound::new(public_config, self.commitments.clone())?;
        round.validate_reveals(public_config, &self.reveals)?;
        Ok(Some(round))
    }
}

/// Stable hash of the public federation DKG/setup transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationDkgTranscriptHash([u8; 32]);

impl FederationDkgTranscriptHash {
    /// Create a transcript hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a transcript hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgTranscriptHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transcript hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this transcript hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this transcript hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn for_payload(payload: &FederationDkgTranscriptPayload) -> Result<Self> {
        let canonical = serde_json::to_vec(payload)?;
        let mut transcript = Vec::new();
        append_bytes(&mut transcript, FEDERATION_DKG_TRANSCRIPT_HASH_VERSION);
        append_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationDkgTranscriptHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationDkgTranscriptHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgTranscriptHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgTranscriptHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Schnorr identity signature over a DKG/setup transcript hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationDkgTranscriptSignature([u8; 64]);

impl FederationDkgTranscriptSignature {
    /// Create a transcript signature from raw Schnorr bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidDkgTranscriptSignatureSize {
                    expected: 64,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transcript signature from hex.
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

    /// Sign a DKG/setup transcript hash with one member identity key.
    pub fn sign(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::sign_with(
            transcript_hash,
            private_config,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    /// Sign a DKG/setup transcript hash without auxiliary randomness.
    pub fn sign_no_aux_rand(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        Self::sign_with(
            transcript_hash,
            private_config,
            FederationSchnorrSigningMode::NoAuxRand,
        )
    }

    fn sign_with(
        transcript_hash: FederationDkgTranscriptHash,
        private_config: &MemberConfigPrivate,
        mode: FederationSchnorrSigningMode,
    ) -> Result<Self> {
        let transcript = dkg_signature_transcript(transcript_hash);
        let signature = match mode {
            FederationSchnorrSigningMode::RandomAux => {
                private_config.identity_secret_key.sign(&transcript)?
            }
            FederationSchnorrSigningMode::NoAuxRand => private_config
                .identity_secret_key
                .sign_no_aux_rand(&transcript)?,
        };
        Ok(Self(signature.serialize()))
    }

    /// Verify this signature against one member identity public key.
    pub fn verify(
        self,
        transcript_hash: FederationDkgTranscriptHash,
        member: &MemberConfigPublic,
    ) -> Result<()> {
        let signature = Signature::from_slice(&self.0)
            .map_err(|_| FederationError::InvalidDkgTranscriptSignature)?;
        member
            .identity_public_key
            .verify(&dkg_signature_transcript(transcript_hash), &signature)
            .map_err(
                |_| FederationError::DkgTranscriptSignatureVerificationFailed {
                    member_id: member.member_id,
                },
            )
    }
}

impl fmt::Display for FederationDkgTranscriptSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederationSchnorrSigningMode {
    RandomAux,
    NoAuxRand,
}

impl FromStr for FederationDkgTranscriptSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationDkgTranscriptSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationDkgTranscriptSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Key generation method recorded by a federation setup transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationDkgSetupMethod {
    /// Test/development setup where one process locally generated all shares.
    TrustedDealer,
    /// Dealer-free Pedersen DKG setup with a complete public ceremony record.
    PedersenDkg,
}

/// Public keyset material committed by a DKG/setup transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgKeysetTranscript {
    /// Federated keyset ID.
    pub keyset_id: Id,
    /// Keyset currency unit.
    pub unit: CurrencyUnit,
    /// Whether the keyset signs new outputs.
    pub active: bool,
    /// Input fee in parts per thousand.
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp.
    pub final_expiry: Option<u64>,
    /// Aggregate BLS public key per amount.
    pub aggregate_keys: BTreeMap<Amount, BlsAggregatePublicKey>,
    /// Public BLS shares per member and amount.
    pub public_shares: BTreeMap<MemberId, BTreeMap<Amount, BlsPublicKeyShare>>,
}

impl FederationDkgKeysetTranscript {
    fn federated_keyset(&self) -> FederatedKeyset {
        FederatedKeyset {
            id: self.keyset_id,
            unit: self.unit.clone(),
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
            final_expiry: self.final_expiry,
            aggregate_keys: self.aggregate_keys.clone(),
            public_shares: self.public_shares.clone(),
        }
    }

    fn matches_immutable_keyset_material(&self, keyset: &FederatedKeyset) -> bool {
        self.keyset_id == keyset.id
            && self.unit == keyset.unit
            && self.input_fee_ppk == keyset.input_fee_ppk
            && self.final_expiry == keyset.final_expiry
            && self.aggregate_keys == keyset.aggregate_keys
            && self.public_shares == keyset.public_shares
    }
}

/// Public, signed DKG/setup transcript payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgTranscriptPayload {
    /// Setup method used to derive the public key material.
    pub setup_method: FederationDkgSetupMethod,
    /// Federation ID bound by this transcript.
    pub federation_id: FederationId,
    /// Threshold parameters bound by this transcript.
    pub threshold_params: ThresholdParams,
    /// Deterministic member order used as DKG participant order.
    pub participant_order: Vec<MemberId>,
    /// Member identity public keys bound by this transcript.
    pub member_identity_public_keys: BTreeMap<MemberId, PublicKey>,
    /// Public aggregate keysets and public member shares.
    pub keysets: Vec<FederationDkgKeysetTranscript>,
    /// Complete public ceremonies, one per Pedersen-DKG keyset in keyset order.
    pub ceremonies: Vec<FederationDkgPublicCeremony>,
}

/// Signed public DKG/setup transcript for audit and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDkgTranscript {
    /// Public transcript payload.
    pub payload: FederationDkgTranscriptPayload,
    /// Canonical hash of the public payload.
    pub transcript_hash: FederationDkgTranscriptHash,
    /// Identity signatures over the transcript hash.
    pub signatures: BTreeMap<MemberId, FederationDkgTranscriptSignature>,
}

impl FederationDkgTranscript {
    /// Build an unsigned public transcript payload from a public config.
    fn payload_from_config(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<FederationDkgTranscriptPayload> {
        let ceremonies = Self::canonical_ceremonies(setup_method, public_config, ceremonies)?;
        Ok(Self::payload_from_canonical_ceremonies(
            setup_method,
            public_config,
            ceremonies,
        ))
    }

    fn payload_from_canonical_ceremonies(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: Vec<FederationDkgPublicCeremony>,
    ) -> FederationDkgTranscriptPayload {
        FederationDkgTranscriptPayload {
            setup_method,
            federation_id: public_config.federation_id,
            threshold_params: public_config.threshold_params,
            participant_order: public_config.members.keys().copied().collect(),
            member_identity_public_keys: public_config
                .members
                .iter()
                .map(|(member_id, member)| (*member_id, member.identity_public_key))
                .collect(),
            keysets: public_config
                .keysets
                .iter()
                .map(|keyset| FederationDkgKeysetTranscript {
                    keyset_id: keyset.id,
                    unit: keyset.unit.clone(),
                    active: keyset.active,
                    input_fee_ppk: keyset.input_fee_ppk,
                    final_expiry: keyset.final_expiry,
                    aggregate_keys: keyset.aggregate_keys.clone(),
                    public_shares: keyset.public_shares.clone(),
                })
                .collect(),
            ceremonies,
        }
    }

    /// Derive the transcript hash that members must sign for this setup method and config.
    ///
    /// Trusted-dealer setup requires an empty ceremony slice. Pedersen DKG
    /// requires exactly one complete, keyset-matching ceremony per public keyset.
    pub fn hash_for_config(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<FederationDkgTranscriptHash> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
        FederationDkgTranscriptHash::for_payload(&payload)
    }

    /// Create a transcript from externally collected member signatures.
    ///
    /// The ceremony requirements are the same as [`Self::hash_for_config`].
    pub fn from_signatures(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        signatures: BTreeMap<MemberId, FederationDkgTranscriptSignature>,
    ) -> Result<Self> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
        let transcript_hash = FederationDkgTranscriptHash::for_payload(&payload)?;
        let transcript = Self {
            payload,
            transcript_hash,
            signatures,
        };
        transcript.validate_against_config(public_config)?;
        Ok(transcript)
    }

    /// Create a signed transcript from already-generated trusted-dealer material.
    pub fn trusted_dealer(
        public_config: &FederationConfig,
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::TrustedDealer,
            public_config,
            &[],
            private_configs,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    /// Create a trusted-dealer transcript with reproducible no-aux-rand signatures.
    pub fn trusted_dealer_no_aux_rand(
        public_config: &FederationConfig,
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::TrustedDealer,
            public_config,
            &[],
            private_configs,
            FederationSchnorrSigningMode::NoAuxRand,
        )
    }

    /// Create a signed transcript from dealer-free Pedersen DKG public material.
    pub fn pedersen_dkg(
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    ) -> Result<Self> {
        Self::signed_setup_with_mode(
            FederationDkgSetupMethod::PedersenDkg,
            public_config,
            ceremonies,
            private_configs,
            FederationSchnorrSigningMode::RandomAux,
        )
    }

    fn signed_setup_with_mode(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
        private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
        mode: FederationSchnorrSigningMode,
    ) -> Result<Self> {
        let payload = Self::payload_from_config(setup_method, public_config, ceremonies)?;
        let transcript_hash = FederationDkgTranscriptHash::for_payload(&payload)?;
        let mut signatures = BTreeMap::new();
        for member_id in payload.member_identity_public_keys.keys() {
            let private_config =
                private_configs
                    .get(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?;
            signatures.insert(
                *member_id,
                FederationDkgTranscriptSignature::sign_with(transcript_hash, private_config, mode)?,
            );
        }

        let transcript = Self {
            payload,
            transcript_hash,
            signatures,
        };
        transcript.validate_against_config(public_config)?;
        Ok(transcript)
    }

    fn canonical_ceremonies(
        setup_method: FederationDkgSetupMethod,
        public_config: &FederationConfig,
        ceremonies: &[FederationDkgPublicCeremony],
    ) -> Result<Vec<FederationDkgPublicCeremony>> {
        match setup_method {
            FederationDkgSetupMethod::TrustedDealer => {
                if !ceremonies.is_empty() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: "trusted-dealer transcript contains DKG ceremonies".to_string(),
                    });
                }
                Ok(Vec::new())
            }
            FederationDkgSetupMethod::PedersenDkg => {
                if ceremonies.is_empty() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: "Pedersen DKG transcript is missing public ceremonies".to_string(),
                    });
                }
                if ceremonies.len() != public_config.keysets.len() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "Pedersen DKG ceremony count mismatch: expected={}, found={}",
                            public_config.keysets.len(),
                            ceremonies.len()
                        ),
                    });
                }

                let mut ceremony_ids = BTreeSet::new();
                let mut ceremonies_by_keyset = BTreeMap::new();
                for ceremony in ceremonies {
                    if !ceremony_ids.insert(ceremony.ceremony_id()) {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!("duplicate DKG ceremony id {}", ceremony.ceremony_id()),
                        });
                    }
                    let keyset = ceremony.derive_federated_keyset(public_config)?;
                    let keyset_id = keyset.id;
                    if ceremonies_by_keyset
                        .insert(keyset_id, (keyset, ceremony.clone()))
                        .is_some()
                    {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!("duplicate DKG ceremony for keyset {keyset_id}"),
                        });
                    }
                }

                let mut canonical = Vec::with_capacity(public_config.keysets.len());
                for expected_keyset in &public_config.keysets {
                    let (derived_keyset, ceremony) = ceremonies_by_keyset
                        .remove(&expected_keyset.id)
                        .ok_or(FederationError::DkgTranscriptMismatch {
                            reason: format!(
                                "missing public DKG ceremony for keyset {}",
                                expected_keyset.id
                            ),
                        })?;
                    if &derived_keyset != expected_keyset {
                        return Err(FederationError::DkgTranscriptMismatch {
                            reason: format!(
                                "public DKG ceremony does not derive keyset {}",
                                expected_keyset.id
                            ),
                        });
                    }
                    canonical.push(ceremony);
                }
                if let Some(keyset_id) = ceremonies_by_keyset.keys().next() {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("unexpected public DKG ceremony for keyset {keyset_id}"),
                    });
                }
                Ok(canonical)
            }
        }
    }

    /// Validate this transcript against a public federation config.
    pub fn validate_against_config(&self, public_config: &FederationConfig) -> Result<()> {
        let expected_hash = FederationDkgTranscriptHash::for_payload(&self.payload)?;
        if expected_hash != self.transcript_hash {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "transcript hash does not match payload".to_string(),
            });
        }
        if self.payload.federation_id != public_config.federation_id {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "federation id mismatch".to_string(),
            });
        }
        if self.payload.threshold_params != public_config.threshold_params {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "threshold parameter mismatch".to_string(),
            });
        }

        let expected_order = public_config.members.keys().copied().collect::<Vec<_>>();
        if self.payload.participant_order != expected_order {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "participant order mismatch".to_string(),
            });
        }
        let expected_identity_keys = public_config
            .members
            .iter()
            .map(|(member_id, member)| (*member_id, member.identity_public_key))
            .collect::<BTreeMap<_, _>>();
        if self.payload.member_identity_public_keys != expected_identity_keys {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "member identity public keys mismatch".to_string(),
            });
        }

        let expected_keysets = public_config
            .keysets
            .iter()
            .map(|keyset| FederationDkgKeysetTranscript {
                keyset_id: keyset.id,
                unit: keyset.unit.clone(),
                active: keyset.active,
                input_fee_ppk: keyset.input_fee_ppk,
                final_expiry: keyset.final_expiry,
                aggregate_keys: keyset.aggregate_keys.clone(),
                public_shares: keyset.public_shares.clone(),
            })
            .collect::<Vec<_>>();
        if self.payload.keysets != expected_keysets {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "keyset public material mismatch".to_string(),
            });
        }
        let canonical_ceremonies = Self::canonical_ceremonies(
            self.payload.setup_method,
            public_config,
            &self.payload.ceremonies,
        )?;
        if self.payload.ceremonies != canonical_ceremonies {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "public DKG ceremonies are not in keyset order".to_string(),
            });
        }

        for member_id in public_config.members.keys() {
            let member =
                public_config
                    .members
                    .get(member_id)
                    .ok_or(FederationError::MissingMember {
                        member_id: *member_id,
                    })?;
            let signature =
                self.signatures
                    .get(member_id)
                    .ok_or(FederationError::DkgTranscriptMismatch {
                        reason: format!("missing transcript signature for member {member_id}"),
                    })?;
            signature.verify(self.transcript_hash, member)?;
        }
        for member_id in self.signatures.keys() {
            if !public_config.members.contains_key(member_id) {
                return Err(FederationError::UnknownMember {
                    member_id: *member_id,
                });
            }
        }

        Ok(())
    }

    /// Validate this historical setup transcript against a later runtime config.
    ///
    /// The signed payload is verified exactly against the setup-time keyset
    /// snapshot. Consensus key rotations may subsequently change activation
    /// flags and append keysets, but may not remove or mutate any immutable
    /// setup key material.
    pub fn validate_setup_lineage_against_config(
        &self,
        public_config: &FederationConfig,
    ) -> Result<()> {
        let mut setup_config = public_config.clone();
        setup_config.keysets = self
            .payload
            .keysets
            .iter()
            .map(FederationDkgKeysetTranscript::federated_keyset)
            .collect();
        setup_config.setup_transcript = None;
        setup_config.frost_wallet = None;
        setup_config.validate()?;
        self.validate_against_config(&setup_config)?;

        for setup_keyset in &self.payload.keysets {
            let current_keyset = public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == setup_keyset.keyset_id)
                .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "setup keyset {} is missing from current config",
                        setup_keyset.keyset_id
                    ),
                })?;
            if !setup_keyset.matches_immutable_keyset_material(current_keyset) {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "setup keyset {} public material changed",
                        setup_keyset.keyset_id
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Final public and local-private member config material assembled from DKG records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgFinalizedMemberConfig {
    /// Public federation config with DKG-derived keysets and setup transcript.
    pub public_config: FederationConfig,
    /// Local private member config with DKG-derived secret shares and transcript binding.
    pub private_config: MemberConfigPrivate,
}

impl FederationDkgFinalizedMemberConfig {
    /// Assemble final member config material from durable DKG completion records.
    pub fn from_records(
        setup_public_config: &FederationConfig,
        local_private_config: &MemberConfigPrivate,
        keyset_shares: &[FederationDkgMemberKeysetShare],
        transcript_signature_records: &[FederationDkgTranscriptSignatureRequest],
    ) -> Result<Self> {
        setup_public_config.validate()?;
        let local_member = local_private_config.member_id;
        if !setup_public_config.members.contains_key(&local_member) {
            return Err(FederationError::MissingMember {
                member_id: local_member,
            });
        }

        let mut keysets_by_id = BTreeMap::new();
        let mut ceremonies_by_keyset_id = BTreeMap::new();
        let mut secret_shares_by_keyset = BTreeMap::new();
        for share in keyset_shares {
            if share.federation_id != setup_public_config.federation_id {
                return Err(FederationError::FederationIdMismatch {
                    expected: setup_public_config.federation_id,
                    found: share.federation_id,
                });
            }
            if share.member_id != local_member {
                return Err(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                });
            }
            let ceremony_keyset = share
                .public_ceremony
                .derive_federated_keyset(setup_public_config)?;
            if ceremony_keyset != share.keyset {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "public DKG ceremony does not match local keyset share {}",
                        share.keyset.id
                    ),
                });
            }
            match keysets_by_id.insert(share.keyset.id, share.keyset.clone()) {
                Some(existing) if existing != share.keyset => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!("conflicting DKG keyset {}", share.keyset.id),
                    });
                }
                Some(_) | None => {}
            }
            match ceremonies_by_keyset_id.insert(share.keyset.id, share.public_ceremony.clone()) {
                Some(existing) if existing != share.public_ceremony => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting public DKG ceremony for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
            match secret_shares_by_keyset.insert(share.keyset.id, share.secret_shares.clone()) {
                Some(existing) if existing != share.secret_shares => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting local DKG secret shares for keyset {}",
                            share.keyset.id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }
        if keysets_by_id.is_empty() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "missing DKG keyset shares".to_string(),
            });
        }

        let mut public_config = setup_public_config.clone();
        public_config.keysets = keysets_by_id.into_values().collect();
        public_config.setup_transcript = None;
        public_config.validate()?;
        let public_ceremonies = ceremonies_by_keyset_id.into_values().collect::<Vec<_>>();

        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &public_config,
            &public_ceremonies,
        )?;
        let mut transcript_signatures = BTreeMap::new();
        for signature_record in transcript_signature_records {
            if signature_record.transcript_hash != transcript_hash {
                continue;
            }
            signature_record.validate_sender(&public_config, signature_record.signer)?;
            match transcript_signatures.insert(signature_record.signer, signature_record.signature)
            {
                Some(existing) if existing != signature_record.signature => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting transcript signature for member {}",
                            signature_record.signer
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let transcript = FederationDkgTranscript::from_signatures(
            FederationDkgSetupMethod::PedersenDkg,
            &public_config,
            &public_ceremonies,
            transcript_signatures,
        )?;
        public_config.setup_transcript = Some(transcript);
        public_config.validate()?;

        let mut private_config = local_private_config.clone();
        private_config.federation_id = Some(public_config.federation_id);
        private_config.setup_transcript_hash = Some(transcript_hash);
        private_config.keyset_secret_shares = secret_shares_by_keyset;
        private_config.validate_against_public(&public_config)?;

        Ok(Self {
            public_config,
            private_config,
        })
    }

    /// Require every roster member to confirm every ceremony in this final config.
    pub fn validate_activation_records(
        &self,
        setup_public_config: &FederationConfig,
        activation_records: &[FederationDkgActivationRequest],
    ) -> Result<()> {
        self.public_config.validate()?;
        self.private_config
            .validate_against_public(&self.public_config)?;
        let transcript = self
            .public_config
            .setup_transcript
            .as_ref()
            .ok_or_else(|| FederationError::DkgTranscriptMismatch {
                reason: "DKG activation requires a complete setup transcript".to_string(),
            })?;
        transcript.validate_setup_lineage_against_config(&self.public_config)?;
        let mut activation_public_config = self.public_config.clone();
        activation_public_config.keysets = transcript
            .payload
            .keysets
            .iter()
            .map(FederationDkgKeysetTranscript::federated_keyset)
            .collect();
        activation_public_config.setup_transcript = Some(transcript.clone());
        // BLS activation confirms the exact config produced by the BLS DKG.
        // A separately validated FROST root/application may be attached after
        // both ceremonies complete, but it must not retroactively change the
        // digest covered by the already-durable BLS activation signatures.
        activation_public_config.frost_wallet = None;
        activation_public_config.validate()?;
        transcript.validate_against_config(&activation_public_config)?;
        let ceremony_ids = transcript
            .payload
            .ceremonies
            .iter()
            .map(FederationDkgPublicCeremony::ceremony_id)
            .collect::<BTreeSet<_>>();
        if ceremony_ids.is_empty() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG activation transcript has no dealer-free ceremonies".to_string(),
            });
        }

        let mut confirmations = BTreeMap::new();
        for activation in activation_records
            .iter()
            .filter(|activation| ceremony_ids.contains(&activation.ceremony_id))
        {
            activation.validate_sender(
                setup_public_config,
                &activation_public_config,
                activation.signer,
            )?;
            let coordinate = (activation.ceremony_id, activation.signer);
            match confirmations.insert(coordinate, *activation) {
                Some(existing) if existing != *activation => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting DKG activation for member {} ceremony {}",
                            activation.signer, activation.ceremony_id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        for ceremony_id in ceremony_ids {
            for member_id in self.public_config.members.keys() {
                if !confirmations.contains_key(&(ceremony_id, *member_id)) {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "missing DKG activation for member {member_id} ceremony {ceremony_id}"
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

fn dkg_signature_transcript(transcript_hash: FederationDkgTranscriptHash) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, FEDERATION_DKG_TRANSCRIPT_SIGNATURE_VERSION);
    append_bytes(&mut transcript, &transcript_hash.to_bytes());
    transcript
}
