//! Authenticated multi-operator federation bootstrap domain.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::base64::Engine as _;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use url::{Host, Url};

use super::{
    FederationBarkPolicyConfig, FederationCheckpointPolicy, FederationConfig,
    FederationDkgCeremonyId, FederationDkgKeysetSpec, FederationError,
    FederationPaymentObservationPolicy, FederationPaymentVerificationPolicy, MemberConfigPublic,
    MemberId, Result, ThresholdParams, FEDERATION_MAX_DKG_KEYSET_AMOUNTS,
    FEDERATION_MAX_DKG_MEMBERS, FEDERATION_WALLET_PROTOCOL_VERSION,
};
use crate::mint_url::MintUrl;
use crate::nuts::nut01::{PublicKey, SecretKey};

mod persistence;
mod review;

pub use self::persistence::{
    FederationSetupActivation, FederationSetupCoordinatorPhase, FederationSetupInvitationRecord,
    FederationSetupInvitationState, FederationSetupLocalRole, FederationSetupParticipantPhase,
    FederationSetupSession,
};
pub use self::review::{FederationSetupMemberReview, FederationSetupProposalReview};

/// Multi-operator setup protocol version implemented by this build.
pub const FEDERATION_SETUP_PROTOCOL_VERSION: u16 = 1;
/// Prefix for operator setup invitations, distinct from wallet invites.
pub const FEDERATION_SETUP_INVITATION_PREFIX: &str = "cashufedsetupA";
/// Bytes of entropy in one setup invitation capability.
pub const FEDERATION_SETUP_INVITATION_CAPABILITY_BYTES: usize = 32;
/// Maximum normalized UTF-8 bytes in a setup URL.
pub const FEDERATION_MAX_SETUP_URL_BYTES: usize = 2_048;
/// Maximum encoded bytes in one setup invitation.
pub const FEDERATION_MAX_SETUP_INVITATION_BYTES: usize = 8 * 1024;
/// Maximum canonical bytes in one signed enrollment.
pub const FEDERATION_MAX_SETUP_ENROLLMENT_BYTES: usize = 16 * 1024;
/// Maximum canonical bytes in a proposal or certified manifest.
pub const FEDERATION_MAX_SETUP_ARTIFACT_BYTES: usize = 1024 * 1024;
/// Maximum response bytes in one setup status document.
pub const FEDERATION_MAX_SETUP_STATUS_BYTES: usize = 256 * 1024;
/// Maximum BLS keyset specifications in one initial setup.
pub const FEDERATION_MAX_SETUP_KEYSETS: usize = 16;
/// Minimum invitation lifetime accepted by setup creation.
pub const FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS: u64 = 5 * 60;
/// Default invitation lifetime used by setup creation.
pub const FEDERATION_SETUP_DEFAULT_INVITATION_LIFETIME_SECS: u64 = 24 * 60 * 60;
/// Maximum invitation lifetime accepted by setup creation.
pub const FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;

/// BLS DKG wire version that carries setup authorization.
pub const FEDERATION_SETUP_BLS_DKG_PROTOCOL_VERSION: u16 = 2;
/// Private federation REST wire version that carries setup authorization.
pub const FEDERATION_SETUP_PRIVATE_REST_PROTOCOL_VERSION: u16 = 2;
/// Cashu keyset v3's CDK enum representation.
pub const FEDERATION_SETUP_BLS_KEYSET_VERSION: u16 = 2;
/// FROST root DKG version that carries setup authorization.
pub const FEDERATION_SETUP_FROST_DKG_PROTOCOL_VERSION: u16 = 2;
/// secp256k1/SHA-256 Taproot FROST ciphersuite version.
pub const FEDERATION_SETUP_FROST_CIPHERSUITE_VERSION: u16 = 1;

const INVITATION_CAPABILITY_DOMAIN: &[u8] = b"cdk/federation/setup/invitation-capability/v1";
const INVITATION_CHECKSUM_DOMAIN: &[u8] = b"cdk/federation/setup/invitation-checksum/v1";
const ENROLLMENT_SIGNATURE_DOMAIN: &[u8] = b"cdk/federation/setup/enrollment-signature/v1";
const PROPOSAL_DOMAIN: &[u8] = b"cdk/federation/setup/proposal/v1";
const PROPOSAL_PUBLICATION_DOMAIN: &[u8] = b"cdk/federation/setup/proposal-publication/v1";
const APPROVAL_DOMAIN: &[u8] = b"cdk/federation/setup/approval/v1";
const MANIFEST_DOMAIN: &[u8] = b"cdk/federation/setup/manifest/v1";
const MANIFEST_PUBLICATION_DOMAIN: &[u8] = b"cdk/federation/setup/manifest-publication/v1";
const CEREMONY_ID_DOMAIN: &[u8] = b"cdk/federation/setup/ceremony-id/v1";

macro_rules! setup_hash_type {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Construct from exactly 32 bytes.
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// Parse from exactly 32 bytes.
            pub fn from_slice(bytes: &[u8]) -> Result<Self> {
                let bytes = bytes.try_into().map_err(|_| invalid(format!(
                    "{} must contain exactly 32 bytes",
                    $label
                )))?;
                Ok(Self(bytes))
            }

            /// Return the raw bytes.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 32] {
                self.0
            }

            /// Return lowercase hexadecimal.
            #[must_use]
            pub fn to_hex(self) -> String {
                crate::util::hex::encode(self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.to_hex())
            }
        }

        impl FromStr for $name {
            type Err = FederationError;

            fn from_str(value: &str) -> Result<Self> {
                Self::from_slice(&crate::util::hex::decode(value)?)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_str(&value).map_err(de::Error::custom)
            }
        }
    };
}

setup_hash_type!(
    /// Random identifier for one multi-operator setup attempt.
    FederationSetupId,
    "setup id"
);
setup_hash_type!(
    /// Public opaque identifier for one invitation slot.
    FederationSetupInvitationId,
    "setup invitation id"
);
setup_hash_type!(
    /// Context-bound hash of a one-use setup invitation capability.
    FederationSetupInvitationCapabilityHash,
    "setup invitation capability hash"
);
setup_hash_type!(
    /// Canonical hash of one immutable setup proposal body.
    FederationSetupProposalHash,
    "setup proposal hash"
);
setup_hash_type!(
    /// Authorization value carried into DKG and final configuration.
    FederationSetupAuthorizationHash,
    "setup authorization hash"
);
setup_hash_type!(
    /// Canonical hash of one complete manifest approval certificate.
    FederationSetupManifestHash,
    "setup manifest hash"
);

impl From<FederationSetupProposalHash> for FederationSetupAuthorizationHash {
    fn from(value: FederationSetupProposalHash) -> Self {
        Self::from_bytes(value.to_bytes())
    }
}

/// Secret one-use capability carried by an operator setup invitation.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationSetupInvitationCapability([u8; 32]);

impl FederationSetupInvitationCapability {
    /// Construct from exactly 32 random bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        if bytes == [0; 32] {
            return Err(invalid("invitation capability must not be all zero"));
        }
        Ok(Self(bytes))
    }

    /// Return the capability bytes for protected transport or storage.
    #[must_use]
    pub const fn expose_secret(self) -> [u8; 32] {
        self.0
    }

    /// Derive the context-bound hash stored by the coordinator.
    #[must_use]
    pub fn capability_hash(
        self,
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
        member_id: MemberId,
    ) -> FederationSetupInvitationCapabilityHash {
        FederationSetupInvitationCapabilityHash::from_bytes(hash_transcript(
            INVITATION_CAPABILITY_DOMAIN,
            &[
                &setup_id.to_bytes(),
                &invitation_id.to_bytes(),
                &member_id.get().to_be_bytes(),
                &self.0,
            ],
        ))
    }
}

impl fmt::Debug for FederationSetupInvitationCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FederationSetupInvitationCapability([REDACTED])")
    }
}

