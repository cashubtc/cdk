//! Accepted and member-local configuration validation.

use core::fmt;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use bark::custody::{KeySourceDescriptor, KeySourceKind};
use bark::{Config as BarkConfig, WalletSeed};
use bitcoin::secp256k1::PublicKey;
use bitcoin::Network;
use cdk_common::federation::{
    FederationBarkFrostApplicationConfig, FederationBarkPolicyConfig,
    FederationBarkReceiveAntiDosPolicy, FederationConfig, FederationDkgSetupMethod,
    FederationFrostApplicationId, FederationId, MemberId,
};
use cdk_common::CurrencyUnit;
use cdk_frost_bark::FrostBark;
use sha2::{Digest, Sha256};

use crate::{Error, FederatedBarkReceiveSecretEncryptionKey};

const CUSTODY_DESCRIPTOR_DOMAIN: &[u8] = b"cdk/bark/custody-descriptor/v1";

/// Immutable accepted identity of one federated Bark runtime.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederatedBarkScope {
    federation_id: FederationId,
    application_id: FederationFrostApplicationId,
    root_key_epoch: u64,
    application_key_epoch: u64,
    custody_fingerprint: [u8; 32],
}

impl fmt::Debug for FederatedBarkScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkScope")
            .field("federation_id", &self.federation_id)
            .field("application_id", &self.application_id)
            .field("root_key_epoch", &self.root_key_epoch)
            .field("application_key_epoch", &self.application_key_epoch)
            .field("custody_fingerprint", &self.custody_fingerprint)
            .finish()
    }
}

impl FederatedBarkScope {
    /// Federation owning this runtime.
    #[must_use]
    pub const fn federation_id(self) -> FederationId {
        self.federation_id
    }

    /// Registered Bark application identifier.
    #[must_use]
    pub const fn application_id(self) -> FederationFrostApplicationId {
        self.application_id
    }

    /// Reusable-root epoch.
    #[must_use]
    pub const fn root_key_epoch(self) -> u64 {
        self.root_key_epoch
    }

    /// Bark application-key epoch.
    #[must_use]
    pub const fn application_key_epoch(self) -> u64 {
        self.application_key_epoch
    }

    /// Stable public fingerprint persisted in Bark's key-source descriptor.
    #[must_use]
    pub const fn custody_fingerprint(self) -> [u8; 32] {
        self.custody_fingerprint
    }
}

/// Explicit public bindings repeated in member-local configuration.
///
/// Repetition is intentional: startup compares each operator-supplied value to
/// accepted consensus config rather than silently substituting one side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkLocalBindings {
    /// Expected federation.
    pub federation_id: FederationId,
    /// Expected Bitcoin network.
    pub network: Network,
    /// Expected single Ark server public key.
    pub ark_server_public_key: PublicKey,
    /// Expected reusable-root epoch.
    pub root_key_epoch: u64,
    /// Expected Bark application identifier.
    pub application_id: FederationFrostApplicationId,
    /// Expected Bark application epoch.
    pub application_key_epoch: u64,
    /// Expected signing threshold.
    pub threshold: u16,
    /// Expected exact member roster.
    pub members: BTreeSet<MemberId>,
    /// Expected accepted Bark policy.
    pub policy: FederationBarkPolicyConfig,
    /// Sole supported Cashu unit.
    pub unit: CurrencyUnit,
}

/// Availability of member-local durable stores required before startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedBarkStoreAvailability {
    /// Durable ordinary-FROST, virtual-MuSig2, and threshold-DH store exists.
    pub frost_protocol_store: bool,
    /// Protected receive-secret store exists.
    pub receive_secret_store: bool,
}

/// Redacted member-local Bark network and chain connection settings.
#[derive(Clone)]
pub struct FederatedBarkConnectionConfig {
    /// Ark gRPC endpoint.
    pub server_address: String,
    /// Optional Ark access token.
    pub server_access_token: Option<String>,
    /// Optional Esplora endpoint; exclusive with bitcoind.
    pub esplora_address: Option<String>,
    /// Optional Bitcoin Core RPC endpoint; exclusive with Esplora.
    pub bitcoind_address: Option<String>,
    /// Optional Bitcoin Core cookie file.
    pub bitcoind_cookiefile: Option<PathBuf>,
    /// Optional Bitcoin Core username.
    pub bitcoind_user: Option<String>,
    /// Optional Bitcoin Core password.
    pub bitcoind_pass: Option<String>,
}

impl fmt::Debug for FederatedBarkConnectionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkConnectionConfig")
            .field("server_address", &"<redacted>")
            .field("server_access_token", &"<redacted>")
            .field("esplora_address", &"<redacted>")
            .field("bitcoind_address", &"<redacted>")
            .field("bitcoind_cookiefile", &"<redacted>")
            .field("bitcoind_user", &"<redacted>")
            .field("bitcoind_pass", &"<redacted>")
            .finish()
    }
}

