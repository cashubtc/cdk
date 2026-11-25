//! Wallet-side registry of imported federations.
//!
//! The registry is the one place a wallet discovers, validates and remembers
//! federations. Each federation is keyed by its federation ID, which the
//! config derives from its roster, so importing the same federation through a
//! different member, an invite or a config file refreshes one entry instead of
//! creating another. Every member's public mint URL, and the wallet's local
//! mint URL, are aliases of that entry; an alias belongs to at most one
//! federation.
//!
//! Entries are persisted in the `federation` namespace of the wallet's
//! key-value store, which every wallet database backend implements, so the
//! registry survives a wallet restart without a schema change.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::{self, WalletDatabase};
use cdk_common::federation::{
    FederationConfig, FederationConfigDigest, FederationError, FederationId, FederationInvite,
    MemberId,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::instrument;

use super::validation::validate_member_mint_info_with_config_digest;
use super::{FederatedMintConnector, FederatedMintConnectorConfig};
use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, MintInfo};
use crate::wallet::mint_connector::DefaultTransport;
use crate::wallet::HttpTransport;

/// Primary key-value namespace that holds wallet federation state.
pub const FEDERATION_KV_NAMESPACE: &str = "federation";
/// Secondary key-value namespace that holds registry entries, keyed by federation ID.
pub const FEDERATION_REGISTRY_KV_NAMESPACE: &str = "registry";
/// Version of the persisted registry entry format.
pub const FEDERATION_REGISTRY_ENTRY_VERSION: u16 = 1;