impl Serialize for FederationSetupInvitationCapability {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for FederationSetupInvitationCapability {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value).map_err(de::Error::custom)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| de::Error::custom("invitation capability must contain 32 bytes"))?;
        Self::from_bytes(bytes).map_err(de::Error::custom)
    }
}

/// Ephemeral public identity pinned by setup invitations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationSetupCoordinatorPublicKey(PublicKey);

impl FederationSetupCoordinatorPublicKey {
    /// Construct from a secp256k1 public identity key.
    pub fn new(public_key: PublicKey) -> Result<Self> {
        if !matches!(public_key, PublicKey::Secp256k1(_)) {
            return Err(invalid("setup coordinator identity must use secp256k1"));
        }
        Ok(Self(public_key))
    }

    /// Return the underlying public identity key.
    #[must_use]
    pub const fn public_key(self) -> PublicKey {
        self.0
    }

    fn to_bytes(self) -> Vec<u8> {
        self.0.to_bytes()
    }
}

impl fmt::Display for FederationSetupCoordinatorPublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.to_hex())
    }
}

impl Serialize for FederationSetupCoordinatorPublicKey {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FederationSetupCoordinatorPublicKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let public_key = PublicKey::deserialize(deserializer)?;
        Self::new(public_key).map_err(de::Error::custom)
    }
}

/// Schnorr signature over one domain-separated setup transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationSetupSignature([u8; 64]);

impl FederationSetupSignature {
    /// Parse a Schnorr signature from exactly 64 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidSetupSignature {
                    context: "artifact",
                })?;
        Signature::from_slice(&bytes).map_err(|_| FederationError::InvalidSetupSignature {
            context: "artifact",
        })?;
        Ok(Self(bytes))
    }

    /// Return the raw Schnorr signature bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    /// Return lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn sign(transcript: &[u8], secret_key: &SecretKey) -> Result<Self> {
        let signature = secret_key.sign(transcript)?;
        Ok(Self(signature.serialize()))
    }

    /// Deterministically sign a frozen test transcript without auxiliary randomness.
    ///
    /// Production setup flows must use their artifact constructors, which use
    /// randomized signing. This method exists only for cross-implementation
    /// fixed vectors.
    pub fn sign_no_aux_rand_for_test_vector(
        transcript: &[u8],
        secret_key: &SecretKey,
    ) -> Result<Self> {
        let signature = secret_key.sign_no_aux_rand(transcript)?;
        Ok(Self(signature.serialize()))
    }

    fn verify(self, transcript: &[u8], public_key: PublicKey, context: &'static str) -> Result<()> {
        let signature = Signature::from_slice(&self.0)
            .map_err(|_| FederationError::InvalidSetupSignature { context })?;
        public_key
            .verify(transcript, &signature)
            .map_err(|_| FederationError::SetupSignatureVerificationFailed { context })
    }
}

impl fmt::Display for FederationSetupSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for FederationSetupSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_slice(&crate::util::hex::decode(value)?)
    }
}

impl Serialize for FederationSetupSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationSetupSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

/// URL validation policy selected by the local setup runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupUrlPolicy {
    /// Require credential-free HTTPS URLs.
    #[default]
    RequireHttps,
    /// Also allow credential-free HTTP URLs whose host is loopback.
    AllowInsecureLocalhost,
}

/// Strict protocol versions supported by one setup participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupProtocolVersions {
    /// Multi-operator setup wire version.
    pub setup: u16,
    /// BLS DKG wire version.
    pub bls_dkg: u16,
    /// Configured-member private REST version.
    pub federation_rest: u16,
    /// Wallet federation protocol version.
    pub wallet: u16,
    /// Cashu BLS keyset version.
    pub bls_keyset: u16,
    /// FROST root DKG version.
    pub frost_dkg: u16,
    /// FROST ciphersuite version.
    pub frost_ciphersuite: u16,
}

impl Default for FederationSetupProtocolVersions {
    fn default() -> Self {
        Self {
            setup: FEDERATION_SETUP_PROTOCOL_VERSION,
            bls_dkg: FEDERATION_SETUP_BLS_DKG_PROTOCOL_VERSION,
            federation_rest: FEDERATION_SETUP_PRIVATE_REST_PROTOCOL_VERSION,
            wallet: FEDERATION_WALLET_PROTOCOL_VERSION,
            bls_keyset: FEDERATION_SETUP_BLS_KEYSET_VERSION,
            frost_dkg: FEDERATION_SETUP_FROST_DKG_PROTOCOL_VERSION,
            frost_ciphersuite: FEDERATION_SETUP_FROST_CIPHERSUITE_VERSION,
        }
    }
}

impl FederationSetupProtocolVersions {
    /// Validate exact version support without negotiation or downgrade.
    pub fn validate(self) -> Result<()> {
        let expected = Self::default();
        for (component, expected, found) in [
            ("setup", expected.setup, self.setup),
            ("bls_dkg", expected.bls_dkg, self.bls_dkg),
            (
                "federation_rest",
                expected.federation_rest,
                self.federation_rest,
            ),
            ("wallet", expected.wallet, self.wallet),
            ("bls_keyset", expected.bls_keyset, self.bls_keyset),
            ("frost_dkg", expected.frost_dkg, self.frost_dkg),
            (
                "frost_ciphersuite",
                expected.frost_ciphersuite,
                self.frost_ciphersuite,
            ),
        ] {
            if found != expected {
                return Err(FederationError::SetupVersionMismatch {
                    component,
                    expected,
                    found,
                });
            }
        }
        Ok(())
    }

    fn append_canonical(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.setup.to_be_bytes());
        output.extend_from_slice(&self.bls_dkg.to_be_bytes());
        output.extend_from_slice(&self.federation_rest.to_be_bytes());
        output.extend_from_slice(&self.wallet.to_be_bytes());
        output.extend_from_slice(&self.bls_keyset.to_be_bytes());
        output.extend_from_slice(&self.frost_dkg.to_be_bytes());
        output.extend_from_slice(&self.frost_ciphersuite.to_be_bytes());
    }
}

/// Protected first-contact invitation for one fixed member slot.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupInvitation {
    /// Strict setup wire version.
    pub version: u16,
    /// Setup attempt being joined.
    pub setup_id: FederationSetupId,
    /// Public invitation identifier.
    pub invitation_id: FederationSetupInvitationId,
    /// Assigned non-zero member slot.
    pub member_id: MemberId,
    /// HTTPS setup coordinator endpoint.
    pub coordinator_url: MintUrl,
    /// Ephemeral coordinator identity pinned out of band.
    pub coordinator_identity: FederationSetupCoordinatorPublicKey,
    /// Absolute Unix expiry in seconds.
    pub expires_at_unix: u64,
    /// One-use high-entropy bearer capability.
    pub capability: FederationSetupInvitationCapability,
}

impl fmt::Debug for FederationSetupInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederationSetupInvitation")
            .field("version", &self.version)
            .field("setup_id", &self.setup_id)
            .field("invitation_id", &self.invitation_id)
            .field("member_id", &self.member_id)
            .field("coordinator_url", &self.coordinator_url)
            .field("coordinator_identity", &self.coordinator_identity)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("capability", &"[REDACTED]")
            .finish()
    }
}

