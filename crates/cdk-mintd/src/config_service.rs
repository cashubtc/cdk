//! Transport-independent management of mintd configuration.
//!
//! Configuration files are import/export documents. Once initialized, the
//! database is authoritative: applying a document stages it explicitly and a
//! successful restart promotes it to active state.

use std::fmt;
use std::path::Path;
#[cfg(feature = "management-rpc")]
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use bitcoin::bip32::Xpriv;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;
use cdk_signatory::signatory::Signatory;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use crate::config::Settings;
use crate::config_store::{ConfigRepository, ConfigStoreError};

const ENV_SECRET_PREFIX: &str = "env:";
const FILE_SECRET_PREFIX: &str = "file:";
const SIGNING_IDENTITY_DOMAIN: &[u8] = b"cdk-mintd/signing-identity/v1\0";

/// Cryptographic identity of the configured local or remote signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigningIdentity {
    pub(crate) pubkey: cdk::nuts::PublicKey,
    pub(crate) fingerprint: String,
}

/// A validated configuration candidate.
#[derive(Clone)]
pub struct ResolvedConfiguration {
    /// Normalized TOML containing secret references, safe to persist.
    pub document: String,
    /// Runtime settings with secret references resolved.
    pub settings: Settings,
}

impl fmt::Debug for ResolvedConfiguration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedConfiguration")
            .field("document", &self.document)
            .field("settings", &"[resolved configuration redacted]")
            .finish()
    }
}

/// Active and optionally staged configuration documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationSnapshot {
    /// Active database-backed configuration document.
    pub active: String,
    /// Configuration staged for the next restart.
    pub pending: Option<String>,
}

/// Result of explicitly applying a configuration document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Whether a restart is required to activate the document.
    pub restart_required: bool,
}

/// Configuration management errors.
#[derive(Debug, Error)]
pub enum ConfigurationServiceError {
    /// Configuration has not been initialized yet.
    #[error("mintd configuration is not initialized; run `cdk-mintd config init --file <path>`")]
    NotInitialized,

    /// A TOML document could not be parsed.
    #[error("invalid mintd configuration document: {0}")]
    Parse(#[from] config::ConfigError),

    /// A normalized TOML document could not be serialized.
    #[error("could not serialize mintd configuration document: {0}")]
    Serialize(#[from] toml::ser::Error),

    /// A secret value was embedded directly instead of referenced.
    #[error("{field} must use an `env:VARIABLE` or `file:/absolute/path` secret reference")]
    LiteralSecret {
        /// Configuration field containing a literal secret.
        field: &'static str,
    },

    /// A referenced environment variable was unavailable.
    #[error("could not resolve {field} from environment variable {name}")]
    EnvironmentSecret {
        /// Configuration field being resolved.
        field: &'static str,
        /// Environment variable name.
        name: String,
    },

    /// A referenced secret file was unavailable.
    #[error("could not resolve {field} from secret file {}: {source}", path.display())]
    FileSecret {
        /// Configuration field being resolved.
        field: &'static str,
        /// Referenced secret file.
        path: std::path::PathBuf,
        /// File read failure.
        #[source]
        source: std::io::Error,
    },

    /// A secret reference was empty or resolved to an empty value.
    #[error("secret reference for {field} resolved to an empty value")]
    EmptySecret {
        /// Configuration field being resolved.
        field: &'static str,
    },

    /// Runtime validation rejected the resolved configuration.
    #[error("invalid mintd configuration: {0}")]
    Validation(String),

    /// A full-file apply attempted to move the authoritative database itself.
    #[error("primary database settings are bootstrap-only and cannot be changed by config apply")]
    PrimaryDatabaseChange,

    /// The configured signer could not be identified without mutating mint state.
    #[error("could not determine configured mint signing identity: {0}")]
    SigningIdentity(String),

    /// The database was initialized without immutable signing-identity metadata.
    #[error("mintd signing identity is not initialized in the configuration store")]
    MissingSigningIdentity,

    /// A configuration attempted to replace the mint's cryptographic signer.
    #[error(
        "configured signing identity does not match this mint database; signer migration is not supported by config apply"
    )]
    SigningIdentityChange,

    /// Persistent configuration storage failed.
    #[error(transparent)]
    Store(#[from] ConfigStoreError),

    /// Reading live canonical mint configuration failed.
    #[error("could not read canonical mint configuration: {0}")]
    Runtime(String),
}

/// Database-backed configuration service shared by local commands and gRPC.
pub struct ConfigurationService {
    repository: ConfigRepository,
    primary_database: crate::config::Database,
    operation_lock: Arc<Mutex<()>>,
    canonical_source: RwLock<Option<CanonicalSource>>,
}

#[derive(Clone)]
enum CanonicalSource {
    Live(Arc<cdk::mint::Mint>),
    Snapshot(Box<CanonicalSnapshot>),
}

#[derive(Clone)]
struct CanonicalSnapshot {
    mint_info: Option<cdk::nuts::MintInfo>,
    quote_ttl: Option<cdk_common::common::QuoteTTL>,
}

impl fmt::Debug for ConfigurationService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigurationService")
            .field("repository", &self.repository)
            .finish_non_exhaustive()
    }
}

