use std::collections::HashMap;

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::nut02::{KeySetInfos, KeySetInfosMethods};
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Load keys for mint keyset
    ///
    /// Returns keys from metadata cache if available.
    /// If keys are not cached, fetches from mint server.
    #[instrument(skip(self))]
    pub async fn load_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        self.metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .keys
            .get(&keyset_id)
            .map(|x| (*x.clone()).clone())
            .ok_or(Error::UnknownKeySet)
    }

    /// Alias of get_mint_keysets, kept for backwards compatibility reasons
    #[instrument(skip(self))]
    pub async fn load_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        self.get_mint_keysets().await
    }

    /// Get keysets from metadata cache (may fetch if not populated)
    ///
    /// Checks the metadata cache for keysets. If cache is not populated,
    /// fetches from mint and updates cache. Returns error if no active keysets found.
    #[instrument(skip(self))]
    #[inline(always)]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .keysets
            .iter()
            .filter_map(|(_, keyset)| {
                if keyset.unit == self.unit && keyset.active {
                    Some((*keyset.clone()).clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    /// Refresh keysets by fetching the latest from mint - always fetches fresh data
    ///
    /// Forces a fresh fetch of keyset information from the mint server,
    /// updating the metadata cache and database. Use this when you need
    /// the most up-to-date keyset information.
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<KeySetInfos, Error> {
        tracing::debug!("Refreshing keysets from mint");

        let keysets = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .keysets
            .iter()
            .filter_map(|(_, keyset)| {
                if keyset.unit == self.unit && keyset.active {
                    Some((*keyset.clone()).clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    /// Get the active keyset with the lowest fees - fetches fresh data from mint
    ///
    /// Forces a fresh fetch of keysets from the mint and returns the active keyset
    /// with the minimum input fees. Use this when you need the most up-to-date
    /// keyset information for operations.
    #[instrument(skip(self))]
    pub async fn fetch_active_keyset(&self) -> Result<KeySetInfo, Error> {
        self.get_mint_keysets()
            .await?
            .active()
            .min_by_key(|k| k.input_fee_ppk)
            .cloned()
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get the active keyset with the lowest fees from cache
    ///
    /// Returns the active keyset with minimum input fees from the metadata cache.
    /// Uses cached data if available, fetches from mint if cache not populated.
    #[instrument(skip(self))]
    pub async fn get_active_keyset(&self) -> Result<KeySetInfo, Error> {
        self.metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .active_keysets
            .iter()
            .min_by_key(|k| k.input_fee_ppk)
            .map(|ks| (**ks).clone())
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get keyset fees and amounts for all keysets from metadata cache
    ///
    /// Returns a HashMap of keyset IDs to their input fee rates (per-proof-per-thousand)
    /// and available amounts. Uses cached data if available, fetches from mint if not.
    pub async fn get_keyset_fees_and_amounts(&self) -> Result<KeysetFeeAndAmounts, Error> {
        let metadata = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?;

        let mut fees = HashMap::new();
        for keyset in metadata.keysets.values() {
            let keys = self.load_keyset_keys(keyset.id).await?;
            fees.insert(
                keyset.id,
                (
                    keyset.input_fee_ppk,
                    keys.iter()
                        .map(|(amount, _)| amount.to_u64())
                        .collect::<Vec<_>>(),
                )
                    .into(),
            );
        }

        Ok(fees)
    }

    /// Get keyset fees and amounts for a specific keyset ID
    ///
    /// Returns the input fee rate (per-proof-per-thousand) and available amounts
    /// for a specific keyset. Uses cached data if available, fetches from mint if not.
    pub async fn get_keyset_fees_and_amounts_by_id(
        &self,
        keyset_id: Id,
    ) -> Result<FeeAndAmounts, Error> {
        self.get_keyset_fees_and_amounts()
            .await?
            .get(&keyset_id)
            .cloned()
            .ok_or(Error::UnknownKeySet)
    }
}