impl FederationSetupInvitation {
    /// Construct and validate one invitation.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
        member_id: MemberId,
        coordinator_url: MintUrl,
        coordinator_identity: FederationSetupCoordinatorPublicKey,
        expires_at_unix: u64,
        capability: FederationSetupInvitationCapability,
        now_unix: u64,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        let invitation = Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id,
            invitation_id,
            member_id,
            coordinator_url,
            coordinator_identity,
            expires_at_unix,
            capability,
        };
        invitation.validate(now_unix, url_policy)?;
        if invitation.expires_at_unix - now_unix < FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS {
            return Err(invalid("new setup invitation lifetime is below minimum"));
        }
        Ok(invitation)
    }

    /// Validate version, lifetime, URL, and encoded size.
    pub fn validate(&self, now_unix: u64, url_policy: FederationSetupUrlPolicy) -> Result<()> {
        validate_setup_version("invitation", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_nonzero_identifier("invitation ID", self.invitation_id.to_bytes())?;
        validate_setup_url(&self.coordinator_url, url_policy, "coordinator")?;
        let lifetime = self
            .expires_at_unix
            .checked_sub(now_unix)
            .ok_or_else(|| invalid("setup invitation is expired"))?;
        if lifetime == 0 || lifetime > FEDERATION_SETUP_MAX_INVITATION_LIFETIME_SECS {
            return Err(invalid("setup invitation lifetime is outside bounds"));
        }
        ensure_size(
            "invitation",
            self.canonical_public_bytes()?.len() + FEDERATION_SETUP_INVITATION_CAPABILITY_BYTES + 6,
            FEDERATION_MAX_SETUP_INVITATION_BYTES,
        )
    }

    /// Return the canonical non-secret invitation fields.
    pub fn canonical_public_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.extend_from_slice(&self.version.to_be_bytes());
        output.extend_from_slice(&self.setup_id.to_bytes());
        output.extend_from_slice(&self.invitation_id.to_bytes());
        output.extend_from_slice(&self.member_id.get().to_be_bytes());
        append_string(&mut output, &self.coordinator_url.to_string())?;
        output.extend_from_slice(&self.coordinator_identity.to_bytes());
        output.extend_from_slice(&self.expires_at_unix.to_be_bytes());
        Ok(output)
    }

    /// Return the coordinator-stored capability hash without exposing the capability.
    #[must_use]
    pub fn capability_hash(&self) -> FederationSetupInvitationCapabilityHash {
        self.capability
            .capability_hash(self.setup_id, self.invitation_id, self.member_id)
    }

    /// Encode this credential as a checksummed URL-safe invitation string.
    pub fn to_invitation_code(&self) -> Result<String> {
        let validation_time = self.expires_at_unix.saturating_sub(1);
        let url_policy = match self.coordinator_url.to_string().starts_with("http://") {
            true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
            false => FederationSetupUrlPolicy::RequireHttps,
        };
        self.validate(validation_time, url_policy)?;
        let public = self.canonical_public_bytes()?;
        let capability = self.capability.expose_secret();
        let checksum = invitation_checksum(&public, capability)?;
        let mut payload = Vec::with_capacity(public.len() + capability.len() + checksum.len());
        payload.extend_from_slice(&public);
        payload.extend_from_slice(&capability);
        payload.extend_from_slice(&checksum);
        let encoded = bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let invitation = format!("{FEDERATION_SETUP_INVITATION_PREFIX}{encoded}");
        ensure_size(
            "invitation",
            invitation.len(),
            FEDERATION_MAX_SETUP_INVITATION_BYTES,
        )?;
        Ok(invitation)
    }
}

impl fmt::Display for FederationSetupInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_invitation_code().map_err(|_| fmt::Error)?)
    }
}

impl FromStr for FederationSetupInvitation {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        ensure_size(
            "invitation",
            value.len(),
            FEDERATION_MAX_SETUP_INVITATION_BYTES,
        )?;
        let payload = value
            .strip_prefix(FEDERATION_SETUP_INVITATION_PREFIX)
            .ok_or_else(|| invalid("unsupported setup invitation prefix"))?;
        let decoded = bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| {
                invalid(format!("setup invitation is not URL-safe base64: {error}"))
            })?;
        ensure_size(
            "invitation",
            decoded.len(),
            FEDERATION_MAX_SETUP_INVITATION_BYTES,
        )?;
        let mut remaining = decoded.as_slice();
        let version = u16::from_be_bytes(take_setup_bytes(&mut remaining, "version")?);
        validate_setup_version("invitation", version)?;
        let setup_id = FederationSetupId::from_bytes(take_setup_bytes(&mut remaining, "setup ID")?);
        let invitation_id = FederationSetupInvitationId::from_bytes(take_setup_bytes(
            &mut remaining,
            "invitation ID",
        )?);
        let member_id = MemberId::new(u16::from_be_bytes(take_setup_bytes(
            &mut remaining,
            "member ID",
        )?))?;
        let coordinator_url = String::from_utf8(take_length_prefixed(&mut remaining, "URL")?)
            .map_err(|_| invalid("setup invitation URL is not UTF-8"))?;
        let coordinator_url = MintUrl::from_str(&coordinator_url)
            .map_err(|error| invalid(format!("invalid setup invitation URL: {error}")))?;
        let coordinator_identity = FederationSetupCoordinatorPublicKey::new(
            PublicKey::from_slice(&take_setup_bytes::<33>(
                &mut remaining,
                "coordinator identity",
            )?)
            .map_err(|error| invalid(format!("invalid coordinator identity: {error}")))?,
        )?;
        let expires_at_unix = u64::from_be_bytes(take_setup_bytes(&mut remaining, "expiry")?);
        let capability_bytes = take_setup_bytes(&mut remaining, "capability")?;
        let capability = FederationSetupInvitationCapability::from_bytes(capability_bytes)?;
        let checksum = take_setup_bytes::<6>(&mut remaining, "checksum")?;
        if !remaining.is_empty() {
            return Err(invalid("setup invitation contains trailing bytes"));
        }
        let public_length = decoded.len() - FEDERATION_SETUP_INVITATION_CAPABILITY_BYTES - 6;
        let expected_checksum = invitation_checksum(&decoded[..public_length], capability_bytes)?;
        if checksum != expected_checksum {
            return Err(invalid("setup invitation checksum mismatch"));
        }
        let invitation = Self {
            version,
            setup_id,
            invitation_id,
            member_id,
            coordinator_url,
            coordinator_identity,
            expires_at_unix,
            capability,
        };
        let validation_time = invitation.expires_at_unix.saturating_sub(1);
        let url_policy = match invitation
            .coordinator_url
            .to_string()
            .starts_with("http://")
        {
            true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
            false => FederationSetupUrlPolicy::RequireHttps,
        };
        invitation.validate(validation_time, url_policy)?;
        Ok(invitation)
    }
}

/// Identity-signed public enrollment for one invitation slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupEnrollment {
    /// Strict setup wire version.
    pub version: u16,
    /// Setup attempt being joined.
    pub setup_id: FederationSetupId,
    /// Invitation consumed by this enrollment.
    pub invitation_id: FederationSetupInvitationId,
    /// Assigned member ID.
    pub member_id: MemberId,
    /// Public Cashu endpoint.
    pub public_mint_url: MintUrl,
    /// Private federation peer endpoint.
    pub federation_api_url: MintUrl,
    /// Locally generated member identity.
    pub identity_public_key: PublicKey,
    /// Locally generated setup nonce.
    pub setup_nonce: [u8; 32],
    /// Exact protocol support advertised by this member.
    pub supported_versions: FederationSetupProtocolVersions,
    /// Identity signature over all preceding fields.
    pub signature: FederationSetupSignature,
}