/// Errors from discovering, validating or persisting federations.
#[derive(Debug, thiserror::Error)]
pub enum FederationRegistryError {
    /// The federation config or a member advertisement is invalid.
    #[error(transparent)]
    Federation(#[from] FederationError),
    /// The wallet database failed.
    #[error(transparent)]
    Database(#[from] database::Error),
    /// A registry entry does not encode or decode.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Building the federated connector failed.
    #[error(transparent)]
    Connector(#[from] crate::Error),
    /// A discovery request failed.
    #[error("GET {url} failed: {reason}")]
    Fetch {
        /// Requested URL.
        url: String,
        /// Transport or status error.
        reason: String,
    },
    /// The federation's thresholds do not tolerate the faults the roster size implies.
    #[error("wallet refuses an unsafe federation threshold profile: {0}")]
    UnsafeThresholdProfile(FederationError),
    /// The federation advertises no active keyset.
    #[error("federation {federation_id} does not advertise an active keyset")]
    NoActiveKeyset {
        /// Federation without an active keyset.
        federation_id: FederationId,
    },
    /// The federation roster is empty.
    #[error("federation {federation_id} does not contain any members")]
    EmptyRoster {
        /// Federation with an empty roster.
        federation_id: FederationId,
    },
    /// The discovery URL is not a member's public mint URL.
    #[error("federation config does not contain discovery mint URL {mint_url}")]
    DiscoveryUrlNotInRoster {
        /// URL the config was fetched from.
        mint_url: MintUrl,
    },
    /// The discovery URL belongs to more than one member.
    #[error("federation config contains discovery mint URL {mint_url} more than once")]
    DuplicateDiscoveryUrl {
        /// URL the config was fetched from.
        mint_url: MintUrl,
    },
    /// The preferred quote coordinator is not a member.
    #[error("federation {federation_id} does not contain quote coordinator member {member_id}")]
    UnknownQuoteCoordinator {
        /// Federation the policy was given for.
        federation_id: FederationId,
        /// Requested coordinator.
        member_id: MemberId,
    },
    /// The member-call policy is invalid.
    #[error("invalid federation connector policy: {0}")]
    InvalidConnectorPolicy(String),
    /// A mint URL is already an alias of another federation.
    #[error(
        "mint URL {mint_url} is already registered to federation {registered}, not {federation_id}"
    )]
    AliasConflict {
        /// Conflicting URL.
        mint_url: MintUrl,
        /// Federation that holds the URL.
        registered: FederationId,
        /// Federation being registered.
        federation_id: FederationId,
    },
    /// A federation already has a different local mint URL.
    #[error("federation {federation_id} is already registered with local mint URL {registered}; refusing to change it to {requested}")]
    LocalMintUrlConflict {
        /// Registered federation.
        federation_id: FederationId,
        /// Local mint URL on record.
        registered: MintUrl,
        /// Requested local mint URL.
        requested: MintUrl,
    },
    /// Rediscovery returned a different federation.
    #[error("rediscovery through {mint_url} returned federation {found}, expected {expected}")]
    FederationChanged {
        /// URL rediscovered through.
        mint_url: MintUrl,
        /// Registered federation.
        expected: FederationId,
        /// Federation the member now serves.
        found: FederationId,
    },
    /// The federation is not registered.
    #[error("federation {0} is not registered")]
    NotRegistered(FederationId),
    /// A persisted entry has an unsupported format version.
    #[error("unsupported federation registry entry version {0}")]
    UnsupportedVersion(u16),
    /// A persisted entry is stored under another federation's key.
    #[error("federation registry key {key} holds federation {federation_id}")]
    KeyMismatch {
        /// Key-value key.
        key: String,
        /// Federation ID in the entry.
        federation_id: FederationId,
    },
}

/// Where a federation config comes from.
#[derive(Debug, Clone)]
pub enum FederationSource {
    /// Any member's public mint URL. The registry fetches
    /// `GET /federation/v1/config` and checks it against that member's
    /// `GET /v1/info` advertisement.
    MemberUrl(MintUrl),
    /// A `cashufed` invite code.
    Invite(String),
    /// A public federation config, for example read from `federation-public.json`.
    PublicConfig(Box<FederationConfig>),
}

impl FederationSource {
    /// Interpret user input as an invite code, a member URL or a config path.
    ///
    /// Returns `None` for input that is neither an invite nor a URL; callers
    /// may treat it as a path to a public config file.
    pub fn parse_invite_or_url(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.starts_with("cashufed") {
            return Some(Self::Invite(value.to_string()));
        }
        MintUrl::from_str(value).ok().map(Self::MemberUrl)
    }
}

/// Persisted wallet-side member-call policy for one federation.
///
/// This is the serializable form of [`FederatedMintConnectorConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConnectorPolicy {
    /// Preferred member for the first quote-creation attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_coordinator_member_id: Option<MemberId>,
    /// Timeout in seconds for sequential first-healthy-member calls.
    pub member_fallback_timeout_secs: u64,
    /// Per-member timeout in seconds for fanout requests.
    pub member_request_timeout_secs: u64,
    /// Additional retry attempts for retryable member fanout failures.
    pub member_request_max_retries: u16,
    /// Per-member timeout in seconds for quote-status polling calls.
    pub quote_status_member_timeout_secs: u64,
    /// Maximum number of concurrent member calls.
    pub fanout_concurrency: usize,
    /// Attempts to wait for a newly created mint quote to become threshold-visible.
    pub quote_visibility_attempts: u16,
    /// Delay in milliseconds between mint quote threshold-visibility attempts.
    pub quote_visibility_retry_delay_ms: u64,
}

impl Default for FederationConnectorPolicy {
    fn default() -> Self {
        Self::from_connector_config(FederatedMintConnectorConfig::default())
    }
}

impl FederationConnectorPolicy {
    /// Convert a runtime connector policy into its persisted form.
    pub fn from_connector_config(config: FederatedMintConnectorConfig) -> Self {
        Self {
            quote_coordinator_member_id: config.quote_coordinator_member_id,
            member_fallback_timeout_secs: config.member_fallback_timeout.as_secs(),
            member_request_timeout_secs: config.member_request_timeout.as_secs(),
            member_request_max_retries: config.member_request_max_retries,
            quote_status_member_timeout_secs: config.quote_status_member_timeout.as_secs(),
            fanout_concurrency: config.fanout_concurrency,
            quote_visibility_attempts: config.quote_visibility_attempts,
            quote_visibility_retry_delay_ms: u64::try_from(
                config.quote_visibility_retry_delay.as_millis(),
            )
            .unwrap_or(u64::MAX),
        }
    }

