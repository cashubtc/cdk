//! Federation-owned transport and activation records for the isolated FROST DKG.

use std::collections::BTreeSet;

use cdk_frost::{
    DkgContext, DkgRound1Package, DkgRound2Package, EncryptedSecret, RootKeyEpoch,
    RootPublicPackage,
};
use serde::{Deserialize, Serialize};

use super::{
    FederationConfig, FederationDkgCeremonyId, FederationDkgMemberKeysetShare, FederationError,
    FederationId, MemberId, Result,
};

/// Versioned federation metadata bound into every FROST DKG message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgContext {
    /// Federation roster identifier.
    pub federation_id: FederationId,
    /// Shared dual-DKG ceremony identifier.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Monotonic application-neutral FROST root epoch.
    pub root_key_epoch: u64,
    /// Protocol version for CDK's canonical FROST DKG conversion.
    pub protocol_version: u16,
    /// Ciphersuite identifier; currently `secp256k1_sha256_tr_v1`.
    pub ciphersuite: String,
    /// Threshold copied exactly from `signature_threshold`.
    pub threshold: u16,
    /// Canonically ordered federation member roster.
    pub participants: BTreeSet<MemberId>,
}

impl FederationFrostDkgContext {
    /// Construct the FROST side of one dual-result federation ceremony.
    pub fn new(
        config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        root_key_epoch: u64,
    ) -> Result<Self> {
        config.validate()?;
        let context = Self {
            federation_id: config.federation_id,
            ceremony_id,
            root_key_epoch,
            protocol_version: 1,
            ciphersuite: "secp256k1_sha256_tr_v1".to_string(),
            threshold: config.threshold_params.signature_threshold,
            participants: config.members.keys().copied().collect(),
        };
        context.to_frost_context()?;
        Ok(context)
    }

    /// Validate and convert to the application-neutral crypto boundary.
    pub fn to_frost_context(&self) -> Result<DkgContext> {
        if self.protocol_version != 1 || self.ciphersuite != "secp256k1_sha256_tr_v1" {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "unsupported FROST DKG protocol or ciphersuite".to_string(),
            });
        }
        DkgContext::new(
            self.federation_id.to_bytes(),
            self.ceremony_id.to_bytes(),
            RootKeyEpoch::new(self.root_key_epoch).map_err(frost_error)?,
            self.threshold,
            self.participants
                .iter()
                .map(|member| frost_member(*member))
                .collect::<Result<_>>()?,
        )
        .map_err(frost_error)
    }

    /// Validate exact roster and threshold equality against public federation config.
    pub fn validate_against(&self, config: &FederationConfig) -> Result<()> {
        if self.federation_id != config.federation_id
            || self.threshold != config.threshold_params.signature_threshold
            || self.participants != config.members.keys().copied().collect()
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "FROST DKG roster or threshold differs from BLS federation setup"
                    .to_string(),
            });
        }
        self.to_frost_context().map(|_| ())
    }
}

/// Signed REST payload for the FROST broadcast/proof round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgRound1Request {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Signed sender that owns the encoded FROST package.
    pub sender: MemberId,
    /// Canonical bounded `cdk-frost` round-one wire bytes.
    pub package: Vec<u8>,
}

impl FederationFrostDkgRound1Request {
    /// Validate signed ownership and cryptographic package encoding.
    pub fn decode_package(
        &self,
        config: &FederationConfig,
        signed_sender: MemberId,
    ) -> Result<DkgRound1Package> {
        self.context.validate_against(config)?;
        if self.sender != signed_sender {
            return Err(FederationError::MemberIdMismatch {
                expected: signed_sender,
                found: self.sender,
            });
        }
        let package = DkgRound1Package::decode(&self.context.to_frost_context()?, &self.package)
            .map_err(frost_error)?;
        if package.sender() != frost_member(self.sender)? {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "FROST round-one sender binding mismatch".to_string(),
            });
        }
        Ok(package)
    }
}

/// Signed confidential REST payload for one FROST share round receiver.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgRound2Request {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Signed sender.
    pub sender: MemberId,
    /// Intended authenticated receiver.
    pub receiver: MemberId,
    /// Canonical bounded secret-contribution wire bytes.
    pub package: Vec<u8>,
}

/// Participant-local sealed persistence record for one round-two contribution.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgRound2Record {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Authenticated sender.
    pub sender: MemberId,
    /// Intended local or remote receiver.
    pub receiver: MemberId,
    /// Authenticated ciphertext; plaintext wire bytes are never serialized at rest.
    pub encrypted_package: Vec<u8>,
}

/// Restartable participant-local FROST DKG phase stored only as ciphertext.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationFrostDkgLocalPhase {
    /// Round one secret package is durable before its public package is acknowledged.
    Round1,
    /// Round two secret package is durable before contributions are sent.
    Round2,
}

/// Durable encrypted participant state for one in-progress FROST ceremony.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgLocalState {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Local owner.
    pub member_id: MemberId,
    /// Restart phase of the encrypted package.
    pub phase: FederationFrostDkgLocalPhase,
    /// Authenticated ciphertext produced by `cdk-frost`.
    pub encrypted_state: Vec<u8>,
}

impl FederationFrostDkgLocalState {
    /// Validate metadata and ciphertext envelope without decrypting secret state.
    pub fn validate(&self, config: &FederationConfig) -> Result<()> {
        self.context.validate_against(config)?;
        if !config.members.contains_key(&self.member_id) {
            return Err(FederationError::MissingMember {
                member_id: self.member_id,
            });
        }
        EncryptedSecret::from_bytes(self.encrypted_state.clone()).map_err(frost_error)?;
        Ok(())
    }
}

