use std::collections::HashMap;

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::nut02::{KeySetInfos, KeySetInfosMethods};
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Load keys for mint keyset
    ///
    /// Returns keys from KeyManager cache if available.
    /// If keys are not cached, triggers a refresh and waits briefly before checking again.
    #[instrument(skip(self))]
    pub async fn load_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        Ok((*self.key_manager.get_keys(&keyset_id).await?).clone())
    }

    /// Get keysets from KeyManager cache or trigger refresh if missing
    ///
    /// First checks the KeyManager cache for keysets. If keysets are not cached,
    /// triggers a refresh from the mint and waits briefly before checking again.
    /// This is the main method for getting keysets in token operations that can work offline
    /// but will fall back to online if needed.
    #[instrument(skip(self))]
    pub async fn load_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        Ok(self
            .key_manager
            .get_keysets()
            .await?
            .into_iter()
            .filter(|x| x.unit == self.unit && x.active)
            .collect::<Vec<_>>())
    }

    /// Get keysets from KeyManager cache only - pure offline operation
    ///
    /// Only checks the KeyManager cache for keysets. If keysets are not cached,
    /// returns an error without going online. This is used for operations that must remain
    /// offline and rely on previously cached keyset data.
    #[instrument(skip(self))]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self
            .key_manager
            .get_keysets()
            .await?
            .into_iter()
            .filter(|k| k.unit == self.unit && k.active)
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    /// Refresh keysets by fetching the latest from mint - always goes online
    ///
    /// This method triggers a KeyManager refresh which fetches the latest keyset
    /// information from the mint. The KeyManager handles updating the cache and database.
    /// This is used when operations need the most up-to-date keyset information.
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<KeySetInfos, Error> {
        tracing::debug!("Refreshing keysets via KeyManager");

        let keysets = self
            .key_manager
            .refresh()
            .await?
            .into_iter()
            .filter(|k| k.unit == self.unit && k.active)
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
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

    /// Get the active keyset with the lowest fees from KeyManager cache - offline operation
    ///
    /// Returns the active keyset with minimum input fees from the KeyManager cache.
    /// This is an offline operation that does not contact the mint. If no keysets are cached,
    /// returns an error. Use this for offline operations or when you want to avoid network calls.
    #[instrument(skip(self))]
    pub async fn get_active_keyset(&self) -> Result<KeySetInfo, Error> {
        let active_keysets = self.key_manager.get_active_keysets().await?;

        active_keysets
            .into_iter()
            .min_by_key(|k| k.input_fee_ppk)
            .map(|ks| (*ks).clone())
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get keyset fees and amounts for mint from KeyManager cache - offline operation
    ///
    /// Returns a HashMap of keyset IDs to their input fee rates (per-proof-per-thousand)
    /// from the KeyManager cache. This is an offline operation that does not contact the mint.
    /// If no keysets are cached, returns an error.
    pub async fn get_keyset_fees_and_amounts(&self) -> Result<KeysetFeeAndAmounts, Error> {
        let keysets = self.key_manager.get_keysets().await?;

        let mut fees = HashMap::new();
        for keyset in keysets {
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
