use std::cmp::Reverse;
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

    /// Get the active keyset with V2 preference and lowest fees - fetches fresh data from mint
    ///
    /// Forces a fresh fetch of keysets from the mint and returns the active keyset,
    /// preferring V2 (Version01) keysets over V1 (Version00), then selecting by
    /// minimum input fees. Use this when you need the most up-to-date keyset
    /// information for operations.
    #[instrument(skip(self))]
    pub async fn fetch_active_keyset(&self) -> Result<KeySetInfo, Error> {
        self.get_mint_keysets()
            .await?
            .active()
            // Prefer V2 keysets (higher version byte), then lowest fees
            .min_by_key(|k| (Reverse(k.id.get_version().to_byte()), k.input_fee_ppk))
            .cloned()
            .ok_or(Error::NoActiveKeyset)
    }

    /// Get the active keyset with V2 preference and lowest fees from cache
    ///
    /// Returns the active keyset, preferring V2 (Version01) keysets over V1 (Version00),
    /// then selecting by minimum input fees. Uses cached data if available, fetches
    /// from mint if cache not populated.
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
            // Prefer V2 keysets (higher version byte), then lowest fees
            .min_by_key(|k| (Reverse(k.id.get_version().to_byte()), k.input_fee_ppk))
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

#[cfg(test)]
mod tests {
    use std::cmp::Reverse;

    use cdk_common::nuts::nut02::{Id, KeySetInfo};
    use cdk_common::CurrencyUnit;

    /// Create a V1 keyset ID (Version00) with a unique suffix
    fn v1_id(suffix: u8) -> Id {
        Id::from_bytes(&[0x00, suffix, 0, 0, 0, 0, 0, 0]).unwrap()
    }

    /// Create a V2 keyset ID (Version01) with a unique suffix
    fn v2_id(suffix: u8) -> Id {
        let mut bytes = [0u8; 33];
        bytes[0] = 0x01; // V2 version prefix
        bytes[1] = suffix;
        Id::from_bytes(&bytes).unwrap()
    }

    /// Create a KeySetInfo with the given ID and fee
    fn keyset_info(id: Id, input_fee_ppk: u64) -> KeySetInfo {
        KeySetInfo {
            id,
            unit: CurrencyUnit::Sat,
            active: true,
            input_fee_ppk,
            final_expiry: None,
        }
    }

    /// Helper to select the preferred keyset using the same logic as fetch_active_keyset
    fn select_preferred_keyset(keysets: &[KeySetInfo]) -> Option<&KeySetInfo> {
        keysets
            .iter()
            .filter(|k| k.active)
            .min_by_key(|k| (Reverse(k.id.get_version().to_byte()), k.input_fee_ppk))
    }

    #[test]
    fn test_v2_preferred_over_v1_same_fees() {
        let v1 = keyset_info(v1_id(1), 100);
        let v2 = keyset_info(v2_id(1), 100);

        let keysets = vec![v1.clone(), v2.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.id.get_version().to_byte(),
            1,
            "V2 keyset should be preferred when fees are equal"
        );
    }

    #[test]
    fn test_v2_preferred_over_v1_even_with_higher_fees() {
        let v1_low_fee = keyset_info(v1_id(1), 50);
        let v2_high_fee = keyset_info(v2_id(1), 200);

        let keysets = vec![v1_low_fee.clone(), v2_high_fee.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.id.get_version().to_byte(),
            1,
            "V2 keyset should be preferred even when V1 has lower fees"
        );
        assert_eq!(selected.input_fee_ppk, 200);
    }

    #[test]
    fn test_lowest_fee_v2_selected_among_multiple_v2() {
        let v2_high = keyset_info(v2_id(1), 200);
        let v2_low = keyset_info(v2_id(2), 50);
        let v2_mid = keyset_info(v2_id(3), 100);

        let keysets = vec![v2_high.clone(), v2_low.clone(), v2_mid.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.id.get_version().to_byte(),
            1,
            "Should select a V2 keyset"
        );
        assert_eq!(
            selected.input_fee_ppk, 50,
            "Should select V2 keyset with lowest fees"
        );
    }

    #[test]
    fn test_lowest_fee_v1_selected_when_no_v2() {
        let v1_high = keyset_info(v1_id(1), 200);
        let v1_low = keyset_info(v1_id(2), 50);
        let v1_mid = keyset_info(v1_id(3), 100);

        let keysets = vec![v1_high.clone(), v1_low.clone(), v1_mid.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.id.get_version().to_byte(),
            0,
            "Should select a V1 keyset when no V2 available"
        );
        assert_eq!(
            selected.input_fee_ppk, 50,
            "Should select V1 keyset with lowest fees"
        );
    }

    #[test]
    fn test_mixed_keysets_v2_lowest_fee_wins() {
        let v1_low = keyset_info(v1_id(1), 10);
        let v1_high = keyset_info(v1_id(2), 500);
        let v2_mid = keyset_info(v2_id(1), 100);
        let v2_high = keyset_info(v2_id(2), 300);

        let keysets = vec![
            v1_low.clone(),
            v1_high.clone(),
            v2_mid.clone(),
            v2_high.clone(),
        ];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.id.get_version().to_byte(),
            1,
            "Should select a V2 keyset"
        );
        assert_eq!(
            selected.input_fee_ppk, 100,
            "Should select the V2 keyset with lowest fees (100), not V1 with 10"
        );
    }

    #[test]
    fn test_inactive_keysets_ignored() {
        let v2_active = keyset_info(v2_id(1), 200);
        let mut v2_inactive = keyset_info(v2_id(2), 50);
        v2_inactive.active = false;

        let keysets = vec![v2_active.clone(), v2_inactive.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(
            selected.input_fee_ppk, 200,
            "Should select active V2 keyset (200), ignoring inactive one (50)"
        );
    }

    #[test]
    fn test_single_v1_keyset() {
        let v1 = keyset_info(v1_id(1), 100);

        let keysets = vec![v1.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(selected.id.get_version().to_byte(), 0);
        assert_eq!(selected.input_fee_ppk, 100);
    }

    #[test]
    fn test_single_v2_keyset() {
        let v2 = keyset_info(v2_id(1), 100);

        let keysets = vec![v2.clone()];
        let selected = select_preferred_keyset(&keysets).unwrap();

        assert_eq!(selected.id.get_version().to_byte(), 1);
        assert_eq!(selected.input_fee_ppk, 100);
    }

    #[test]
    fn test_empty_keysets_returns_none() {
        let keysets: Vec<KeySetInfo> = vec![];
        let selected = select_preferred_keyset(&keysets);

        assert!(selected.is_none());
    }

    #[test]
    fn test_all_inactive_returns_none() {
        let mut v1 = keyset_info(v1_id(1), 100);
        v1.active = false;
        let mut v2 = keyset_info(v2_id(1), 100);
        v2.active = false;

        let keysets = vec![v1, v2];
        let selected = select_preferred_keyset(&keysets);

        assert!(selected.is_none());
    }
}