    /// Convert the persisted policy into the runtime connector policy.
    pub fn connector_config(&self) -> FederatedMintConnectorConfig {
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: self.quote_coordinator_member_id,
            member_fallback_timeout: Duration::from_secs(self.member_fallback_timeout_secs),
            member_request_timeout: Duration::from_secs(self.member_request_timeout_secs),
            member_request_max_retries: self.member_request_max_retries,
            quote_status_member_timeout: Duration::from_secs(self.quote_status_member_timeout_secs),
            fanout_concurrency: self.fanout_concurrency,
            quote_visibility_attempts: self.quote_visibility_attempts,
            quote_visibility_retry_delay: Duration::from_millis(
                self.quote_visibility_retry_delay_ms,
            ),
        }
    }
}

/// Options for importing a federation.
#[derive(Debug, Clone, Default)]
pub struct FederationImportOptions {
    /// Local wallet mint URL for a newly registered federation.
    ///
    /// Defaults to the discovery member's URL for [`FederationSource::MemberUrl`]
    /// and to the first member's URL otherwise. A federation that is already
    /// registered keeps its local mint URL; requesting a different one fails.
    pub local_mint_url: Option<MintUrl>,
    /// Member-call policy. `None` keeps a registered federation's policy, or
    /// uses the default policy for a new one.
    pub connector_policy: Option<FederationConnectorPolicy>,
}

/// One validated federation in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFederation {
    version: u16,
    local_mint_url: MintUrl,
    public_config: FederationConfig,
    connector_policy: FederationConnectorPolicy,
}

