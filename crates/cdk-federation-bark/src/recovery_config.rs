//! Bark-owned threshold recovery wallet registration and public descriptors.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::Network;
use cdk_common::federation::{
    FederationConfig, FederationFrostApplicationId, FederationFrostExtensionApplicationConfig,
    FederationFrostRootPublicConfig, FederationSetupFrostApplication, Result,
};
use cdk_frost::{
    ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch, ApplicationRegistration,
    DerivedPublicPackage,
};
use cdk_frost_bip32::{application_chain_code, FrostBip32, WatchOnlyDescriptors};
use cdk_frost_bip32::{WalletBranch, WalletIndex};
use serde::{Deserialize, Serialize};

use crate::invalid_bark;

/// Stable Bark recovery-wallet application label.
pub const FEDERATION_BARK_RECOVERY_APPLICATION_LABEL: &str =
    "org.cashubtc.cdk.federated-bark-recovery";
/// Current Bark recovery application derivation contract.
pub const FEDERATION_BARK_RECOVERY_DERIVATION_VERSION: u16 = 1;
const FEDERATION_BARK_RECOVERY_CONFIG_VERSION: u16 = 1;
const FEDERATION_BARK_RECOVERY_AUTHORIZATION_DOMAIN: &[u8] =
    b"cdk/frost/federated-bark-recovery-authorization/v1";
const FEDERATION_BARK_RECOVERY_DESCRIPTOR_DOMAIN: &[u8] =
    b"cdk/federation/bark-recovery-descriptors/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationBarkRecoveryPublicConfiguration {
    version: u16,
    network: Network,
    minimum_confirmations: u32,
    fee_policy_version: u16,
}

/// Setup parameters for Bark's independently scoped recovery wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationBarkRecoverySetupConfig {
    /// Recovery application key epoch.
    pub application_key_epoch: u64,
    /// Bitcoin network used by Bark and its recovery wallet.
    pub network: Network,
    /// Confirmation depth required for terminal recovery.
    pub minimum_confirmations: u32,
    /// Consensus recovery transaction fee-policy version.
    pub fee_policy_version: u16,
}