impl FederationSetupEnrollment {
    /// Sign and validate an enrollment using randomized BIP340 signing.
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
        member_id: MemberId,
        public_mint_url: MintUrl,
        federation_api_url: MintUrl,
        setup_nonce: [u8; 32],
        supported_versions: FederationSetupProtocolVersions,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(
            setup_id,
            invitation_id,
            member_id,
            public_mint_url,
            federation_api_url,
            setup_nonce,
            supported_versions,
            identity_secret_key,
            url_policy,
            false,
        )
    }

    /// Deterministically sign a frozen enrollment test vector.
    #[allow(clippy::too_many_arguments)]
    pub fn sign_no_aux_rand_for_test_vector(
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
        member_id: MemberId,
        public_mint_url: MintUrl,
        federation_api_url: MintUrl,
        setup_nonce: [u8; 32],
        supported_versions: FederationSetupProtocolVersions,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(
            setup_id,
            invitation_id,
            member_id,
            public_mint_url,
            federation_api_url,
            setup_nonce,
            supported_versions,
            identity_secret_key,
            url_policy,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn sign_with(
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
        member_id: MemberId,
        public_mint_url: MintUrl,
        federation_api_url: MintUrl,
        setup_nonce: [u8; 32],
        supported_versions: FederationSetupProtocolVersions,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
        deterministic: bool,
    ) -> Result<Self> {
        if !matches!(identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(invalid("setup member identity must use secp256k1"));
        }
        let mut enrollment = Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id,
            invitation_id,
            member_id,
            public_mint_url,
            federation_api_url,
            identity_public_key: identity_secret_key.public_key(),
            setup_nonce,
            supported_versions,
            signature: FederationSetupSignature([0; 64]),
        };
        let transcript = enrollment.signature_transcript()?;
        enrollment.signature = match deterministic {
            true => FederationSetupSignature::sign_no_aux_rand_for_test_vector(
                &transcript,
                identity_secret_key,
            )?,
            false => FederationSetupSignature::sign(&transcript, identity_secret_key)?,
        };
        enrollment.validate(url_policy)?;
        Ok(enrollment)
    }

    /// Validate structure, URL policy, protocol support, size, and identity signature.
    pub fn validate(&self, url_policy: FederationSetupUrlPolicy) -> Result<()> {
        validate_setup_version("enrollment", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_nonzero_identifier("invitation ID", self.invitation_id.to_bytes())?;
        self.supported_versions.validate()?;
        validate_setup_url(&self.public_mint_url, url_policy, "public mint")?;
        validate_setup_url(&self.federation_api_url, url_policy, "federation API")?;
        if self.public_mint_url == self.federation_api_url {
            return Err(invalid(
                "public mint and federation API URLs must be distinct",
            ));
        }
        if !matches!(self.identity_public_key, PublicKey::Secp256k1(_)) {
            return Err(invalid("setup member identity must use secp256k1"));
        }
        if self.setup_nonce == [0; 32] {
            return Err(invalid("setup member nonce must not be all zero"));
        }
        let canonical = self.canonical_bytes()?;
        ensure_size(
            "enrollment",
            canonical.len(),
            FEDERATION_MAX_SETUP_ENROLLMENT_BYTES,
        )?;
        self.signature.verify(
            &self.signature_transcript()?,
            self.identity_public_key,
            "enrollment",
        )
    }

    /// Return the canonical signed enrollment bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut output = self.canonical_unsigned_bytes()?;
        output.extend_from_slice(&self.signature.to_bytes());
        Ok(output)
    }

    fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.extend_from_slice(&self.version.to_be_bytes());
        output.extend_from_slice(&self.setup_id.to_bytes());
        output.extend_from_slice(&self.invitation_id.to_bytes());
        output.extend_from_slice(&self.member_id.get().to_be_bytes());
        append_string(&mut output, &self.public_mint_url.to_string())?;
        append_string(&mut output, &self.federation_api_url.to_string())?;
        output.extend_from_slice(&self.identity_public_key.to_bytes());
        output.extend_from_slice(&self.setup_nonce);
        output.extend_from_slice(&self.supported_versions.bls_dkg.to_be_bytes());
        output.extend_from_slice(&self.supported_versions.federation_rest.to_be_bytes());
        output.extend_from_slice(&self.supported_versions.wallet.to_be_bytes());
        output.extend_from_slice(&self.supported_versions.bls_keyset.to_be_bytes());
        output.extend_from_slice(&self.supported_versions.frost_dkg.to_be_bytes());
        output.extend_from_slice(&self.supported_versions.frost_ciphersuite.to_be_bytes());
        Ok(output)
    }

    fn signature_transcript(&self) -> Result<Vec<u8>> {
        let unsigned = self.canonical_unsigned_bytes()?;
        Ok(transcript(
            ENROLLMENT_SIGNATURE_DOMAIN,
            &[&length_prefixed(&unsigned)?],
        ))
    }
}

/// Bitcoin network committed by the federation setup proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupBitcoinNetwork {
    /// Bitcoin mainnet.
    Bitcoin,
    /// Bitcoin testnet.
    Testnet,
    /// Bitcoin signet.
    Signet,
    /// Bitcoin regtest.
    Regtest,
}

impl FederationSetupBitcoinNetwork {
    const fn discriminant(self) -> u8 {
        match self {
            Self::Bitcoin => 1,
            Self::Testnet => 2,
            Self::Signet => 3,
            Self::Regtest => 4,
        }
    }

    /// Convert to the corresponding Bitcoin network.
    #[must_use]
    pub const fn bitcoin_network(self) -> bitcoin::Network {
        match self {
            Self::Bitcoin => bitcoin::Network::Bitcoin,
            Self::Testnet => bitcoin::Network::Testnet,
            Self::Signet => bitcoin::Network::Signet,
            Self::Regtest => bitcoin::Network::Regtest,
        }
    }
}

/// FROST application policy authorized before the root DKG starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationSetupFrostApplication {
    /// CDK on-chain recovery and payment application.
    CdkOnchain {
        /// Application derivation epoch.
        application_key_epoch: u64,
        /// Confirmation policy schema version.
        confirmation_policy_version: u16,
        /// Required on-chain confirmations.
        minimum_confirmations: u32,
        /// Transaction fee policy version.
        fee_policy_version: u16,
    },
    /// Bark VTXO application derived from the same FROST root.
    Bark {
        /// Application derivation epoch.
        application_key_epoch: u64,
        /// Bitcoin network accepted by the Bark application.
        network: FederationSetupBitcoinNetwork,
        /// Single accepted Ark server key.
        ark_server_public_key: Box<PublicKey>,
        /// Exact Bark fee, CLTV, retention, and recovery policy.
        policy: FederationBarkPolicyConfig,
    },
}

impl FederationSetupFrostApplication {
    fn tag(&self) -> u8 {
        match self {
            Self::CdkOnchain { .. } => 1,
            Self::Bark { .. } => 2,
        }
    }