impl ConfigurationService {
    /// Creates a configuration service.
    pub(crate) fn new(
        repository: ConfigRepository,
        primary_database: crate::config::Database,
    ) -> Self {
        Self {
            repository,
            primary_database,
            operation_lock: Arc::new(Mutex::new(())),
            canonical_source: RwLock::new(None),
        }
    }

    /// Attaches the live mint so exported configuration can be composed with
    /// canonical metadata and quote TTL records.
    pub(crate) async fn attach_mint(&self, mint: Arc<cdk::mint::Mint>) {
        *self.canonical_source.write().await = Some(CanonicalSource::Live(mint));
    }

    /// Attaches a direct-access canonical snapshot for complete show/export output.
    pub(crate) async fn attach_canonical_snapshot(
        &self,
        mint_info: Option<cdk::nuts::MintInfo>,
        quote_ttl: Option<cdk_common::common::QuoteTTL>,
    ) {
        *self.canonical_source.write().await =
            Some(CanonicalSource::Snapshot(Box::new(CanonicalSnapshot {
                mint_info,
                quote_ttl,
            })));
    }

    /// Parses, resolves, and validates an import document without changing state.
    pub fn validate_document(
        document: &str,
    ) -> Result<ResolvedConfiguration, ConfigurationServiceError> {
        let mut referenced_settings = Settings::try_from_toml(document)?;
        prune_inactive_configuration(&mut referenced_settings);
        let normalized_document = toml::to_string_pretty(&referenced_settings)?;
        let mut settings = referenced_settings;
        resolve_secrets(&mut settings)?;
        crate::validate_settings(&settings)
            .map_err(|error| ConfigurationServiceError::Validation(error.to_string()))?;

        Ok(ResolvedConfiguration {
            document: normalized_document,
            settings,
        })
    }

