use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cdk_common::database::{self, MintKeysDatabase};
use cdk_signatory::signatory::RotateKeyArguments;
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

mod auth;

pub(super) struct SharedKeysets {
    database: Arc<dyn MintKeysDatabase<Err = database::Error> + Send + Sync>,
    loaded_epoch: AtomicU64,
}

impl SharedKeysets {
    pub(super) fn new(
        database: Arc<dyn MintKeysDatabase<Err = database::Error> + Send + Sync>,
    ) -> Self {
        Self {
            database,
            loaded_epoch: AtomicU64::new(0),
        }
    }
}

impl Mint {
    /// Check for keyset rotations before serving a request in shared-keystore mode.
    /// A matching database epoch keeps the in-memory keysets and avoids contacting the signatory.
    #[instrument(skip_all)]
    pub async fn refresh_shared_keysets(&self) -> Result<(), Error> {
        let Some(shared) = &self.shared_keysets else {
            return Ok(());
        };

        let epoch = shared.database.keysets_epoch().await?;
        if shared.loaded_epoch.load(Ordering::Acquire) == epoch {
            return Ok(());
        }

        let _store = self.keyset_store_lock.lock().await;
        // Re-read under the lock so a slow observation cannot replace a newer epoch.
        let epoch = shared.database.keysets_epoch().await?;
        if shared.loaded_epoch.load(Ordering::Relaxed) != epoch {
            let snapshot = self.signatory.keysets().await?;
            self.keysets.store(snapshot.keysets.into());
            shared.loaded_epoch.store(epoch, Ordering::Release);
        }
        Ok(())
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.keysets
            .load()
            .iter()
            .find(|keyset| &keyset.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|key| KeysResponse {
                keysets: vec![key.into()],
            })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub fn pubkeys(&self) -> KeysResponse {
        KeysResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                .map(|key| key.into())
                .collect::<Vec<_>>(),
        }
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub fn keysets(&self) -> KeysetResponse {
        KeysetResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|k| k.unit != CurrencyUnit::Auth)
                .map(|k| KeySetInfo {
                    id: k.id,
                    unit: k.unit.clone(),
                    active: k.active,
                    input_fee_ppk: k.input_fee_ppk,
                    final_expiry: k.final_expiry,
                })
                .collect(),
        }
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub fn keyset(&self, id: &Id) -> Option<KeySet> {
        self.keysets
            .load()
            .iter()
            .find(|key| &key.id == id)
            .map(|x| x.into())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: u64,
        use_keyset_v2: bool,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Error> {
        let keyset_id_type = if use_keyset_v2 {
            cdk_common::nut02::KeySetVersion::Version01
        } else {
            cdk_common::nut02::KeySetVersion::Version00
        };
        self.rotate_keyset_by_version(unit, amounts, input_fee_ppk, keyset_id_type, final_expiry)
            .await
    }

    /// Add current keyset to inactive keysets and generate a new keyset
    /// with the specified keyset version.
    #[instrument(skip(self))]
    pub async fn rotate_keyset_by_version(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: u64,
        keyset_id_type: cdk_common::nut02::KeySetVersion,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Error> {
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                amounts,
                input_fee_ppk,
                keyset_id_type,
                final_expiry,
            })
            .await?;

        // Read the freshest snapshot and store it under the shared lock. The
        // subscription drain task writes the same ArcSwap; serializing the
        // read-then-store here keeps the last write the newest one, so a
        // concurrent rotation's snapshot can never be clobbered by a stale read.
        let _store = self.keyset_store_lock.lock().await;
        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.keysets.into());

        Ok(result.into())
    }
}
