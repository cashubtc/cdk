use std::collections::HashMap;
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Get keys for mint keyset
    ///
    /// Selected keys from localstore if they are already known
    /// If they are not known queries mint for keyset id and stores the [`Keys`]
    #[instrument(skip(self))]
    pub async fn get_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
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

    /// Get keysets from DB or fetch them
    ///
    /// Checks the database for keysets and queries the Mint if
    /// it can't find any.
    #[instrument(skip(self))]
    pub async fn load_mint_keysets(&self)-> Result<Vec<KeySetInfo>, Error> {
        match self.localstore.get_mint_keysets(self.mint_url.clone()).await? {
            Some(keysets_info) => Ok(keysets_info),
            None => self.get_mint_keysets().await // Hit the keysets endpoint if we don't have the keysets for this Mint
        }
    }

    /// Get keysets for wallet's mint
    ///
    /// Queries mint for all keysets
    #[instrument(skip(self))]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets().await?;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.keysets.clone())
            .await?;

        Ok(keysets.keysets)
    }

    /// Get active keyset for mint
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets().await?;
        let keysets = keysets.keysets;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.clone())
            .await?;

        let active_keysets = keysets
            .clone()
            .into_iter()
            .filter(|k| k.active && k.unit == self.unit)
            .collect::<Vec<KeySetInfo>>();

        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(known_keysets) => {
                let unknown_keysets: Vec<&KeySetInfo> = keysets
                    .iter()
                    .filter(|k| known_keysets.contains(k))
                    .collect();

                for keyset in unknown_keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
            None => {
                for keyset in keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
        }

        Ok(active_keysets)
    }

    /// Get active keyset for mint with the lowest fees
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_keyset(&self) -> Result<KeySetInfo, Error> {
        // Important
        let _ = self.get_mint_info().await?;
        let active_keysets = self.get_active_mint_keysets().await?;

        let keyset_with_lowest_fee = active_keysets
            .into_iter()
            .min_by_key(|key| key.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)?;
        Ok(keyset_with_lowest_fee)
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