    /// Initializes database-backed configuration for service-level tests.
    #[cfg(all(test, feature = "sqlite", feature = "fakewallet"))]
    async fn initialize(
        &self,
        document: &str,
    ) -> Result<ResolvedConfiguration, ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        let resolved = Self::validate_document(document)?;
        let signing_identity = discover_signing_identity(&resolved.settings).await?;
        validate_authored_mint_pubkey(&resolved.settings, &signing_identity)?;
        self.repository
            .initialize(
                resolved.document.clone(),
                signing_identity.fingerprint.clone(),
            )
            .await?;
        Ok(resolved)
    }

    /// Returns active and pending database-backed documents.
    pub async fn snapshot(&self) -> Result<ConfigurationSnapshot, ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        let active = self
            .repository
            .active()
            .await?
            .ok_or(ConfigurationServiceError::NotInitialized)?;
        let active = self.effective_active_document(active).await?;
        let pending = self.repository.pending().await?;

        Ok(ConfigurationSnapshot { active, pending })
    }

    /// Validates and explicitly stages a complete document.
    ///
    /// All file imports are restart-bound in this first iteration. Existing
    /// field-specific management RPCs remain the immediate-update interface.
    pub async fn apply(
        &self,
        document: &str,
        validate_only: bool,
    ) -> Result<ApplyOutcome, ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        let resolved = Self::validate_document(document)?;
        self.repository
            .active()
            .await?
            .ok_or(ConfigurationServiceError::NotInitialized)?;
        if !same_primary_database(&self.primary_database, &resolved.settings.database) {
            return Err(ConfigurationServiceError::PrimaryDatabaseChange);
        }
        let signing_identity = discover_signing_identity(&resolved.settings).await?;
        validate_authored_mint_pubkey(&resolved.settings, &signing_identity)?;
        self.require_signing_identity(&signing_identity).await?;

        if !validate_only {
            self.repository.stage(resolved.document).await?;
        }

        Ok(ApplyOutcome {
            restart_required: true,
        })
    }

    /// Discards the configuration staged for the next restart.
    pub async fn discard_pending(&self) -> Result<(), ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        self.repository.discard_pending().await?;
        Ok(())
    }

    /// Loads the configuration to use for startup.
    ///
    /// Pending configuration takes precedence only as a candidate. It remains
    /// pending until the caller successfully constructs the mint and promotes it.
    pub(crate) async fn startup_candidate(
        &self,
    ) -> Result<(ResolvedConfiguration, Option<String>, SigningIdentity), ConfigurationServiceError>
    {
        let _operation = self.operation_lock.lock().await;
        let pending = self.repository.pending().await?;
        let (document, pending_document) = match pending {
            Some(document) => (document.clone(), Some(document)),
            None => (
                self.repository
                    .active()
                    .await?
                    .ok_or(ConfigurationServiceError::NotInitialized)?,
                None,
            ),
        };

        let resolved = Self::validate_document(&document)?;
        let signing_identity = discover_signing_identity(&resolved.settings).await?;
        validate_authored_mint_pubkey(&resolved.settings, &signing_identity)?;
        self.require_signing_identity(&signing_identity).await?;

        Ok((resolved, pending_document, signing_identity))
    }

    /// Creates or restores the durable canonical rollback point before pending
    /// startup can mutate mint records.
    pub(crate) async fn prepare_pending_activation(
        &self,
        expected_document: &str,
        mint_info: Option<&cdk::nuts::MintInfo>,
        quote_ttl: Option<&cdk_common::common::QuoteTTL>,
    ) -> Result<(), ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        self.repository
            .prepare_pending_activation(expected_document, mint_info, quote_ttl)
            .await?;
        Ok(())
    }

    /// Atomically promotes the pending daemon document together with canonical
    /// mint metadata/NUT policy and quote TTL.
    pub(crate) async fn promote_pending_with_canonical(
        &self,
        expected_document: &str,
        mint_info: &cdk::nuts::MintInfo,
        quote_ttl: &cdk_common::common::QuoteTTL,
    ) -> Result<String, ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        let pubkey = mint_info.pubkey.ok_or_else(|| {
            ConfigurationServiceError::SigningIdentity(
                "candidate mint configuration has no signing public key".to_owned(),
            )
        })?;
        self.require_signing_identity(&signing_identity_from_pubkey(pubkey))
            .await?;
        Ok(self
            .repository
            .promote_pending_with_canonical(expected_document, mint_info, quote_ttl)
            .await?)
    }

    /// Restores the durable canonical rollback point after pending startup
    /// fails. Active and pending daemon documents are unchanged.
    pub(crate) async fn rollback_pending_activation(
        &self,
    ) -> Result<(), ConfigurationServiceError> {
        let _operation = self.operation_lock.lock().await;
        self.repository.rollback_pending_activation().await?;
        Ok(())
    }

    /// Returns the durable canonical rollback point for direct active-config
    /// inspection after an interrupted pending startup.
    pub(crate) async fn canonical_backup(
        &self,
    ) -> Result<
        Option<(
            Option<cdk::nuts::MintInfo>,
            Option<cdk_common::common::QuoteTTL>,
        )>,
        ConfigurationServiceError,
    > {
        let _operation = self.operation_lock.lock().await;
        Ok(self.repository.canonical_backup().await?)
    }

    async fn effective_active_document(
        &self,
        document: String,
    ) -> Result<String, ConfigurationServiceError> {
        let canonical_source = self.canonical_source.read().await.clone();
        let Some(canonical_source) = canonical_source else {
            return Ok(document);
        };
        let (mint_info, quote_ttl) = match canonical_source {
            CanonicalSource::Live(mint) => {
                let mint_info = mint
                    .mint_info()
                    .await
                    .map_err(|error| ConfigurationServiceError::Runtime(error.to_string()))?;
                let quote_ttl = mint
                    .quote_ttl()
                    .await
                    .map_err(|error| ConfigurationServiceError::Runtime(error.to_string()))?;
                (Some(mint_info), Some(quote_ttl))
            }
            CanonicalSource::Snapshot(snapshot) => (snapshot.mint_info, snapshot.quote_ttl),
        };
        let mut settings = Settings::try_from_toml(&document)?;
        prune_inactive_configuration(&mut settings);

        if let Some(mint_info) = mint_info {
            settings.mint_info.name = mint_info.name.unwrap_or_default();
            settings.mint_info.pubkey = mint_info.pubkey;
            settings.mint_info.description = mint_info.description.unwrap_or_default();
            settings.mint_info.description_long = mint_info.description_long;
            settings.mint_info.icon_url = mint_info.icon_url;
            settings.mint_info.urls = mint_info.urls.unwrap_or_default();
            settings.mint_info.motd = mint_info.motd;
            settings.mint_info.tos_url = mint_info.tos_url;
            settings.mint_info.contact_nostr_public_key = None;
            settings.mint_info.contact_email = None;
            settings.mint_info.contacts = mint_info.contact.unwrap_or_default();
            settings.mint_info.nuts = Some(crate::config::ManagedNuts {
                nut04: mint_info.nuts.nut04,
                nut05: mint_info.nuts.nut05,
            });
        }
        if let Some(quote_ttl) = quote_ttl {
            settings.info.quote_ttl = Some(quote_ttl);
        }

        Ok(toml::to_string_pretty(&settings)?)
    }

    async fn require_signing_identity(
        &self,
        candidate: &SigningIdentity,
    ) -> Result<(), ConfigurationServiceError> {
        let persisted = self
            .repository
            .signing_identity()
            .await?
            .ok_or(ConfigurationServiceError::MissingSigningIdentity)?;
        if persisted != candidate.fingerprint {
            return Err(ConfigurationServiceError::SigningIdentityChange);
        }

        Ok(())
    }
}

pub(crate) async fn discover_signing_identity(
    settings: &Settings,
) -> Result<SigningIdentity, ConfigurationServiceError> {
    let pubkey = if let Some(signatory) = settings.enabled_signatory() {
        let client = cdk_signatory::SignatoryRpcClient::new(
            &signatory.address,
            signatory.port,
            signatory.tls_dir.clone(),
        )
        .await
        .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?;
        client
            .keysets()
            .await
            .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?
            .pubkey
    } else if let Some(seed) = settings
        .info
        .seed
        .as_deref()
        .filter(|seed| !seed.is_empty())
    {
        root_pubkey(seed.as_bytes())?
    } else if let Some(mnemonic) = settings.info.mnemonic.as_deref() {
        let mnemonic = Mnemonic::from_str(mnemonic)
            .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?;
        root_pubkey(&mnemonic.to_seed_normalized(""))?
    } else {
        return Err(ConfigurationServiceError::SigningIdentity(
            "no signing source is configured".to_owned(),
        ));
    };

    Ok(signing_identity_from_pubkey(pubkey))
}

