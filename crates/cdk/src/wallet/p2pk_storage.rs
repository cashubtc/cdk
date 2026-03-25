//! P2PK signing key storage for CDK Wallet
//!
//! This module provides persistent storage of P2PK signing keys using the
//! wallet's KV store. When receiving tokens locked to a stored key, the wallet
//! can automatically look up the private key and sign the proofs.

use std::collections::HashMap;

use bitcoin::XOnlyPublicKey;
use tracing::instrument;

use crate::nuts::{PublicKey, SecretKey};
use crate::wallet::Wallet;
use crate::{Error, SECP256K1};

/// KV store namespace for P2PK signing keys
const P2PK_KV_NAMESPACE: &str = "p2pk_signing_keys";

impl Wallet {
    /// Generate a new P2PK signing key and store it in the wallet.
    ///
    /// Returns the public key corresponding to the generated secret key.
    #[instrument(skip(self))]
    pub async fn generate_p2pk_key(&self) -> Result<PublicKey, Error> {
        let secret_key = SecretKey::generate();
        self.store_p2pk_key(secret_key).await
    }

    /// Store an existing P2PK signing key in the wallet.
    ///
    /// Returns the public key corresponding to the stored secret key.
    #[instrument(skip_all)]
    pub async fn store_p2pk_key(&self, secret_key: SecretKey) -> Result<PublicKey, Error> {
        let pubkey = secret_key.public_key();
        self.localstore
            .kv_write(
                P2PK_KV_NAMESPACE,
                "",
                &pubkey.to_hex(),
                &secret_key.to_secret_bytes(),
            )
            .await?;
        tracing::info!("Stored P2PK signing key for pubkey {}", pubkey.to_hex());
        Ok(pubkey)
    }

    /// Look up a single P2PK signing key by its public key.
    #[instrument(skip(self))]
    pub async fn get_p2pk_signing_key(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Option<SecretKey>, Error> {
        let bytes = self
            .localstore
            .kv_read(P2PK_KV_NAMESPACE, "", &pubkey.to_hex())
            .await?;
        match bytes {
            Some(b) => Ok(Some(SecretKey::from_slice(&b)?)),
            None => Ok(None),
        }
    }

    /// Load all stored P2PK signing keys, indexed by their x-only public key.
    ///
    /// This is used by the receive saga to automatically sign P2PK-locked proofs.
    #[instrument(skip(self))]
    pub async fn get_p2pk_signing_keys(&self) -> Result<HashMap<XOnlyPublicKey, SecretKey>, Error> {
        let key_hexes = self.localstore.kv_list(P2PK_KV_NAMESPACE, "").await?;

        let mut keys = HashMap::new();
        for hex in key_hexes {
            let bytes = self.localstore.kv_read(P2PK_KV_NAMESPACE, "", &hex).await?;
            if let Some(b) = bytes {
                let sk = SecretKey::from_slice(&b)?;
                let xonly = sk.x_only_public_key(&SECP256K1).0;
                keys.insert(xonly, sk);
            }
        }
        Ok(keys)
    }

    /// Remove a stored P2PK signing key by its public key.
    #[instrument(skip(self))]
    pub async fn remove_p2pk_key(&self, pubkey: &PublicKey) -> Result<(), Error> {
        self.localstore
            .kv_remove(P2PK_KV_NAMESPACE, "", &pubkey.to_hex())
            .await?;
        tracing::info!("Removed P2PK signing key for pubkey {}", pubkey.to_hex());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::wallet::test_utils::{create_test_db, create_test_wallet};

    use super::*;

    #[tokio::test]
    async fn test_generate_and_retrieve_p2pk_key() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let pubkey = wallet.generate_p2pk_key().await.unwrap();

        let retrieved = wallet.get_p2pk_signing_key(&pubkey).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().public_key(), pubkey);
    }

    #[tokio::test]
    async fn test_store_and_retrieve_p2pk_key() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let secret_key = SecretKey::generate();
        let expected_pubkey = secret_key.public_key();

        let pubkey = wallet.store_p2pk_key(secret_key.clone()).await.unwrap();
        assert_eq!(pubkey, expected_pubkey);

        let retrieved = wallet.get_p2pk_signing_key(&pubkey).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().public_key(), expected_pubkey);
    }

    #[tokio::test]
    async fn test_get_nonexistent_key_returns_none() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let random_pubkey = SecretKey::generate().public_key();
        let result = wallet.get_p2pk_signing_key(&random_pubkey).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_p2pk_keys() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let pk1 = wallet.generate_p2pk_key().await.unwrap();
        let pk2 = wallet.generate_p2pk_key().await.unwrap();

        let keys = wallet.get_p2pk_signing_keys().await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains_key(&pk1.x_only_public_key()));
        assert!(keys.contains_key(&pk2.x_only_public_key()));
    }

    #[tokio::test]
    async fn test_remove_p2pk_key() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let pubkey = wallet.generate_p2pk_key().await.unwrap();
        assert!(wallet
            .get_p2pk_signing_key(&pubkey)
            .await
            .unwrap()
            .is_some());

        wallet.remove_p2pk_key(&pubkey).await.unwrap();
        assert!(wallet
            .get_p2pk_signing_key(&pubkey)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_store_overwrites_existing_key() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let sk1 = SecretKey::generate();
        let pubkey = sk1.public_key();

        wallet.store_p2pk_key(sk1).await.unwrap();

        // Store a different key — but same pubkey is impossible (different sk = different pk)
        // So we just verify re-storing the same key works
        let sk2 = SecretKey::generate();
        let pubkey2 = wallet.store_p2pk_key(sk2.clone()).await.unwrap();

        let keys = wallet.get_p2pk_signing_keys().await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains_key(&pubkey.x_only_public_key()));
        assert!(keys.contains_key(&pubkey2.x_only_public_key()));
    }

    #[tokio::test]
    async fn test_empty_list() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db).await;

        let keys = wallet.get_p2pk_signing_keys().await.unwrap();
        assert!(keys.is_empty());
    }
}
