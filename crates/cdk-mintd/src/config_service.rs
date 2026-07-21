//! Validation and lifecycle rules for database-backed mintd configuration.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use bip39::Mnemonic;
use bitcoin::bip32::Xpriv;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;
use cdk_signatory::signatory::Signatory;
use thiserror::Error;

use crate::config::{Database, DatabaseEngine, Settings};
use crate::config_store::{ConfigEnvelope, ConfigRepository, ConfigStoreError};

const ENV_SECRET_PREFIX: &str = "env:";
const FILE_SECRET_PREFIX: &str = "file:";
const SIGNING_IDENTITY_DOMAIN: &[u8] = b"cdk-mintd/signing-identity/v1\0";

/// Cryptographic identity of the configured signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigningIdentity {
    pub(crate) pubkey: cdk::nuts::PublicKey,
    pub(crate) fingerprint: String,
}

/// A validated document and its resolved runtime settings.
#[derive(Clone)]
pub struct ResolvedConfiguration {
    /// Original import document containing secret references.
    pub document: String,
    /// Runtime settings with secret references resolved.
    pub settings: Settings,
}

impl fmt::Debug for ResolvedConfiguration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedConfiguration")
            .field("document", &self.document)
            .field("settings", &"[resolved configuration redacted]")
            .finish_non_exhaustive()
    }
}

/// Configuration selected for daemon startup.
#[derive(Debug, Clone)]
pub(crate) struct StartupConfiguration {
    pub(crate) resolved: ResolvedConfiguration,
    pub(crate) applied: bool,
}

/// Result of a configuration apply operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Applying a new document requires a daemon restart.
    pub restart_required: bool,
}

/// Database-backed configuration failures.
#[derive(Debug, Error)]
pub enum ConfigurationServiceError {
    /// The TOML document could not be parsed.
    #[error("invalid mintd configuration document: {0}")]
    Parse(#[from] config::ConfigError),

    /// A secret was embedded directly instead of referenced.
    #[error("{field} must use an `env:VARIABLE` or `file:/absolute/path` secret reference")]
    LiteralSecret {
        /// Configuration field containing the literal.
        field: &'static str,
    },

    /// An environment secret could not be resolved.
    #[error("could not resolve {field} from environment variable {name}")]
    EnvironmentSecret {
        /// Configuration field being resolved.
        field: &'static str,
        /// Referenced variable.
        name: String,
    },

    /// A file secret could not be resolved.
    #[error("could not resolve {field} from secret file {}: {source}", path.display())]
    FileSecret {
        /// Configuration field being resolved.
        field: &'static str,
        /// Referenced file.
        path: std::path::PathBuf,
        /// File access failure.
        #[source]
        source: std::io::Error,
    },

    /// A secret reference was empty or resolved to an empty value.
    #[error("secret reference for {field} resolved to an empty value")]
    EmptySecret {
        /// Configuration field being resolved.
        field: &'static str,
    },

    /// Runtime validation rejected the resolved settings.
    #[error("invalid mintd configuration: {0}")]
    Validation(String),

    /// The document points at a different primary database.
    #[error("primary database settings do not match the bootstrap database settings")]
    PrimaryDatabaseChange,

    /// The configured signer could not be identified.
    #[error("could not determine configured mint signing identity: {0}")]
    SigningIdentity(String),

    /// The configured signer differs from the database identity.
    #[error(
        "configured signing identity does not match this mint database; signer migration is not supported by config apply"
    )]
    SigningIdentityChange,

    /// Persistent configuration storage failed.
    #[error(transparent)]
    Store(#[from] ConfigStoreError),
}

/// Service for the single authoritative configuration record.
#[derive(Debug, Clone)]
pub(crate) struct ConfigurationService {
    repository: ConfigRepository,
    primary_database: Database,
}

impl ConfigurationService {
    pub(crate) fn new(repository: ConfigRepository, primary_database: Database) -> Self {
        Self {
            repository,
            primary_database,
        }
    }

