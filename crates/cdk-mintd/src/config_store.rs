//! Persistent storage for the authoritative mintd configuration document.

use std::fmt;
use std::sync::Arc;

use cdk::cdk_database::{self, KVStore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PRIMARY_NAMESPACE: &str = "cdk_mintd";
const SECONDARY_NAMESPACE: &str = "config";
const ACTIVE_KEY: &str = "active";

/// Serialization version for [`ConfigEnvelope`].
pub(crate) const CONFIG_FORMAT_VERSION: u32 = 1;

/// The single authoritative configuration record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigEnvelope {
    pub(crate) format_version: u32,
    pub(crate) toml: String,
    pub(crate) signing_identity: String,
    pub(crate) applied: bool,
}

impl ConfigEnvelope {
    pub(crate) fn new(toml: String, signing_identity: String) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            toml,
            signing_identity,
            applied: false,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, ConfigStoreError> {
        serde_json::to_vec(self).map_err(|source| ConfigStoreError::Encode { source })
    }

    fn decode(bytes: &[u8]) -> Result<Self, ConfigStoreError> {
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|source| ConfigStoreError::CorruptRecord { source })?;
        if envelope.format_version != CONFIG_FORMAT_VERSION {
            return Err(ConfigStoreError::UnsupportedFormatVersion {
                found: envelope.format_version,
                supported: CONFIG_FORMAT_VERSION,
            });
        }
        Ok(envelope)
    }
}

/// Configuration repository failures.
#[derive(Debug, Error)]
pub enum ConfigStoreError {
    /// Configuration already exists.
    #[error("mintd configuration is already initialized")]
    AlreadyInitialized,

    /// Configuration has not been initialized.
    #[error("mintd configuration is not initialized; run `cdk-mintd config init --file <path>`")]
    NotInitialized,

    /// A replacement attempted to change the immutable signer.
    #[error("configured signing identity does not match this mint database")]
    SigningIdentityMismatch,

    /// The stored envelope uses an unsupported serialization version.
    #[error(
        "unsupported mintd configuration format version {found}; supported version is {supported}"
    )]
    UnsupportedFormatVersion {
        /// Version read from the database.
        found: u32,
        /// Version understood by this binary.
        supported: u32,
    },

    /// Encoding the envelope failed.
    #[error("could not encode mintd configuration: {source}")]
    Encode {
        /// JSON encoding failure.
        #[source]
        source: serde_json::Error,
    },

    /// The stored envelope is malformed.
    #[error("persisted mintd configuration is malformed: {source}")]
    CorruptRecord {
        /// JSON decoding failure.
        #[source]
        source: serde_json::Error,
    },

    /// The underlying key-value database failed.
    #[error(transparent)]
    Database(#[from] cdk_database::Error),
}

