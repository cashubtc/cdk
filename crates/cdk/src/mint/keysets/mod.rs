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
        let keysets = self
            .signatory
            .keysets()
            .await?
            .into_iter()
            .filter(|k| k.key.unit != CurrencyUnit::Auth)
            .map(|k| KeySetInfo {
                id: k.key.id,
                unit: k.key.unit,
                active: k.info.active,
                input_fee_ppk: k.info.input_fee_ppk,
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
                derivation_path_index: Some(derivation_path_index),
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
        self.signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                max_order,
                derivation_path_index: None,
                input_fee_ppk,
            })
            .await
    }
}