    /// Parses, resolves, and validates an import document.
    pub fn validate_document(
        document: &str,
    ) -> Result<ResolvedConfiguration, ConfigurationServiceError> {
        let mut settings = Settings::try_from_toml(document)?;
        prune_inactive_configuration(&mut settings);
        resolve_secrets(&mut settings)?;
        crate::validate_settings(&settings)
            .map_err(|error| ConfigurationServiceError::Validation(error.to_string()))?;
        Ok(ResolvedConfiguration {
            document: document.to_owned(),
            settings,
        })
    }

    /// Validates an import document and verifies its configured signer.
    pub(crate) async fn validate_import(
        document: &str,
    ) -> Result<ResolvedConfiguration, ConfigurationServiceError> {
        Ok(Self::validated_import(document).await?.0)
    }

    /// Initializes an empty configuration repository.
    pub(crate) async fn initialize(
        &self,
        document: &str,
        database_pubkey: Option<cdk::nuts::PublicKey>,
    ) -> Result<(), ConfigurationServiceError> {
        let (resolved, signing_identity) = Self::validated_import(document).await?;
        self.require_primary_database(&resolved.settings.database)?;
        if database_pubkey.is_some_and(|pubkey| pubkey != signing_identity.pubkey) {
            return Err(ConfigurationServiceError::SigningIdentityChange);
        }
        self.repository
            .initialize(ConfigEnvelope::new(
                resolved.document,
                signing_identity.fingerprint,
            ))
            .await?;
        Ok(())
    }

    /// Validates and optionally replaces the authoritative document.
    pub(crate) async fn apply(
        &self,
        document: &str,
        validate_only: bool,
    ) -> Result<ApplyOutcome, ConfigurationServiceError> {
        let (resolved, signing_identity) = Self::validated_import(document).await?;
        self.require_primary_database(&resolved.settings.database)?;
        let current = self.repository.active().await?;
        if current.signing_identity != signing_identity.fingerprint {
            return Err(ConfigurationServiceError::SigningIdentityChange);
        }
        if !validate_only {
            self.repository
                .replace(resolved.document, &signing_identity.fingerprint)
                .await?;
        }
        Ok(ApplyOutcome {
            restart_required: !validate_only,
        })
    }

    /// Loads and validates the document selected for startup.
    pub(crate) async fn startup(&self) -> Result<StartupConfiguration, ConfigurationServiceError> {
        let envelope = self.repository.active().await?;
        let resolved = Self::validate_document(&envelope.toml)?;
        self.require_primary_database(&resolved.settings.database)?;
        let signing_identity = discover_signing_identity_async(&resolved.settings).await?;
        validate_authored_mint_pubkey(&resolved.settings, &signing_identity)?;
        if envelope.signing_identity != signing_identity.fingerprint {
            return Err(ConfigurationServiceError::SigningIdentityChange);
        }
        Ok(StartupConfiguration {
            resolved,
            applied: envelope.applied,
        })
    }

    /// Returns the stored import document without resolved secrets.
    pub(crate) async fn document(&self) -> Result<String, ConfigurationServiceError> {
        Ok(self.repository.active().await?.toml)
    }

    /// Marks the current startup document applied if it has not been replaced.
    pub(crate) async fn mark_applied(
        &self,
        expected_toml: &str,
    ) -> Result<bool, ConfigurationServiceError> {
        Ok(self.repository.mark_applied(expected_toml).await?)
    }

    fn require_primary_database(
        &self,
        configured: &Database,
    ) -> Result<(), ConfigurationServiceError> {
        if !same_primary_database(configured, &self.primary_database) {
            return Err(ConfigurationServiceError::PrimaryDatabaseChange);
        }
        Ok(())
    }

    async fn validated_import(
        document: &str,
    ) -> Result<(ResolvedConfiguration, SigningIdentity), ConfigurationServiceError> {
        let resolved = Self::validate_document(document)?;
        let signing_identity = discover_signing_identity_async(&resolved.settings).await?;
        validate_authored_mint_pubkey(&resolved.settings, &signing_identity)?;
        Ok((resolved, signing_identity))
    }
}

pub(crate) fn discover_signing_identity(
    settings: &Settings,
) -> Result<SigningIdentity, ConfigurationServiceError> {
    let pubkey = if settings.enabled_signatory().is_some() {
        return Err(ConfigurationServiceError::SigningIdentity(
            "remote signatory identity requires asynchronous validation".to_owned(),
        ));
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
            "no local signing source is configured".to_owned(),
        ));
    };
    Ok(signing_identity_from_pubkey(pubkey))
}