impl RegisteredFederation {
    /// Build an entry from a config that already passed full validation.
    fn new(
        public_config: FederationConfig,
        local_mint_url: MintUrl,
        connector_policy: FederationConnectorPolicy,
    ) -> Result<Self, FederationRegistryError> {
        let entry = Self {
            version: FEDERATION_REGISTRY_ENTRY_VERSION,
            local_mint_url,
            public_config,
            connector_policy,
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Check the invariants every registry entry holds.
    ///
    /// This repeats the cheap structural checks of
    /// [`FederationConfig::validate_wallet_runtime_metadata`] but not the
    /// threshold key interpolation and setup-proof checks, which ran when the
    /// config was imported.
    pub fn validate(&self) -> Result<(), FederationRegistryError> {
        if self.version != FEDERATION_REGISTRY_ENTRY_VERSION {
            return Err(FederationRegistryError::UnsupportedVersion(self.version));
        }
        let federation_id = self.public_config.federation_id;
        self.public_config.validate_wallet_runtime_metadata()?;
        self.public_config
            .threshold_params
            .validate_bft_safety()
            .map_err(FederationRegistryError::UnsafeThresholdProfile)?;
        if self.public_config.members.is_empty() {
            return Err(FederationRegistryError::EmptyRoster { federation_id });
        }
        if !self
            .public_config
            .keysets
            .iter()
            .any(|keyset| keyset.active)
        {
            return Err(FederationRegistryError::NoActiveKeyset { federation_id });
        }
        self.connector_policy
            .connector_config()
            .validate()
            .map_err(|error| FederationRegistryError::InvalidConnectorPolicy(error.to_string()))?;
        if let Some(member_id) = self.connector_policy.quote_coordinator_member_id {
            if !self.public_config.members.contains_key(&member_id) {
                return Err(FederationRegistryError::UnknownQuoteCoordinator {
                    federation_id,
                    member_id,
                });
            }
        }
        Ok(())
    }

    /// Federation ID, the registry key.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.public_config.federation_id
    }

    /// Mint URL the wallet stores this federation's proofs and quotes under.
    #[must_use]
    pub fn local_mint_url(&self) -> &MintUrl {
        &self.local_mint_url
    }

    /// Validated public federation config.
    #[must_use]
    pub fn public_config(&self) -> &FederationConfig {
        &self.public_config
    }

    /// Persisted member-call policy.
    #[must_use]
    pub fn connector_policy(&self) -> &FederationConnectorPolicy {
        &self.connector_policy
    }

    /// Digest of the public config, as members advertise it.
    pub fn config_digest(&self) -> Result<FederationConfigDigest, FederationRegistryError> {
        Ok(self.public_config.config_digest_for_validated_config()?)
    }

    /// Every mint URL that maps to this federation: the local mint URL and
    /// each member's public mint URL.
    #[must_use]
    pub fn aliases(&self) -> BTreeSet<MintUrl> {
        std::iter::once(self.local_mint_url.clone())
            .chain(
                self.public_config
                    .members
                    .values()
                    .map(|member| member.public_mint_url.clone()),
            )
            .collect()
    }

    /// Whether a mint URL is an alias of this federation.
    #[must_use]
    pub fn matches_mint_url(&self, mint_url: &MintUrl) -> bool {
        &self.local_mint_url == mint_url || self.member_id_for_mint_url(mint_url).is_some()
    }

    /// Member whose public mint URL is `mint_url`.
    #[must_use]
    pub fn member_id_for_mint_url(&self, mint_url: &MintUrl) -> Option<MemberId> {
        self.public_config
            .members
            .iter()
            .find(|(_, member)| &member.public_mint_url == mint_url)
            .map(|(member_id, _)| *member_id)
    }

    /// Units with an active keyset, in order.
    #[must_use]
    pub fn active_units(&self) -> Vec<CurrencyUnit> {
        self.public_config
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .map(|keyset| keyset.unit.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Whether the federation has an active keyset for `unit`.
    #[must_use]
    pub fn supports_unit(&self, unit: &CurrencyUnit) -> bool {
        self.public_config
            .keysets
            .iter()
            .any(|keyset| keyset.active && &keyset.unit == unit)
    }

    /// Build a connector over the default HTTP transport.
    pub fn connector(&self) -> Result<FederatedMintConnector, FederationRegistryError> {
        self.connector_with_transport(DefaultTransport::default())
    }

    /// Build a connector whose member clients share `transport`.
    pub fn connector_with_transport<T>(
        &self,
        transport: T,
    ) -> Result<FederatedMintConnector, FederationRegistryError>
    where
        T: HttpTransport + 'static,
    {
        Ok(
            FederatedMintConnector::from_validated_public_config_urls_with_transport_and_config(
                self.public_config.clone(),
                transport,
                self.connector_policy.connector_config(),
            )?,
        )
    }
}

/// Persistent registry of the federations a wallet has imported.
///
/// Clones share one in-memory view, loaded from the wallet database on first
/// use and kept in step with every write made through the registry.
#[derive(Clone)]
pub struct FederationRegistry {
    localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    entries: Arc<RwLock<Option<BTreeMap<FederationId, RegisteredFederation>>>>,
}

impl std::fmt::Debug for FederationRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationRegistry").finish_non_exhaustive()
    }
}

impl FederationRegistry {
    /// Open the registry stored in a wallet database.
    pub fn new(localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>) -> Self {
        Self {
            localstore,
            entries: Arc::new(RwLock::new(None)),
        }
    }

    /// Fetch a federation config from a member and check it against that
    /// member's mint-info advertisement.
    #[instrument(skip_all)]
    pub async fn discover<T>(
        transport: &T,
        mint_url: &MintUrl,
    ) -> Result<FederationConfig, FederationRegistryError>
    where
        T: HttpTransport,
    {
        let public_config: FederationConfig =
            fetch_json(transport, mint_url, &["federation", "v1", "config"]).await?;
        let mint_info: MintInfo = fetch_json(transport, mint_url, &["v1", "info"]).await?;
        Self::validate_discovered(mint_url, public_config, &mint_info)
    }