    fn validate(&self, network: FederationSetupBitcoinNetwork) -> Result<()> {
        match self {
            Self::CdkOnchain {
                application_key_epoch,
                confirmation_policy_version,
                minimum_confirmations,
                fee_policy_version,
            } => {
                if *application_key_epoch == 0
                    || *confirmation_policy_version != 1
                    || *minimum_confirmations == 0
                    || *fee_policy_version == 0
                {
                    return Err(invalid("unsupported CDK on-chain FROST application policy"));
                }
            }
            Self::Bark {
                application_key_epoch,
                network: bark_network,
                ark_server_public_key,
                policy,
            } => {
                if *application_key_epoch == 0 || *bark_network != network {
                    return Err(invalid("Bark FROST application network or epoch mismatch"));
                }
                if !matches!(ark_server_public_key.as_ref(), PublicKey::Secp256k1(_)) {
                    return Err(invalid("Ark server identity must use secp256k1"));
                }
                policy.validate()?;
            }
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = vec![self.tag()];
        match self {
            Self::CdkOnchain {
                application_key_epoch,
                confirmation_policy_version,
                minimum_confirmations,
                fee_policy_version,
            } => {
                output.extend_from_slice(&application_key_epoch.to_be_bytes());
                output.extend_from_slice(&confirmation_policy_version.to_be_bytes());
                output.extend_from_slice(&minimum_confirmations.to_be_bytes());
                output.extend_from_slice(&fee_policy_version.to_be_bytes());
            }
            Self::Bark {
                application_key_epoch,
                network,
                ark_server_public_key,
                policy,
            } => {
                output.extend_from_slice(&application_key_epoch.to_be_bytes());
                output.push(network.discriminant());
                output.extend_from_slice(&ark_server_public_key.to_bytes());
                output.extend_from_slice(&policy.version.to_be_bytes());
                output.extend_from_slice(&policy.receive_base_fee_sat.to_be_bytes());
                output.extend_from_slice(&policy.receive_fee_ppm.to_be_bytes());
                output.extend_from_slice(&policy.maximum_outgoing_base_fee_sat.to_be_bytes());
                output.extend_from_slice(&policy.maximum_outgoing_fee_ppm.to_be_bytes());
                output.extend_from_slice(&policy.minimum_incoming_cltv_delta.to_be_bytes());
                output.extend_from_slice(&policy.minimum_outgoing_cltv_delta.to_be_bytes());
                output.extend_from_slice(&policy.refresh_before_expiry_blocks.to_be_bytes());
                output.extend_from_slice(&policy.emergency_exit_before_expiry_blocks.to_be_bytes());
                output.extend_from_slice(&policy.receive_secret_retention_days.to_be_bytes());
                output.push(match policy.receive_anti_dos {
                    super::FederationBarkReceiveAntiDosPolicy::NoProof => 1,
                    super::FederationBarkReceiveAntiDosPolicy::ProtectedToken => 2,
                    super::FederationBarkReceiveAntiDosPolicy::AcceptedVtxo => 3,
                });
            }
        }
        output
    }
}

/// FROST root and application inputs authorized by a setup proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupFrostPolicy {
    /// Initial reusable-root epoch.
    pub root_key_epoch: u64,
    /// Canonically ordered enabled applications.
    pub applications: Vec<FederationSetupFrostApplication>,
}

impl FederationSetupFrostPolicy {
    /// Construct and canonicalize a FROST setup policy.
    pub fn new(
        root_key_epoch: u64,
        mut applications: Vec<FederationSetupFrostApplication>,
        network: FederationSetupBitcoinNetwork,
    ) -> Result<Self> {
        applications.sort_by_key(FederationSetupFrostApplication::tag);
        let policy = Self {
            root_key_epoch,
            applications,
        };
        policy.validate(network)?;
        Ok(policy)
    }

    /// Validate root epoch, unique canonical applications, and application policy.
    pub fn validate(&self, network: FederationSetupBitcoinNetwork) -> Result<()> {
        if self.root_key_epoch == 0 {
            return Err(invalid("FROST root key epoch must be non-zero"));
        }
        if self.applications.is_empty() {
            return Err(invalid(
                "at least the CDK on-chain FROST application is required",
            ));
        }
        let tags = self
            .applications
            .iter()
            .map(FederationSetupFrostApplication::tag)
            .collect::<Vec<_>>();
        if !is_strictly_sorted(&tags) || tags.first() != Some(&1) {
            return Err(invalid(
                "FROST applications must be unique, canonical, and include CDK on-chain",
            ));
        }
        for application in &self.applications {
            application.validate(network)?;
        }
        Ok(())
    }

    fn append_canonical(&self, output: &mut Vec<u8>) -> Result<()> {
        output.extend_from_slice(&self.root_key_epoch.to_be_bytes());
        append_count(output, self.applications.len(), "FROST application")?;
        for application in &self.applications {
            output.extend_from_slice(&application.canonical_bytes());
        }
        Ok(())
    }
}

/// Complete canonical policy and enrollment set proposed for unanimous approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupProposalBody {
    /// Strict setup protocol version.
    pub version: u16,
    /// Setup attempt being authorized.
    pub setup_id: FederationSetupId,
    /// Ephemeral coordinator identity.
    pub coordinator_identity: FederationSetupCoordinatorPublicKey,
    /// Complete signed enrollments in member-ID order.
    pub enrollments: BTreeMap<MemberId, FederationSetupEnrollment>,
    /// Mint-signature and consensus thresholds.
    pub threshold_params: ThresholdParams,
    /// Exact protocol tuple required from every member.
    pub required_versions: FederationSetupProtocolVersions,
    /// Consensus Bitcoin network.
    pub network: FederationSetupBitcoinNetwork,
    /// Mint and melt payment-observation policy.
    pub payment_observation_policy: FederationPaymentObservationPolicy,
    /// Scheduled checkpoint policy.
    pub checkpoint_policy: FederationCheckpointPolicy,
    /// Canonically ordered BLS DKG keyset policies.
    pub keysets: Vec<FederationDkgKeysetSpec>,
    /// FROST root epoch and enabled application commitments.
    pub frost: FederationSetupFrostPolicy,
}

