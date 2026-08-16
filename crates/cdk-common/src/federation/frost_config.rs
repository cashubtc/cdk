//! Versioned public and participant-local configuration for federated FROST wallets.

use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::bip32::{ChainCode, ChildNumber, Fingerprint, Xpub};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, Network};
use cdk_frost::{
    derive_application_public, ApplicationAuthorization, ApplicationId, ApplicationKeyEpoch,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest,
    CanonicalRootKeyPackage, DerivedPublicPackage, EncryptedSecret, FrostCiphersuite,
    RootPublicPackage, SecretEncryptionKey,
};
use cdk_frost_bip32::{
    application_chain_code, application_registration_for_epoch, FrostBip32,
    APPLICATION_DERIVATION_VERSION,
};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{
    append_bytes, FederationAddressKeychain, FederationCdkOnchainFrostAuthorization,
    FederationConfig, FederationDkgCeremonyId, FederationError, FederationFrostRootRecord,
    FederationId, FederationSetupAuthorizationHash, MemberId, Result,
};

const ROOT_CONFIG_VERSION: u16 = 2;
const ROOT_PRIVATE_CONFIG_VERSION: u16 = 1;
const APPLICATION_CONFIG_VERSION: u16 = 1;
const WALLET_CONFIG_VERSION: u16 = 3;
const EXTENSION_APPLICATION_CONFIG_VERSION: u16 = 1;
const CONFIRMATION_POLICY_VERSION: u16 = 1;
const SEALED_STORAGE_VERSION: u16 = 1;
const ROOT_CONFIG_DIGEST_DOMAIN: &[u8] = b"cdk/federation/frost-root-config/v1";
const DESCRIPTOR_COMMITMENT_DOMAIN: &[u8] = b"cdk/federation/frost-descriptors/v1";
const WALLET_CONFIG_DIGEST_DOMAIN: &[u8] = b"cdk/federation/frost-wallet-config/v1";
/// Current participant-local sealed storage provider used by the DKG boundary.
pub const MEMBER_IDENTITY_DERIVED_STORAGE_PROVIDER: &str = "member_identity_derived_v1";

/// Stable public identifier for a registered FROST application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationFrostApplicationId([u8; 32]);

impl FederationFrostApplicationId {
    /// Construct from canonical non-zero bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        ApplicationId::from_bytes(bytes).map_err(frost_error)?;
        Ok(Self(bytes))
    }

    /// Canonical bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Lowercase hexadecimal representation.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn to_frost(self) -> Result<ApplicationId> {
        ApplicationId::from_bytes(self.0).map_err(frost_error)
    }
}

impl fmt::Display for FederationFrostApplicationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for FederationFrostApplicationId {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        let bytes = crate::util::hex::decode(value)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            invalid_config(format!(
                "application id must be 32 bytes, found {}",
                bytes.len()
            ))
        })?;
        Self::from_bytes(bytes)
    }
}

impl Serialize for FederationFrostApplicationId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationFrostApplicationId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

/// Canonical public root snapshot shared by every member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRootPublicConfig {
    /// Config schema version.
    pub version: u16,
    /// Owning federation.
    pub federation_id: FederationId,
    /// Certified setup authorization that permitted the root DKG.
    pub setup_authorization: FederationSetupAuthorizationHash,
    /// Ceremony that produced this root.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Root key epoch.
    pub root_key_epoch: u64,
    /// Canonical root DKG protocol version.
    pub dkg_protocol_version: u16,
    /// Canonical ciphersuite version (`1` is secp256k1/SHA-256 Taproot FROST).
    pub ciphersuite_version: u16,
    /// Exact federation signing threshold.
    pub threshold: u16,
    /// Explicit federation-member to FROST-identifier mapping.
    pub participant_mapping: BTreeMap<MemberId, u16>,
    /// Public verification shares keyed by federation member.
    #[serde(with = "point_map_serde")]
    pub verification_shares: BTreeMap<MemberId, [u8; 33]>,
    /// Untweaked aggregate compressed secp256k1 key.
    #[serde(with = "point_serde")]
    pub untweaked_group_key: [u8; 33],
    /// Root ceremony transcript binding.
    pub transcript_binding: [u8; 32],
    /// Canonical portable `cdk-frost` root package.
    pub canonical_public_package: Vec<u8>,
}