    /// Validate a config fetched from `mint_url` against the mint info that
    /// member serves.
    ///
    /// The URL must be exactly one member's public mint URL, and that member
    /// must advertise this federation and this config's digest.
    pub fn validate_discovered(
        mint_url: &MintUrl,
        public_config: FederationConfig,
        mint_info: &MintInfo,
    ) -> Result<FederationConfig, FederationRegistryError> {
        public_config.validate()?;
        let mut matching_members = public_config
            .members
            .iter()
            .filter(|(_, member)| &member.public_mint_url == mint_url);
        let Some((member_id, _)) = matching_members.next() else {
            return Err(FederationRegistryError::DiscoveryUrlNotInRoster {
                mint_url: mint_url.clone(),
            });
        };
        if matching_members.next().is_some() {
            return Err(FederationRegistryError::DuplicateDiscoveryUrl {
                mint_url: mint_url.clone(),
            });
        }
        let config_digest = public_config.config_digest_for_validated_config()?;
        validate_member_mint_info_with_config_digest(
            &public_config,
            *member_id,
            mint_info,
            config_digest,
        )
        .map_err(|error| match error {
            crate::Error::Federation(error) => FederationRegistryError::Federation(error),
            error => FederationRegistryError::Connector(error),
        })?;
        Ok(public_config)
    }

    /// Import a federation over the default HTTP transport.
    #[instrument(skip_all)]
    pub async fn import(
        &self,
        source: FederationSource,
        options: FederationImportOptions,
    ) -> Result<RegisteredFederation, FederationRegistryError> {
        self.import_with_transport(source, options, &DefaultTransport::default())
            .await
    }

    /// Import a federation, discovering it over `transport` when the source
    /// is a member URL.
    ///
    /// Importing a federation that is already registered refreshes its config
    /// and keeps its local mint URL.
    #[instrument(skip_all)]
    pub async fn import_with_transport<T>(
        &self,
        source: FederationSource,
        options: FederationImportOptions,
        transport: &T,
    ) -> Result<RegisteredFederation, FederationRegistryError>
    where
        T: HttpTransport,
    {
        let (public_config, discovery_mint_url) = load_source(source, transport).await?;
        self.register_validated(public_config, discovery_mint_url, options)
            .await
    }

    /// Validate a federation source and return the entry a first import
    /// would create, without storing anything.
    #[instrument(skip_all)]
    pub async fn inspect_with_transport<T>(
        source: FederationSource,
        options: FederationImportOptions,
        transport: &T,
    ) -> Result<RegisteredFederation, FederationRegistryError>
    where
        T: HttpTransport,
    {
        let (public_config, discovery_mint_url) = load_source(source, transport).await?;
        let federation_id = public_config.federation_id;
        let local_mint_url = match options.local_mint_url.or(discovery_mint_url) {
            Some(mint_url) => mint_url,
            None => first_member_url(&public_config)
                .ok_or(FederationRegistryError::EmptyRoster { federation_id })?,
        };
        RegisteredFederation::new(
            public_config,
            local_mint_url,
            options.connector_policy.unwrap_or_default(),
        )
    }