impl std::fmt::Debug for FederationFrostDkgRound2Request {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederationFrostDkgRound2Request")
            .field("context", &self.context)
            .field("sender", &self.sender)
            .field("receiver", &self.receiver)
            .field("package", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for FederationFrostDkgRound2Record {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederationFrostDkgRound2Record")
            .field("context", &self.context)
            .field("sender", &self.sender)
            .field("receiver", &self.receiver)
            .field("encrypted_package", &"<redacted>")
            .finish()
    }
}

impl FederationFrostDkgRound2Record {
    /// Validate metadata and parse the bounded ciphertext envelope.
    pub fn encrypted(&self, config: &FederationConfig) -> Result<EncryptedSecret> {
        self.context.validate_against(config)?;
        if !config.members.contains_key(&self.sender)
            || !config.members.contains_key(&self.receiver)
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "sealed FROST round-two sender or receiver is not configured".to_string(),
            });
        }
        EncryptedSecret::from_bytes(self.encrypted_package.clone()).map_err(frost_error)
    }

    /// Open and authenticate this participant-local retry contribution.
    pub fn open(
        &self,
        config: &FederationConfig,
        key: &cdk_frost::SecretEncryptionKey,
    ) -> Result<DkgRound2Package> {
        let context = self.context.to_frost_context()?;
        let encrypted = self.encrypted(config)?;
        DkgRound2Package::open(
            &context,
            frost_member(self.sender)?,
            frost_member(self.receiver)?,
            key,
            &encrypted,
        )
        .map_err(frost_error)
    }
}

impl FederationFrostDkgRound2Request {
    /// Validate signed sender/receiver ownership and cryptographic encoding.
    pub fn decode_package(
        &self,
        config: &FederationConfig,
        signed_sender: MemberId,
        expected_receiver: MemberId,
    ) -> Result<DkgRound2Package> {
        self.context.validate_against(config)?;
        if self.sender != signed_sender || self.receiver != expected_receiver {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "FROST round-two sender or receiver binding mismatch".to_string(),
            });
        }
        DkgRound2Package::decode(
            &self.context.to_frost_context()?,
            frost_member(expected_receiver)?,
            &self.package,
        )
        .map_err(frost_error)
    }
}

/// Durable participant-local activated FROST root result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRootRecord {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Local member that owns the encrypted root share.
    pub member_id: MemberId,
    /// Canonical untweaked public root package.
    pub public_package: Vec<u8>,
    /// Authenticated ciphertext for only this member's canonical root share.
    pub encrypted_root_share: Vec<u8>,
}

/// Signed all-member confirmation of one identical canonical public FROST root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostDkgConfirmationRequest {
    /// Exact ceremony context.
    pub context: FederationFrostDkgContext,
    /// Confirming signed member.
    pub member_id: MemberId,
    /// Canonical untweaked public root bytes confirmed by this member.
    pub public_package: Vec<u8>,
}

impl FederationFrostDkgConfirmationRequest {
    /// Validate signed ownership and the canonical root against ceremony metadata.
    pub fn validate(
        &self,
        config: &FederationConfig,
        signed_sender: MemberId,
    ) -> Result<RootPublicPackage> {
        if self.member_id != signed_sender {
            return Err(FederationError::MemberIdMismatch {
                expected: signed_sender,
                found: self.member_id,
            });
        }
        self.context.validate_against(config)?;
        validate_public_root(&self.context, &self.public_package)
    }
}

impl FederationFrostRootRecord {
    /// Validate public root metadata and bounded ciphertext without opening the share.
    pub fn validate(&self, config: &FederationConfig) -> Result<RootPublicPackage> {
        self.context.validate_against(config)?;
        if !config.members.contains_key(&self.member_id) {
            return Err(FederationError::MissingMember {
                member_id: self.member_id,
            });
        }
        let public = validate_public_root(&self.context, &self.public_package)?;
        EncryptedSecret::from_bytes(self.encrypted_root_share.clone()).map_err(frost_error)?;
        Ok(public)
    }
}

fn validate_public_root(
    context: &FederationFrostDkgContext,
    encoded: &[u8],
) -> Result<RootPublicPackage> {
    let public = RootPublicPackage::decode(encoded).map_err(frost_error)?;
    if public.epoch().get() != context.root_key_epoch
        || public.threshold() != context.threshold
        || public
            .verifying_shares()
            .keys()
            .map(|member| MemberId::new(member.get()))
            .collect::<Result<BTreeSet<_>>>()?
            != context.participants
    {
        return Err(FederationError::DkgTranscriptMismatch {
            reason: "canonical FROST root does not match ceremony metadata".to_string(),
        });
    }
    Ok(public)
}

/// Fail-closed activation product requiring both independent DKG results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDualDkgResult {
    /// Existing Cashu BLS keyset result, unchanged.
    pub bls: FederationDkgMemberKeysetShare,
    /// Application-neutral secp256k1 FROST root result.
    pub frost: FederationFrostRootRecord,
}

impl FederationDualDkgResult {
    /// Validate that BLS and FROST results belong to the same member, roster, and ceremony.
    pub fn validate(&self, config: &FederationConfig) -> Result<RootPublicPackage> {
        if self.bls.federation_id != config.federation_id
            || self.bls.member_id != self.frost.member_id
            || self.bls.public_ceremony.ceremony_id() != self.frost.context.ceremony_id
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "BLS and FROST DKG results do not form one ceremony".to_string(),
            });
        }
        self.frost.validate(config)
    }
}

fn frost_member(member: MemberId) -> Result<cdk_frost::MemberId> {
    cdk_frost::MemberId::new(member.get()).map_err(frost_error)
}

fn frost_error(error: cdk_frost::Error) -> FederationError {
    FederationError::DkgTranscriptMismatch {
        reason: format!("FROST DKG validation failed: {error}"),
    }
}
