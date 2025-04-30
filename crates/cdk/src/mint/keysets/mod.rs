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
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.keysets
            .load()
            .iter()
            .find(|keyset| &keyset.key.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|key| KeysResponse {
                keysets: vec![key.key.clone()],
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
                .filter(|keyset| keyset.info.active && keyset.info.unit != CurrencyUnit::Auth)
                .map(|key| key.key.clone())
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
                .filter(|k| k.key.unit != CurrencyUnit::Auth)
                .map(|k| KeySetInfo {
                    id: k.key.id,
                    unit: k.key.unit.clone(),
                    active: k.info.active,
                    input_fee_ppk: k.info.input_fee_ppk,
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
            .find(|key| &key.key.id == id)
            .map(|x| x.key.clone())
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
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                derivation_path_index: Some(derivation_path_index),
                max_order,
                input_fee_ppk,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.into());

        Ok(result)
    }

    /// Rotate to next keyset for unit
    #[instrument(skip(self))]
    pub async fn rotate_next_keyset(
        &self,
        unit: CurrencyUnit,
        max_order: u8,
        input_fee_ppk: u64,
    ) -> Result<MintKeySetInfo, Error> {
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                max_order,
                derivation_path_index: None,
                input_fee_ppk,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.into());

        Ok(result)
    }
}
