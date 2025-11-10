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
        max_order: u8,
        input_fee_ppk: u64,
    ) -> Result<MintKeySetInfo, Error> {
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                amounts: (0..max_order).map(|n| 2u64.pow(n.into())).collect(),
                input_fee_ppk,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.keysets.into());

        Ok(result.into())
    }
}
