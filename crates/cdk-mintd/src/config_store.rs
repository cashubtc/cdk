//! Persistent storage for the authoritative mintd configuration document.

use std::fmt;
use std::sync::Arc;

use cdk::cdk_database::{self, KVStoreCompareAndSwap};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PRIMARY_NAMESPACE: &str = "cdk_mintd";
const SECONDARY_NAMESPACE: &str = "config";
const ACTIVE_KEY: &str = "active";
const MAX_CAS_ATTEMPTS: usize = 8;

/// Serialization version for [`ConfigEnvelope`].
pub(crate) const CONFIG_FORMAT_VERSION: u32 = 1;

/// Lifecycle state of the stored configuration document.
///
/// The state machine has two persisted states:
///
/// ```text
///   ┌─────────┐  initialize / replace / rollback  ┌─────────┐
///   │ Pending │ ────────────────────────────────▶ │         │
///   │         │ ◀──────────────────────────────── │ Applied │
///   └─────────┘   (a write always stages Pending) └─────────┘
///        │                                            ▲
///        └────────── completed daemon startup ────────┘
///                 (mark_applied, revision-guarded CAS)
/// ```
///
/// Every document write stages a `Pending` record; only a daemon startup
/// that brought every service up transitions it to `Applied`. A startup
/// that fails midway leaves the record `Pending`, so the next start
/// reconciles the document again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentState {
    /// Stored but never served by a daemon. The next startup forces the
    /// document's canonical mint info and quote TTL into the database and
    /// commits `Applied` once all services are up.
    Pending,
    /// Served by a completed daemon startup. Later starts preserve
    /// RPC-managed canonical values when management RPC is enabled.
    Applied,
}

/// The single authoritative configuration record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigEnvelope {
    pub(crate) format_version: u32,
    #[serde(default)]
    pub(crate) revision: u64,
    pub(crate) toml: String,
    #[serde(default)]
    pub(crate) previous_applied_toml: Option<String>,
    pub(crate) signing_identity: String,
    pub(crate) applied: bool,
    #[serde(default)]
    pub(crate) allow_new_bdk_wallet: bool,
}

impl ConfigEnvelope {
    pub(crate) fn new(toml: String, signing_identity: String) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            revision: 1,
            toml,
            previous_applied_toml: None,
            signing_identity,
            applied: false,
            allow_new_bdk_wallet: false,
        }
    }

    pub(crate) fn with_new_bdk_wallet_allowed(mut self, allowed: bool) -> Self {
        self.allow_new_bdk_wallet = allowed;
        self
    }

    /// Lifecycle state of the stored document.
    pub(crate) fn state(&self) -> DocumentState {
        if self.applied {
            DocumentState::Applied
        } else {
            DocumentState::Pending
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
    #[error(
        "mintd configuration is not initialized; run `cdk-mintd config init --new-mint --file <path>` for a new mint or `cdk-mintd config init --existing-mint --file <path>` for an existing mint"
    )]
    NotInitialized,

    /// A replacement attempted to change the immutable signer.
    #[error("configured signing identity does not match this mint database")]
    SigningIdentityMismatch,

    /// Too many concurrent writers prevented a configuration update.
    #[error("mintd configuration changed concurrently; retry the operation")]
    ConcurrentModification,

    /// The persisted configuration revision cannot be incremented.
    #[error("mintd configuration revision overflow")]
    RevisionOverflow,

    /// No previously applied document is available.
    #[error("no previously applied mintd configuration is available to roll back to")]
    NoRollbackConfiguration,

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
    store: Arc<dyn KVStoreCompareAndSwap<Err = cdk_database::Error> + Send + Sync>,
}

impl fmt::Debug for ConfigRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigRepository").finish_non_exhaustive()
    }
}

impl ConfigRepository {
    pub(crate) fn new(
        store: Arc<dyn KVStoreCompareAndSwap<Err = cdk_database::Error> + Send + Sync>,
    ) -> Self {
        Self { store }
    }

