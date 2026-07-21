//! Persistent storage for authoritative mintd configuration documents.
//!
//! This module deliberately treats TOML as an opaque interchange document. Parsing,
//! validation, redaction, and classifying restart-required changes belong to the
//! configuration service. The repository only provides transactional active/pending
//! state transitions.

use std::fmt;
use std::sync::Arc;

use cdk::cdk_database::{self, KVStore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PRIMARY_NAMESPACE: &str = "cdk_mintd";
const SECONDARY_NAMESPACE: &str = "config";
const ACTIVE_KEY: &str = "active";
const PENDING_KEY: &str = "pending";
const ACTIVATION_BACKUP_KEY: &str = "activation_backup";
const INITIAL_ACTIVATION_KEY: &str = "initial_activation";
const SIGNING_IDENTITY_KEY: &str = "signing_identity";
const INITIAL_ACTIVATION_VALUE: &[u8] = b"1";

/// Schema version used for persisted configuration envelopes.
///
/// This is a serialization schema version, not a configuration revision or an
/// optimistic-concurrency token.
pub(crate) const CONFIG_FORMAT_VERSION: u32 = 1;

/// Errors returned by [`ConfigRepository`].
#[derive(Debug, Error)]
pub enum ConfigStoreError {
    /// An active configuration already exists.
    #[error("mintd configuration is already initialized")]
    AlreadyInitialized,

    /// A pending configuration already exists.
    #[error("a pending mintd configuration already exists")]
    PendingConfigurationExists,

    /// The pending document changed while a startup candidate was being prepared.
    #[error(
        "the pending mintd configuration changed during startup; retry with the current pending document"
    )]
    PendingConfigurationChanged,

    /// No pending configuration exists.
    #[error("no pending mintd configuration exists")]
    NoPendingConfiguration,

    /// An initialization attempted to bind a different signer to this database.
    #[error("mintd signing identity does not match the identity bound to this database")]
    SigningIdentityMismatch,

    /// Pending activation was not prepared before promotion or rollback.
    #[error("pending mintd configuration activation is not prepared")]
    ActivationNotPrepared,

    /// A stored document uses an unsupported schema version.
    #[error(
        "unsupported mintd configuration format version {found}; supported version is {supported}"
    )]
    UnsupportedFormatVersion {
        /// Version found in the persisted envelope.
        found: u32,
        /// Version supported by this binary.
        supported: u32,
    },

    /// A configuration envelope could not be encoded.
    #[error("could not encode mintd configuration envelope: {source}")]
    Encode {
        /// Serialization error.
        #[source]
        source: serde_json::Error,
    },

    /// A persisted configuration envelope is malformed.
    #[error("persisted mintd configuration record `{key}` is malformed: {source}")]
    CorruptRecord {
        /// KV key containing the malformed value.
        key: &'static str,
        /// Deserialization error.
        #[source]
        source: serde_json::Error,
    },

    /// The underlying database operation failed.
    #[error(transparent)]
    Database(#[from] cdk_database::Error),

    /// Writing canonical mint configuration failed.
    #[error(transparent)]
    Canonical(#[from] cdk::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigEnvelope {
    format_version: u32,
    toml: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalBackup {
    format_version: u32,
    mint_info: Option<cdk::nuts::MintInfo>,
    quote_ttl: Option<cdk_common::common::QuoteTTL>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SigningIdentityEnvelope {
    format_version: u32,
    fingerprint: String,
}

fn encode_record<T>(value: &T) -> Result<Vec<u8>, ConfigStoreError>
where
    T: Serialize,
{
    serde_json::to_vec(value).map_err(|source| ConfigStoreError::Encode { source })
}

fn decode_record<T>(key: &'static str, bytes: &[u8]) -> Result<T, ConfigStoreError>
where
    T: for<'de> Deserialize<'de>,
{
    let value: T = serde_json::from_slice(bytes)
        .map_err(|source| ConfigStoreError::CorruptRecord { key, source })?;
    Ok(value)
}

fn require_format_version(found: u32) -> Result<(), ConfigStoreError> {
    if found != CONFIG_FORMAT_VERSION {
        return Err(ConfigStoreError::UnsupportedFormatVersion {
            found,
            supported: CONFIG_FORMAT_VERSION,
        });
    }
    Ok(())
}

impl ConfigEnvelope {
    fn new(toml: String) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            toml,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, ConfigStoreError> {
        encode_record(self)
    }

    fn decode(key: &'static str, bytes: &[u8]) -> Result<Self, ConfigStoreError> {
        let envelope: Self = decode_record(key, bytes)?;
        require_format_version(envelope.format_version)?;
        Ok(envelope)
    }
}

impl CanonicalBackup {
    fn new(
        mint_info: Option<cdk::nuts::MintInfo>,
        quote_ttl: Option<cdk_common::common::QuoteTTL>,
    ) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            mint_info,
            quote_ttl,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, ConfigStoreError> {
        encode_record(self)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ConfigStoreError> {
        let backup: Self = decode_record(ACTIVATION_BACKUP_KEY, bytes)?;
        require_format_version(backup.format_version)?;
        Ok(backup)
    }
}

impl SigningIdentityEnvelope {
    fn new(fingerprint: String) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            fingerprint,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, ConfigStoreError> {
        encode_record(self)
    }

    fn decode(bytes: &[u8]) -> Result<Self, ConfigStoreError> {
        let envelope: Self = decode_record(SIGNING_IDENTITY_KEY, bytes)?;
        require_format_version(envelope.format_version)?;
        Ok(envelope)
    }
}

/// Repository for the active and pending mintd configuration documents.
///
/// The repository does not expose general-purpose revisions or compare-and-set
/// updates. Online mutations are serialized at the service layer; activation
/// additionally verifies the exact pending document selected for startup so a
/// concurrently replaced candidate cannot be promoted accidentally.
#[derive(Clone)]
pub(crate) struct ConfigRepository {
    store: Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync>,
}

impl fmt::Debug for ConfigRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigRepository").finish_non_exhaustive()
    }
}

impl ConfigRepository {
    /// Creates a configuration repository backed by the mint's KV store.
    pub(crate) fn new(store: Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync>) -> Self {
        Self { store }
    }

    /// Returns the active TOML document, if configuration has been initialized.
    pub(crate) async fn active(&self) -> Result<Option<String>, ConfigStoreError> {
        self.read_document(ACTIVE_KEY).await
    }

    /// Returns the pending TOML document, if a restart-required update is staged.
    pub(crate) async fn pending(&self) -> Result<Option<String>, ConfigStoreError> {
        self.read_document(PENDING_KEY).await
    }

    /// Initializes the active configuration without staging a first activation.
    ///
    /// This is used by unit tests that need an already-active document without
    /// going through the pending-activation path.
    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) async fn initialize(
        &self,
        toml: String,
        signing_identity: String,
    ) -> Result<(), ConfigStoreError> {
        self.initialize_with_options(toml, signing_identity, false)
            .await
    }

    /// Initializes the active document and stages the same document for its
    /// first authoritative activation.
    ///
    /// Keeping the first import pending makes startup use the same prepared,
    /// rollback-capable promotion path as later file replacements. Writing the
    /// two records in one transaction also keeps an interrupted `config init`
    /// from leaving a document that would still be overlaid by legacy canonical
    /// mint metadata.
    pub(crate) async fn initialize_for_activation(
        &self,
        toml: String,
        signing_identity: String,
    ) -> Result<(), ConfigStoreError> {
        self.initialize_with_options(toml, signing_identity, true)
            .await
    }

    async fn initialize_with_options(
        &self,
        toml: String,
        signing_identity: String,
        stage_for_activation: bool,
    ) -> Result<(), ConfigStoreError> {
        let bytes = ConfigEnvelope::new(toml).encode()?;
        let signing_identity_bytes =
            SigningIdentityEnvelope::new(signing_identity.clone()).encode()?;
        let mut transaction = self.store.begin_transaction().await?;

        let active_exists = transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .is_some();
        let pending_exists = if stage_for_activation {
            transaction
                .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
                .await?
                .is_some()
        } else {
            false
        };
        if active_exists || pending_exists {
            transaction.rollback().await?;
            return Err(ConfigStoreError::AlreadyInitialized);
        }

        match transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, SIGNING_IDENTITY_KEY)
            .await?
        {
            Some(bytes)
                if SigningIdentityEnvelope::decode(&bytes)?.fingerprint != signing_identity =>
            {
                transaction.rollback().await?;
                return Err(ConfigStoreError::SigningIdentityMismatch);
            }
            _ => {}
        }

        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY, &bytes)
            .await?;
        if stage_for_activation {
            transaction
                .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY, &bytes)
                .await?;
            transaction
                .kv_write(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    INITIAL_ACTIVATION_KEY,
                    INITIAL_ACTIVATION_VALUE,
                )
                .await?;
        }
        transaction
            .kv_write(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                SIGNING_IDENTITY_KEY,
                &signing_identity_bytes,
            )
            .await?;
        transaction.commit().await?;

        Ok(())
    }

    /// Stages a TOML document for activation after a restart.
    ///
    /// This operation refuses to overwrite an existing pending configuration.
    pub(crate) async fn stage(&self, toml: String) -> Result<(), ConfigStoreError> {
        let bytes = ConfigEnvelope::new(toml).encode()?;
        let mut transaction = self.store.begin_transaction().await?;

        if transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?
            .is_some()
        {
            transaction.rollback().await?;
            return Err(ConfigStoreError::PendingConfigurationExists);
        }

        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY, &bytes)
            .await?;
        transaction.commit().await?;

        Ok(())
    }

    /// Discards the staged configuration.
    ///
    /// Returns an error when no pending configuration exists.
    pub(crate) async fn discard_pending(&self) -> Result<(), ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;

        if transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?
            .is_none()
        {
            transaction.rollback().await?;
            return Err(ConfigStoreError::NoPendingConfiguration);
        }

        let is_initial_activation = transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                INITIAL_ACTIVATION_KEY,
            )
            .await?
            .is_some();

        if let Some(backup_bytes) = transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?
        {
            let backup = CanonicalBackup::decode(&backup_bytes)?;
            cdk::mint::replace_mint_configuration_in_transaction(
                transaction.as_mut(),
                backup.mint_info.as_ref(),
                backup.quote_ttl.as_ref(),
            )
            .await?;
            transaction
                .kv_remove(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    ACTIVATION_BACKUP_KEY,
                )
                .await?;
        }

        transaction
            .kv_remove(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?;
        if is_initial_activation {
            transaction
                .kv_remove(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
                .await?;
            transaction
                .kv_remove(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    INITIAL_ACTIVATION_KEY,
                )
                .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    /// Creates a durable rollback point before candidate startup can mutate
    /// canonical mint records.
    ///
    /// If a prior startup crashed, its existing rollback point is restored
    /// first and retained for the new attempt.
    pub(crate) async fn prepare_pending_activation(
        &self,
        expected_toml: &str,
        mint_info: Option<&cdk::nuts::MintInfo>,
        quote_ttl: Option<&cdk_common::common::QuoteTTL>,
    ) -> Result<(), ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let pending_bytes = match transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?
        {
            Some(bytes) => bytes,
            None => {
                transaction.rollback().await?;
                return Err(ConfigStoreError::NoPendingConfiguration);
            }
        };
        if ConfigEnvelope::decode(PENDING_KEY, &pending_bytes)?.toml != expected_toml {
            transaction.rollback().await?;
            return Err(ConfigStoreError::PendingConfigurationChanged);
        }

        match transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?
        {
            Some(backup_bytes) => {
                let backup = CanonicalBackup::decode(&backup_bytes)?;
                cdk::mint::replace_mint_configuration_in_transaction(
                    transaction.as_mut(),
                    backup.mint_info.as_ref(),
                    backup.quote_ttl.as_ref(),
                )
                .await?;
            }
            None => {
                let backup = CanonicalBackup::new(mint_info.cloned(), quote_ttl.copied());
                transaction
                    .kv_write(
                        PRIMARY_NAMESPACE,
                        SECONDARY_NAMESPACE,
                        ACTIVATION_BACKUP_KEY,
                        &backup.encode()?,
                    )
                    .await?;
            }
        }

        transaction.commit().await?;
        Ok(())
    }

    /// Restores the durable canonical rollback point while retaining both the
    /// pending document and rollback point for retry or explicit discard.
    pub(crate) async fn rollback_pending_activation(&self) -> Result<(), ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let backup_bytes = match transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?
        {
            Some(bytes) => bytes,
            None => {
                transaction.rollback().await?;
                return Err(ConfigStoreError::ActivationNotPrepared);
            }
        };
        let backup = CanonicalBackup::decode(&backup_bytes)?;
        cdk::mint::replace_mint_configuration_in_transaction(
            transaction.as_mut(),
            backup.mint_info.as_ref(),
            backup.quote_ttl.as_ref(),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Atomically promotes pending daemon, mint-info, and quote-TTL state.
    pub(crate) async fn promote_pending_with_canonical(
        &self,
        expected_toml: &str,
        mint_info: &cdk::nuts::MintInfo,
        quote_ttl: &cdk_common::common::QuoteTTL,
    ) -> Result<String, ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let pending_bytes = match transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?
        {
            Some(bytes) => bytes,
            None => {
                transaction.rollback().await?;
                return Err(ConfigStoreError::NoPendingConfiguration);
            }
        };

        let envelope = match ConfigEnvelope::decode(PENDING_KEY, &pending_bytes) {
            Ok(envelope) => envelope,
            Err(error) => {
                transaction.rollback().await?;
                return Err(error);
            }
        };
        if envelope.toml != expected_toml {
            transaction.rollback().await?;
            return Err(ConfigStoreError::PendingConfigurationChanged);
        }

        if transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?
            .is_none()
        {
            transaction.rollback().await?;
            return Err(ConfigStoreError::ActivationNotPrepared);
        }

        cdk::mint::write_mint_info_to_transaction(transaction.as_mut(), mint_info).await?;
        cdk::mint::write_quote_ttl_to_transaction(transaction.as_mut(), quote_ttl).await?;
        transaction
            .kv_write(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVE_KEY,
                &pending_bytes,
            )
            .await?;
        transaction
            .kv_remove(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, PENDING_KEY)
            .await?;
        transaction
            .kv_remove(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?;
        transaction
            .kv_remove(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                INITIAL_ACTIVATION_KEY,
            )
            .await?;
        transaction.commit().await?;

        Ok(envelope.toml)
    }

    /// Returns the durable canonical rollback point, when a pending activation
    /// has started but not completed.
    pub(crate) async fn canonical_backup(
        &self,
    ) -> Result<
        Option<(
            Option<cdk::nuts::MintInfo>,
            Option<cdk_common::common::QuoteTTL>,
        )>,
        ConfigStoreError,
    > {
        let mut transaction = self.store.begin_transaction().await?;
        let bytes = transaction
            .kv_read(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVATION_BACKUP_KEY,
            )
            .await?;
        transaction.commit().await?;

        bytes
            .map(|bytes| {
                CanonicalBackup::decode(&bytes).map(|backup| (backup.mint_info, backup.quote_ttl))
            })
            .transpose()
    }

    /// Returns the immutable signing-identity fingerprint established at init.
    pub(crate) async fn signing_identity(&self) -> Result<Option<String>, ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let bytes = transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, SIGNING_IDENTITY_KEY)
            .await?;
        transaction.commit().await?;

        bytes
            .map(|bytes| {
                SigningIdentityEnvelope::decode(&bytes).map(|envelope| envelope.fingerprint)
            })
            .transpose()
    }

    async fn read_document(&self, key: &'static str) -> Result<Option<String>, ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let bytes = transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, key)
            .await?;
        transaction.commit().await?;

        bytes
            .map(|bytes| ConfigEnvelope::decode(key, &bytes).map(|envelope| envelope.toml))
            .transpose()
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    const TEST_SIGNING_IDENTITY: &str = "test-signing-identity";

    async fn repository() -> ConfigRepository {
        let database = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory SQLite database");
        ConfigRepository::new(Arc::new(database))
    }

    async fn write_raw(repository: &ConfigRepository, key: &str, value: &[u8]) {
        let mut transaction = repository
            .store
            .begin_transaction()
            .await
            .expect("begin transaction");
        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, key, value)
            .await
            .expect("write raw configuration");
        transaction.commit().await.expect("commit transaction");
    }

    async fn write_canonical(
        repository: &ConfigRepository,
        mint_info: Option<&cdk::nuts::MintInfo>,
        quote_ttl: Option<&cdk_common::common::QuoteTTL>,
    ) {
        let mut transaction = repository
            .store
            .begin_transaction()
            .await
            .expect("begin transaction");
        cdk::mint::replace_mint_configuration_in_transaction(
            transaction.as_mut(),
            mint_info,
            quote_ttl,
        )
        .await
        .expect("write canonical configuration");
        transaction.commit().await.expect("commit transaction");
    }

    async fn read_canonical(
        repository: &ConfigRepository,
    ) -> (
        Option<cdk::nuts::MintInfo>,
        Option<cdk_common::common::QuoteTTL>,
    ) {
        let mint_info = repository
            .store
            .kv_read("cdk_mint", "config", "mint_info")
            .await
            .expect("read mint info")
            .map(|bytes| serde_json::from_slice(&bytes).expect("decode mint info"));
        let quote_ttl = repository
            .store
            .kv_read("cdk_mint", "config", "quote_ttl")
            .await
            .expect("read quote TTL")
            .map(|bytes| serde_json::from_slice(&bytes).expect("decode quote TTL"));
        (mint_info, quote_ttl)
    }

    #[tokio::test]
    async fn initialize_refuses_to_replace_active_configuration() {
        let repository = repository().await;

        assert_eq!(repository.active().await.expect("read active"), None);
        repository
            .initialize(
                "[mint]\nname = \"first\"\n".to_owned(),
                TEST_SIGNING_IDENTITY.to_owned(),
            )
            .await
            .expect("initialize configuration");

        let error = repository
            .initialize(
                "[mint]\nname = \"second\"\n".to_owned(),
                "different-signing-identity".to_owned(),
            )
            .await
            .expect_err("must refuse to replace active configuration");

        assert!(matches!(error, ConfigStoreError::AlreadyInitialized));
        assert_eq!(
            repository.active().await.expect("read active"),
            Some("[mint]\nname = \"first\"\n".to_owned())
        );
    }

    #[tokio::test]
    async fn first_import_is_staged_for_authoritative_activation() {
        let repository = repository().await;
        let imported = "[mint]\nname = \"imported\"\n".to_owned();
        let legacy_info = cdk::nuts::MintInfo {
            name: Some("legacy".to_string()),
            ..Default::default()
        };
        let legacy_ttl = cdk_common::common::QuoteTTL::new(10, 20);
        let imported_info = cdk::nuts::MintInfo {
            name: Some("imported".to_string()),
            ..Default::default()
        };
        let imported_ttl = cdk_common::common::QuoteTTL::new(30, 40);
        write_canonical(&repository, Some(&legacy_info), Some(&legacy_ttl)).await;

        repository
            .initialize_for_activation(imported.clone(), TEST_SIGNING_IDENTITY.to_owned())
            .await
            .expect("stage first authoritative import");

        assert_eq!(
            repository.active().await.expect("read active"),
            Some(imported.clone())
        );
        assert_eq!(
            repository.pending().await.expect("read pending"),
            Some(imported.clone())
        );
        assert_eq!(
            repository
                .signing_identity()
                .await
                .expect("read signing identity"),
            Some(TEST_SIGNING_IDENTITY.to_owned())
        );
        assert_eq!(
            read_canonical(&repository).await,
            (Some(legacy_info.clone()), Some(legacy_ttl))
        );

        repository
            .prepare_pending_activation(&imported, Some(&legacy_info), Some(&legacy_ttl))
            .await
            .expect("prepare first activation");
        repository
            .promote_pending_with_canonical(&imported, &imported_info, &imported_ttl)
            .await
            .expect("promote first import");

        assert_eq!(
            repository.active().await.expect("read active"),
            Some(imported)
        );
        assert_eq!(repository.pending().await.expect("read pending"), None);
        assert_eq!(
            read_canonical(&repository).await,
            (Some(imported_info), Some(imported_ttl))
        );
    }

    #[tokio::test]
    async fn first_import_refuses_existing_pending_configuration() {
        let repository = repository().await;
        repository
            .stage("[mint]\nname = \"pending\"\n".to_owned())
            .await
            .expect("stage configuration");

        let error = repository
            .initialize_for_activation(
                "[mint]\nname = \"imported\"\n".to_owned(),
                TEST_SIGNING_IDENTITY.to_owned(),
            )
            .await
            .expect_err("first import must not replace existing state");

        assert!(matches!(error, ConfigStoreError::AlreadyInitialized));
        assert_eq!(repository.active().await.expect("read active"), None);
        assert_eq!(
            repository.pending().await.expect("read pending"),
            Some("[mint]\nname = \"pending\"\n".to_owned())
        );
    }

    #[tokio::test]
    async fn discarding_first_activation_restores_uninitialized_state() {
        let repository = repository().await;
        let imported = "[mint]\nname = \"imported\"\n".to_owned();
        let legacy_info = cdk::nuts::MintInfo {
            name: Some("legacy".to_string()),
            ..Default::default()
        };
        let legacy_ttl = cdk_common::common::QuoteTTL::new(10, 20);
        let candidate_info = cdk::nuts::MintInfo {
            name: Some("candidate".to_string()),
            ..Default::default()
        };
        let candidate_ttl = cdk_common::common::QuoteTTL::new(30, 40);
        write_canonical(&repository, Some(&legacy_info), Some(&legacy_ttl)).await;
        repository
            .initialize_for_activation(imported.clone(), TEST_SIGNING_IDENTITY.to_owned())
            .await
            .expect("stage first activation");
        repository
            .prepare_pending_activation(&imported, Some(&legacy_info), Some(&legacy_ttl))
            .await
            .expect("prepare first activation");
        write_canonical(&repository, Some(&candidate_info), Some(&candidate_ttl)).await;

        repository
            .discard_pending()
            .await
            .expect("discard first activation");

        assert_eq!(repository.active().await.expect("read active"), None);
        assert_eq!(repository.pending().await.expect("read pending"), None);
        assert_eq!(
            repository
                .signing_identity()
                .await
                .expect("read signing identity"),
            Some(TEST_SIGNING_IDENTITY.to_owned())
        );
        assert_eq!(
            read_canonical(&repository).await,
            (Some(legacy_info), Some(legacy_ttl))
        );
        assert!(repository
            .canonical_backup()
            .await
            .expect("read activation backup")
            .is_none());

        let error = repository
            .initialize_for_activation(
                "[mint]\nname = \"different signer\"\n".to_owned(),
                "different-signing-identity".to_owned(),
            )
            .await
            .expect_err("discard must not permit rebinding the mint signer");
        assert!(matches!(error, ConfigStoreError::SigningIdentityMismatch));
    }

    #[tokio::test]
    async fn stage_and_discard_preserve_active_configuration() {
        let repository = repository().await;
        let active = "[mint]\nname = \"active\"\n".to_owned();
        let pending = "[mint]\nname = \"pending\"\n".to_owned();

        repository
            .initialize(active.clone(), TEST_SIGNING_IDENTITY.to_owned())
            .await
            .expect("initialize configuration");
        repository
            .stage(pending.clone())
            .await
            .expect("stage configuration");

        assert_eq!(
            repository.pending().await.expect("read pending"),
            Some(pending)
        );
        assert!(matches!(
            repository
                .stage("replacement".to_owned())
                .await
                .expect_err("must refuse to replace pending configuration"),
            ConfigStoreError::PendingConfigurationExists
        ));

        repository
            .discard_pending()
            .await
            .expect("discard pending configuration");

        assert_eq!(
            repository.active().await.expect("read active"),
            Some(active)
        );
        assert_eq!(repository.pending().await.expect("read pending"), None);
        assert!(matches!(
            repository
                .discard_pending()
                .await
                .expect_err("must report missing pending configuration"),
            ConfigStoreError::NoPendingConfiguration
        ));
    }

    #[tokio::test]
    async fn activation_refuses_a_pending_document_replaced_during_startup() {
        let repository = repository().await;
        let active = "[mint]\nname = \"active\"\n".to_owned();
        let pending_a = "[mint]\nname = \"pending-a\"\n".to_owned();
        let pending_b = "[mint]\nname = \"pending-b\"\n".to_owned();
        repository
            .initialize(active, TEST_SIGNING_IDENTITY.to_owned())
            .await
            .expect("initialize configuration");
        repository
            .stage(pending_a.clone())
            .await
            .expect("stage first candidate");
        repository
            .discard_pending()
            .await
            .expect("discard first candidate");
        repository
            .stage(pending_b.clone())
            .await
            .expect("stage replacement candidate");

        let prepare_error = repository
            .prepare_pending_activation(&pending_a, None, None)
            .await
            .expect_err("preparation must be tied to the selected candidate");
        assert!(matches!(
            prepare_error,
            ConfigStoreError::PendingConfigurationChanged
        ));
        assert_eq!(
            repository.pending().await.expect("read pending"),
            Some(pending_b.clone())
        );

        repository
            .prepare_pending_activation(&pending_b, None, None)
            .await
            .expect("prepare replacement candidate");
        repository
            .discard_pending()
            .await
            .expect("discard replacement candidate");
        repository
            .stage(pending_a.clone())
            .await
            .expect("stage another candidate");
        let promotion_error = repository
            .promote_pending_with_canonical(
                &pending_b,
                &cdk::nuts::MintInfo::default(),
                &cdk_common::common::QuoteTTL::default(),
            )
            .await
            .expect_err("promotion must be tied to the prepared candidate");
        assert!(matches!(
            promotion_error,
            ConfigStoreError::PendingConfigurationChanged
        ));
        assert_eq!(
            repository.pending().await.expect("read pending"),
            Some(pending_a)
        );
    }

    #[tokio::test]
    async fn promote_pending_replaces_active_and_canonical_state_atomically() {
        let repository = repository().await;
        let pending = "[mint]\nname = \"promoted\"\n".to_owned();
        let old_info = cdk::nuts::MintInfo {
            name: Some("old".to_string()),
            ..Default::default()
        };
        let old_ttl = cdk_common::common::QuoteTTL::new(10, 20);
        let promoted_info = cdk::nuts::MintInfo {
            name: Some("promoted".to_string()),
            ..Default::default()
        };
        let promoted_ttl = cdk_common::common::QuoteTTL::new(30, 40);

        repository
            .initialize(
                "[mint]\nname = \"old\"\n".to_owned(),
                TEST_SIGNING_IDENTITY.to_owned(),
            )
            .await
            .expect("initialize configuration");
        repository
            .stage(pending.clone())
            .await
            .expect("stage configuration");
        write_canonical(&repository, Some(&old_info), Some(&old_ttl)).await;
        repository
            .prepare_pending_activation(&pending, Some(&old_info), Some(&old_ttl))
            .await
            .expect("prepare pending activation");

        let promoted = repository
            .promote_pending_with_canonical(&pending, &promoted_info, &promoted_ttl)
            .await
            .expect("promote pending configuration");

        assert_eq!(promoted, pending);
        assert_eq!(
            repository.active().await.expect("read active"),
            Some(promoted)
        );
        assert_eq!(repository.pending().await.expect("read pending"), None);
        assert_eq!(
            read_canonical(&repository).await,
            (Some(promoted_info), Some(promoted_ttl))
        );
        assert!(repository
            .canonical_backup()
            .await
            .expect("read activation backup")
            .is_none());
        assert!(matches!(
            repository
                .promote_pending_with_canonical(
                    &pending,
                    &cdk::nuts::MintInfo::default(),
                    &cdk_common::common::QuoteTTL::default(),
                )
                .await
                .expect_err("must report missing pending configuration"),
            ConfigStoreError::NoPendingConfiguration
        ));
    }

    #[tokio::test]
    async fn discard_after_interrupted_activation_restores_canonical_backup() {
        let repository = repository().await;
        let active = "[mint]\nname = \"active\"\n".to_owned();
        let active_info = cdk::nuts::MintInfo {
            name: Some("active".to_string()),
            ..Default::default()
        };
        let active_ttl = cdk_common::common::QuoteTTL::new(10, 20);
        let candidate_info = cdk::nuts::MintInfo {
            name: Some("candidate".to_string()),
            ..Default::default()
        };
        let candidate_ttl = cdk_common::common::QuoteTTL::new(30, 40);
        let candidate_document = "[mint]\nname = \"candidate\"\n".to_owned();

        repository
            .initialize(active.clone(), TEST_SIGNING_IDENTITY.to_owned())
            .await
            .expect("initialize configuration");
        repository
            .stage(candidate_document.clone())
            .await
            .expect("stage configuration");
        write_canonical(&repository, Some(&active_info), Some(&active_ttl)).await;
        repository
            .prepare_pending_activation(&candidate_document, Some(&active_info), Some(&active_ttl))
            .await
            .expect("prepare pending activation");

        write_canonical(&repository, Some(&candidate_info), Some(&candidate_ttl)).await;
        repository
            .prepare_pending_activation(
                &candidate_document,
                Some(&candidate_info),
                Some(&candidate_ttl),
            )
            .await
            .expect("retry restores the original rollback point");
        assert_eq!(
            read_canonical(&repository).await,
            (Some(active_info.clone()), Some(active_ttl))
        );

        write_canonical(&repository, Some(&candidate_info), Some(&candidate_ttl)).await;
        repository
            .discard_pending()
            .await
            .expect("discard interrupted activation");

        assert_eq!(
            repository.active().await.expect("read active"),
            Some(active)
        );
        assert_eq!(repository.pending().await.expect("read pending"), None);
        assert_eq!(
            read_canonical(&repository).await,
            (Some(active_info), Some(active_ttl))
        );
        assert!(repository
            .canonical_backup()
            .await
            .expect("read activation backup")
            .is_none());
    }

    #[tokio::test]
    async fn unsupported_format_version_is_rejected() {
        let repository = repository().await;
        let envelope = serde_json::to_vec(&serde_json::json!({
            "format_version": CONFIG_FORMAT_VERSION + 1,
            "toml": "[mint]\nname = \"future\"\n",
        }))
        .expect("serialize test envelope");
        write_raw(&repository, ACTIVE_KEY, &envelope).await;

        let error = repository
            .active()
            .await
            .expect_err("unsupported format must fail");

        assert!(matches!(
            error,
            ConfigStoreError::UnsupportedFormatVersion {
                found,
                supported: CONFIG_FORMAT_VERSION,
            } if found == CONFIG_FORMAT_VERSION + 1
        ));
    }

    #[tokio::test]
    async fn unsupported_signing_identity_format_version_is_rejected() {
        let repository = repository().await;
        let unsupported = serde_json::json!({
            "format_version": CONFIG_FORMAT_VERSION + 1,
            "fingerprint": TEST_SIGNING_IDENTITY,
        });
        let bytes = serde_json::to_vec(&unsupported).expect("encode unsupported identity");
        write_raw(&repository, SIGNING_IDENTITY_KEY, &bytes).await;

        let error = repository
            .signing_identity()
            .await
            .expect_err("unsupported signing identity version must fail");

        assert!(matches!(
            error,
            ConfigStoreError::UnsupportedFormatVersion { .. }
        ));
    }
}
