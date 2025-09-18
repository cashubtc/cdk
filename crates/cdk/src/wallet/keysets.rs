use std::collections::HashMap;

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::nut02::{KeySetInfos, KeySetInfosMethods};
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Load keys for mint keyset
    ///
    /// Returns keys from local database if they are already stored.
    /// If keys are not found locally, goes online to query the mint for the keyset and stores the [`Keys`] in local database.
    #[instrument(skip(self))]
    pub async fn load_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            tracing::debug!(
                "Keyset {} not in db fetching from mint {}",
                keyset_id,
                self.mint_url
            );

            let keys = self.client.get_mint_keyset(keyset_id).await?;

            keys.verify_id()?;

            self.localstore.add_keys(keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    /// Get keysets from local database or go online if missing
    ///
    /// First checks the local database for cached keysets. If keysets are not found locally,
    /// goes online to refresh keysets from the mint and updates the local database.
    /// This is the main method for getting keysets in token operations that can work offline
    /// but will fall back to online if needed.
    #[instrument(skip(self))]
    pub async fn load_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(keysets_info) => Ok(keysets_info),
            None => {
                // If we don't have any keysets, fetch them from the mint
                let keysets = self.refresh_keysets().await?;
                Ok(keysets)
            }
        }
    }

    /// Get keysets from local database only - pure offline operation
    ///
    /// Only checks the local database for cached keysets. If keysets are not found locally,
    /// returns an error without going online. This is used for operations that must remain
    /// offline and rely on previously cached keyset data.
    #[instrument(skip(self))]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(keysets_info) => Ok(keysets_info),
            None => Err(Error::UnknownKeySet),
        }
    }

    /// Refresh keysets by fetching the latest from mint - always goes online
    ///
    /// This method always goes online to fetch the latest keyset information from the mint.
    /// It updates the local database with the fetched keysets and ensures we have keys
    /// for all active keysets. This is used when operations need the most up-to-date
    /// keyset information and are willing to go online.
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<KeySetInfos, Error> {
        tracing::debug!("Refreshing keysets and ensuring we have keys");
        let _ = self.fetch_mint_info().await?;

        // Fetch all current keysets from mint
        let keysets_response = self.client.get_mint_keysets().await?;
        let all_keysets = keysets_response.keysets;

        // Update local storage with keyset info
        self.localstore
            .add_mint_keysets(self.mint_url.clone(), all_keysets.clone())
            .await?;

        // Filter for active keysets matching our unit
        let keysets: KeySetInfos = all_keysets.unit(self.unit.clone()).cloned().collect();

        // Ensure we have keys for all active keysets
        for keyset in &keysets {
            self.load_keyset_keys(keyset.id).await?;
        }

        Ok(keysets)
    }

    /// Get the active keyset with the lowest fees - always goes online
    ///
    /// This method always goes online to refresh keysets from the mint and then returns
    /// the active keyset with the minimum input fees. Use this when you need the most
    /// up-to-date keyset information for operations.
    #[instrument(skip(self))]
    pub async fn fetch_active_keyset(&self) -> Result<KeySetInfo, Error> {
        self.refresh_keysets()
            .await?
            .active()
            .min_by_key(|k| k.input_fee_ppk)
            .cloned()
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get the active keyset with the lowest fees from local database only - offline operation
    ///
    /// Returns the active keyset with minimum input fees from cached keysets in the local database.
    /// This is an offline operation that does not contact the mint. If no keysets are found locally,
    /// returns an error. Use this for offline operations or when you want to avoid network calls.
    #[instrument(skip(self))]
    pub async fn get_active_keyset(&self) -> Result<KeySetInfo, Error> {
        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(keysets_info) => keysets_info
                .into_iter()
                .min_by_key(|k| k.input_fee_ppk)
                .ok_or(Error::NoActiveKeyset),
            None => Err(Error::UnknownKeySet),
        }
    }

    /// Get keyset fees and amounts for mint from local database only - offline operation
    ///
    /// Returns a HashMap of keyset IDs to their input fee rates (per-proof-per-thousand)
    /// from cached keysets in the local database. This is an offline operation that does
    /// not contact the mint. If no keysets are found locally, returns an error.
    pub async fn get_keyset_fees_and_amounts(&self) -> Result<KeysetFeeAndAmounts, Error> {
        let keysets = self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let mut fees = HashMap::new();
        for keyset in keysets {
            fees.insert(
                keyset.id,
                (
                    keyset.input_fee_ppk,
                    self.load_keyset_keys(keyset.id)
                        .await?
                        .iter()
                        .map(|(amount, _)| amount.to_u64())
                        .collect::<Vec<_>>(),
                )
                    .into(),
            );
        }

        Ok(fees)
    }

    /// Get keyset fees and amounts for mint by keyset id from local database only - offline operation
    ///
    /// Returns the input fee rate (per-proof-per-thousand) for a specific keyset ID from
    /// cached keysets in the local database. This is an offline operation that does not
    /// contact the mint. If the keyset is not found locally, returns an error.
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