/// Resolves the signer, including a configured remote signatory.
pub(crate) async fn discover_signing_identity_async(
    settings: &Settings,
) -> Result<SigningIdentity, ConfigurationServiceError> {
    if let Some(signatory) = settings.enabled_signatory() {
        let client = cdk_signatory::SignatoryRpcClient::new(
            &signatory.address,
            signatory.port,
            signatory.tls_dir.clone(),
        )
        .await
        .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?;
        let pubkey = client
            .keysets()
            .await
            .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?
            .pubkey;
        Ok(signing_identity_from_pubkey(pubkey))
    } else {
        discover_signing_identity(settings)
    }
}

fn root_pubkey(seed: &[u8]) -> Result<cdk::nuts::PublicKey, ConfigurationServiceError> {
    let secp = Secp256k1::new();
    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)
        .map_err(|error| ConfigurationServiceError::SigningIdentity(error.to_string()))?;
    Ok(xpriv.to_keypair(&secp).public_key().into())
}

fn signing_identity_from_pubkey(pubkey: cdk::nuts::PublicKey) -> SigningIdentity {
    let mut input = SIGNING_IDENTITY_DOMAIN.to_vec();
    input.extend_from_slice(&pubkey.to_bytes());
    SigningIdentity {
        pubkey,
        fingerprint: sha256::Hash::hash(&input).to_string(),
    }
}