fn root_pubkey(seed: &[u8]) -> Result<cdk::nuts::PublicKey, ConfigurationServiceError> {
    let secp = Secp256k1::new();
    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)
        .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?;
    Ok(xpriv.to_keypair(&secp).public_key().into())
}

fn signing_identity_from_pubkey(pubkey: cdk::nuts::PublicKey) -> SigningIdentity {
    let mut fingerprint_input = SIGNING_IDENTITY_DOMAIN.to_vec();
    fingerprint_input.extend_from_slice(&pubkey.to_bytes());
    SigningIdentity {
        pubkey,
        fingerprint: sha256::Hash::hash(&fingerprint_input).to_string(),
    }
}

pub(crate) fn validate_authored_mint_pubkey(
    settings: &Settings,
    signing_identity: &SigningIdentity,
) -> Result<(), ConfigurationServiceError> {
    if settings
        .mint_info
        .pubkey
        .is_some_and(|pubkey| pubkey != signing_identity.pubkey)
    {
        return Err(ConfigurationServiceError::SigningIdentityChange);
    }

    Ok(())
}

fn same_primary_database(
    active: &crate::config::Database,
    proposed: &crate::config::Database,
) -> bool {
    if active.engine != proposed.engine {
        return false;
    }

    match (&active.postgres, &proposed.postgres) {
        (Some(active), Some(proposed)) => {
            active.url == proposed.url
                && active.tls_mode == proposed.tls_mode
                && active.max_connections == proposed.max_connections
                && active.connection_timeout_seconds == proposed.connection_timeout_seconds
        }
        (None, None) => true,
        _ => false,
    }
}

fn prune_inactive_configuration(settings: &mut Settings) {
    if settings.database.engine != crate::config::DatabaseEngine::Postgres {
        settings.database.postgres = None;
    }
    if settings
        .auth
        .as_ref()
        .is_some_and(|auth| !auth.auth_enabled)
    {
        settings.auth = None;
    }
    if settings.auth.is_none()
        || settings.database.engine != crate::config::DatabaseEngine::Postgres
    {
        settings.auth_database = None;
    }
    if settings
        .signatory
        .as_ref()
        .is_some_and(|signatory| !signatory.enabled)
    {
        settings.signatory = None;
    }

    #[cfg(feature = "lnbits")]
    if !settings
        .ln
        .iter()
        .any(|ln| ln.ln_backend == crate::config::LnBackend::LNbits)
    {
        settings.lnbits = None;
    }

    #[cfg(feature = "ldk-node")]
    if !settings
        .ln
        .iter()
        .any(|ln| ln.ln_backend == crate::config::LnBackend::LdkNode)
    {
        settings.ldk_node = None;
    }

    #[cfg(feature = "bdk")]
    if !settings
        .onchain
        .as_ref()
        .is_some_and(|onchain| onchain.onchain_backend == crate::config::OnchainBackend::Bdk)
    {
        settings.bdk = None;
    }
}

fn resolve_secrets(settings: &mut Settings) -> Result<(), ConfigurationServiceError> {
    resolve_optional_secret(&mut settings.info.seed, "info.seed")?;
    resolve_optional_secret(&mut settings.info.mnemonic, "info.mnemonic")?;

    if let Some(postgres) = settings.database.postgres.as_mut() {
        resolve_secret(&mut postgres.url, "database.postgres.url")?;
    }
    if let Some(postgres) = settings
        .auth_database
        .as_mut()
        .and_then(|database| database.postgres.as_mut())
    {
        resolve_secret(&mut postgres.url, "auth_database.postgres.url")?;
    }

    #[cfg(feature = "lnbits")]
    if let Some(lnbits) = settings.lnbits.as_mut() {
        resolve_secret(&mut lnbits.admin_api_key, "lnbits.admin_api_key")?;
        resolve_secret(&mut lnbits.invoice_api_key, "lnbits.invoice_api_key")?;
    }

    #[cfg(feature = "bdk")]
    if let Some(bdk) = settings.bdk.as_mut() {
        resolve_optional_secret(&mut bdk.bitcoind_rpc_password, "bdk.bitcoind_rpc_password")?;
        resolve_optional_secret(&mut bdk.mnemonic, "bdk.mnemonic")?;
    }

    #[cfg(feature = "ldk-node")]
    if let Some(ldk_node) = settings.ldk_node.as_mut() {
        resolve_optional_secret(
            &mut ldk_node.bitcoind_rpc_password,
            "ldk_node.bitcoind_rpc_password",
        )?;
        resolve_optional_secret(
            &mut ldk_node.ldk_node_mnemonic,
            "ldk_node.ldk_node_mnemonic",
        )?;
    }

    #[cfg(feature = "redis")]
    if let cdk_axum::cache::Backend::Redis(redis) = &mut settings.info.http_cache.backend {
        resolve_secret(
            &mut redis.connection_string,
            "info.http_cache.connection_string",
        )?;
        if let Some(cluster_nodes) = redis.cluster_nodes.as_mut() {
            for node in cluster_nodes {
                resolve_secret(node, "info.http_cache.cluster_nodes")?;
            }
        }
    }

    Ok(())
}