    /// Rediscover a registered federation and store the refreshed config.
    ///
    /// Members are tried starting with the local mint URL; the first member
    /// that serves a valid config for the same federation wins.
    #[instrument(skip_all)]
    pub async fn refresh_with_transport<T>(
        &self,
        federation_id: FederationId,
        transport: &T,
    ) -> Result<RegisteredFederation, FederationRegistryError>
    where
        T: HttpTransport,
    {
        let registered = self
            .get(&federation_id)
            .await?
            .ok_or(FederationRegistryError::NotRegistered(federation_id))?;
        let local = registered.local_mint_url().clone();
        let candidates = std::iter::once(local.clone())
            .filter(|url| registered.member_id_for_mint_url(url).is_some())
            .chain(
                registered
                    .public_config()
                    .members
                    .values()
                    .map(|member| member.public_mint_url.clone())
                    .filter(|url| url != &local),
            )
            .collect::<Vec<_>>();

        let mut last_error = FederationRegistryError::EmptyRoster { federation_id };
        for mint_url in candidates {
            match Self::discover(transport, &mint_url).await {
                Ok(public_config) if public_config.federation_id == federation_id => {
                    return self
                        .register_validated(
                            public_config,
                            Some(mint_url),
                            FederationImportOptions::default(),
                        )
                        .await;
                }
                Ok(public_config) => {
                    last_error = FederationRegistryError::FederationChanged {
                        mint_url,
                        expected: federation_id,
                        found: public_config.federation_id,
                    };
                }
                Err(error) => {
                    tracing::warn!(
                        "Federation {} rediscovery through {} failed: {}",
                        federation_id,
                        mint_url,
                        error
                    );
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }

    /// Validate wallet requirements and persist a fully validated config.
    async fn register_validated(
        &self,
        public_config: FederationConfig,
        discovery_mint_url: Option<MintUrl>,
        options: FederationImportOptions,
    ) -> Result<RegisteredFederation, FederationRegistryError> {
        let federation_id = public_config.federation_id;
        let mut guard = self.entries.write().await;
        let entries = match guard.as_mut() {
            Some(entries) => entries,
            None => guard.insert(self.load_entries().await?),
        };

        let existing = entries.get(&federation_id);
        let local_mint_url = match (existing, options.local_mint_url) {
            (Some(existing), Some(requested)) if requested != existing.local_mint_url => {
                return Err(FederationRegistryError::LocalMintUrlConflict {
                    federation_id,
                    registered: existing.local_mint_url.clone(),
                    requested,
                });
            }
            (Some(existing), _) => existing.local_mint_url.clone(),
            (None, Some(requested)) => requested,
            (None, None) => match discovery_mint_url {
                Some(mint_url) => mint_url,
                None => first_member_url(&public_config)
                    .ok_or(FederationRegistryError::EmptyRoster { federation_id })?,
            },
        };
        let connector_policy = options
            .connector_policy
            .or_else(|| existing.map(|existing| existing.connector_policy))
            .unwrap_or_default();

        let entry = RegisteredFederation::new(public_config, local_mint_url, connector_policy)?;
        for alias in entry.aliases() {
            if let Some(other) = entries.values().find(|other| {
                other.federation_id() != federation_id && other.matches_mint_url(&alias)
            }) {
                return Err(FederationRegistryError::AliasConflict {
                    mint_url: alias,
                    registered: other.federation_id(),
                    federation_id,
                });
            }
        }

        self.store_entry(&entry).await?;
        entries.insert(federation_id, entry.clone());
        Ok(entry)
    }

    /// Replace a registered federation's member-call policy.
    #[instrument(skip_all)]
    pub async fn set_connector_policy(
        &self,
        federation_id: FederationId,
        connector_policy: FederationConnectorPolicy,
    ) -> Result<RegisteredFederation, FederationRegistryError> {
        let mut guard = self.entries.write().await;
        let entries = match guard.as_mut() {
            Some(entries) => entries,
            None => guard.insert(self.load_entries().await?),
        };
        let existing = entries
            .get(&federation_id)
            .ok_or(FederationRegistryError::NotRegistered(federation_id))?;
        let entry = RegisteredFederation {
            connector_policy,
            ..existing.clone()
        };
        entry.validate()?;
        self.store_entry(&entry).await?;
        entries.insert(federation_id, entry.clone());
        Ok(entry)
    }

    /// Remove a federation. Its proofs and quotes stay in the wallet database.
    #[instrument(skip_all)]
    pub async fn remove(
        &self,
        federation_id: FederationId,
    ) -> Result<bool, FederationRegistryError> {
        let mut guard = self.entries.write().await;
        let entries = match guard.as_mut() {
            Some(entries) => entries,
            None => guard.insert(self.load_entries().await?),
        };
        if entries.remove(&federation_id).is_none() {
            return Ok(false);
        }
        self.localstore
            .kv_remove(
                FEDERATION_KV_NAMESPACE,
                FEDERATION_REGISTRY_KV_NAMESPACE,
                &federation_id.to_string(),
            )
            .await?;
        Ok(true)
    }

    /// Every registered federation, ordered by federation ID.
    #[instrument(skip_all)]
    pub async fn list(&self) -> Result<Vec<RegisteredFederation>, FederationRegistryError> {
        Ok(self.entries().await?.into_values().collect())
    }

    /// A registered federation by ID.
    #[instrument(skip_all)]
    pub async fn get(
        &self,
        federation_id: &FederationId,
    ) -> Result<Option<RegisteredFederation>, FederationRegistryError> {
        Ok(self.entries().await?.remove(federation_id))
    }

    /// The federation a mint URL is an alias of.
    #[instrument(skip_all)]
    pub async fn find_by_mint_url(
        &self,
        mint_url: &MintUrl,
    ) -> Result<Option<RegisteredFederation>, FederationRegistryError> {
        Ok(self
            .entries()
            .await?
            .into_values()
            .find(|entry| entry.matches_mint_url(mint_url)))
    }

    async fn entries(
        &self,
    ) -> Result<BTreeMap<FederationId, RegisteredFederation>, FederationRegistryError> {
        if let Some(entries) = self.entries.read().await.as_ref() {
            return Ok(entries.clone());
        }
        let mut guard = self.entries.write().await;
        match guard.as_ref() {
            Some(entries) => Ok(entries.clone()),
            None => Ok(guard.insert(self.load_entries().await?).clone()),
        }
    }

    async fn load_entries(
        &self,
    ) -> Result<BTreeMap<FederationId, RegisteredFederation>, FederationRegistryError> {
        let keys = self
            .localstore
            .kv_list(FEDERATION_KV_NAMESPACE, FEDERATION_REGISTRY_KV_NAMESPACE)
            .await?;
        let mut entries = BTreeMap::new();
        for key in keys {
            let Some(bytes) = self
                .localstore
                .kv_read(
                    FEDERATION_KV_NAMESPACE,
                    FEDERATION_REGISTRY_KV_NAMESPACE,
                    &key,
                )
                .await?
            else {
                continue;
            };
            let entry: RegisteredFederation = serde_json::from_slice(&bytes)?;
            entry.validate()?;
            if entry.federation_id().to_string() != key {
                return Err(FederationRegistryError::KeyMismatch {
                    key,
                    federation_id: entry.federation_id(),
                });
            }
            entries.insert(entry.federation_id(), entry);
        }
        Ok(entries)
    }

    async fn store_entry(
        &self,
        entry: &RegisteredFederation,
    ) -> Result<(), FederationRegistryError> {
        let bytes = serde_json::to_vec(entry)?;
        self.localstore
            .kv_write(
                FEDERATION_KV_NAMESPACE,
                FEDERATION_REGISTRY_KV_NAMESPACE,
                &entry.federation_id().to_string(),
                &bytes,
            )
            .await?;
        Ok(())
    }
}

/// Read and fully validate a federation config from its source. Returns the
/// config and, for discovery, the member URL it came from.
async fn load_source<T>(
    source: FederationSource,
    transport: &T,
) -> Result<(FederationConfig, Option<MintUrl>), FederationRegistryError>
where
    T: HttpTransport,
{
    match source {
        FederationSource::MemberUrl(mint_url) => {
            let public_config = FederationRegistry::discover(transport, &mint_url).await?;
            Ok((public_config, Some(mint_url)))
        }
        // Parsing validates the invite and its config.
        FederationSource::Invite(invite_code) => {
            Ok((FederationInvite::from_str(&invite_code)?.config, None))
        }
        FederationSource::PublicConfig(public_config) => {
            public_config.validate()?;
            Ok((*public_config, None))
        }
    }
}

/// Default local mint URL: the lowest member ID's public mint URL.
fn first_member_url(public_config: &FederationConfig) -> Option<MintUrl> {
    public_config
        .members
        .values()
        .next()
        .map(|member| member.public_mint_url.clone())
}

async fn fetch_json<T, R>(
    transport: &T,
    mint_url: &MintUrl,
    path: &[&str],
) -> Result<R, FederationRegistryError>
where
    T: HttpTransport,
    R: DeserializeOwned,
{
    let url = mint_url
        .join_paths(path)
        .map_err(|error| FederationRegistryError::Fetch {
            url: format!("{mint_url}/{}", path.join("/")),
            reason: error.to_string(),
        })?;
    let fetch_error = |reason: String| FederationRegistryError::Fetch {
        url: url.to_string(),
        reason,
    };
    let response = transport
        .http_get(url.clone(), None)
        .await
        .map_err(|error| fetch_error(error.to_string()))?;
    if !response.is_success() {
        return Err(fetch_error(format!("HTTP {}", response.status())));
    }
    response
        .json()
        .await
        .map_err(|error| fetch_error(error.to_string()))
}