    /// Reads the authoritative configuration envelope.
    pub(crate) async fn active(&self) -> Result<ConfigEnvelope, ConfigStoreError> {
        Ok(self.active_record().await?.1)
    }

    async fn active_record(&self) -> Result<(Vec<u8>, ConfigEnvelope), ConfigStoreError> {
        let bytes = self
            .store
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, ACTIVE_KEY)
            .await?
            .ok_or(ConfigStoreError::NotInitialized)?;
        let envelope = ConfigEnvelope::decode(&bytes)?;
        Ok((bytes, envelope))
    }

    /// Creates the authoritative record without replacing an existing one.
    ///
    /// Transition: uninitialized → `Pending`.
    pub(crate) async fn initialize(
        &self,
        envelope: ConfigEnvelope,
    ) -> Result<(), ConfigStoreError> {
        let bytes = envelope.encode()?;
        if !self
            .store
            .kv_compare_and_swap(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVE_KEY,
                None,
                &bytes,
            )
            .await?
        {
            return Err(ConfigStoreError::AlreadyInitialized);
        }
        Ok(())
    }

    /// Atomically replaces the document and marks it for next-start application.
    ///
    /// Transition: `Pending | Applied` → `Pending`. The currently applied
    /// document, when there is one, is retained for rollback.
    #[cfg(test)]
    pub(crate) async fn replace(
        &self,
        toml: String,
        signing_identity: &str,
    ) -> Result<(), ConfigStoreError> {
        self.replace_with_bdk_policy(toml, signing_identity, false)
            .await
    }

    pub(crate) async fn replace_with_bdk_policy(
        &self,
        toml: String,
        signing_identity: &str,
        allow_new_bdk_wallet: bool,
    ) -> Result<(), ConfigStoreError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let (current_bytes, current) = self.active_record().await?;
            if current.signing_identity != signing_identity {
                return Err(ConfigStoreError::SigningIdentityMismatch);
            }
            let revision = current
                .revision
                .checked_add(1)
                .ok_or(ConfigStoreError::RevisionOverflow)?;
            let previous_applied_toml = if current.state() == DocumentState::Applied {
                Some(current.toml)
            } else {
                current.previous_applied_toml
            };
            let replacement = ConfigEnvelope {
                format_version: CONFIG_FORMAT_VERSION,
                revision,
                toml: toml.clone(),
                previous_applied_toml,
                signing_identity: current.signing_identity,
                applied: false,
                allow_new_bdk_wallet,
            }
            .encode()?;
            if self
                .store
                .kv_compare_and_swap(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    ACTIVE_KEY,
                    Some(&current_bytes),
                    &replacement,
                )
                .await?
            {
                return Ok(());
            }
        }
        Err(ConfigStoreError::ConcurrentModification)
    }

    /// Stages the last applied document for activation on the next restart.
    ///
    /// Transition: `Pending | Applied` → `Pending` with the restored
    /// document. Returns `true` because every restored document must be
    /// activated by a restart. Startup may have partially applied a pending
    /// document before failing, so rollback cannot safely mark the restored
    /// document applied.
    pub(crate) async fn rollback(&self) -> Result<bool, ConfigStoreError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let (current_bytes, mut current) = self.active_record().await?;
            let previous = current
                .previous_applied_toml
                .take()
                .ok_or(ConfigStoreError::NoRollbackConfiguration)?;
            current.revision = current
                .revision
                .checked_add(1)
                .ok_or(ConfigStoreError::RevisionOverflow)?;
            if current.state() == DocumentState::Applied {
                current.previous_applied_toml = Some(current.toml);
            }
            current.applied = false;
            current.allow_new_bdk_wallet = false;
            current.toml = previous;
            let replacement = current.encode()?;
            if self
                .store
                .kv_compare_and_swap(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    ACTIVE_KEY,
                    Some(&current_bytes),
                    &replacement,
                )
                .await?
            {
                return Ok(true);
            }
        }
        Err(ConfigStoreError::ConcurrentModification)
    }

    /// Marks `expected_revision` applied if it is still the current revision.
    ///
    /// Transition: `Pending` → `Applied`. Returns `false` when another apply
    /// replaced the document during startup; the replacement then stays
    /// `Pending` for the next restart.
    pub(crate) async fn mark_applied(
        &self,
        expected_revision: u64,
    ) -> Result<bool, ConfigStoreError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let (current_bytes, mut current) = self.active_record().await?;
            if current.revision != expected_revision {
                return Ok(false);
            }
            if current.state() == DocumentState::Applied && !current.allow_new_bdk_wallet {
                return Ok(true);
            }
            current.applied = true;
            current.allow_new_bdk_wallet = false;
            let replacement = current.encode()?;
            if self
                .store
                .kv_compare_and_swap(
                    PRIMARY_NAMESPACE,
                    SECONDARY_NAMESPACE,
                    ACTIVE_KEY,
                    Some(&current_bytes),
                    &replacement,
                )
                .await?
            {
                return Ok(true);
            }
        }
        Err(ConfigStoreError::ConcurrentModification)
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
        assert!(repository
            .store
            .kv_compare_and_swap(
                PRIMARY_NAMESPACE,
                SECONDARY_NAMESPACE,
                ACTIVE_KEY,
                None,
                bytes,
            )
            .await
            .expect("write raw record"));
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
        assert_eq!(active.revision, 2);
    }

    #[tokio::test]
    async fn successful_startup_clears_new_bdk_wallet_permission() {
        let repository = repository().await;
        repository
            .initialize(
                ConfigEnvelope::new("first".to_owned(), "signer".to_owned())
                    .with_new_bdk_wallet_allowed(true),
            )
            .await
            .expect("initialize configuration");
        let pending = repository
            .active()
            .await
            .expect("read pending configuration");
        assert!(pending.allow_new_bdk_wallet);

        assert!(repository
            .mark_applied(pending.revision)
            .await
            .expect("mark configuration applied"));
        let applied = repository
            .active()
            .await
            .expect("read applied configuration");
        assert!(applied.applied);
        assert!(!applied.allow_new_bdk_wallet);
    }

    #[tokio::test]
    async fn older_startup_cannot_mark_replacement_applied() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        let first_revision = repository
            .active()
            .await
            .expect("read first revision")
            .revision;
        repository
            .replace("second".to_owned(), "signer")
            .await
            .expect("replace configuration");
        let second_revision = repository
            .active()
            .await
            .expect("read second revision")
            .revision;

        assert!(!repository
            .mark_applied(first_revision)
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
            .mark_applied(second_revision)
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
            revision: 1,
            toml: "document".to_owned(),
            previous_applied_toml: None,
            signing_identity: "signer".to_owned(),
            applied: false,
            allow_new_bdk_wallet: false,
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
            repository.mark_applied(1).await,
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
        let revision = repository
            .active()
            .await
            .expect("read configuration")
            .revision;

        assert!(repository
            .mark_applied(revision)
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
            .mark_applied(revision)
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

    #[tokio::test]
    async fn rollback_of_pending_document_stages_previous_applied_document() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        assert!(repository
            .mark_applied(1)
            .await
            .expect("mark first document applied"));
        repository
            .replace("second".to_owned(), "signer")
            .await
            .expect("stage second document");

        let pending = repository.active().await.expect("read pending document");
        assert_eq!(pending.previous_applied_toml.as_deref(), Some("first"));
        assert!(repository
            .rollback()
            .await
            .expect("stage previous applied document"));

        let restored = repository.active().await.expect("read restored document");
        assert_eq!(restored.toml, "first");
        assert!(!restored.applied);
        assert_eq!(restored.revision, 3);
        assert!(restored.previous_applied_toml.is_none());
    }

    #[tokio::test]
    async fn rollback_of_applied_document_stages_previous_applied_document() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        assert!(repository
            .mark_applied(1)
            .await
            .expect("mark first document applied"));
        repository
            .replace("second".to_owned(), "signer")
            .await
            .expect("stage second document");
        assert!(repository
            .mark_applied(2)
            .await
            .expect("mark second document applied"));

        assert!(repository
            .rollback()
            .await
            .expect("stage previous applied document"));
        let restored = repository.active().await.expect("read rollback document");
        assert_eq!(restored.toml, "first");
        assert!(!restored.applied);
        assert_eq!(restored.revision, 3);
        assert_eq!(restored.previous_applied_toml.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn rollback_requires_a_previous_applied_document() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");

        assert!(matches!(
            repository.rollback().await,
            Err(ConfigStoreError::NoRollbackConfiguration)
        ));
    }

    #[tokio::test]
    async fn reapplying_same_document_advances_revision_and_stays_pending() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("doc".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        let startup_revision = repository
            .active()
            .await
            .expect("read startup configuration")
            .revision;

        repository
            .replace("doc".to_owned(), "signer")
            .await
            .expect("reapply configuration");

        assert!(!repository
            .mark_applied(startup_revision)
            .await
            .expect("compare revisions"));
        let active = repository.active().await.expect("read replacement");
        assert_eq!(active.toml, "doc");
        assert_eq!(active.revision, startup_revision + 1);
        assert!(!active.applied);
    }

    #[tokio::test]
    async fn concurrent_initialization_has_one_winner() {
        let repository = repository().await;
        let left =
            repository.initialize(ConfigEnvelope::new("left".to_owned(), "signer".to_owned()));
        let right =
            repository.initialize(ConfigEnvelope::new("right".to_owned(), "signer".to_owned()));
        let (left, right) = tokio::join!(left, right);

        assert_ne!(left.is_ok(), right.is_ok());
        let loser = if left.is_err() { left } else { right };
        assert!(matches!(loser, Err(ConfigStoreError::AlreadyInitialized)));
    }

    #[tokio::test]
    async fn concurrent_applies_both_succeed_with_monotonic_revisions() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");

        let left = repository.replace("left".to_owned(), "signer");
        let right = repository.replace("right".to_owned(), "signer");
        let (left, right) = tokio::join!(left, right);
        left.expect("apply left configuration");
        right.expect("apply right configuration");

        let active = repository.active().await.expect("read final configuration");
        assert!(matches!(active.toml.as_str(), "left" | "right"));
        assert_eq!(active.revision, 3);
        assert!(!active.applied);
    }

    #[tokio::test]
    async fn concurrent_apply_and_startup_mark_leave_replacement_pending() {
        let repository = repository().await;
        repository
            .initialize(ConfigEnvelope::new("first".to_owned(), "signer".to_owned()))
            .await
            .expect("initialize configuration");
        let startup_revision = repository
            .active()
            .await
            .expect("read startup configuration")
            .revision;

        let apply = repository.replace("second".to_owned(), "signer");
        let mark = repository.mark_applied(startup_revision);
        let (apply, mark) = tokio::join!(apply, mark);
        apply.expect("apply replacement");
        mark.expect("mark startup revision");

        let active = repository.active().await.expect("read replacement");
        assert_eq!(active.toml, "second");
        assert_eq!(active.revision, startup_revision + 1);
        assert!(!active.applied);
    }

    #[tokio::test]
    async fn legacy_envelope_without_revision_defaults_to_zero() {
        let repository = repository().await;
        write_raw(
            &repository,
            br#"{"format_version":1,"toml":"legacy","signing_identity":"signer","applied":false}"#,
        )
        .await;

        let active = repository.active().await.expect("decode legacy record");
        assert_eq!(active.revision, 0);
        assert!(active.previous_applied_toml.is_none());
        assert!(!active.allow_new_bdk_wallet);
        assert!(repository
            .mark_applied(0)
            .await
            .expect("mark legacy revision"));
    }
}