fn resolve_optional_secret(
    value: &mut Option<String>,
    field: &'static str,
) -> Result<(), ConfigurationServiceError> {
    if let Some(value) = value.as_mut() {
        resolve_secret(value, field)?;
    }
    Ok(())
}

fn resolve_secret(
    value: &mut String,
    field: &'static str,
) -> Result<(), ConfigurationServiceError> {
    if value.is_empty() {
        return Ok(());
    }

    let resolved = if let Some(name) = value.strip_prefix(ENV_SECRET_PREFIX) {
        if name.is_empty() {
            return Err(ConfigurationServiceError::EmptySecret { field });
        }
        std::env::var(name).map_err(|_| ConfigurationServiceError::EnvironmentSecret {
            field,
            name: name.to_owned(),
        })?
    } else if let Some(path) = value.strip_prefix(FILE_SECRET_PREFIX) {
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(ConfigurationServiceError::LiteralSecret { field });
        }
        std::fs::read_to_string(path).map_err(|source| ConfigurationServiceError::FileSecret {
            field,
            path: path.to_path_buf(),
            source,
        })?
    } else {
        return Err(ConfigurationServiceError::LiteralSecret { field });
    };

    let resolved = resolved.trim().to_owned();
    if resolved.is_empty() {
        return Err(ConfigurationServiceError::EmptySecret { field });
    }
    *value = resolved;

    Ok(())
}

#[cfg(feature = "management-rpc")]
fn rpc_error(error: ConfigurationServiceError) -> cdk_mint_rpc::ConfigurationError {
    use cdk_mint_rpc::ConfigurationError;

    let message = error.to_string();
    match error {
        ConfigurationServiceError::NotInitialized
        | ConfigurationServiceError::Store(ConfigStoreError::AlreadyInitialized)
        | ConfigurationServiceError::Store(ConfigStoreError::PendingConfigurationExists)
        | ConfigurationServiceError::Store(ConfigStoreError::PendingConfigurationChanged)
        | ConfigurationServiceError::Store(ConfigStoreError::NoPendingConfiguration)
        | ConfigurationServiceError::Store(ConfigStoreError::SigningIdentityMismatch)
        | ConfigurationServiceError::MissingSigningIdentity
        | ConfigurationServiceError::SigningIdentityChange => {
            ConfigurationError::FailedPrecondition { message }
        }
        ConfigurationServiceError::Parse(_)
        | ConfigurationServiceError::LiteralSecret { .. }
        | ConfigurationServiceError::EnvironmentSecret { .. }
        | ConfigurationServiceError::FileSecret { .. }
        | ConfigurationServiceError::EmptySecret { .. }
        | ConfigurationServiceError::Validation(_)
        | ConfigurationServiceError::PrimaryDatabaseChange
        | ConfigurationServiceError::SigningIdentity(_) => ConfigurationError::Invalid { message },
        ConfigurationServiceError::Serialize(_)
        | ConfigurationServiceError::Store(_)
        | ConfigurationServiceError::Runtime(_) => ConfigurationError::Internal { message },
    }
}

#[cfg(feature = "management-rpc")]
#[derive(Debug)]
pub(crate) struct RpcConfigurationManager {
    service: Arc<ConfigurationService>,
    work_dir: PathBuf,
    database: crate::config::Database,
}

#[cfg(feature = "management-rpc")]
impl RpcConfigurationManager {
    pub(crate) fn new(
        service: Arc<ConfigurationService>,
        work_dir: PathBuf,
        database: crate::config::Database,
    ) -> Self {
        Self {
            service,
            work_dir,
            database,
        }
    }
}

#[cfg(feature = "management-rpc")]
#[derive(Debug)]
struct RpcConfigurationMutationGuard {
    cancellation: Option<tokio::sync::oneshot::Sender<()>>,
    _access: crate::database_lock::DatabaseAccessGuard,
}

#[cfg(feature = "management-rpc")]
impl RpcConfigurationMutationGuard {
    fn new(access: crate::database_lock::DatabaseAccessGuard) -> Self {
        let lock_loss = access.loss_signal();
        let (cancellation, cancelled) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancelled => {},
                () = lock_loss.wait() => {
                    crate::database_lock::fail_stop_after_lock_loss(
                        "management RPC configuration mutation",
                    )
                },
            }
        });

        Self {
            cancellation: Some(cancellation),
            _access: access,
        }
    }
}

#[cfg(feature = "management-rpc")]
impl Drop for RpcConfigurationMutationGuard {
    fn drop(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            let _ = cancellation.send(());
        }
    }
}

#[cfg(feature = "management-rpc")]
impl cdk_mint_rpc::ConfigurationMutationGuard for RpcConfigurationMutationGuard {}