impl FederationSetupProposalBody {
    /// Construct, canonicalize, and validate one proposal body.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        setup_id: FederationSetupId,
        coordinator_identity: FederationSetupCoordinatorPublicKey,
        enrollments: BTreeMap<MemberId, FederationSetupEnrollment>,
        threshold_params: ThresholdParams,
        required_versions: FederationSetupProtocolVersions,
        network: FederationSetupBitcoinNetwork,
        payment_observation_policy: FederationPaymentObservationPolicy,
        checkpoint_policy: FederationCheckpointPolicy,
        keysets: Vec<FederationDkgKeysetSpec>,
        frost: FederationSetupFrostPolicy,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        let mut canonical_keysets = keysets
            .into_iter()
            .map(|keyset| canonical_keyset_bytes(&keyset).map(|bytes| (bytes, keyset)))
            .collect::<Result<Vec<_>>>()?;
        canonical_keysets.sort_by(|left, right| left.0.cmp(&right.0));
        let keysets = canonical_keysets
            .into_iter()
            .map(|(_, keyset)| keyset)
            .collect();
        let body = Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id,
            coordinator_identity,
            enrollments,
            threshold_params,
            required_versions,
            network,
            payment_observation_policy,
            checkpoint_policy,
            keysets,
            frost,
        };
        body.validate(url_policy)?;
        Ok(body)
    }

    /// Validate the complete roster, thresholds, policies, ordering, and size.
    pub fn validate(&self, url_policy: FederationSetupUrlPolicy) -> Result<()> {
        validate_setup_version("proposal", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        self.required_versions.validate()?;
        self.threshold_params.validate_bft_safety()?;
        let member_count = usize::from(self.threshold_params.member_count);
        if member_count != self.enrollments.len() {
            return Err(invalid(
                "proposal enrollment count does not match thresholds",
            ));
        }
        if member_count > FEDERATION_MAX_DKG_MEMBERS {
            return Err(FederationError::DkgMemberLimitExceeded {
                found: member_count,
                max: FEDERATION_MAX_DKG_MEMBERS,
            });
        }

        let mut urls = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut invitation_ids = BTreeSet::new();
        for expected in 1..=self.threshold_params.member_count {
            let member_id = MemberId::new(expected)?;
            let enrollment = self
                .enrollments
                .get(&member_id)
                .ok_or_else(|| invalid(format!("proposal is missing member {member_id}")))?;
            enrollment.validate(url_policy)?;
            if enrollment.setup_id != self.setup_id
                || enrollment.member_id != member_id
                || enrollment.supported_versions != self.required_versions
            {
                return Err(invalid(format!(
                    "enrollment binding mismatch for member {member_id}"
                )));
            }
            for url in [
                enrollment.public_mint_url.to_string(),
                enrollment.federation_api_url.to_string(),
            ] {
                if !urls.insert(url) {
                    return Err(invalid("proposal contains a duplicate member URL"));
                }
            }
            if !identities.insert(enrollment.identity_public_key.to_bytes()) {
                return Err(invalid("proposal contains a duplicate member identity"));
            }
            if !invitation_ids.insert(enrollment.invitation_id) {
                return Err(invalid("proposal contains a duplicate invitation ID"));
            }
            if enrollment.identity_public_key == self.coordinator_identity.public_key() {
                return Err(invalid(
                    "coordinator and member identities must use distinct keys",
                ));
            }
        }

        self.payment_observation_policy
            .validate(self.threshold_params)?;
        if matches!(
            self.payment_observation_policy.mint_quotes,
            FederationPaymentVerificationPolicy::DevelopmentSingleObservation
        ) || matches!(
            self.payment_observation_policy.melt_quotes,
            FederationPaymentVerificationPolicy::DevelopmentSingleObservation
        ) {
            return Err(invalid(
                "production setup requires member-quorum payment observation policies",
            ));
        }
        self.checkpoint_policy.validate()?;

        if self.keysets.is_empty() || self.keysets.len() > FEDERATION_MAX_SETUP_KEYSETS {
            return Err(invalid(format!(
                "setup keyset count must be between 1 and {FEDERATION_MAX_SETUP_KEYSETS}"
            )));
        }
        let mut previous = None;
        for keyset in &self.keysets {
            keyset.validate()?;
            if keyset.amounts.len() > FEDERATION_MAX_DKG_KEYSET_AMOUNTS {
                return Err(FederationError::DkgKeysetAmountLimitExceeded {
                    found: keyset.amounts.len(),
                    max: FEDERATION_MAX_DKG_KEYSET_AMOUNTS,
                });
            }
            let canonical = canonical_keyset_bytes(keyset)?;
            if previous.as_ref().is_some_and(|value| value >= &canonical) {
                return Err(invalid("setup keysets are not unique and canonical"));
            }
            previous = Some(canonical);
        }
        self.frost.validate(self.network)?;
        ensure_size(
            "proposal",
            self.canonical_bytes()?.len(),
            FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
        )
    }

    /// Return the exact binary proposal body committed by approvals.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.extend_from_slice(&self.version.to_be_bytes());
        output.extend_from_slice(&self.setup_id.to_bytes());
        output.extend_from_slice(&self.coordinator_identity.to_bytes());
        append_count(&mut output, self.enrollments.len(), "enrollment")?;
        for enrollment in self.enrollments.values() {
            append_bytes(&mut output, &enrollment.canonical_bytes()?)?;
        }
        output.extend_from_slice(&self.threshold_params.member_count.to_be_bytes());
        output.extend_from_slice(&self.threshold_params.signature_threshold.to_be_bytes());
        output.extend_from_slice(&self.threshold_params.consensus_threshold.to_be_bytes());
        self.required_versions.append_canonical(&mut output);
        output.push(self.network.discriminant());
        append_payment_policy(&mut output, self.payment_observation_policy.mint_quotes);
        append_payment_policy(&mut output, self.payment_observation_policy.melt_quotes);
        output.extend_from_slice(
            &self
                .checkpoint_policy
                .checkpoint_interval_journal_operations
                .to_be_bytes(),
        );
        output.extend_from_slice(
            &self
                .checkpoint_policy
                .checkpoint_interval_finalized_items
                .to_be_bytes(),
        );
        output.extend_from_slice(
            &self
                .checkpoint_policy
                .checkpoint_interval_canonical_bytes
                .to_be_bytes(),
        );
        append_count(&mut output, self.keysets.len(), "keyset")?;
        for keyset in &self.keysets {
            output.extend_from_slice(&canonical_keyset_bytes(keyset)?);
        }
        self.frost.append_canonical(&mut output)?;
        Ok(output)
    }

    /// Compute the canonical proposal hash.
    pub fn proposal_hash(&self) -> Result<FederationSetupProposalHash> {
        let body = self.canonical_bytes()?;
        Ok(FederationSetupProposalHash::from_bytes(hash_transcript(
            PROPOSAL_DOMAIN,
            &[&length_prefixed(&body)?],
        )))
    }

    /// Return the v1 DKG authorization value.
    pub fn setup_authorization_hash(&self) -> Result<FederationSetupAuthorizationHash> {
        Ok(self.proposal_hash()?.into())
    }

    /// Derive the domain-separated BLS/FROST ceremony identifier.
    pub fn ceremony_id(&self) -> Result<FederationDkgCeremonyId> {
        let proposal_hash = self.proposal_hash()?;
        Ok(FederationDkgCeremonyId::from_bytes(hash_transcript(
            CEREMONY_ID_DOMAIN,
            &[&self.setup_id.to_bytes(), &proposal_hash.to_bytes()],
        )))
    }

    /// Return the public configured-member roster authorized by this proposal.
    pub fn member_configurations(&self) -> BTreeMap<MemberId, MemberConfigPublic> {
        self.enrollments
            .iter()
            .map(|(member_id, enrollment)| {
                (
                    *member_id,
                    MemberConfigPublic {
                        member_id: *member_id,
                        public_mint_url: enrollment.public_mint_url.clone(),
                        federation_api_url: enrollment.federation_api_url.clone(),
                        identity_public_key: enrollment.identity_public_key,
                    },
                )
            })
            .collect()
    }
}

/// Coordinator-signed immutable proposal publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupProposal {
    /// Complete canonical proposal body.
    pub body: FederationSetupProposalBody,
    /// Canonical body hash approved by members.
    pub proposal_hash: FederationSetupProposalHash,
    /// Coordinator identity signature over publication context.
    pub coordinator_signature: FederationSetupSignature,
}

impl FederationSetupProposal {
    /// Sign and validate an immutable proposal publication.
    pub fn sign(
        body: FederationSetupProposalBody,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(body, coordinator_secret_key, url_policy, false)
    }

    /// Deterministically sign a frozen proposal test vector.
    pub fn sign_no_aux_rand_for_test_vector(
        body: FederationSetupProposalBody,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(body, coordinator_secret_key, url_policy, true)
    }

    fn sign_with(
        body: FederationSetupProposalBody,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
        deterministic: bool,
    ) -> Result<Self> {
        body.validate(url_policy)?;
        if coordinator_secret_key.public_key() != body.coordinator_identity.public_key() {
            return Err(invalid(
                "proposal coordinator secret does not match proposal identity",
            ));
        }
        let proposal_hash = body.proposal_hash()?;
        let transcript = proposal_publication_transcript(body.setup_id, proposal_hash);
        let coordinator_signature = match deterministic {
            true => FederationSetupSignature::sign_no_aux_rand_for_test_vector(
                &transcript,
                coordinator_secret_key,
            )?,
            false => FederationSetupSignature::sign(&transcript, coordinator_secret_key)?,
        };
        let proposal = Self {
            body,
            proposal_hash,
            coordinator_signature,
        };
        proposal.validate(url_policy)?;
        Ok(proposal)
    }

    /// Validate body hash and coordinator publication signature.
    pub fn validate(&self, url_policy: FederationSetupUrlPolicy) -> Result<()> {
        self.body.validate(url_policy)?;
        let expected = self.body.proposal_hash()?;
        if self.proposal_hash != expected {
            return Err(FederationError::SetupProposalHashMismatch {
                expected: expected.to_string(),
                found: self.proposal_hash.to_string(),
            });
        }
        self.coordinator_signature.verify(
            &proposal_publication_transcript(self.body.setup_id, self.proposal_hash),
            self.body.coordinator_identity.public_key(),
            "proposal publication",
        )
    }
}

/// One member's durable approval of an exact proposal hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupApproval {
    /// Strict setup protocol version.
    pub version: u16,
    /// Setup attempt being approved.
    pub setup_id: FederationSetupId,
    /// Exact proposal hash being approved.
    pub proposal_hash: FederationSetupProposalHash,
    /// Approving configured member.
    pub member_id: MemberId,
    /// Member identity signature.
    pub signature: FederationSetupSignature,
}

impl FederationSetupApproval {
    /// Sign an exact proposal approval using randomized BIP340 signing.
    pub fn sign(
        proposal: &FederationSetupProposal,
        member_id: MemberId,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(proposal, member_id, identity_secret_key, url_policy, false)
    }

