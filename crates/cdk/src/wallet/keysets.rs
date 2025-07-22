use std::collections::HashMap;

use cdk_common::nut02::{KeySetInfos, KeySetInfosMethods};
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Fetch keys for mint keyset
    ///
    /// Selected keys from localstore if they are already known
    /// If they are not known queries mint for keyset id and stores the [`Keys`]
    #[instrument(skip(self))]
    pub async fn fetch_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self.client.get_mint_keyset(keyset_id).await?;

            keys.verify_id()?;

            self.localstore.add_keys(keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    /// Get keysets from local storage or fetch if missing
    ///
    /// Checks the database for keysets and queries the Mint if not found.
    /// This is the main method for getting keysets for token operations.
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

    /// Get keysets from local storage or error if missing
    ///
    /// Checks the database for keysets if unknown we error wallet must go online.
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

    /// Refresh keysets and ensure we have keys for active ones
    ///
    /// This is the main method for operations that need up-to-date keyset info.
    /// - Fetches current keysets from mint
    /// - Updates local storage  
    /// - Ensures we have keys for all active keysets
    /// - Returns filtered active keysets for this unit
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<KeySetInfos, Error> {
        tracing::debug!("Refreshing keysets and ensuring we have keys");
        let _ = self.get_mint_info().await?;

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
            if self.localstore.get_keys(&keyset.id).await?.is_none() {
                tracing::debug!("Fetching missing keys for keyset {}", keyset.id);
                self.fetch_keyset_keys(keyset.id).await?;
            }
        }

        Ok(keysets)
    }

    /// Fetch the active keyset with the lowest fees
    ///
    /// Refreshes keysets and returns the one with minimum input fees
    #[instrument(skip(self))]
    pub async fn fetch_active_keyset(&self) -> Result<KeySetInfo, Error> {
        self.refresh_keysets()
            .await?
            .active()
            .min_by_key(|k| k.input_fee_ppk)
            .cloned()
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get the active keyset with the lowest fees
    ///
    /// Refreshes keysets and returns the one with minimum input fees
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

    /// Get keyset fees for mint
    pub async fn get_keyset_fees(&self) -> Result<HashMap<Id, u64>, Error> {
        let keysets = self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let mut fees = HashMap::new();
        for keyset in keysets {
            fees.insert(keyset.id, keyset.input_fee_ppk);
        }

        Ok(fees)
    }

    /// Get keyset fees for mint by keyset id
    pub async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Error> {
        self.get_keyset_fees()
            .await?
            .get(&keyset_id)
            .cloned()
            .ok_or(Error::UnknownKeySet)
    }
}