#[cfg(feature = "management-rpc")]
#[async_trait::async_trait]
impl cdk_mint_rpc::ConfigurationManager for RpcConfigurationManager {
    async fn acquire_configuration_mutation(
        &self,
    ) -> Result<Box<dyn cdk_mint_rpc::ConfigurationMutationGuard>, cdk_mint_rpc::ConfigurationError>
    {
        let access = crate::database_lock::DatabaseAccessGuard::try_acquire_configuration_mutation(
            &self.work_dir,
            &self.database,
        )
        .await
        .map_err(|error| match error {
            crate::database_lock::DatabaseAccessError::Busy => {
                cdk_mint_rpc::ConfigurationError::Busy {
                    message: crate::database_lock::CONFIGURATION_BUSY_MESSAGE.to_owned(),
                }
            }
            error => cdk_mint_rpc::ConfigurationError::Internal {
                message: error.to_string(),
            },
        })?;

        Ok(Box::new(RpcConfigurationMutationGuard::new(access)))
    }

    async fn get_configuration(
        &self,
    ) -> Result<cdk_mint_rpc::ConfigurationSnapshot, cdk_mint_rpc::ConfigurationError> {
        let snapshot = self.service.snapshot().await.map_err(rpc_error)?;
        let restart_required = snapshot.pending.is_some();
        Ok(cdk_mint_rpc::ConfigurationSnapshot {
            active_toml: snapshot.active,
            pending_toml: snapshot.pending,
            restart_required,
        })
    }

    async fn apply_configuration(
        &self,
        config_toml: String,
        validate_only: bool,
    ) -> Result<cdk_mint_rpc::ApplyConfigurationOutcome, cdk_mint_rpc::ConfigurationError> {
        let outcome = self
            .service
            .apply(&config_toml, validate_only)
            .await
            .map_err(rpc_error)?;
        Ok(cdk_mint_rpc::ApplyConfigurationOutcome {
            restart_required: outcome.restart_required,
            changed_fields: vec!["configuration".to_string()],
        })
    }

    async fn discard_pending_configuration(
        &self,
    ) -> Result<cdk_mint_rpc::ConfigurationSnapshot, cdk_mint_rpc::ConfigurationError> {
        self.service.discard_pending().await.map_err(rpc_error)?;
        self.get_configuration().await
    }
}

#[cfg(all(test, feature = "sqlite", feature = "fakewallet"))]
mod tests {
    use std::fs;

    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const DIFFERENT_TEST_MNEMONIC: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    fn secret_file(name: &str) -> std::path::PathBuf {
        crate::test_utils::unique_temp_path(name)
    }

    fn document(secret_reference: &str, name: &str) -> String {
        format!(
            r#"
[info]
mnemonic = "{secret_reference}"

[database]
engine = "sqlite"

[[ln]]
ln_backend = "fakewallet"

[mint_info]
name = "{name}"
"#
        )
    }

    fn postgres_document(
        signing_secret_reference: &str,
        database_secret_reference: &str,
        name: &str,
    ) -> String {
        format!(
            r#"
[info]
mnemonic = "{signing_secret_reference}"

[database]
engine = "postgres"

[database.postgres]
url = "{database_secret_reference}"

[[ln]]
ln_backend = "fakewallet"

[mint_info]
name = "{name}"
"#
        )
    }

    async fn service_with_database(
        primary_database: crate::config::Database,
    ) -> ConfigurationService {
        let database = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory SQLite database");
        ConfigurationService::new(ConfigRepository::new(Arc::new(database)), primary_database)
    }

    async fn service() -> ConfigurationService {
        service_with_database(crate::config::Database::default()).await
    }

    #[tokio::test]
    async fn apply_cannot_replace_the_mint_signing_identity() {
        let active_secret = secret_file("managed_config_active_signer");
        let proposed_secret = secret_file("managed_config_proposed_signer");
        fs::write(&active_secret, TEST_MNEMONIC).expect("write active signing secret");
        fs::write(&proposed_secret, DIFFERENT_TEST_MNEMONIC)
            .expect("write proposed signing secret");
        let active_reference = format!("file:{}", active_secret.display());
        let proposed_reference = format!("file:{}", proposed_secret.display());
        let service = service().await;
        service
            .initialize(&document(&active_reference, "active"))
            .await
            .expect("initialize configuration");

        let error = service
            .apply(&document(&proposed_reference, "proposed"), true)
            .await
            .expect_err("validate-only apply must reject a different signer");

        assert!(matches!(
            error,
            ConfigurationServiceError::SigningIdentityChange
        ));
        assert_eq!(
            service.repository.pending().await.expect("read pending"),
            None
        );

        let _ = fs::remove_file(active_secret);
        let _ = fs::remove_file(proposed_secret);
    }

    #[tokio::test]
    async fn startup_rejects_signing_secret_drift_before_mint_construction() {
        let secret = secret_file("managed_config_signer_drift");
        fs::write(&secret, TEST_MNEMONIC).expect("write initial signing secret");
        let reference = format!("file:{}", secret.display());
        let service = service().await;
        service
            .initialize(&document(&reference, "active"))
            .await
            .expect("initialize configuration");
        fs::write(&secret, DIFFERENT_TEST_MNEMONIC).expect("replace referenced signing secret");

        let error = service
            .startup_candidate()
            .await
            .expect_err("startup must reject signer drift");

        assert!(matches!(
            error,
            ConfigurationServiceError::SigningIdentityChange
        ));
        let _ = fs::remove_file(secret);
    }

