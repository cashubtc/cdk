use std::collections::{HashMap, HashSet};

use bitcoin::bip32::DerivationPath;
use tracing::instrument;

use super::{
    create_new_keyset, derivation_path_from_unit, CurrencyUnit, Id, KeySet, KeySetInfo,
    KeysResponse, KeysetResponse, Mint, MintKeySet, MintKeySetInfo,
};
use crate::Error;

impl Mint {
    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.ensure_keyset_loaded(keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?.clone();
        Ok(KeysResponse {
            keysets: vec![keyset.into()],
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        let active_keysets = self.localstore.get_active_keysets().await?;

        let active_keysets: HashSet<&Id> = active_keysets.values().collect();

        for id in active_keysets.iter() {
            self.ensure_keyset_loaded(id).await?;
        }

        let keysets = self.keysets.read().await;
        Ok(KeysResponse {
            keysets: keysets
                .values()
                .filter_map(|k| match active_keysets.contains(&k.id) {
                    true => Some(k.clone().into()),
                    false => None,
                })
                .collect(),
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
        self.ensure_keyset_loaded(id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(id).map(|k| k.clone().into());
        Ok(keyset)
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
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<(), Error> {
        let derivation_path = match custom_paths.get(&unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(derivation_path_index),
            unit.clone(),
            max_order,
            input_fee_ppk,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info).await?;
        self.localstore.set_active_keyset(unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        Ok(())
    }

    /// Ensure Keyset is loaded in mint
    #[instrument(skip(self))]
    pub async fn ensure_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        let keysets = self.keysets.read().await;
        if keysets.contains_key(id) {
            return Ok(());
        }
        drop(keysets);

        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;
        let id = keyset_info.id;
        let mut keysets = self.keysets.write().await;
        keysets.insert(id, self.generate_keyset(keyset_info));
        Ok(())
    }

    /// Generate [`MintKeySet`] from [`MintKeySetInfo`]
    #[instrument(skip_all)]
    pub fn generate_keyset(&self, keyset_info: MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            keyset_info.max_order,
            keyset_info.unit,
            keyset_info.derivation_path,
        )
    }
}
