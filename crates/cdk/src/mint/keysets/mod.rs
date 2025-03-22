use std::collections::HashSet;

use cdk_signatory::signatory::RotateKeyArguments;
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

#[cfg(feature = "auth")]
mod auth;

impl Mint {
    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.signatory
            .keysets()
            .await?
            .into_iter()
            .find(|keyset| &keyset.key.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|key| KeysResponse {
                keysets: vec![key.key],
            })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        Ok(KeysResponse {
            keysets: self
                .signatory
                .keysets()
                .await?
                .into_iter()
                .map(|key| key.key)
                .collect::<Vec<_>>(),
        })
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub async fn keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;
        let active_keysets: HashSet<Id> = self
            .localstore
            .get_active_keysets()
            .await?
            .values()
            .cloned()
            .collect();

        let keysets = keysets
            .into_iter()
            .filter(|k| k.unit != CurrencyUnit::Auth)
            .map(|k| KeySetInfo {
                id: k.id,
                unit: k.unit,
                active: active_keysets.contains(&k.id),
                input_fee_ppk: k.input_fee_ppk,
            })
            .collect();

        Ok(KeysetResponse { keysets })
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        Ok(self
            .signatory
            .keysets()
            .await?
            .into_iter()
            .find(|key| &key.key.id == id)
            .map(|x| x.key))
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
    ) -> Result<MintKeySetInfo, Error> {
        self.signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                derivation_path_index,
                max_order,
                input_fee_ppk,
            })
            .await
    }

    /// Rotate to next keyset for unit
    #[instrument(skip(self))]
    pub async fn rotate_next_keyset(
        &self,
        unit: CurrencyUnit,
        max_order: u8,
        input_fee_ppk: u64,
    ) -> Result<MintKeySetInfo, Error> {
        let current_keyset_id = self
            .localstore
            .get_active_keyset_id(&unit)
            .await?
            .ok_or(Error::UnsupportedUnit)?;

        let keyset_info = self
            .localstore
            .get_keyset_info(&current_keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        tracing::debug!(
            "Current active keyset {} path index {:?}",
            keyset_info.id,
            keyset_info.derivation_path_index
        );

        let keyset_info = self
            .rotate_keyset(
                unit,
                keyset_info.derivation_path_index.unwrap_or(1) + 1,
                max_order,
                input_fee_ppk,
            )
            .await?;

        Ok(keyset_info)
    }
}