    #[tokio::test]
    async fn apply_allows_a_new_reference_to_the_same_signer() {
        let active_secret = secret_file("managed_config_same_signer_active");
        let proposed_secret = secret_file("managed_config_same_signer_proposed");
        fs::write(&active_secret, TEST_MNEMONIC).expect("write active signing secret");
        fs::write(&proposed_secret, TEST_MNEMONIC).expect("write proposed signing secret");
        let active_reference = format!("file:{}", active_secret.display());
        let proposed_reference = format!("file:{}", proposed_secret.display());
        let service = service().await;
        service
            .initialize(&document(&active_reference, "active"))
            .await
            .expect("initialize configuration");

        service
            .apply(&document(&proposed_reference, "proposed"), false)
            .await
            .expect("the same signing identity should be accepted");

        assert!(service
            .repository
            .pending()
            .await
            .expect("read pending")
            .is_some());
        let _ = fs::remove_file(active_secret);
        let _ = fs::remove_file(proposed_secret);
    }

    #[tokio::test]
    async fn apply_compares_resolved_primary_database_settings() {
        let signing_secret = secret_file("managed_config_database_signer");
        let active_database_secret = secret_file("managed_config_database_active");
        let proposed_database_secret = secret_file("managed_config_database_proposed");
        fs::write(&signing_secret, TEST_MNEMONIC).expect("write signing secret");
        fs::write(
            &active_database_secret,
            "postgres://mint:password@localhost/mint",
        )
        .expect("write active database secret");
        fs::write(
            &proposed_database_secret,
            "postgres://mint:password@localhost/mint",
        )
        .expect("write proposed database secret");
        let signing_reference = format!("file:{}", signing_secret.display());
        let active_database_reference = format!("file:{}", active_database_secret.display());
        let proposed_database_reference = format!("file:{}", proposed_database_secret.display());
        let active_document =
            postgres_document(&signing_reference, &active_database_reference, "active");
        let primary_database = ConfigurationService::validate_document(&active_document)
            .expect("resolve active configuration")
            .settings
            .database;
        let service = service_with_database(primary_database).await;
        service
            .initialize(&active_document)
            .await
            .expect("initialize configuration");

        service
            .apply(
                &postgres_document(&signing_reference, &proposed_database_reference, "proposed"),
                false,
            )
            .await
            .expect("equivalent database settings should be accepted");

        assert!(service
            .repository
            .pending()
            .await
            .expect("read pending")
            .is_some());
        let _ = fs::remove_file(signing_secret);
        let _ = fs::remove_file(active_database_secret);
        let _ = fs::remove_file(proposed_database_secret);
    }

    #[tokio::test]
    async fn apply_rejects_a_resolved_primary_database_change() {
        let signing_secret = secret_file("managed_config_database_change_signer");
        let active_database_secret = secret_file("managed_config_database_change_active");
        let proposed_database_secret = secret_file("managed_config_database_change_proposed");
        fs::write(&signing_secret, TEST_MNEMONIC).expect("write signing secret");
        fs::write(
            &active_database_secret,
            "postgres://mint:password@localhost/mint",
        )
        .expect("write active database secret");
        fs::write(
            &proposed_database_secret,
            "postgres://mint:password@localhost/other",
        )
        .expect("write proposed database secret");
        let signing_reference = format!("file:{}", signing_secret.display());
        let active_database_reference = format!("file:{}", active_database_secret.display());
        let proposed_database_reference = format!("file:{}", proposed_database_secret.display());
        let active_document =
            postgres_document(&signing_reference, &active_database_reference, "active");
        let primary_database = ConfigurationService::validate_document(&active_document)
            .expect("resolve active configuration")
            .settings
            .database;
        let service = service_with_database(primary_database).await;
        service
            .initialize(&active_document)
            .await
            .expect("initialize configuration");

        let error = service
            .apply(
                &postgres_document(&signing_reference, &proposed_database_reference, "proposed"),
                false,
            )
            .await
            .expect_err("a database change must be rejected");

        assert!(matches!(
            error,
            ConfigurationServiceError::PrimaryDatabaseChange
        ));
        assert_eq!(
            service.repository.pending().await.expect("read pending"),
            None
        );
        let _ = fs::remove_file(signing_secret);
        let _ = fs::remove_file(active_database_secret);
        let _ = fs::remove_file(proposed_database_secret);
    }

