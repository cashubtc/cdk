//! Key-Value Store Database traits and utilities
//!
//! This module provides shared KVStore functionality that can be used by both
//! mint and wallet database implementations.

use async_trait::async_trait;

use super::{DbTransactionFinalizer, Error};

/// Valid ASCII characters for namespace and key strings in KV store
pub const KVSTORE_NAMESPACE_KEY_ALPHABET: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// Maximum length for namespace and key strings in KV store
pub const KVSTORE_NAMESPACE_KEY_MAX_LEN: usize = 120;

/// Validates that a string contains only valid KV store characters and is within length limits
pub fn validate_kvstore_string(s: &str) -> Result<(), Error> {
    if s.len() > KVSTORE_NAMESPACE_KEY_MAX_LEN {
        return Err(Error::KVStoreInvalidKey(format!(
            "{KVSTORE_NAMESPACE_KEY_MAX_LEN} exceeds maximum length of key characters"
        )));
    }

    if !s
        .chars()
        .all(|c| KVSTORE_NAMESPACE_KEY_ALPHABET.contains(c))
    {
        return Err(Error::KVStoreInvalidKey("key contains invalid characters. Only ASCII letters, numbers, underscore, and hyphen are allowed".to_string()));
    }

    Ok(())
}

/// Validates namespace and key parameters for KV store operations
pub fn validate_kvstore_params(
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<(), Error> {
    // Validate primary namespace
    validate_kvstore_string(primary_namespace)?;

    // Validate secondary namespace
    validate_kvstore_string(secondary_namespace)?;

    // Validate key
    validate_kvstore_string(key)?;

    // Check empty namespace rules
    if primary_namespace.is_empty() && !secondary_namespace.is_empty() {
        return Err(Error::KVStoreInvalidKey(
            "If primary_namespace is empty, secondary_namespace must also be empty".to_string(),
        ));
    }

    // Check for potential collisions between keys and namespaces in the same namespace
    let namespace_key = format!("{primary_namespace}/{secondary_namespace}");
    if key == primary_namespace || key == secondary_namespace || key == namespace_key {
        return Err(Error::KVStoreInvalidKey(format!(
            "Key '{key}' conflicts with namespace names"
        )));
    }

    Ok(())
}

/// Key-Value Store Transaction trait
#[async_trait]
pub trait KVStoreTransaction<Error>: DbTransactionFinalizer<Err = Error> {
    /// Read value from key-value store
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error>;

    /// Write value to key-value store
    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error>;

    /// Remove value from key-value store
    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error>;

    /// List keys in a namespace
    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error>;
}

/// Key-Value Store Database trait
#[async_trait]
pub trait KVStoreDatabase {
    /// KV Store Database Error
    type Err: Into<Error> + From<Error>;

    /// Read value from key-value store
    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err>;

    /// List keys in a namespace
    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err>;
}

/// Key-Value Store trait combining read operations with transaction support
#[async_trait]
pub trait KVStore: KVStoreDatabase {
    /// Begins a KV transaction
    async fn begin_transaction(
        &self,
    ) -> Result<Box<dyn KVStoreTransaction<Self::Err> + Send + Sync>, Error>;
}
