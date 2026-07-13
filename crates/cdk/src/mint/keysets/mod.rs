#[cfg(feature = "federation")]
use bitcoin::bip32::DerivationPath;
#[cfg(feature = "federation")]
use cdk_common::federation::FederatedKeyset;
use cdk_signatory::signatory::RotateKeyArguments;
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

mod auth;

impl Mint {
    /// Retrieve public keys from the replay-derived federation config, if enabled.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub async fn effective_federation_keyset_pubkeys(
        &self,
        keyset_id: &Id,
    ) -> Result<Option<KeysResponse>, Error> {
        let Some(config) = self.effective_federation_config().await? else {
            return Ok(None);
        };

        config
            .public_config()
            .keysets
            .iter()
            .find(|keyset| &keyset.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|keyset| {
                Some(KeysResponse {
                    keysets: vec![federated_keyset_to_keyset(keyset)],
                })
            })
    }

    /// Retrieve active public keys from the replay-derived federation config, if enabled.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub async fn effective_federation_pubkeys(&self) -> Result<Option<KeysResponse>, Error> {
        let Some(config) = self.effective_federation_config().await? else {
            return Ok(None);
        };

        Ok(Some(KeysResponse {
            keysets: config
                .public_config()
                .keysets
                .iter()
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                .map(federated_keyset_to_keyset)
                .collect(),
        }))
    }

    /// Retrieve all supported keysets from the replay-derived federation config, if enabled.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub async fn effective_federation_keysets(&self) -> Result<Option<KeysetResponse>, Error> {
        let Some(config) = self.effective_federation_config().await? else {
            return Ok(None);
        };

        Ok(Some(KeysetResponse {
            keysets: config
                .public_config()
                .keysets
                .iter()
                .filter(|keyset| keyset.unit != CurrencyUnit::Auth)
                .map(federated_keyset_to_keyset_info)
                .collect(),
        }))
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        #[cfg(feature = "federation")]
        if let Some(config) = self.federation_config() {
            return config
                .public_config()
                .keysets
                .iter()
                .find(|keyset| &keyset.id == keyset_id)
                .ok_or(Error::UnknownKeySet)
                .map(|keyset| KeysResponse {
                    keysets: vec![federated_keyset_to_keyset(keyset)],
                });
        }

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
        #[cfg(feature = "federation")]
        if let Some(config) = self.federation_config() {
            return KeysResponse {
                keysets: config
                    .public_config()
                    .keysets
                    .iter()
                    .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                    .map(federated_keyset_to_keyset)
                    .collect(),
            };
        }

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
        #[cfg(feature = "federation")]
        if let Some(config) = self.federation_config() {
            return KeysetResponse {
                keysets: config
                    .public_config()
                    .keysets
                    .iter()
                    .filter(|keyset| keyset.unit != CurrencyUnit::Auth)
                    .map(federated_keyset_to_keyset_info)
                    .collect(),
            };
        }

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
        #[cfg(feature = "federation")]
        if let Some(config) = self.federation_config() {
            return config
                .public_config()
                .keysets
                .iter()
                .find(|keyset| &keyset.id == id)
                .map(federated_keyset_to_keyset);
        }

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

#[cfg(feature = "federation")]
fn federated_keyset_to_keyset(keyset: &FederatedKeyset) -> KeySet {
    KeySet {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: Some(keyset.active),
        keys: keyset.keys(),
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

#[cfg(feature = "federation")]
fn federated_keyset_to_keyset_info(keyset: &FederatedKeyset) -> KeySetInfo {
    KeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: keyset.active,
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

#[cfg(feature = "federation")]
pub(crate) fn federated_keyset_to_mint_keyset_info(keyset: &FederatedKeyset) -> MintKeySetInfo {
    MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: keyset.active,
        valid_from: 0,
        derivation_path: DerivationPath::master(),
        derivation_path_index: None,
        amounts: keyset
            .aggregate_keys
            .keys()
            .map(|amount| amount.to_u64())
            .collect(),
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
        issuer_version: None,
    }
}