/// Member-local protected receive-secret database and at-rest key.
#[derive(Clone)]
pub struct FederatedBarkReceiveSecretStoreConfig {
    /// Application-scoped Redb file, separate from Bark SQLite.
    pub database_path: PathBuf,
    /// Operator-provided key that must be backed up independently.
    pub encryption_key: Arc<FederatedBarkReceiveSecretEncryptionKey>,
}

impl fmt::Debug for FederatedBarkReceiveSecretStoreConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkReceiveSecretStoreConfig")
            .field("database_path", &"<redacted>")
            .field("encryption_key", &"<redacted>")
            .finish()
    }
}

/// Accepted satoshi limits advertised by the payment adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedBarkLimits {
    /// Smallest accepted payment in satoshis.
    minimum_sat: u64,
    /// Largest accepted payment in satoshis.
    maximum_sat: u64,
}

/// Member-local automation switches. They can disable execution but never
/// relax accepted federation timing, fee, destination, or signer bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederatedBarkMaintenanceControls {
    /// Automatically propose and execute accepted refresh rounds.
    pub automatic_refresh: bool,
    /// Automatically escalate an overdue accepted refresh to emergency exit.
    pub automatic_emergency_exit: bool,
}

impl Default for FederatedBarkMaintenanceControls {
    fn default() -> Self {
        Self {
            automatic_refresh: true,
            automatic_emergency_exit: false,
        }
    }
}

impl FederatedBarkLimits {
    /// Construct non-zero, ordered payment limits.
    pub fn new(minimum_sat: u64, maximum_sat: u64) -> Result<Self, Error> {
        if minimum_sat == 0 || maximum_sat < minimum_sat {
            return Err(Error::InvalidLimits);
        }
        Ok(Self {
            minimum_sat,
            maximum_sat,
        })
    }

    /// Smallest accepted payment in satoshis.
    #[must_use]
    pub const fn minimum_sat(self) -> u64 {
        self.minimum_sat
    }

    /// Largest accepted payment in satoshis.
    #[must_use]
    pub const fn maximum_sat(self) -> u64 {
        self.maximum_sat
    }
}

/// Member-local Bark configuration and operational seed.
#[derive(Clone)]
pub struct FederatedBarkLocalConfig {
    /// Explicit operator enablement.
    pub enabled: bool,
    /// Public values that must match accepted federation state.
    pub bindings: FederatedBarkLocalBindings,
    /// Member-local Bark networking and chain configuration.
    pub bark: BarkConfig,
    /// Fresh application-scoped Bark data directory.
    pub data_directory: PathBuf,
    /// Member-local mailbox/database seed, never used as VTXO custody.
    pub operational_seed: WalletSeed,
    /// Whether a fresh wallet database may be initialized.
    pub create_if_missing: bool,
    /// Required durable-store availability.
    pub stores: FederatedBarkStoreAvailability,
    /// Protected receive-secret persistence opened before the Bark wallet.
    pub receive_secret_store: FederatedBarkReceiveSecretStoreConfig,
    /// Accepted satoshi payment limits.
    pub limits: FederatedBarkLimits,
    /// Explicit member-local maintenance automation controls.
    pub maintenance: FederatedBarkMaintenanceControls,
}

impl FederatedBarkLocalConfig {
    /// Construct member-local Bark state without exposing Bark types to the mint daemon.
    #[allow(clippy::too_many_arguments)]
    pub fn from_seed_bytes(
        bindings: FederatedBarkLocalBindings,
        connection: FederatedBarkConnectionConfig,
        data_directory: PathBuf,
        receive_secret_database_path: PathBuf,
        operational_seed: [u8; 64],
        receive_secret_key: [u8; 32],
        create_if_missing: bool,
        stores: FederatedBarkStoreAvailability,
        limits: FederatedBarkLimits,
        maintenance: FederatedBarkMaintenanceControls,
    ) -> Result<Self, Error> {
        let mut bark = BarkConfig::network_default(bindings.network);
        bark.server_address = connection.server_address;
        #[allow(deprecated)]
        {
            bark.server_access_token = connection.server_access_token;
        }
        bark.user_agent = Some(format!("cdk/{}", env!("CARGO_PKG_VERSION")));
        bark.esplora_address = connection.esplora_address;
        bark.bitcoind_address = connection.bitcoind_address;
        bark.bitcoind_cookiefile = connection.bitcoind_cookiefile;
        bark.bitcoind_user = connection.bitcoind_user;
        bark.bitcoind_pass = connection.bitcoind_pass;
        bark.vtxo_refresh_expiry_threshold = bindings.policy.refresh_before_expiry_blocks;
        bark.vtxo_exit_margin = bindings
            .policy
            .emergency_exit_before_expiry_blocks
            .try_into()
            .map_err(|_| Error::UnsupportedReceivePolicy)?;
        bark.htlc_recv_claim_delta = bindings
            .policy
            .minimum_incoming_cltv_delta
            .try_into()
            .map_err(|_| Error::UnsupportedReceivePolicy)?;
        let operational_seed = WalletSeed::new_from_seed(bindings.network, &operational_seed);
        let encryption_key = Arc::new(FederatedBarkReceiveSecretEncryptionKey::new(
            receive_secret_key,
        )?);
        Ok(Self {
            enabled: true,
            bindings,
            bark,
            data_directory,
            operational_seed,
            create_if_missing,
            stores,
            receive_secret_store: FederatedBarkReceiveSecretStoreConfig {
                database_path: receive_secret_database_path,
                encryption_key,
            },
            limits,
            maintenance,
        })
    }
}

