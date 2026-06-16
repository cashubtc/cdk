use std::collections::HashMap;

use cdk_common::amount::{FeeAndAmounts, KeysetFeeAndAmounts};
use cdk_common::nut02::KeySetInfosMethods;
pub use cdk_common::wallet::KeysetFilter;
use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys, Proofs, Token};
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
        self.get_mint_keysets(KeysetFilter::Active).await
    }

    /// Get keysets for this wallet's unit from the metadata cache
    ///
    /// Checks the metadata cache for keysets. If cache is not populated,
    /// fetches from mint and updates cache. Use [`KeysetFilter::Active`] for
    /// operations that need current keysets, or [`KeysetFilter::All`] to
    /// include rotated keysets (e.g. for restore).
    #[instrument(skip(self))]
    #[inline(always)]
    pub async fn get_mint_keysets(&self, filter: KeysetFilter) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .keysets
            .values()
            .filter_map(|keyset| {
                if keyset.unit != self.unit {
                    return None;
                }

                if matches!(filter, KeysetFilter::Active) && !keyset.active {
                    return None;
                }

                Some((*keyset.clone()).clone())
            })
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    /// Decode proofs from a token using all known keysets for this mint.
    ///
    /// Tokens may contain proofs from inactive keysets. Inactive keysets no
    /// longer issue new signatures, but mints can still accept their proofs for
    /// redemption.
    #[instrument(skip(self, token))]
    pub(crate) async fn token_proofs(&self, token: &Token) -> Result<Proofs, Error> {
        let keysets = self.get_mint_keysets(KeysetFilter::All).await?;
        Ok(token.proofs(&keysets)?)
    }

    /// Refresh keysets by fetching the latest from mint - always fetches fresh data
    ///
    /// Forces a fresh fetch of keyset information from the mint server,
    /// updating the metadata cache and database. Use this when you need
    /// the most up-to-date keyset information.
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        tracing::debug!("Refreshing keysets from mint");

        let keysets = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .keysets
            .values()
            .filter_map(|keyset| {
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
        self.get_mint_keysets(KeysetFilter::Active)
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

    /// Get the input fee rate for a specific keyset ID
    pub async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Error> {
        Ok(self
            .get_keyset_fees_and_amounts_by_id(keyset_id)
            .await?
            .fee())
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use super::*;
    use crate::nuts::{CurrencyUnit, Token};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, test_mint_url, test_proof, MockMintConnector,
    };

    #[tokio::test]
    async fn token_proofs_decodes_inactive_keyset_proofs() {
        let active_id =
            Id::from_str("01fb5c0e707d1a26e1ea8e8a70f6117beecc22b4797ac3548e802ec7ee477ec627")
                .expect("valid active id");
        let inactive_id =
            Id::from_str("01950154227f1f2b94eb0f14cb460fa7ec35c096457afdf2b4c09fddda15dc0c44")
                .expect("valid inactive id");

        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None)
            .await
            .expect("mint should be stored");
        db.add_mint_keysets(
            mint_url.clone(),
            vec![
                KeySetInfo {
                    id: active_id,
                    unit: CurrencyUnit::Sat,
                    active: true,
                    input_fee_ppk: 100,
                    final_expiry: None,
                },
                KeySetInfo {
                    id: inactive_id,
                    unit: CurrencyUnit::Sat,
                    active: false,
                    input_fee_ppk: 0,
                    final_expiry: None,
                },
            ],
        )
        .await
        .expect("keysets should be stored");

        let wallet = create_test_wallet_with_mock(db, Arc::new(MockMintConnector::new())).await;
        let token = Token::new(
            mint_url,
            vec![test_proof(inactive_id, 1)],
            None,
            CurrencyUnit::Sat,
        );

        let active_keysets = wallet
            .get_mint_keysets(KeysetFilter::Active)
            .await
            .expect("active keysets should load");
        assert!(
            token.proofs(&active_keysets).is_err(),
            "active-only keysets should not decode an inactive keyset proof"
        );

        let proofs = wallet
            .token_proofs(&token)
            .await
            .expect("all keysets should decode an inactive keyset proof");
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].keyset_id, inactive_id);
    }
}