    /// Deterministically sign a frozen approval test vector.
    pub fn sign_no_aux_rand_for_test_vector(
        proposal: &FederationSetupProposal,
        member_id: MemberId,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::sign_with(proposal, member_id, identity_secret_key, url_policy, true)
    }

    fn sign_with(
        proposal: &FederationSetupProposal,
        member_id: MemberId,
        identity_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
        deterministic: bool,
    ) -> Result<Self> {
        proposal.validate(url_policy)?;
        let enrollment = proposal
            .body
            .enrollments
            .get(&member_id)
            .ok_or_else(|| invalid(format!("proposal has no member {member_id}")))?;
        if identity_secret_key.public_key() != enrollment.identity_public_key {
            return Err(invalid("approval secret does not match enrolled identity"));
        }
        let transcript =
            approval_transcript(proposal.body.setup_id, proposal.proposal_hash, member_id);
        let signature = match deterministic {
            true => FederationSetupSignature::sign_no_aux_rand_for_test_vector(
                &transcript,
                identity_secret_key,
            )?,
            false => FederationSetupSignature::sign(&transcript, identity_secret_key)?,
        };
        let approval = Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id: proposal.body.setup_id,
            proposal_hash: proposal.proposal_hash,
            member_id,
            signature,
        };
        approval.validate(proposal)?;
        Ok(approval)
    }

    /// Validate exact setup/proposal/member binding and identity signature.
    pub fn validate(&self, proposal: &FederationSetupProposal) -> Result<()> {
        validate_setup_version("approval", self.version)?;
        if self.setup_id != proposal.body.setup_id || self.proposal_hash != proposal.proposal_hash {
            return Err(invalid("approval belongs to another setup or proposal"));
        }
        let enrollment = proposal
            .body
            .enrollments
            .get(&self.member_id)
            .ok_or_else(|| invalid(format!("approval has unknown member {}", self.member_id)))?;
        self.signature.verify(
            &approval_transcript(self.setup_id, self.proposal_hash, self.member_id),
            enrollment.identity_public_key,
            "approval",
        )
    }
}

/// Complete unanimously approved setup certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupManifest {
    /// Coordinator-signed immutable proposal.
    pub proposal: FederationSetupProposal,
    /// Exactly one valid approval per enrolled member.
    pub approvals: BTreeMap<MemberId, FederationSetupApproval>,
    /// Canonical approval-certificate hash.
    pub manifest_hash: FederationSetupManifestHash,
    /// Coordinator signature over the published complete certificate.
    pub coordinator_signature: FederationSetupSignature,
}

impl FederationSetupManifest {
    /// Certify a complete approval set using randomized BIP340 signing.
    pub fn certify(
        proposal: FederationSetupProposal,
        approvals: BTreeMap<MemberId, FederationSetupApproval>,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::certify_with(
            proposal,
            approvals,
            coordinator_secret_key,
            url_policy,
            false,
        )
    }

    /// Deterministically certify a frozen manifest test vector.
    pub fn certify_no_aux_rand_for_test_vector(
        proposal: FederationSetupProposal,
        approvals: BTreeMap<MemberId, FederationSetupApproval>,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        Self::certify_with(
            proposal,
            approvals,
            coordinator_secret_key,
            url_policy,
            true,
        )
    }

    fn certify_with(
        proposal: FederationSetupProposal,
        approvals: BTreeMap<MemberId, FederationSetupApproval>,
        coordinator_secret_key: &SecretKey,
        url_policy: FederationSetupUrlPolicy,
        deterministic: bool,
    ) -> Result<Self> {
        proposal.validate(url_policy)?;
        if coordinator_secret_key.public_key() != proposal.body.coordinator_identity.public_key() {
            return Err(invalid(
                "manifest coordinator secret does not match proposal identity",
            ));
        }
        validate_approvals(&proposal, &approvals)?;
        let manifest_hash = manifest_hash(&proposal, &approvals)?;
        let transcript = manifest_publication_transcript(
            proposal.body.setup_id,
            proposal.proposal_hash,
            manifest_hash,
        );
        let coordinator_signature = match deterministic {
            true => FederationSetupSignature::sign_no_aux_rand_for_test_vector(
                &transcript,
                coordinator_secret_key,
            )?,
            false => FederationSetupSignature::sign(&transcript, coordinator_secret_key)?,
        };
        let manifest = Self {
            proposal,
            approvals,
            manifest_hash,
            coordinator_signature,
        };
        manifest.validate(url_policy)?;
        Ok(manifest)
    }

    /// Validate proposal, unanimous approval certificate, hash, signature, and size.
    pub fn validate(&self, url_policy: FederationSetupUrlPolicy) -> Result<()> {
        self.proposal.validate(url_policy)?;
        validate_approvals(&self.proposal, &self.approvals)?;
        let expected = manifest_hash(&self.proposal, &self.approvals)?;
        if self.manifest_hash != expected {
            return Err(FederationError::SetupManifestHashMismatch {
                expected: expected.to_string(),
                found: self.manifest_hash.to_string(),
            });
        }
        self.coordinator_signature.verify(
            &manifest_publication_transcript(
                self.proposal.body.setup_id,
                self.proposal.proposal_hash,
                self.manifest_hash,
            ),
            self.proposal.body.coordinator_identity.public_key(),
            "manifest publication",
        )?;
        ensure_size(
            "manifest",
            self.canonical_bytes()?.len(),
            FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
        )
    }

    /// Return the setup authorization hash carried into DKG.
    pub fn setup_authorization_hash(&self) -> Result<FederationSetupAuthorizationHash> {
        self.proposal.body.setup_authorization_hash()
    }

    /// Return the domain-separated ceremony ID derived from the proposal.
    pub fn ceremony_id(&self) -> Result<FederationDkgCeremonyId> {
        self.proposal.body.ceremony_id()
    }

    /// Return the exact authorized BLS DKG keyset specifications.
    #[must_use]
    pub fn keyset_specs(&self) -> &[FederationDkgKeysetSpec] {
        &self.proposal.body.keysets
    }

    /// Return the exact authorized FROST root/application policy.
    #[must_use]
    pub const fn frost_policy(&self) -> &FederationSetupFrostPolicy {
        &self.proposal.body.frost
    }

    /// Derive the setup-only public federation configuration authorized by this manifest.
    pub fn setup_public_config(&self) -> Result<FederationConfig> {
        let mut config = FederationConfig::new(
            self.setup_authorization_hash()?,
            self.proposal.body.threshold_params,
            self.proposal.body.member_configurations(),
            Vec::new(),
        )?;
        config.payment_observation_policy = self.proposal.body.payment_observation_policy.clone();
        config.checkpoint_policy = self.proposal.body.checkpoint_policy;
        config.validate()?;
        Ok(config)
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let proposal_body = self.proposal.body.canonical_bytes()?;
        let mut output = Vec::new();
        append_bytes(&mut output, &proposal_body)?;
        output.extend_from_slice(&self.proposal.proposal_hash.to_bytes());
        output.extend_from_slice(&self.proposal.coordinator_signature.to_bytes());
        append_count(&mut output, self.approvals.len(), "approval")?;
        for approval in self.approvals.values() {
            output.extend_from_slice(&approval.member_id.get().to_be_bytes());
            output.extend_from_slice(&approval.signature.to_bytes());
        }
        output.extend_from_slice(&self.manifest_hash.to_bytes());
        output.extend_from_slice(&self.coordinator_signature.to_bytes());
        Ok(output)
    }
}