fn validate_authored_mint_pubkey(
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

fn same_primary_database(configured: &Database, bootstrap: &Database) -> bool {
    if configured.engine != bootstrap.engine {
        return false;
    }
    if configured.engine != DatabaseEngine::Postgres {
        return true;
    }
    match (&configured.postgres, &bootstrap.postgres) {
        (Some(configured), Some(bootstrap)) => {
            configured.url == bootstrap.url
                && configured.tls_mode == bootstrap.tls_mode
                && configured.max_connections == bootstrap.max_connections
                && configured.connection_timeout_seconds == bootstrap.connection_timeout_seconds
        }
        _ => false,
    }
}

fn prune_inactive_configuration(settings: &mut Settings) {
    if settings.database.engine != DatabaseEngine::Postgres {
        settings.database.postgres = None;
    }
    if settings
        .auth
        .as_ref()
        .is_some_and(|auth| !auth.auth_enabled)
    {
        settings.auth = None;
    }
    if settings.auth.is_none() || settings.database.engine != DatabaseEngine::Postgres {
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

#[cfg(test)]
mod tests {
    #[cfg(feature = "sqlite")]
    use std::sync::Arc;

    #[cfg(feature = "sqlite")]
    use cdk_sqlite::mint::memory;

    use super::*;

    const TEST_MNEMONIC_ONE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TEST_MNEMONIC_TWO: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    fn document(secret_reference: &str, name: &str) -> String {
        format!(
            r#"
[info]
mnemonic = "{secret_reference}"

[mint_info]
name = "{name}"

[database]
engine = "sqlite"
"#
        )
    }

    #[cfg(feature = "sqlite")]
    async fn service() -> ConfigurationService {
        let database = Arc::new(memory::empty().await.expect("in-memory database"));
        ConfigurationService::new(ConfigRepository::new(database), Database::default())
    }

    #[test]
    fn literal_signing_secret_is_rejected() {
        let error = ConfigurationService::validate_document(
            r#"
[info]
mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
"#,
        )
        .expect_err("literal mnemonic should fail");
        assert!(matches!(
            error,
            ConfigurationServiceError::LiteralSecret {
                field: "info.mnemonic"
            }
        ));
    }

    #[test]
    fn missing_empty_and_relative_secret_references_are_rejected() {
        let _env_lock = crate::test_utils::env_lock();
        const MISSING: &str = "CDK_MINTD_TEST_MISSING_CONFIG_SECRET";
        const EMPTY: &str = "CDK_MINTD_TEST_EMPTY_CONFIG_SECRET";
        std::env::remove_var(MISSING);
        std::env::set_var(EMPTY, "  ");

        assert!(matches!(
            ConfigurationService::validate_document(&document(
                &format!("env:{MISSING}"),
                "missing"
            )),
            Err(ConfigurationServiceError::EnvironmentSecret { .. })
        ));
        assert!(matches!(
            ConfigurationService::validate_document(&document(&format!("env:{EMPTY}"), "empty")),
            Err(ConfigurationServiceError::EmptySecret { .. })
        ));
        assert!(matches!(
            ConfigurationService::validate_document(&document("file:relative/secret", "relative")),
            Err(ConfigurationServiceError::LiteralSecret { .. })
        ));

        std::env::remove_var(EMPTY);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn initialize_apply_and_validate_only_use_one_record() {
        let secret_path = crate::test_utils::unique_temp_path("atomic_config_secret");
        std::fs::write(&secret_path, TEST_MNEMONIC_ONE).expect("write signing secret");
        let secret_reference = format!("file:{}", secret_path.display());
        let service = service().await;
        let first = document(&secret_reference, "first");
        let second = document(&secret_reference, "second");

        service
            .initialize(&first, None)
            .await
            .expect("initialize configuration");
        assert!(matches!(
            service.initialize(&first, None).await,
            Err(ConfigurationServiceError::Store(
                ConfigStoreError::AlreadyInitialized
            ))
        ));

        let outcome = service
            .apply(&second, true)
            .await
            .expect("validate replacement");
        assert!(!outcome.restart_required);
        assert_eq!(service.document().await.expect("stored document"), first);

        let running_snapshot = service.startup().await.expect("running snapshot");
        let outcome = service
            .apply(&second, false)
            .await
            .expect("replace configuration");
        assert!(outcome.restart_required);
        assert_eq!(service.document().await.expect("stored document"), second);
        assert_eq!(running_snapshot.resolved.settings.mint_info.name, "first");
        let next_startup = service.startup().await.expect("startup document");
        assert_eq!(next_startup.resolved.settings.mint_info.name, "second");
        assert!(!next_startup.applied);

        let _ = std::fs::remove_file(secret_path);
    }

    #[test]
    fn startup_document_ignores_general_operational_environment_overrides() {
        let _env_lock = crate::test_utils::env_lock();
        let secret_path = crate::test_utils::unique_temp_path("startup_config_secret");
        std::fs::write(&secret_path, TEST_MNEMONIC_ONE).expect("write signing secret");
        std::env::set_var(crate::env_vars::ENV_LISTEN_PORT, "6553");
        let document = format!(
            r#"
[info]
listen_port = 8091
mnemonic = "file:{}"

[database]
engine = "sqlite"
"#,
            secret_path.display()
        );
        let resolved =
            ConfigurationService::validate_document(&document).expect("validate startup document");
        assert_eq!(resolved.settings.info.listen_port, 8091);

        std::env::remove_var(crate::env_vars::ENV_LISTEN_PORT);
        let _ = std::fs::remove_file(secret_path);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn apply_rejects_signer_and_primary_database_changes() {
        let signer_path = crate::test_utils::unique_temp_path("signer_config_secret");
        let postgres_path = crate::test_utils::unique_temp_path("postgres_config_secret");
        std::fs::write(&signer_path, TEST_MNEMONIC_ONE).expect("write signing secret");
        std::fs::write(&postgres_path, "postgresql://localhost/cdk-test")
            .expect("write postgres secret");
        let service = service().await;
        let first = document(&format!("file:{}", signer_path.display()), "first");
        service
            .initialize(&first, None)
            .await
            .expect("initialize configuration");

        std::fs::write(&signer_path, TEST_MNEMONIC_TWO).expect("replace signing secret");
        assert!(matches!(
            service.apply(&first, false).await,
            Err(ConfigurationServiceError::SigningIdentityChange)
        ));

        std::fs::write(&signer_path, TEST_MNEMONIC_ONE).expect("restore signing secret");
        let postgres = format!(
            r#"
[info]
mnemonic = "file:{}"

[database]
engine = "postgres"

[database.postgres]
url = "file:{}"
"#,
            signer_path.display(),
            postgres_path.display()
        );
        assert!(matches!(
            service.apply(&postgres, false).await,
            Err(ConfigurationServiceError::PrimaryDatabaseChange)
        ));

        let _ = std::fs::remove_file(signer_path);
        let _ = std::fs::remove_file(postgres_path);
    }
}