impl fmt::Debug for FederatedBarkLocalConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkLocalConfig")
            .field("enabled", &self.enabled)
            .field("bindings", &self.bindings)
            .field("bark", &"<redacted endpoints and credentials>")
            .field("data_directory", &"<redacted>")
            .field("operational_seed", &"<redacted>")
            .field("create_if_missing", &self.create_if_missing)
            .field("stores", &self.stores)
            .field("receive_secret_store", &self.receive_secret_store)
            .field("limits", &self.limits)
            .field("maintenance", &self.maintenance)
            .finish()
    }
}

/// Fully validated accepted configuration used to construct custody and Bark.
#[derive(Clone)]
pub struct ValidatedFederatedBarkConfig {
    scope: FederatedBarkScope,
    network: Network,
    ark_server_public_key: PublicKey,
    public_adapter: FrostBark,
    descriptor: KeySourceDescriptor,
    policy: FederationBarkPolicyConfig,
}

impl fmt::Debug for ValidatedFederatedBarkConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedFederatedBarkConfig")
            .field("scope", &self.scope)
            .field("network", &self.network)
            .field("ark_server_public_key", &self.ark_server_public_key)
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

impl ValidatedFederatedBarkConfig {
    /// Validate public federation state and every local binding before any I/O.
    pub fn validate(
        federation_enabled: bool,
        federation: &FederationConfig,
        local: &FederatedBarkLocalConfig,
    ) -> Result<Self, Error> {
        if !federation_enabled {
            return Err(Error::FederationRequired);
        }
        if !local.enabled {
            return Err(Error::Disabled);
        }
        federation
            .validate()
            .map_err(|_| Error::InvalidPublicConfig)?;
        if matches!(
            federation
                .setup_transcript
                .as_ref()
                .map(|transcript| transcript.payload.setup_method),
            Some(FederationDkgSetupMethod::TrustedDealer)
        ) && !cfg!(any(test, feature = "federation-test-utils"))
        {
            return Err(Error::TrustedDealerForbidden);
        }
        let wallet = federation
            .frost_wallet
            .as_ref()
            .ok_or(Error::MissingApplication)?;
        let application = wallet
            .bark_application
            .as_ref()
            .ok_or(Error::MissingApplication)?;
        let ark_server_public_key = PublicKey::from_slice(&application.ark_server_public_key)
            .map_err(|_| Error::InvalidPublicConfig)?;
        let public_adapter = application
            .open_public_adapter(
                federation,
                &wallet.root,
                application.network,
                ark_server_public_key,
                application.root_key_epoch,
                application.application_key_epoch,
            )
            .map_err(|_| Error::InvalidPublicConfig)?;
        let fingerprint = custody_fingerprint(federation, application);
        let group_public_key = PublicKey::from_slice(&application.derived_group_key)
            .map_err(|_| Error::InvalidPublicConfig)?;
        let descriptor = KeySourceDescriptor::frost(
            fingerprint,
            group_public_key,
            application.root_key_epoch,
            application.locator_derivation_version,
        )
        .map_err(|_| Error::InvalidPublicConfig)?;
        let scope = FederatedBarkScope {
            federation_id: federation.federation_id,
            application_id: application.application_id,
            root_key_epoch: application.root_key_epoch,
            application_key_epoch: application.application_key_epoch,
            custody_fingerprint: fingerprint,
        };
        let validated = Self {
            scope,
            network: application.network,
            ark_server_public_key,
            public_adapter,
            descriptor,
            policy: application.policy.clone(),
        };
        validated.validate_local(federation, application, local)?;
        Ok(validated)
    }