/// Repository for the single active configuration envelope.
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
    pub(crate) fn new(store: Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync>) -> Self {
        Self { store }
    }

    /// Reads the authoritative configuration envelope.
    pub(crate) async fn active(&self) -> Result<ConfigEnvelope, ConfigStoreError> {
        let bytes = self
            .store
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .ok_or(ConfigStoreError::NotInitialized)?;
        ConfigEnvelope::decode(&bytes)
    }

    /// Creates the authoritative record without replacing an existing one.
    pub(crate) async fn initialize(
        &self,
        envelope: ConfigEnvelope,
    ) -> Result<(), ConfigStoreError> {
        let bytes = envelope.encode()?;
        let mut transaction = self.store.begin_transaction().await?;
        if transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .is_some()
        {
            transaction.rollback().await?;
            return Err(ConfigStoreError::AlreadyInitialized);
        }
        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY, &bytes)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Atomically replaces the document and marks it for next-start application.
    pub(crate) async fn replace(
        &self,
        toml: String,
        signing_identity: &str,
    ) -> Result<(), ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let bytes = transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .ok_or(ConfigStoreError::NotInitialized)?;
        let current = ConfigEnvelope::decode(&bytes)?;
        if current.signing_identity != signing_identity {
            transaction.rollback().await?;
            return Err(ConfigStoreError::SigningIdentityMismatch);
        }
        let replacement = ConfigEnvelope::new(toml, current.signing_identity).encode()?;
        transaction
            .kv_write(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVE_KEY,
                &replacement,
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Marks `expected_toml` applied if it is still the current document.
    ///
    /// Returns `false` when another apply replaced the document during startup.
    pub(crate) async fn mark_applied(&self, expected_toml: &str) -> Result<bool, ConfigStoreError> {
        let mut transaction = self.store.begin_transaction().await?;
        let bytes = transaction
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .ok_or(ConfigStoreError::NotInitialized)?;
        let mut current = ConfigEnvelope::decode(&bytes)?;
        if current.toml != expected_toml {
            transaction.commit().await?;
            return Ok(false);
        }
        if current.applied {
            transaction.commit().await?;
            return Ok(true);
        }
        current.applied = true;
        let bytes = current.encode()?;
        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY, &bytes)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use cdk_sqlite::mint::memory;

    use super::*;

    async fn repository() -> ConfigRepository {
        let database = Arc::new(memory::empty().await.expect("in-memory database"));
        ConfigRepository::new(database)
    }

    async fn write_raw(repository: &ConfigRepository, bytes: &[u8]) {
        let mut transaction = repository
            .store
            .begin_transaction()
            .await
            .expect("begin transaction");
        transaction
            .kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY, bytes)
            .await
            .expect("write raw record");
        transaction.commit().await.expect("commit transaction");
    }

    #[tokio::test]
    async fn initialize_and_replace_are_single_record_transitions() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        assert!(matches!(
            repository
                .initialize(ConfigEnvelope::new("again".to_owned(), "signer".to_owned()))
                .await,
            Err(ConfigStoreError::AlreadyInitialized)
        ));

        repository
            .replace("second".to_owned(), "signer")
            .await
            .expect("replace configuration");
        let active = repository.active().await.expect("read configuration");
        assert_eq!(active.toml, "second");
        assert!(!active.applied);
    }

    #[tokio::test]
    async fn older_startup_cannot_mark_replacement_applied() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        repository
            .replace("second".to_owned(), "signer")
            .await
            .expect("replace configuration");

        assert!(!repository
            .mark_applied("first")
            .await
            .expect("compare configuration"));
        assert!(
            !repository
                .active()
                .await
                .expect("read configuration")
                .applied
        );
        assert!(repository
            .mark_applied("second")
            .await
            .expect("mark current configuration"));
        assert!(
            repository
                .active()
                .await
                .expect("read configuration")
                .applied
        );
    }

    #[tokio::test]
    async fn corrupt_record_is_rejected() {
        let repository = repository().await;
        write_raw(&repository, b"{").await;

        assert!(matches!(
            repository.active().await,
            Err(ConfigStoreError::CorruptRecord { .. })
        ));
    }

    #[tokio::test]
    async fn unsupported_format_version_is_rejected() {
        let repository = repository().await;
        let unsupported = serde_json::to_vec(&ConfigEnvelope {
            format_version: CONFIG_FORMAT_VERSION + 1,
            toml: "document".to_owned(),
            signing_identity: "signer".to_owned(),
            applied: false,
        })
        .expect("encode test record");
        write_raw(&repository, &unsupported).await;

        assert!(matches!(
            repository.active().await,
            Err(ConfigStoreError::UnsupportedFormatVersion { .. })
        ));
    }

    #[tokio::test]
    async fn not_initialized_and_signing_identity_mismatch_are_rejected() {
        let repository = repository().await;
        assert!(matches!(
            repository.active().await,
            Err(ConfigStoreError::NotInitialized)
        ));
        assert!(matches!(
            repository.replace("next".to_owned(), "signer").await,
            Err(ConfigStoreError::NotInitialized)
        ));
        assert!(matches!(
            repository.mark_applied("next").await,
            Err(ConfigStoreError::NotInitialized)
        ));

        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        assert!(matches!(
            repository
                .replace("second".to_owned(), "other-signer")
                .await,
            Err(ConfigStoreError::SigningIdentityMismatch)
        ));
        assert_eq!(
            repository.active().await.expect("read configuration").toml,
            "first"
        );
    }

    #[tokio::test]
    async fn mark_applied_is_idempotent_for_current_document() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("doc".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");

        assert!(repository
            .mark_applied("doc")
            .await
            .expect("mark applied once"));
        assert!(
            repository
                .active()
                .await
                .expect("read configuration")
                .applied
        );
        assert!(repository
            .mark_applied("doc")
            .await
            .expect("mark applied twice"));
        assert!(
            repository
                .active()
                .await
                .expect("read configuration")
                .applied
        );

        let debug = format!("{repository:?}");
        assert!(debug.contains("ConfigRepository"));
        assert!(!debug.contains("store:"));
    }
}