impl FederationFrostRootPublicConfig {
    /// Construct from one independently validated local DKG root record.
    pub fn from_dkg_record(
        federation: &FederationConfig,
        record: &FederationFrostRootRecord,
    ) -> Result<Self> {
        let public = record.validate(federation)?;
        let verification_shares = public
            .verifying_shares()
            .iter()
            .map(|(member, share)| Ok((common_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let config = Self {
            version: ROOT_CONFIG_VERSION,
            federation_id: federation.federation_id,
            setup_authorization: federation.setup_authorization,
            ceremony_id: record.context.ceremony_id,
            root_key_epoch: public.epoch().get(),
            dkg_protocol_version: public.dkg_protocol_version(),
            ciphersuite_version: ciphersuite_version(public.ciphersuite()),
            threshold: public.threshold(),
            participant_mapping: federation
                .members
                .keys()
                .map(|member| (*member, member.get()))
                .collect(),
            verification_shares,
            untweaked_group_key: public.group_key(),
            transcript_binding: public.transcript_binding(),
            canonical_public_package: public.encode(),
        };
        config.validate(federation)?;
        Ok(config)
    }

    /// Decode and validate the canonical root package against every explicit field.
    pub fn root_public_package(&self, federation: &FederationConfig) -> Result<RootPublicPackage> {
        self.validate_metadata(federation)?;
        let public =
            RootPublicPackage::decode(&self.canonical_public_package).map_err(frost_error)?;
        let shares = public
            .verifying_shares()
            .iter()
            .map(|(member, share)| Ok((common_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if public.epoch().get() != self.root_key_epoch
            || public.dkg_protocol_version() != self.dkg_protocol_version
            || ciphersuite_version(public.ciphersuite()) != self.ciphersuite_version
            || public.threshold() != self.threshold
            || shares != self.verification_shares
            || public.group_key() != self.untweaked_group_key
            || public.transcript_binding() != self.transcript_binding
        {
            return Err(invalid_config(
                "canonical root package differs from explicit root fields",
            ));
        }
        Ok(public)
    }

    /// Validate this public root against the federation roster and threshold.
    pub fn validate(&self, federation: &FederationConfig) -> Result<()> {
        self.root_public_package(federation).map(|_| ())
    }

    /// Canonical digest used by private bindings and materialized state.
    pub fn digest(&self) -> Result<[u8; 32]> {
        let encoded = serde_json::to_vec(self)?;
        Ok(domain_hash(ROOT_CONFIG_DIGEST_DOMAIN, &encoded))
    }

    fn validate_metadata(&self, federation: &FederationConfig) -> Result<()> {
        if self.version != ROOT_CONFIG_VERSION {
            return Err(invalid_config("unsupported public root config version"));
        }
        if self.federation_id != federation.federation_id {
            return Err(invalid_config("root config federation id mismatch"));
        }
        if self.setup_authorization != federation.setup_authorization {
            return Err(invalid_config("root config setup authorization mismatch"));
        }
        if self.root_key_epoch == 0
            || self.threshold != federation.threshold_params.signature_threshold
        {
            return Err(invalid_config("root epoch or signing threshold mismatch"));
        }
        if self
            .participant_mapping
            .iter()
            .any(|(member, identifier)| member.get() != *identifier)
            || self
                .participant_mapping
                .keys()
                .copied()
                .collect::<BTreeSet<_>>()
                != federation.members.keys().copied().collect()
        {
            return Err(invalid_config("root participant mapping mismatch"));
        }
        if self
            .verification_shares
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != federation.members.keys().copied().collect()
        {
            return Err(invalid_config("root verification-share roster mismatch"));
        }
        if self.transcript_binding == [0; 32] {
            return Err(invalid_config("root transcript binding is empty"));
        }
        Ok(())
    }
}

/// Metadata identifying how a local root share is sealed without containing a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostSealedStorageMetadata {
    /// Metadata schema version.
    pub version: u16,
    /// Sealed-storage provider identifier.
    pub provider: String,
    /// Non-secret key identifier/binding.
    pub key_id: [u8; 32],
}

impl FederationFrostSealedStorageMetadata {
    /// Metadata for the current participant identity-derived local provider.
    #[must_use]
    pub fn member_identity_derived(
        federation_id: FederationId,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        root_key_epoch: u64,
    ) -> Self {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&federation_id.to_bytes());
        bytes.extend_from_slice(&ceremony_id.to_bytes());
        bytes.extend_from_slice(&member_id.get().to_be_bytes());
        bytes.extend_from_slice(&root_key_epoch.to_be_bytes());
        Self {
            version: SEALED_STORAGE_VERSION,
            provider: MEMBER_IDENTITY_DERIVED_STORAGE_PROVIDER.to_string(),
            key_id: domain_hash(b"cdk/federation/frost-sealed-key-id/v1", &bytes),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version != SEALED_STORAGE_VERSION
            || self.provider != MEMBER_IDENTITY_DERIVED_STORAGE_PROVIDER
            || self.key_id == [0; 32]
        {
            return Err(invalid_config("unsupported sealed root storage metadata"));
        }
        Ok(())
    }
}

/// Participant-local sealed root configuration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostRootPrivateConfig {
    /// Private config schema version.
    pub version: u16,
    /// Owning federation.
    pub federation_id: FederationId,
    /// Local member owner.
    pub member_id: MemberId,
    /// Root epoch.
    pub root_key_epoch: u64,
    /// Digest of the matching public root config.
    pub public_root_digest: [u8; 32],
    /// This member's public verification share.
    #[serde(with = "point_serde")]
    pub local_verification_share: [u8; 33],
    /// Authenticated ciphertext containing only this member's root signing share.
    pub encrypted_root_share: Vec<u8>,
    /// Provider/version/key identifier metadata; never the sealing key.
    pub sealed_storage: FederationFrostSealedStorageMetadata,
}

impl fmt::Debug for FederationFrostRootPrivateConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederationFrostRootPrivateConfig")
            .field("version", &self.version)
            .field("federation_id", &self.federation_id)
            .field("member_id", &self.member_id)
            .field("root_key_epoch", &self.root_key_epoch)
            .field("public_root_digest", &self.public_root_digest)
            .field("local_verification_share", &self.local_verification_share)
            .field("encrypted_root_share", &"<redacted>")
            .field("sealed_storage", &self.sealed_storage)
            .finish()
    }
}

impl FederationFrostRootPrivateConfig {
    /// Construct a local private config from the DKG root record.
    pub fn from_dkg_record(
        public: &FederationFrostRootPublicConfig,
        record: &FederationFrostRootRecord,
    ) -> Result<Self> {
        let local_verification_share =
            public
                .verification_shares
                .get(&record.member_id)
                .copied()
                .ok_or_else(|| invalid_config("local verification share is missing"))?;
        let config = Self {
            version: ROOT_PRIVATE_CONFIG_VERSION,
            federation_id: public.federation_id,
            member_id: record.member_id,
            root_key_epoch: public.root_key_epoch,
            public_root_digest: public.digest()?,
            local_verification_share,
            encrypted_root_share: record.encrypted_root_share.clone(),
            sealed_storage: FederationFrostSealedStorageMetadata::member_identity_derived(
                public.federation_id,
                public.ceremony_id,
                record.member_id,
                public.root_key_epoch,
            ),
        };
        config.validate_metadata(public, record.member_id)?;
        Ok(config)
    }

    /// Validate metadata and reopen the local canonical root share.
    pub fn open(
        &self,
        federation: &FederationConfig,
        public: &FederationFrostRootPublicConfig,
        expected_member: MemberId,
        key: &SecretEncryptionKey,
    ) -> Result<CanonicalRootKeyPackage> {
        self.validate_metadata(public, expected_member)?;
        let root = public.root_public_package(federation)?;
        let encrypted =
            EncryptedSecret::from_bytes(self.encrypted_root_share.clone()).map_err(frost_error)?;
        CanonicalRootKeyPackage::open(frost_member(expected_member)?, root, key, &encrypted)
            .map_err(frost_error)
    }

    /// Validate metadata without decrypting the root share.
    pub fn validate_metadata(
        &self,
        public: &FederationFrostRootPublicConfig,
        expected_member: MemberId,
    ) -> Result<()> {
        if self.version != ROOT_PRIVATE_CONFIG_VERSION
            || self.federation_id != public.federation_id
            || self.member_id != expected_member
            || self.root_key_epoch != public.root_key_epoch
            || self.public_root_digest != public.digest()?
            || public.verification_shares.get(&expected_member)
                != Some(&self.local_verification_share)
        {
            return Err(invalid_config("private root binding mismatch"));
        }
        self.sealed_storage.validate()?;
        let expected_storage = FederationFrostSealedStorageMetadata::member_identity_derived(
            self.federation_id,
            public.ceremony_id,
            self.member_id,
            self.root_key_epoch,
        );
        if self.sealed_storage != expected_storage {
            return Err(invalid_config(
                "private root sealed-storage key binding mismatch",
            ));
        }
        EncryptedSecret::from_bytes(self.encrypted_root_share.clone()).map_err(frost_error)?;
        Ok(())
    }
}

/// Consensus-visible confirmation rule for federated on-chain funds and settlements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainConfirmationPolicy {
    /// Policy schema version.
    pub version: u16,
    /// Blocks required before a deposit or outgoing melt may become payable.
    pub minimum_confirmations: u32,
}

impl FederationOnchainConfirmationPolicy {
    /// Construct the current confirmation policy.
    pub fn new(minimum_confirmations: u32) -> Result<Self> {
        if minimum_confirmations == 0 {
            return Err(invalid_config("minimum confirmations must be non-zero"));
        }
        Ok(Self {
            version: CONFIRMATION_POLICY_VERSION,
            minimum_confirmations,
        })
    }