impl FederationBarkRecoverySetupConfig {
    fn validate(self) -> Result<()> {
        if self.application_key_epoch == 0
            || self.minimum_confirmations == 0
            || self.fee_policy_version == 0
        {
            return Err(invalid_bark(
                "Bark recovery epoch, confirmations, and fee policy must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Validated typed view of Bark's generic recovery application registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationBarkRecoveryApplicationConfig {
    extension: FederationFrostExtensionApplicationConfig,
    public: FederationBarkRecoveryPublicConfiguration,
}

impl FederationBarkRecoveryApplicationConfig {
    /// Derive a new recovery application from the reusable federation root.
    pub fn new(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        setup: FederationBarkRecoverySetupConfig,
    ) -> Result<Self> {
        setup.validate()?;
        let registration = federation_bark_recovery_registration(setup.application_key_epoch)?;
        let public = FederationBarkRecoveryPublicConfiguration {
            version: FEDERATION_BARK_RECOVERY_CONFIG_VERSION,
            network: setup.network,
            minimum_confirmations: setup.minimum_confirmations,
            fee_policy_version: setup.fee_policy_version,
        };
        let extension = FederationFrostExtensionApplicationConfig::new(
            federation,
            root,
            registration,
            &public,
        )?;
        Self::from_extension(federation, root, &extension)
    }

    /// Decode and validate Bark's typed recovery view of a generic registration.
    pub fn from_extension(
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        extension: &FederationFrostExtensionApplicationConfig,
    ) -> Result<Self> {
        extension.validate(federation, root)?;
        let public: FederationBarkRecoveryPublicConfiguration = extension.decode_configuration()?;
        let config = Self {
            extension: extension.clone(),
            public,
        };
        config.validate(federation, root)?;
        Ok(config)
    }

    /// Generic registration stored in the federation wallet configuration.
    #[must_use]
    pub fn extension(&self) -> FederationFrostExtensionApplicationConfig {
        self.extension.clone()
    }

    /// Stable application identifier.
    #[must_use]
    pub fn application_id(&self) -> FederationFrostApplicationId {
        self.extension.application_id
    }

    /// Application key epoch.
    #[must_use]
    pub fn application_key_epoch(&self) -> u64 {
        self.extension.application_key_epoch
    }

    /// Reusable root epoch.
    #[must_use]
    pub fn root_key_epoch(&self) -> u64 {
        self.extension.root_key_epoch
    }

    /// Recovery wallet Bitcoin network.
    #[must_use]
    pub fn network(&self) -> Network {
        self.public.network
    }

    /// Required terminal confirmation depth.
    #[must_use]
    pub fn minimum_confirmations(&self) -> u32 {
        self.public.minimum_confirmations
    }

    /// Consensus fee-policy version.
    #[must_use]
    pub fn fee_policy_version(&self) -> u16 {
        self.public.fee_policy_version
    }

    /// Reconstruct the validated public FROST package.
    pub fn derived_public_package(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<DerivedPublicPackage> {
        self.validate(federation, root)?;
        self.extension.derived_public_package(federation, root)
    }

    /// Canonical FROST registration used by participant authorization adapters.
    pub fn registration(&self) -> Result<ApplicationRegistration> {
        self.extension.registration()
    }

    /// Produce public Taproot descriptors for the watch-only wallet engine.
    pub fn descriptors(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<WatchOnlyDescriptors> {
        let package = self.derived_public_package(federation, root)?;
        let adapter = FrostBip32::new(package, self.public.network)
            .map_err(|error| invalid_bark(error.to_string()))?;
        Ok(adapter.descriptors())
    }

    /// Derive one accepted recovery-wallet script without private key material.
    pub fn derive_script(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
        internal: bool,
        index: u32,
    ) -> Result<bitcoin::ScriptBuf> {
        let package = self.derived_public_package(federation, root)?;
        let adapter = FrostBip32::new(package, self.public.network)
            .map_err(|error| invalid_bark(error.to_string()))?;
        let branch = match internal {
            true => WalletBranch::Internal,
            false => WalletBranch::External,
        };
        let index = WalletIndex::new(index).map_err(|error| invalid_bark(error.to_string()))?;
        adapter
            .derive(branch, index)
            .and_then(|key| key.address(self.public.network))
            .map(|address| address.script_pubkey())
            .map_err(|error| invalid_bark(error.to_string()))
    }

    /// Deterministic BIP32 chain code for the recovery descriptor root.
    pub fn chain_code(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<[u8; 32]> {
        self.derived_public_package(federation, root)
            .map(|package| application_chain_code(&package))
    }

    /// Commitment isolating the recovery wallet's persisted descriptor scope.
    pub fn descriptor_commitment(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<[u8; 32]> {
        let descriptors = self.descriptors(federation, root)?;
        let mut transcript = Vec::new();
        transcript.extend_from_slice(&self.application_id().to_bytes());
        transcript.extend_from_slice(&self.application_key_epoch().to_be_bytes());
        transcript.extend_from_slice(&self.root_key_epoch().to_be_bytes());
        append_bytes(&mut transcript, self.public.network.to_string().as_bytes());
        append_bytes(&mut transcript, descriptors.external.as_bytes());
        append_bytes(&mut transcript, descriptors.internal.as_bytes());
        let mut scoped = Vec::new();
        append_bytes(&mut scoped, FEDERATION_BARK_RECOVERY_DESCRIPTOR_DOMAIN);
        append_bytes(&mut scoped, &transcript);
        Ok(Sha256Hash::hash(&scoped).to_byte_array())
    }

    /// Validate registration identity, typed policy, root, and derivation.
    pub fn validate(
        &self,
        federation: &FederationConfig,
        root: &FederationFrostRootPublicConfig,
    ) -> Result<()> {
        let expected = federation_bark_recovery_registration(self.application_key_epoch())?;
        if self.public.version != FEDERATION_BARK_RECOVERY_CONFIG_VERSION
            || self.public.minimum_confirmations == 0
            || self.public.fee_policy_version == 0
            || self.extension.registration()? != expected
        {
            return Err(invalid_bark(
                "unsupported Bark recovery application registration",
            ));
        }
        self.extension.validate(federation, root)
    }
}

/// Encode Bark recovery setup parameters as a generic setup application.
pub fn federation_bark_recovery_setup_application(
    setup: FederationBarkRecoverySetupConfig,
) -> Result<FederationSetupFrostApplication> {
    setup.validate()?;
    let registration = federation_bark_recovery_registration(setup.application_key_epoch)?;
    let public = FederationBarkRecoveryPublicConfiguration {
        version: FEDERATION_BARK_RECOVERY_CONFIG_VERSION,
        network: setup.network,
        minimum_confirmations: setup.minimum_confirmations,
        fee_policy_version: setup.fee_policy_version,
    };
    Ok(FederationSetupFrostApplication::Extension {
        application_id: FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?,
        application_key_epoch: setup.application_key_epoch,
        derivation_version: registration.derivation_version().get(),
        authorization_policy: registration.authorization_policy(),
        configuration: serde_json::to_value(public)?,
    })
}

/// Decode a generic setup entry when it is Bark's recovery application.
pub fn federation_bark_recovery_setup_config(
    application: &FederationSetupFrostApplication,
) -> Result<Option<FederationBarkRecoverySetupConfig>> {
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
    let registration = federation_bark_recovery_registration(*application_key_epoch)?;
    let expected_id = FederationFrostApplicationId::from_bytes(registration.id().to_bytes())?;
    if *application_id != expected_id {
        return Ok(None);
    }
    if *derivation_version != registration.derivation_version().get()
        || *authorization_policy != registration.authorization_policy()
    {
        return Err(invalid_bark(
            "Bark recovery setup registration metadata is inconsistent",
        ));
    }
    let public: FederationBarkRecoveryPublicConfiguration =
        serde_json::from_value(configuration.clone())?;
    let setup = FederationBarkRecoverySetupConfig {
        application_key_epoch: *application_key_epoch,
        network: public.network,
        minimum_confirmations: public.minimum_confirmations,
        fee_policy_version: public.fee_policy_version,
    };
    setup.validate()?;
    Ok(Some(setup))
}

/// Return Bark recovery's stable application identifier independent of epoch.
pub fn federation_bark_recovery_application_id() -> Result<FederationFrostApplicationId> {
    let registration = federation_bark_recovery_registration(1)?;
    FederationFrostApplicationId::from_bytes(registration.id().to_bytes())
}

/// Decode the Bark recovery application from one validated federation config.
pub fn federation_bark_recovery_application(
    federation: &FederationConfig,
) -> Result<Option<FederationBarkRecoveryApplicationConfig>> {
    let Some(wallet) = federation.frost_wallet.as_ref() else {
        return Ok(None);
    };
    let application_id = federation_bark_recovery_application_id()?;
    wallet
        .extension_application(application_id)
        .map(|extension| {
            FederationBarkRecoveryApplicationConfig::from_extension(
                federation,
                &wallet.root,
                extension,
            )
        })
        .transpose()
}

fn federation_bark_recovery_registration(epoch: u64) -> Result<ApplicationRegistration> {
    let id = ApplicationId::from_label(FEDERATION_BARK_RECOVERY_APPLICATION_LABEL)
        .map_err(|error| invalid_bark(error.to_string()))?;
    let key_epoch =
        ApplicationKeyEpoch::new(epoch).map_err(|error| invalid_bark(error.to_string()))?;
    let version = ApplicationDerivationVersion::new(FEDERATION_BARK_RECOVERY_DERIVATION_VERSION)
        .map_err(|error| invalid_bark(error.to_string()))?;
    let authorization_policy =
        Sha256Hash::hash(FEDERATION_BARK_RECOVERY_AUTHORIZATION_DOMAIN).to_byte_array();
    ApplicationRegistration::new(id, key_epoch, version, authorization_policy)
        .map_err(|error| invalid_bark(error.to_string()))
}

fn append_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}
