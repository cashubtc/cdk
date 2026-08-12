//! Bark-owned FROST application configuration.

use std::collections::BTreeMap;

use bitcoin::secp256k1::PublicKey;
use bitcoin::Network;
use cdk_common::federation::{
    FederationConfig, FederationFrostApplicationConfig, FederationFrostApplicationId,
    FederationFrostExtensionApplicationConfig, FederationFrostRootPublicConfig,
    FederationFrostWalletConfig, FederationSetupFrostApplication, MemberId, Result,
};
use cdk_common::Amount;
use cdk_frost::{
    ApplicationAuthorization, ApplicationRegistration, AuthorizationRequest,
    AuthorizedSigningRequest, DerivedPublicPackage,
};
use cdk_frost_bark::{
    application_registration_for_epoch, FrostBark,
    APPLICATION_DERIVATION_VERSION as BARK_APPLICATION_DERIVATION_VERSION,
    LOCATOR_DERIVATION_VERSION as BARK_LOCATOR_DERIVATION_VERSION,
};
use serde::{Deserialize, Serialize};

use crate::{invalid_bark, FederationBarkFrostAuthorization};

const BARK_APPLICATION_CONFIG_VERSION: u16 = 1;
const BARK_POLICY_CONFIG_VERSION: u16 = 1;

/// Accepted first-receive proof mode for a federated Bark application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkReceiveAntiDosPolicy {
    /// The accepted Ark deployment does not require a first-receive proof.
    NoProof,
    /// Each eligible actor must have the separately protected operator token.
    ProtectedToken,
    /// Each receive intent must reserve an accepted federation VTXO attestation.
    AcceptedVtxo,
}

/// Exact accepted fee, CLTV, retention, and recovery bounds for Bark v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkPolicyConfig {
    /// Policy schema version.
    pub version: u16,
    /// Fixed component of the gross incoming invoice fee formula, in satoshis.
    pub receive_base_fee_sat: u64,
    /// Proportional component of the gross incoming invoice fee formula.
    pub receive_fee_ppm: u32,
    /// Fixed component of the maximum accepted outgoing fee, in satoshis.
    pub maximum_outgoing_base_fee_sat: u64,
    /// Proportional component of the maximum accepted outgoing fee.
    pub maximum_outgoing_fee_ppm: u32,
    /// Smallest accepted incoming invoice final-CLTV delta.
    pub minimum_incoming_cltv_delta: u32,
    /// Smallest accepted outgoing invoice final-CLTV delta.
    pub minimum_outgoing_cltv_delta: u32,
    /// Refresh must begin at least this many blocks before VTXO expiry.
    pub refresh_before_expiry_blocks: u32,
    /// Emergency exit must begin at least this many blocks before VTXO expiry.
    pub emergency_exit_before_expiry_blocks: u32,
    /// Minimum local retention after terminal checkpoint coverage.
    pub receive_secret_retention_days: u16,
    /// Accepted first-receive anti-DoS mode.
    pub receive_anti_dos: FederationBarkReceiveAntiDosPolicy,
}