    fn validate(self) -> Result<()> {
        if self.version != CONFIRMATION_POLICY_VERSION || self.minimum_confirmations == 0 {
            return Err(invalid_config("unsupported on-chain confirmation policy"));
        }
        Ok(())
    }
}

/// Versioned public CDK on-chain application key configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostApplicationConfig {
    /// Application config schema version.
    pub version: u16,
    /// Stable CDK on-chain application identifier.
    pub application_id: FederationFrostApplicationId,
    /// Application key epoch.
    pub application_key_epoch: u64,
    /// Referenced root epoch.
    pub root_key_epoch: u64,
    /// Additive application derivation contract version.
    pub derivation_version: u16,
    /// Authorization policy commitment.
    pub authorization_policy: [u8; 32],
    /// Derived application signing threshold.
    pub threshold: u16,
    /// Derived application verification shares.
    #[serde(with = "point_map_serde")]
    pub verification_shares: BTreeMap<MemberId, [u8; 33]>,
    /// Derived application aggregate key before BIP32 child derivation.
    #[serde(with = "point_serde")]
    pub derived_group_key: [u8; 33],
    /// Deterministic BIP32 chain code.
    pub chain_code: [u8; 32],
    /// External watch-only Taproot descriptor.
    pub external_descriptor: String,
    /// Internal/change watch-only Taproot descriptor.
    pub internal_descriptor: String,
    /// Commitment to both descriptors, network, and application scope.
    pub descriptor_commitment: [u8; 32],
    /// Consensus Bitcoin network.
    pub network: Network,
    /// Consensus deposit confirmation rule.
    pub confirmation_policy: FederationOnchainConfirmationPolicy,
    /// Version of the consensus fee policy interpreted by transaction validation.
    pub fee_policy_version: u16,
}

