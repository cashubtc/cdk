use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1::All;
use cdk_common::database::{self, MintDatabase};
use tracing::instrument;

use super::{
    create_new_keyset, derivation_path_from_unit, CurrencyUnit, Id, KeySet, KeySetInfo,
    KeysResponse, KeysetResponse, Mint, MintKeySet, MintKeySetInfo,
};
use crate::Error;

#[cfg(feature = "auth")]
mod auth;

impl Mint {
    /// Initialize keysets and returns a [`Result`] with a tuple of the following:
    /// * a [`HashMap`] mapping each active keyset `Id` to `MintKeySet`
    /// * a [`Vec`] of `CurrencyUnit` containing active keysets units
    pub async fn init_keysets(
        xpriv: Xpriv,
        secp_ctx: &Secp256k1<All>,
        localstore: &Arc<dyn MintDatabase<Err = database::Error> + Send + Sync>,
        supported_units: &HashMap<CurrencyUnit, (u64, u8)>,
        custom_paths: &HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<(HashMap<Id, MintKeySet>, Vec<CurrencyUnit>), Error> {
        let mut active_keysets: HashMap<Id, MintKeySet> = HashMap::new();
        let mut active_keyset_units: Vec<CurrencyUnit> = vec![];

        // Get keysets info from DB
        let keysets_infos = localstore.get_keyset_infos().await?;

        if !keysets_infos.is_empty() {
            tracing::debug!("Setting all saved keysets to inactive");
            for keyset in keysets_infos.clone() {
                // Set all to in active
                let mut keyset = keyset;
                keyset.active = false;
                localstore.add_keyset_info(keyset).await?;
            }

            let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
                keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
                    acc.entry(ks.unit.clone()).or_default().push(ks.clone());
                    acc
                });

            for (unit, keysets) in keysets_by_unit {
                let mut keysets = keysets;
                keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

                // Get the keyset with the highest counter
                let highest_index_keyset = keysets
                    .first()
                    .cloned()
                    .expect("unit will not be added to hashmap if empty");

                let keysets: Vec<MintKeySetInfo> = keysets
                    .into_iter()
                    .filter(|ks| ks.derivation_path_index.is_some())
                    .collect();

                if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
                    if !keysets.is_empty()
                        && &highest_index_keyset.input_fee_ppk == input_fee_ppk
                        && &highest_index_keyset.max_order == max_order
                    {
                        tracing::debug!("Current highest index keyset matches expect fee and max order. Setting active");
                        let id = highest_index_keyset.id;
                        let keyset = MintKeySet::generate_from_xpriv(
                            secp_ctx,
                            xpriv,
                            highest_index_keyset.max_order,
                            highest_index_keyset.unit.clone(),
                            highest_index_keyset.derivation_path.clone(),
                        );
                        active_keysets.insert(id, keyset);
                        let mut keyset_info = highest_index_keyset;
                        keyset_info.active = true;
                        localstore.add_keyset_info(keyset_info).await?;
                        active_keyset_units.push(unit.clone());
                        localstore.set_active_keyset(unit, id).await?;
                    } else {
                        // Check to see if there are not keysets by this unit
                        let derivation_path_index = if keysets.is_empty() {
                            1
                        } else {
                            highest_index_keyset.derivation_path_index.unwrap_or(0) + 1
                        };

                        let derivation_path = match custom_paths.get(&unit) {
                            Some(path) => path.clone(),
                            None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                                .ok_or(Error::UnsupportedUnit)?,
                        };

                        let (keyset, keyset_info) = create_new_keyset(
                            secp_ctx,
                            xpriv,
                            derivation_path,
                            Some(derivation_path_index),
                            unit.clone(),
                            *max_order,
                            *input_fee_ppk,
                        );

                        let id = keyset_info.id;
                        localstore.add_keyset_info(keyset_info).await?;
                        localstore.set_active_keyset(unit.clone(), id).await?;
                        active_keysets.insert(id, keyset);
                        active_keyset_units.push(unit.clone());
                    };
                }
            }
        }

        Ok((active_keysets, active_keyset_units))
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.ensure_keyset_loaded(keyset_id).await?;
        let keyset = self
            .keysets
            .read()
            .await
            .get(keyset_id)
            .ok_or(Error::UnknownKeySet)?
            .clone();
        Ok(KeysResponse {
            keysets: vec![keyset.into()],
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        let mut active_keysets = self.localstore.get_active_keysets().await?;

        // We don't want to return auth keys here even though in the db we treat them the same
        active_keysets.remove(&CurrencyUnit::Auth);

        let active_keysets: HashSet<&Id> = active_keysets.values().collect();

        for id in active_keysets.iter() {
            self.ensure_keyset_loaded(id).await?;
        }

        Ok(KeysResponse {
            keysets: self
                .keysets
                .read()
                .await
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
        self.ensure_keyset_loaded(id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(id).map(|k| k.clone().into());
        Ok(keyset)
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self, custom_paths))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
        custom_paths: &HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<MintKeySetInfo, Error> {
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
        self.localstore.add_keyset_info(keyset_info.clone()).await?;
        self.localstore.set_active_keyset(unit.clone(), id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        tracing::info!("Rotated to new keyset {} for {}", id, unit);

        Ok(keyset_info)
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
                &self.custom_paths,
            )
            .await?;

        Ok(keyset_info)
    }

    /// Ensure Keyset is loaded in mint
    #[instrument(skip(self))]
    pub async fn ensure_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        {
            let keysets = self.keysets.read().await;
            if keysets.contains_key(id) {
                return Ok(());
            }
        }

        let mut keysets = self.keysets.write().await;
        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;
        let id = keyset_info.id;
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