impl FederationBarkPolicyConfig {
    /// Construct the current Bark policy after checking deterministic bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receive_base_fee_sat: u64,
        receive_fee_ppm: u32,
        maximum_outgoing_base_fee_sat: u64,
        maximum_outgoing_fee_ppm: u32,
        minimum_incoming_cltv_delta: u32,
        minimum_outgoing_cltv_delta: u32,
        refresh_before_expiry_blocks: u32,
        emergency_exit_before_expiry_blocks: u32,
        receive_secret_retention_days: u16,
        receive_anti_dos: FederationBarkReceiveAntiDosPolicy,
    ) -> Result<Self> {
        let config = Self {
            version: BARK_POLICY_CONFIG_VERSION,
            receive_base_fee_sat,
            receive_fee_ppm,
            maximum_outgoing_base_fee_sat,
            maximum_outgoing_fee_ppm,
            minimum_incoming_cltv_delta,
            minimum_outgoing_cltv_delta,
            refresh_before_expiry_blocks,
            emergency_exit_before_expiry_blocks,
            receive_secret_retention_days,
            receive_anti_dos,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate deterministic Bark policy bounds.
    pub fn validate(&self) -> Result<()> {
        let outgoing_safety_window = self
            .refresh_before_expiry_blocks
            .saturating_sub(self.emergency_exit_before_expiry_blocks);
        if self.version != BARK_POLICY_CONFIG_VERSION
            || self.receive_fee_ppm > 1_000_000
            || self.maximum_outgoing_fee_ppm > 1_000_000
            || self.minimum_incoming_cltv_delta == 0
            || self.minimum_outgoing_cltv_delta == 0
            || self.emergency_exit_before_expiry_blocks == 0
            || self.refresh_before_expiry_blocks <= self.emergency_exit_before_expiry_blocks
            || outgoing_safety_window < self.minimum_outgoing_cltv_delta
            || self.receive_secret_retention_days < 30
        {
            return Err(invalid_bark("unsupported or unsafe Bark policy config"));
        }
        Ok(())
    }

    /// Return the conservative accepted outgoing fee bound.
    pub fn maximum_outgoing_fee(&self, amount: Amount) -> Result<Amount> {
        let proportional_numerator = u64::from(amount)
            .checked_mul(u64::from(self.maximum_outgoing_fee_ppm))
            .ok_or_else(|| invalid_bark("Bark outgoing fee calculation overflow"))?;
        let proportional = proportional_numerator
            .checked_add(999_999)
            .ok_or_else(|| invalid_bark("Bark outgoing fee calculation overflow"))?
            / 1_000_000;
        self.maximum_outgoing_base_fee_sat
            .checked_add(proportional)
            .map(Amount::from)
            .ok_or_else(|| invalid_bark("Bark outgoing fee calculation overflow"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationBarkPublicConfiguration {
    version: u16,
    network: Network,
    #[serde(with = "point_serde")]
    ark_server_public_key: [u8; 33],
    locator_derivation_version: u16,
    policy: FederationBarkPolicyConfig,
}

/// Bark-owned setup parameters embedded in a generic federation extension entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationBarkSetupConfig {
    /// Bark application key epoch.
    pub application_key_epoch: u64,
    /// Bitcoin network shared with the recovery wallet.
    pub network: Network,
    /// Exact Ark server public key.
    pub ark_server_public_key: PublicKey,
    /// Accepted Bark policy.
    pub policy: FederationBarkPolicyConfig,
}

/// Encode Bark setup parameters as a generic extension application.
pub fn federation_bark_setup_application(
    setup: FederationBarkSetupConfig,
) -> Result<FederationSetupFrostApplication> {
    setup.policy.validate()?;
    let registration = application_registration_for_epoch(setup.application_key_epoch)
        .map_err(|error| invalid_bark(error.to_string()))?;
    let configuration = FederationBarkPublicConfiguration {
        version: BARK_APPLICATION_CONFIG_VERSION,
        network: setup.network,
        ark_server_public_key: setup.ark_server_public_key.serialize(),
        locator_derivation_version: BARK_LOCATOR_DERIVATION_VERSION,
        policy: setup.policy,
    };
    Ok(FederationSetupFrostApplication::Extension {
        application_id: FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?,
        application_key_epoch: setup.application_key_epoch,
        derivation_version: registration.derivation_version().get(),
        authorization_policy: registration.authorization_policy(),
        configuration: serde_json::to_value(configuration)?,
    })
}

/// Decode a generic setup extension when it is Bark's registered application.
pub fn federation_bark_setup_config(
    application: &FederationSetupFrostApplication,
) -> Result<Option<FederationBarkSetupConfig>> {
    let FederationSetupFrostApplication::Extension {
        application_id,
        application_key_epoch,
        derivation_version,
        authorization_policy,
        configuration,
    } = application
    else {
        return Ok(None);
    };
    let registration = application_registration_for_epoch(*application_key_epoch)
        .map_err(|error| invalid_bark(error.to_string()))?;
    let expected_id = FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?;
    if *application_id != expected_id {
        return Ok(None);
    }
    if *derivation_version != registration.derivation_version().get()
        || *authorization_policy != registration.authorization_policy()
    {
        return Err(invalid_bark(
            "Bark setup registration metadata is inconsistent",
        ));
    }
    let public: FederationBarkPublicConfiguration = serde_json::from_value(configuration.clone())?;
    public.policy.validate()?;
    Ok(Some(FederationBarkSetupConfig {
        application_key_epoch: *application_key_epoch,
        network: public.network,
        ark_server_public_key: PublicKey::from_slice(&public.ark_server_public_key)
            .map_err(|error| invalid_bark(error.to_string()))?,
        policy: public.policy,
    }))
}

/// Versioned public Bark application derived from the reusable federation root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkFrostApplicationConfig {
    /// Application config schema version.
    pub version: u16,
    /// Stable Bark application identifier.
    pub application_id: FederationFrostApplicationId,
    /// Bark application key epoch.
    pub application_key_epoch: u64,
    /// Referenced reusable-root epoch.
    pub root_key_epoch: u64,
    /// Additive application derivation contract version.
    pub derivation_version: u16,
    /// Commitment to the accepted typed Bark authorization policy.
    pub authorization_policy: [u8; 32],
    /// Derived Bark application signing threshold.
    pub threshold: u16,
    /// Derived Bark application verification shares.
    #[serde(with = "point_map_serde")]
    pub verification_shares: BTreeMap<MemberId, [u8; 33]>,
    /// Derived Bark application aggregate key before locator derivation.
    #[serde(with = "point_serde")]
    pub derived_group_key: [u8; 33],
    /// Immutable Bitcoin network accepted for this application.
    pub network: Network,
    /// Exact compressed public key of the single accepted Ark server.
    #[serde(with = "point_serde")]
    pub ark_server_public_key: [u8; 33],
    /// Version of the CDK-owned VTXO locator derivation.
    pub locator_derivation_version: u16,
    /// Accepted fee, CLTV, recovery, retention, and anti-DoS bounds.
    pub policy: FederationBarkPolicyConfig,
}

impl FederationBarkFrostApplicationConfig {
    /// Derive a complete public Bark application config from the reusable root.
    pub fn new(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        application_key_epoch: u64,
        network: Network,
        ark_server_public_key: PublicKey,
        policy: FederationBarkPolicyConfig,
    ) -> Result<Self> {
        let registration = application_registration_for_epoch(application_key_epoch)
            .map_err(|error| invalid_bark(error.to_string()))?;
        let public = FederationBarkPublicConfiguration {
            version: BARK_APPLICATION_CONFIG_VERSION,
            network,
            ark_server_public_key: ark_server_public_key.serialize(),
            locator_derivation_version: BARK_LOCATOR_DERIVATION_VERSION,
            policy,
        };
        let extension = FederationFrostExtensionApplicationConfig::new(
            federation,
            root,
            registration,
            &public,
        )?;
        Self::from_extension(federation, root, &extension)
    }

    /// Decode and fully validate Bark's typed view of a generic registration.
    pub fn from_extension(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        extension: &FederationFrostExtensionApplicationConfig,
    ) -> Result<Self> {
        extension.validate(federation, root)?;
        let public: FederationBarkPublicConfiguration = extension.decode_configuration()?;
        let config = Self::from_parts(extension, public);
        config.validate(federation, root)?;
        Ok(config)
    }

    fn from_parts(
        extension: &FederationFrostExtensionApplicationConfig,
        public: FederationBarkPublicConfiguration,
    ) -> Self {
        Self {
            version: public.version,
            application_id: extension.application_id,
            application_key_epoch: extension.application_key_epoch,
            root_key_epoch: extension.root_key_epoch,
            derivation_version: extension.derivation_version,
            authorization_policy: extension.authorization_policy,
            threshold: extension.threshold,
            verification_shares: extension.verification_shares.clone(),
            derived_group_key: extension.derived_group_key,
            network: public.network,
            ark_server_public_key: public.ark_server_public_key,
            locator_derivation_version: public.locator_derivation_version,
            policy: public.policy,
        }
    }

    /// Convert this typed config into its generic wallet registration.
    pub fn extension(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<FederationFrostExtensionApplicationConfig> {
        self.validate(federation, root)?;
        FederationFrostExtensionApplicationConfig::new(
            federation,
            root,
            self.registration()?,
            &FederationBarkPublicConfiguration {
                version: self.version,
                network: self.network,
                ark_server_public_key: self.ark_server_public_key,
                locator_derivation_version: self.locator_derivation_version,
                policy: self.policy.clone(),
            },
        )
    }

    /// Reconstruct the public Bark application package after full validation.
    pub fn derived_public_package(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<DerivedPublicPackage> {
        self.extension(federation, root)?
            .derived_public_package(federation, root)
    }

    /// Open the public locator adapter only for the exact accepted runtime scope.
    pub fn open_public_adapter(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        expected_network: Network,
        expected_ark_server: PublicKey,
        expected_root_epoch: u64,
        expected_application_key_epoch: u64,
    ) -> Result<FrostBark> {
        if self.network != expected_network
            || self.ark_server_public_key != expected_ark_server.serialize()
            || self.root_key_epoch != expected_root_epoch
            || self.application_key_epoch != expected_application_key_epoch
        {
            return Err(invalid_bark(
                "Bark runtime network, Ark server, or application epoch mismatch",
            ));
        }
        FrostBark::new(self.derived_public_package(federation, root)?)
            .map_err(|error| invalid_bark(error.to_string()))
    }

    /// Validate registration, root derivation, roster, Ark key, and policy.
    pub fn validate(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<()> {
        self.policy.validate()?;
        PublicKey::from_slice(&self.ark_server_public_key)
            .map_err(|error| invalid_bark(error.to_string()))?;
        let expected = application_registration_for_epoch(self.application_key_epoch)
            .map_err(|error| invalid_bark(error.to_string()))?;
        if self.version != BARK_APPLICATION_CONFIG_VERSION
            || self.root_key_epoch != root.root_key_epoch
            || self.derivation_version != BARK_APPLICATION_DERIVATION_VERSION
            || self.locator_derivation_version != BARK_LOCATOR_DERIVATION_VERSION
            || self.registration()? != expected
        {
            return Err(invalid_bark("unsupported Bark application registration"));
        }
        let extension = self.extension_unchecked()?;
        extension.validate(federation, root)
    }

    fn extension_unchecked(&self) -> Result<FederationFrostExtensionApplicationConfig> {
        Ok(FederationFrostExtensionApplicationConfig {
            version: 1,
            application_id: self.application_id,
            application_key_epoch: self.application_key_epoch,
            root_key_epoch: self.root_key_epoch,
            derivation_version: self.derivation_version,
            authorization_policy: self.authorization_policy,
            threshold: self.threshold,
            verification_shares: self.verification_shares.clone(),
            derived_group_key: self.derived_group_key,
            configuration: serde_json::to_value(FederationBarkPublicConfiguration {
                version: self.version,
                network: self.network,
                ark_server_public_key: self.ark_server_public_key,
                locator_derivation_version: self.locator_derivation_version,
                policy: self.policy.clone(),
            })?,
        })
    }

    fn registration(&self) -> Result<ApplicationRegistration> {
        self.extension_unchecked()?.registration()
    }
}

/// Construct a wallet config containing the Bark extension registration.
pub fn federation_frost_wallet_with_bark(
    federation: &FederationConfig,
    root: FederationFrostRootPublicConfig,
    cdk_onchain_application: FederationFrostApplicationConfig,
    bark: FederationBarkFrostApplicationConfig,
) -> Result<FederationFrostWalletConfig> {
    let extension = bark.extension(federation, &root)?;
    FederationFrostWalletConfig::new_with_extensions(
        federation,
        root,
        cdk_onchain_application,
        BTreeMap::from([(extension.application_id, extension)]),
    )
}

/// Decode the Bark registration from one validated federation config.
pub fn federation_bark_application(
    federation: &FederationConfig,
) -> Result<Option<FederationBarkFrostApplicationConfig>> {
    let Some(wallet) = federation.frost_wallet.as_ref() else {
        return Ok(None);
    };
    let registration =
        application_registration_for_epoch(1).map_err(|error| invalid_bark(error.to_string()))?;
    let application_id = FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?;
    wallet
        .extension_application(application_id)
        .map(|extension| {
            FederationBarkFrostApplicationConfig::from_extension(
                federation,
                &wallet.root,
                extension,
            )
        })
        .transpose()
}

/// Bark adapter for a payload already revalidated against accepted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationBarkFrostRegistrationAdapter {
    registration: ApplicationRegistration,
}

impl FederationBarkFrostRegistrationAdapter {
    /// Construct from a fully validated Bark application config.
    pub fn new(application: &FederationBarkFrostApplicationConfig) -> Result<Self> {
        Ok(Self {
            registration: application.registration()?,
        })
    }
}

impl ApplicationAuthorization for FederationBarkFrostRegistrationAdapter {
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
                "Bark authorization uses an unregistered application epoch".to_string(),
            ));
        }
        let authorization = FederationBarkFrostAuthorization::from_bytes(request.payload())
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

mod point_serde {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(point: &[u8; 33], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&cdk_common::util::hex::encode(point))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 33], D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = cdk_common::util::hex::decode(&encoded).map_err(de::Error::custom)?;
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

    use cdk_common::federation::MemberId;
    use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(
        points: &BTreeMap<MemberId, [u8; 33]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        points
            .iter()
            .map(|(member, point)| (*member, cdk_common::util::hex::encode(point)))
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
                let bytes = cdk_common::util::hex::decode(&encoded).map_err(de::Error::custom)?;
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