/// Application-neutral public registration derived from the reusable FROST root.
///
/// The generic federation layer validates the root derivation and stores a
/// bounded typed configuration value. The owning application crate validates
/// and interprets that value; `cdk-common` never imports backend code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostExtensionApplicationConfig {
    /// Generic extension-registration schema version.
    pub version: u16,
    /// Stable application identifier.
    pub application_id: FederationFrostApplicationId,
    /// Application key epoch.
    pub application_key_epoch: u64,
    /// Referenced reusable-root epoch.
    pub root_key_epoch: u64,
    /// Additive application derivation contract version.
    pub derivation_version: u16,
    /// Commitment to the application's typed authorization policy.
    pub authorization_policy: [u8; 32],
    /// Derived application signing threshold.
    pub threshold: u16,
    /// Derived application verification shares.
    #[serde(with = "point_map_serde")]
    pub verification_shares: BTreeMap<MemberId, [u8; 33]>,
    /// Derived application aggregate key before application-specific derivation.
    #[serde(with = "point_serde")]
    pub derived_group_key: [u8; 33],
    /// Application-owned public configuration.
    pub configuration: serde_json::Value,
}

impl FederationFrostExtensionApplicationConfig {
    /// Derive one generic application registration from the reusable root.
    pub fn new<T>(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        registration: ApplicationRegistration,
        configuration: &T,
    ) -> Result<Self>
    where
        T: Serialize + ?Sized,
    {
        let root_public = root.root_public_package(federation)?;
        let derived = derive_application_public(&root_public, registration).map_err(frost_error)?;
        let config = Self {
            version: EXTENSION_APPLICATION_CONFIG_VERSION,
            application_id: FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?,
            application_key_epoch: registration.key_epoch().get(),
            root_key_epoch: root.root_key_epoch,
            derivation_version: registration.derivation_version().get(),
            authorization_policy: registration.authorization_policy(),
            threshold: derived.threshold(),
            verification_shares: derived
                .verifying_shares()
                .iter()
                .map(|(member, share)| Ok((common_member(*member)?, *share)))
                .collect::<Result<BTreeMap<_, _>>>()?,
            derived_group_key: derived.group_key(),
            configuration: serde_json::to_value(configuration)?,
        };
        config.validate(federation, root)?;
        Ok(config)
    }