    fn validate_local(
        &self,
        federation: &FederationConfig,
        application: &FederationBarkFrostApplicationConfig,
        local: &FederatedBarkLocalConfig,
    ) -> Result<(), Error> {
        let bindings = &local.bindings;
        check(
            bindings.federation_id == federation.federation_id,
            "federation_id",
        )?;
        check(bindings.network == application.network, "network")?;
        check(
            bindings.ark_server_public_key == self.ark_server_public_key,
            "ark_server_public_key",
        )?;
        check(
            bindings.root_key_epoch == application.root_key_epoch,
            "root_key_epoch",
        )?;
        check(
            bindings.application_id == application.application_id,
            "application_id",
        )?;
        check(
            bindings.application_key_epoch == application.application_key_epoch,
            "application_key_epoch",
        )?;
        check(bindings.threshold == application.threshold, "threshold")?;
        check(
            bindings.members == federation.members.keys().copied().collect(),
            "member_roster",
        )?;
        check(bindings.policy == application.policy, "policy")?;
        if application.policy.receive_base_fee_sat != 0
            || application.policy.receive_fee_ppm != 0
            || application.policy.receive_anti_dos != FederationBarkReceiveAntiDosPolicy::NoProof
        {
            return Err(Error::UnsupportedReceivePolicy);
        }
        if bindings.unit != CurrencyUnit::Sat {
            return Err(Error::UnsupportedUnit);
        }
        if !local.stores.frost_protocol_store {
            return Err(Error::MissingFrostProtocolStore);
        }
        if !local.stores.receive_secret_store {
            return Err(Error::MissingReceiveSecretStore);
        }
        if local
            .receive_secret_store
            .database_path
            .as_os_str()
            .is_empty()
        {
            return Err(Error::MissingReceiveSecretStore);
        }
        if local.bark.server_address.trim().is_empty() {
            return Err(Error::ConfigMismatch {
                field: "ark_endpoint",
            });
        }
        if local.bark.esplora_address.is_some() == local.bark.bitcoind_address.is_some() {
            return Err(Error::InvalidChainSource);
        }
        check(
            local.bark.vtxo_refresh_expiry_threshold
                == application.policy.refresh_before_expiry_blocks,
            "refresh_before_expiry_blocks",
        )?;
        check(
            u32::from(local.bark.vtxo_exit_margin)
                == application.policy.emergency_exit_before_expiry_blocks,
            "emergency_exit_before_expiry_blocks",
        )?;
        check(
            u32::from(local.bark.htlc_recv_claim_delta)
                >= application.policy.minimum_incoming_cltv_delta,
            "minimum_incoming_cltv_delta",
        )?;
        FederatedBarkLimits::new(local.limits.minimum_sat(), local.limits.maximum_sat())?;
        self.validate_descriptor(&self.descriptor)
    }

    /// Validate a Bark database/custody descriptor against accepted state.
    pub fn validate_descriptor(&self, descriptor: &KeySourceDescriptor) -> Result<(), Error> {
        if descriptor.kind() != KeySourceKind::Frost {
            return Err(Error::StandaloneCustodyForbidden);
        }
        if descriptor != &self.descriptor {
            return Err(Error::CustodyDescriptorMismatch);
        }
        Ok(())
    }

    /// Immutable runtime scope.
    #[must_use]
    pub const fn scope(&self) -> FederatedBarkScope {
        self.scope
    }

    /// Accepted network.
    #[must_use]
    pub const fn network(&self) -> Network {
        self.network
    }

    /// Accepted Ark server public key.
    #[must_use]
    pub const fn ark_server_public_key(&self) -> PublicKey {
        self.ark_server_public_key
    }

    /// Accepted production-v1 Bark policy.
    #[must_use]
    pub const fn policy(&self) -> &FederationBarkPolicyConfig {
        &self.policy
    }

    pub(crate) fn public_adapter(&self) -> FrostBark {
        self.public_adapter.clone()
    }

    pub(crate) fn descriptor(&self) -> KeySourceDescriptor {
        self.descriptor.clone()
    }
}

fn check(valid: bool, field: &'static str) -> Result<(), Error> {
    if valid {
        Ok(())
    } else {
        Err(Error::ConfigMismatch { field })
    }
}

fn custody_fingerprint(
    federation: &FederationConfig,
    application: &FederationBarkFrostApplicationConfig,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CUSTODY_DESCRIPTOR_DOMAIN);
    hasher.update(federation.federation_id.to_bytes());
    hasher.update(application.root_key_epoch.to_be_bytes());
    hasher.update(application.application_id.to_bytes());
    hasher.update(application.application_key_epoch.to_be_bytes());
    hasher.update(application.derivation_version.to_be_bytes());
    hasher.update(application.authorization_policy);
    hasher.update(application.threshold.to_be_bytes());
    for (member, share) in &application.verification_shares {
        hasher.update(member.get().to_be_bytes());
        hasher.update(share);
    }
    hasher.update(application.derived_group_key);
    hasher.update(application.locator_derivation_version.to_be_bytes());
    hasher.finalize().into()
}