    #[test]
    fn validation_resolves_but_never_persists_secret_material() {
        let secret_file = secret_file("managed_config_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());

        let resolved = ConfigurationService::validate_document(&document(&reference, "mint"))
            .expect("configuration should validate");

        assert_eq!(
            resolved.settings.info.mnemonic.as_deref(),
            Some(TEST_MNEMONIC)
        );
        assert!(resolved.document.contains(&reference));
        assert!(!resolved.document.contains(TEST_MNEMONIC));

        let _ = fs::remove_file(secret_file);
    }

    #[test]
    fn validation_rejects_literal_secrets() {
        let error = ConfigurationService::validate_document(&document(TEST_MNEMONIC, "mint"))
            .expect_err("literal mnemonic must be rejected");

        assert!(matches!(
            error,
            ConfigurationServiceError::LiteralSecret {
                field: "info.mnemonic"
            }
        ));
    }

    #[test]
    fn validation_rejects_unknown_configuration_keys() {
        let secret_file = secret_file("managed_config_unknown_key_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());
        let document = format!(
            "{}\n[auth]\nauth_enabled = true\nopenid_discovery = \"https://issuer.example\"\nopenid_client_id = \"mintd\"\nopenid_client_secret = \"ignored\"\n",
            document(&reference, "mint")
        );

        let error = ConfigurationService::validate_document(&document)
            .expect_err("unknown configuration keys must be rejected");

        assert!(matches!(error, ConfigurationServiceError::Parse(_)));
        assert!(error
            .to_string()
            .contains("unknown field `openid_client_secret`"));

        let _ = fs::remove_file(secret_file);
    }

    #[tokio::test]
    async fn apply_rejects_unknown_configuration_keys_without_staging() {
        let secret_file = secret_file("managed_config_apply_unknown_key_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());
        let service = service().await;
        service
            .initialize(&document(&reference, "active"))
            .await
            .expect("initialize configuration");
        let invalid = format!(
            "{}\n[mint_info_typo]\nname = \"ignored\"\n",
            document(&reference, "pending")
        );

        let error = service
            .apply(&invalid, false)
            .await
            .expect_err("apply must reject unknown sections");

        assert!(matches!(error, ConfigurationServiceError::Parse(_)));
        assert!(service
            .snapshot()
            .await
            .expect("configuration snapshot")
            .pending
            .is_none());

        let _ = fs::remove_file(secret_file);
    }

    #[test]
    fn disabled_auth_is_pruned_before_validation_and_secret_resolution() {
        let secret_file = secret_file("managed_config_disabled_auth_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());
        let document = format!(
            r#"
[info]
mnemonic = "{reference}"

[database]
engine = "postgres"

[database.postgres]
url = "{reference}"

[[ln]]
ln_backend = "fakewallet"

[mint_info]
name = "mint"

[auth]
auth_enabled = false

[auth_database.postgres]
url = "literal-unused-secret"
"#
        );

        let resolved = ConfigurationService::validate_document(&document)
            .expect("disabled auth must not require OIDC fields or auth database secrets");

        assert!(resolved.settings.auth.is_none());
        assert!(resolved.settings.auth_database.is_none());
        assert!(!resolved.document.contains("[auth]"));
        assert!(!resolved.document.contains("[auth_database"));
        assert!(!resolved.document.contains("literal-unused-secret"));

        let _ = fs::remove_file(secret_file);
    }

    #[test]
    fn canonical_nut_policy_round_trips_through_toml() {
        let secret_file = secret_file("managed_config_nuts_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());
        let mut settings = Settings::try_from_toml(&document(&reference, "mint"))
            .expect("parse base configuration");
        settings.mint_info.nuts = Some(crate::config::ManagedNuts::default());

        let serialized = toml::to_string_pretty(&settings).expect("serialize NUT policy");
        let parsed = Settings::try_from_toml(&serialized).expect("parse NUT policy");
        assert!(parsed.mint_info.nuts.is_some());

        let _ = fs::remove_file(secret_file);
    }

    #[test]
    fn environment_secret_errors_do_not_include_secret_values() {
        let error = ConfigurationServiceError::EnvironmentSecret {
            field: "info.mnemonic",
            name: "MINT_SECRET".to_string(),
        };

        assert_eq!(
            error.to_string(),
            "could not resolve info.mnemonic from environment variable MINT_SECRET"
        );
    }

    #[tokio::test]
    async fn apply_is_explicit_staged_and_has_no_revision_state() {
        let secret_file = secret_file("managed_config_apply_secret");
        fs::write(&secret_file, TEST_MNEMONIC).expect("write secret file");
        let reference = format!("file:{}", secret_file.display());
        let service = service().await;

        service
            .initialize(&document(&reference, "active"))
            .await
            .expect("initialize configuration");
        service
            .apply(&document(&reference, "validated"), true)
            .await
            .expect("validate without mutation");
        assert!(service
            .snapshot()
            .await
            .expect("snapshot")
            .pending
            .is_none());

        let outcome = service
            .apply(&document(&reference, "pending"), false)
            .await
            .expect("stage configuration");
        assert!(outcome.restart_required);
        let snapshot = service.snapshot().await.expect("snapshot");
        assert!(snapshot.active.contains("active"));
        assert!(snapshot
            .pending
            .as_deref()
            .is_some_and(|pending| pending.contains("pending")));

        let resolved = ConfigurationService::validate_document(&document(&reference, "pending"))
            .expect("resolve pending configuration");
        service
            .prepare_pending_activation(&resolved.document, None, None)
            .await
            .expect("prepare pending activation");
        let signing_identity = discover_signing_identity(&resolved.settings)
            .await
            .expect("derive signing identity");
        let canonical_mint_info = cdk::nuts::MintInfo {
            pubkey: Some(signing_identity.pubkey),
            ..Default::default()
        };
        service
            .promote_pending_with_canonical(
                &resolved.document,
                &canonical_mint_info,
                &cdk_common::common::QuoteTTL::default(),
            )
            .await
            .expect("promote pending configuration");
        let snapshot = service.snapshot().await.expect("snapshot");
        assert!(snapshot.active.contains("pending"));
        assert!(snapshot.pending.is_none());

        let _ = fs::remove_file(secret_file);
    }
}