    /// Decode the application-owned public configuration.
    pub fn decode_configuration<T>(&self) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_value(self.configuration.clone()).map_err(FederationError::from)
    }

    /// Validate the generic registration and reconstruct its derived public package.
    pub fn derived_public_package(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<DerivedPublicPackage> {
        self.validate(federation, root)?;
        let shares = self
            .verification_shares
            .iter()
            .map(|(member, share)| Ok((frost_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        DerivedPublicPackage::new(
            cdk_frost::RootKeyEpoch::new(self.root_key_epoch).map_err(frost_error)?,
            self.registration()?,
            self.threshold,
            shares,
            self.derived_group_key,
        )
        .map_err(frost_error)
    }

    /// Validate this registration against the reusable root and roster.
    pub fn validate(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<()> {
        if self.version != EXTENSION_APPLICATION_CONFIG_VERSION
            || self.application_key_epoch == 0
            || self.root_key_epoch != root.root_key_epoch
            || self.configuration.is_null()
        {
            return Err(invalid_config(
                "extension application config version, epoch, or payload is invalid",
            ));
        }
        let root_public = root.root_public_package(federation)?;
        let expected =
            derive_application_public(&root_public, self.registration()?).map_err(frost_error)?;
        let expected_shares = expected
            .verifying_shares()
            .iter()
            .map(|(member, share)| Ok((common_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if self.threshold != federation.threshold_params.signature_threshold
            || self.threshold != expected.threshold()
            || self.verification_shares != expected_shares
            || self.derived_group_key != expected.group_key()
        {
            return Err(invalid_config(
                "extension application derivation differs from root",
            ));
        }
        Ok(())
    }

    /// Reconstruct the canonical application registration.
    pub fn registration(&self) -> Result<ApplicationRegistration> {
        ApplicationRegistration::new(
            self.application_id.to_frost()?,
            ApplicationKeyEpoch::new(self.application_key_epoch).map_err(frost_error)?,
            cdk_frost::ApplicationDerivationVersion::new(self.derivation_version)
                .map_err(frost_error)?,
            self.authorization_policy,
        )
        .map_err(frost_error)
    }
}

/// Reviewed CDK on-chain application adapter for the generic FROST key manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationCdkOnchainFrostAuthorizationAdapter {
    registration: ApplicationRegistration,
}

impl FederationCdkOnchainFrostAuthorizationAdapter {
    /// Construct an adapter only from a validated application registration.
    pub fn new(application: &FederationFrostApplicationConfig) -> Result<Self> {
        Ok(Self {
            registration: application.registration()?,
        })
    }
}

impl ApplicationAuthorization for FederationCdkOnchainFrostAuthorizationAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(
        &self,
        request: &AuthorizationRequest,
    ) -> std::result::Result<AuthorizedSigningRequest, cdk_frost::Error> {
        if request.application_id() != self.registration.id()
            || request.application_key_epoch() != self.registration.key_epoch()
            || request.derivation_version() != self.registration.derivation_version()
        {
            return Err(cdk_frost::Error::Application(
                "CDK authorization uses an unregistered application epoch".to_string(),
            ));
        }
        let authorization = FederationCdkOnchainFrostAuthorization::from_bytes(request.payload())
            .map_err(|error| cdk_frost::Error::Application(error.to_string()))?;
        let derivation = authorization
            .signing_derivation()
            .map_err(|error| cdk_frost::Error::Application(error.to_string()))?;
        Ok(AuthorizedSigningRequest::new(
            authorization.message,
            derivation,
        ))
    }
}

impl FederationFrostApplicationConfig {
    /// Derive a complete CDK on-chain public application config from a root.
    pub fn new_cdk_onchain(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        application_key_epoch: u64,
        network: Network,
        confirmation_policy: FederationOnchainConfirmationPolicy,
        fee_policy_version: u16,
    ) -> Result<Self> {
        let registration =
            application_registration_for_epoch(application_key_epoch).map_err(bip32_error)?;
        let root_public = root.root_public_package(federation)?;
        let derived = derive_application_public(&root_public, registration).map_err(frost_error)?;
        let adapter = FrostBip32::new(derived.clone(), network).map_err(bip32_error)?;
        let descriptors = adapter.descriptors();
        let application_id =
            FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?;
        let verification_shares = derived
            .verifying_shares()
            .iter()
            .map(|(member, share)| Ok((common_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let chain_code = application_chain_code(&derived);
        let descriptor_commitment = descriptor_commitment(
            application_id,
            application_key_epoch,
            root.root_key_epoch,
            network,
            &descriptors.external,
            &descriptors.internal,
        );
        let config = Self {
            version: APPLICATION_CONFIG_VERSION,
            application_id,
            application_key_epoch,
            root_key_epoch: root.root_key_epoch,
            derivation_version: registration.derivation_version().get(),
            authorization_policy: registration.authorization_policy(),
            threshold: derived.threshold(),
            verification_shares,
            derived_group_key: derived.group_key(),
            chain_code,
            external_descriptor: descriptors.external,
            internal_descriptor: descriptors.internal,
            descriptor_commitment,
            network,
            confirmation_policy,
            fee_policy_version,
        };
        config.validate(federation, root)?;
        Ok(config)
    }

    /// Reconstruct the public application package after full validation.
    pub fn derived_public_package(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<DerivedPublicPackage> {
        self.validate(federation, root)?;
        let registration = self.registration()?;
        let shares = self
            .verification_shares
            .iter()
            .map(|(member, share)| Ok((frost_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        DerivedPublicPackage::new(
            cdk_frost::RootKeyEpoch::new(self.root_key_epoch).map_err(frost_error)?,
            registration,
            self.threshold,
            shares,
            self.derived_group_key,
        )
        .map_err(frost_error)
    }

    /// Fail closed on any root, derivation, descriptor, network, or policy mismatch.
    pub fn validate(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<()> {
        if self.version != APPLICATION_CONFIG_VERSION
            || self.application_key_epoch == 0
            || self.root_key_epoch != root.root_key_epoch
            || self.fee_policy_version == 0
        {
            return Err(invalid_config(
                "application config version or epoch mismatch",
            ));
        }
        self.confirmation_policy.validate()?;
        let registration = self.registration()?;
        let expected_registration =
            application_registration_for_epoch(self.application_key_epoch).map_err(bip32_error)?;
        if registration != expected_registration
            || self.derivation_version != APPLICATION_DERIVATION_VERSION
        {
            return Err(invalid_config(
                "unregistered or unsupported CDK application",
            ));
        }
        let root_public = root.root_public_package(federation)?;
        let expected =
            derive_application_public(&root_public, registration).map_err(frost_error)?;
        let expected_shares = expected
            .verifying_shares()
            .iter()
            .map(|(member, share)| Ok((common_member(*member)?, *share)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if self.threshold != federation.threshold_params.signature_threshold
            || self.threshold != expected.threshold()
            || self.verification_shares != expected_shares
            || self.derived_group_key != expected.group_key()
            || self.chain_code != application_chain_code(&expected)
        {
            return Err(invalid_config("application derivation differs from root"));
        }
        let adapter = FrostBip32::new(expected, self.network).map_err(bip32_error)?;
        let descriptors = adapter.descriptors();
        let commitment = descriptor_commitment(
            self.application_id,
            self.application_key_epoch,
            self.root_key_epoch,
            self.network,
            &descriptors.external,
            &descriptors.internal,
        );
        if self.external_descriptor != descriptors.external
            || self.internal_descriptor != descriptors.internal
            || self.descriptor_commitment != commitment
        {
            return Err(invalid_config(
                "BDK descriptor or network commitment mismatch",
            ));
        }
        Ok(())
    }

    fn registration(&self) -> Result<ApplicationRegistration> {
        ApplicationRegistration::new(
            self.application_id.to_frost()?,
            ApplicationKeyEpoch::new(self.application_key_epoch).map_err(frost_error)?,
            cdk_frost::ApplicationDerivationVersion::new(self.derivation_version)
                .map_err(frost_error)?,
            self.authorization_policy,
        )
        .map_err(frost_error)
    }
}

/// Complete public reusable-root and CDK application configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostWalletConfig {
    /// Aggregate schema version.
    pub version: u16,
    /// Immutable FROST root produced by the initial dual DKG.
    pub root: FederationFrostRootPublicConfig,
    /// Optional immutable CDK on-chain payment application derived from the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdk_onchain_application: Option<FederationFrostApplicationConfig>,
    /// Feature-gated application registrations derived from the same root.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_applications:
        BTreeMap<FederationFrostApplicationId, FederationFrostExtensionApplicationConfig>,
}

impl FederationFrostWalletConfig {
    /// Construct and validate an aggregate public wallet configuration.
    pub fn new(
        federation: &FederationConfig,
        root: FederationFrostRootPublicConfig,
        cdk_onchain_application: Option<FederationFrostApplicationConfig>,
        extension_applications: BTreeMap<
            FederationFrostApplicationId,
            FederationFrostExtensionApplicationConfig,
        >,
    ) -> Result<Self> {
        let config = Self {
            version: WALLET_CONFIG_VERSION,
            root,
            cdk_onchain_application,
            extension_applications,
        };
        config.validate(federation)?;
        Ok(config)
    }

    /// Validate the immutable root and application derivation invariants.
    pub fn validate(&self, federation: &FederationConfig) -> Result<()> {
        if self.version != WALLET_CONFIG_VERSION {
            return Err(invalid_config("unsupported FROST wallet config"));
        }
        self.root.validate(federation)?;
        if self.cdk_onchain_application.is_none() && self.extension_applications.is_empty() {
            return Err(invalid_config(
                "FROST wallet config has no registered applications",
            ));
        }
        if let Some(application) = &self.cdk_onchain_application {
            if application.root_key_epoch != self.root.root_key_epoch {
                return Err(invalid_config("application references an unknown root"));
            }
            if self
                .extension_applications
                .contains_key(&application.application_id)
            {
                return Err(invalid_config(
                    "CDK application ID overlaps an extension application",
                ));
            }
            application.validate(federation, &self.root)?;
        }
        for (application_id, application) in &self.extension_applications {
            if *application_id != application.application_id {
                return Err(invalid_config(
                    "extension application map key differs from its registration",
                ));
            }
            application.validate(federation, &self.root)?;
        }
        Ok(())
    }

    /// Deterministic public wallet config digest for snapshots and test vectors.
    pub fn digest(&self) -> Result<[u8; 32]> {
        let encoded = serde_json::to_vec(self)?;
        Ok(domain_hash(WALLET_CONFIG_DIGEST_DOMAIN, &encoded))
    }

    /// Find one root epoch.
    #[must_use]
    pub fn root(&self, epoch: u64) -> Option<&FederationFrostRootPublicConfig> {
        (self.root.root_key_epoch == epoch).then_some(&self.root)
    }

    /// Find one separately feature-gated application registration.
    #[must_use]
    pub fn extension_application(
        &self,
        application_id: FederationFrostApplicationId,
    ) -> Option<&FederationFrostExtensionApplicationConfig> {
        self.extension_applications.get(&application_id)
    }

    /// Derive the exact accepted P2TR script for one CDK application address.
    pub fn derive_cdk_onchain_script(
        &self,
        federation: &FederationConfig,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
        keychain: FederationAddressKeychain,
        index: u32,
    ) -> Result<bitcoin::ScriptBuf> {
        let application = self
            .cdk_onchain_application
            .as_ref()
            .ok_or_else(|| invalid_config("CDK application is not registered"))?;
        if application.application_id != application_id
            || application.application_key_epoch != application_key_epoch
        {
            return Err(invalid_config("CDK application epoch is missing"));
        }
        if application.threshold != federation.threshold_params.signature_threshold
            || application
                .verification_shares
                .keys()
                .copied()
                .ne(federation.members.keys().copied())
        {
            return Err(invalid_config(
                "CDK application does not match the federation",
            ));
        }
        let branch = match keychain {
            FederationAddressKeychain::External => 0,
            FederationAddressKeychain::Internal => 1,
        };
        let root_xpub = Xpub {
            network: application.network.into(),
            depth: 0,
            parent_fingerprint: Fingerprint::default(),
            child_number: ChildNumber::Normal { index: 0 },
            public_key: PublicKey::from_slice(&application.derived_group_key)
                .map_err(|error| invalid_config(error.to_string()))?,
            chain_code: ChainCode::from(application.chain_code),
        };
        let secp = Secp256k1::verification_only();
        let child = root_xpub
            .derive_pub(
                &secp,
                &[
                    ChildNumber::Normal { index: branch },
                    ChildNumber::from_normal_idx(index)
                        .map_err(|error| invalid_config(error.to_string()))?,
                ],
            )
            .map_err(|error| invalid_config(error.to_string()))?;
        let (internal_key, _) = child.public_key.x_only_public_key();
        Ok(Address::p2tr(&secp, internal_key, None, application.network).script_pubkey())
    }
}

fn descriptor_commitment(
    application_id: FederationFrostApplicationId,
    application_key_epoch: u64,
    root_key_epoch: u64,
    network: Network,
    external: &str,
    internal: &str,
) -> [u8; 32] {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(&application_id.to_bytes());
    transcript.extend_from_slice(&application_key_epoch.to_be_bytes());
    transcript.extend_from_slice(&root_key_epoch.to_be_bytes());
    append_bytes(&mut transcript, network.to_string().as_bytes());
    append_bytes(&mut transcript, external.as_bytes());
    append_bytes(&mut transcript, internal.as_bytes());
    domain_hash(DESCRIPTOR_COMMITMENT_DOMAIN, &transcript)
}

fn domain_hash(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, domain);
    append_bytes(&mut transcript, bytes);
    Sha256Hash::hash(&transcript).to_byte_array()
}

fn ciphersuite_version(ciphersuite: FrostCiphersuite) -> u16 {
    match ciphersuite {
        FrostCiphersuite::Secp256k1Sha256TrV1 => 1,
    }
}

fn frost_member(member: MemberId) -> Result<cdk_frost::MemberId> {
    cdk_frost::MemberId::new(member.get()).map_err(frost_error)
}

fn common_member(member: cdk_frost::MemberId) -> Result<MemberId> {
    MemberId::new(member.get())
}

fn invalid_config(reason: impl Into<String>) -> FederationError {
    FederationError::FrostConfigurationInvalid {
        reason: reason.into(),
    }
}

fn frost_error(error: cdk_frost::Error) -> FederationError {
    invalid_config(error.to_string())
}

fn bip32_error(error: cdk_frost_bip32::Error) -> FederationError {
    invalid_config(error.to_string())
}

mod point_serde {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(point: &[u8; 33], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(point))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 33], D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&encoded).map_err(de::Error::custom)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            de::Error::custom(format!(
                "compressed secp256k1 point must be 33 bytes, found {}",
                bytes.len()
            ))
        })
    }
}

mod point_map_serde {
    use std::collections::BTreeMap;

    use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

    use super::MemberId;

    pub fn serialize<S>(
        points: &BTreeMap<MemberId, [u8; 33]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        points
            .iter()
            .map(|(member, point)| (*member, crate::util::hex::encode(point)))
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<MemberId, [u8; 33]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        BTreeMap::<MemberId, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(member, encoded)| {
                let bytes = crate::util::hex::decode(&encoded).map_err(de::Error::custom)?;
                let point = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    de::Error::custom(format!(
                        "compressed secp256k1 point must be 33 bytes, found {}",
                        bytes.len()
                    ))
                })?;
                Ok((member, point))
            })
            .collect()
    }
}