fn validate_approvals(
    proposal: &FederationSetupProposal,
    approvals: &BTreeMap<MemberId, FederationSetupApproval>,
) -> Result<()> {
    if approvals.len() != proposal.body.enrollments.len() {
        return Err(invalid(
            "manifest does not contain exactly one approval per member",
        ));
    }
    for member_id in proposal.body.enrollments.keys() {
        let approval = approvals.get(member_id).ok_or_else(|| {
            invalid(format!(
                "manifest is missing approval from member {member_id}"
            ))
        })?;
        if approval.member_id != *member_id {
            return Err(invalid("manifest approval map key does not match signer"));
        }
        approval.validate(proposal)?;
    }
    Ok(())
}

fn manifest_hash(
    proposal: &FederationSetupProposal,
    approvals: &BTreeMap<MemberId, FederationSetupApproval>,
) -> Result<FederationSetupManifestHash> {
    let mut body = Vec::new();
    body.extend_from_slice(&proposal.body.setup_id.to_bytes());
    body.extend_from_slice(&proposal.proposal_hash.to_bytes());
    append_count(&mut body, approvals.len(), "approval")?;
    for (member_id, approval) in approvals {
        body.extend_from_slice(&member_id.get().to_be_bytes());
        body.extend_from_slice(&approval.signature.to_bytes());
    }
    Ok(FederationSetupManifestHash::from_bytes(hash_transcript(
        MANIFEST_DOMAIN,
        &[&body],
    )))
}

fn proposal_publication_transcript(
    setup_id: FederationSetupId,
    proposal_hash: FederationSetupProposalHash,
) -> Vec<u8> {
    transcript(
        PROPOSAL_PUBLICATION_DOMAIN,
        &[
            &setup_id.to_bytes(),
            &proposal_hash.to_bytes(),
            &FEDERATION_SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
}

fn approval_transcript(
    setup_id: FederationSetupId,
    proposal_hash: FederationSetupProposalHash,
    member_id: MemberId,
) -> Vec<u8> {
    transcript(
        APPROVAL_DOMAIN,
        &[
            &setup_id.to_bytes(),
            &proposal_hash.to_bytes(),
            &member_id.get().to_be_bytes(),
            &FEDERATION_SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
}

fn manifest_publication_transcript(
    setup_id: FederationSetupId,
    proposal_hash: FederationSetupProposalHash,
    manifest_hash: FederationSetupManifestHash,
) -> Vec<u8> {
    transcript(
        MANIFEST_PUBLICATION_DOMAIN,
        &[
            &setup_id.to_bytes(),
            &proposal_hash.to_bytes(),
            &manifest_hash.to_bytes(),
            &FEDERATION_SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
}

fn canonical_keyset_bytes(keyset: &FederationDkgKeysetSpec) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    append_string(&mut output, &keyset.unit.to_string())?;
    output.push(u8::from(keyset.active));
    output.extend_from_slice(&keyset.input_fee_ppk.to_be_bytes());
    match keyset.final_expiry {
        Some(expiry) => {
            output.push(1);
            output.extend_from_slice(&expiry.to_be_bytes());
        }
        None => output.push(0),
    }
    append_count(&mut output, keyset.amounts.len(), "keyset amount")?;
    for amount in &keyset.amounts {
        output.extend_from_slice(&u64::from(*amount).to_be_bytes());
    }
    Ok(output)
}

fn append_payment_policy(output: &mut Vec<u8>, policy: FederationPaymentVerificationPolicy) {
    match policy {
        FederationPaymentVerificationPolicy::MemberQuorum { required } => {
            output.push(1);
            output.extend_from_slice(&required.to_be_bytes());
        }
        FederationPaymentVerificationPolicy::DevelopmentSingleObservation => output.push(2),
    }
}

fn validate_setup_url(
    value: &MintUrl,
    policy: FederationSetupUrlPolicy,
    context: &'static str,
) -> Result<()> {
    let normalized = value.to_string();
    ensure_size(context, normalized.len(), FEDERATION_MAX_SETUP_URL_BYTES)?;
    let parsed =
        Url::parse(&normalized).map_err(|_| invalid(format!("{context} URL is not absolute")))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.host().is_none()
    {
        return Err(invalid(format!(
            "{context} URL must not contain credentials, query, or fragment"
        )));
    }
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if policy == FederationSetupUrlPolicy::AllowInsecureLocalhost => {
            match parsed.host() {
                Some(Host::Domain(host)) if host.eq_ignore_ascii_case("localhost") => Ok(()),
                Some(Host::Ipv4(address)) if address.is_loopback() => Ok(()),
                Some(Host::Ipv6(address)) if address.is_loopback() => Ok(()),
                Some(_) | None => Err(invalid(format!(
                    "{context} insecure URL must use a loopback host"
                ))),
            }
        }
        _ => Err(invalid(format!("{context} URL must use HTTPS"))),
    }
}

fn validate_setup_version(component: &'static str, found: u16) -> Result<()> {
    if found != FEDERATION_SETUP_PROTOCOL_VERSION {
        return Err(FederationError::SetupVersionMismatch {
            component,
            expected: FEDERATION_SETUP_PROTOCOL_VERSION,
            found,
        });
    }
    Ok(())
}

fn validate_nonzero_identifier(context: &'static str, value: [u8; 32]) -> Result<()> {
    if value == [0; 32] {
        return Err(invalid(format!("{context} must not be all zero")));
    }
    Ok(())
}

fn ensure_size(artifact: &'static str, found: usize, max: usize) -> Result<()> {
    if found > max {
        return Err(FederationError::SetupSizeLimitExceeded {
            artifact,
            found,
            max,
        });
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> FederationError {
    FederationError::InvalidSetup {
        reason: reason.into(),
    }
}

fn invitation_checksum(public: &[u8], capability: [u8; 32]) -> Result<[u8; 6]> {
    let public = length_prefixed(public)?;
    let hash = hash_transcript(INVITATION_CHECKSUM_DOMAIN, &[&public, &capability]);
    hash[..6]
        .try_into()
        .map_err(|_| invalid("setup invitation checksum length is invalid"))
}

fn take_setup_bytes<const N: usize>(input: &mut &[u8], field: &'static str) -> Result<[u8; N]> {
    if input.len() < N {
        return Err(invalid(format!("setup invitation {field} is truncated")));
    }
    let (value, remaining) = input.split_at(N);
    *input = remaining;
    value
        .try_into()
        .map_err(|_| invalid(format!("setup invitation {field} has an invalid length")))
}

fn take_length_prefixed(input: &mut &[u8], field: &'static str) -> Result<Vec<u8>> {
    let length = u32::from_be_bytes(take_setup_bytes(input, field)?) as usize;
    if length > input.len() {
        return Err(invalid(format!("setup invitation {field} is truncated")));
    }
    let (value, remaining) = input.split_at(length);
    *input = remaining;
    Ok(value.to_vec())
}

fn append_string(output: &mut Vec<u8>, value: &str) -> Result<()> {
    append_bytes(output, value.as_bytes())
}

fn append_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| invalid("canonical setup field length does not fit u32"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn append_count(output: &mut Vec<u8>, count: usize, context: &'static str) -> Result<()> {
    let count =
        u16::try_from(count).map_err(|_| invalid(format!("{context} count does not fit u16")))?;
    output.extend_from_slice(&count.to_be_bytes());
    Ok(())
}

fn length_prefixed(value: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    append_bytes(&mut output, value)?;
    Ok(output)
}

fn transcript(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::new();
    let domain_length = u32::try_from(domain.len()).expect("fixed setup domain length fits u32");
    output.extend_from_slice(&domain_length.to_be_bytes());
    output.extend_from_slice(domain);
    for field in fields {
        output.extend_from_slice(field);
    }
    output
}

fn hash_transcript(domain: &[u8], fields: &[&[u8]]) -> [u8; 32] {
    Sha256Hash::hash(&transcript(domain, fields)).to_byte_array()
}

fn is_strictly_sorted<T>(values: &[T]) -> bool
where
    T: Ord,
{
    values.windows(2).all(|pair| pair[0] < pair[1])
}
