//! Restore coins from seed

use std::collections::HashMap;

use crate::{Error, Wallet};
use cashu_kvac::{
    models::Coin,
    recovery::recover_amounts,
    secp::{GroupElement, Scalar},
};
use cdk_common::{
    common::KvacCoinInfo,
    kvac::{KvacCoin, KvacIssuedMac, KvacPreCoin, KvacRestoreRequest},
    Amount, Id, State,
};

impl Wallet {
    /// Restores coins for each keyset of the Mint
    /// and returns a [`HashMap`] mapping [`Id`]s to [`Amount`]s recovered
    pub async fn kvac_restore(
        &self,
        expected_maximum_amount: u64,
    ) -> Result<HashMap<Id, Amount>, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let keysets = self.get_mint_keysets().await?;
        let mut keyset_recovered_map: HashMap<Id, Amount> = HashMap::new();

        tracing::info!("Start Restore");
        for keyset in keysets {
            tracing::info!("Checking keyset {}", keyset.id);
            //let keys = self.get_keyset_keys(keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;

            while empty_batch.lt(&3) {
                let pre_coins = (start_counter..start_counter + 100)
                    .map(|counter| {
                        KvacPreCoin::from_xpriv(
                            keyset.id,
                            Amount::ZERO,
                            keyset.unit.clone(),
                            None,
                            counter,
                            self.xpriv,
                        )
                        .expect("RNG busted")
                    })
                    .collect::<Vec<KvacPreCoin>>();

                tracing::debug!(
                    "Attempting to restore counter {}-{} for mint {} keyset {}",
                    start_counter,
                    start_counter + 100,
                    self.mint_url,
                    keyset.id
                );

                let restore_request = KvacRestoreRequest {
                    tags: pre_coins.iter().map(|p| p.t_tag.clone()).collect(),
                };

                let response = self.client.post_kvac_restore(restore_request).await?;

                if response.issued_macs.is_empty() {
                    empty_batch += 1;
                    start_counter += 100;
                    continue;
                }

                // Get the tags from the response
                let pre_coins_tags: Vec<Scalar> =
                    pre_coins.iter().map(|i| i.t_tag.clone()).collect();
                let issued_tags: Vec<Scalar> = response
                    .issued_macs
                    .iter()
                    .map(|i| i.mac.t.clone())
                    .collect();

                // Filter the [`KvacPreCoin`]s and get only the ones that were issued a [`MAC`]
                let issued_macs: Vec<KvacIssuedMac> = response
                    .issued_macs
                    .into_iter()
                    .filter(|p| pre_coins_tags.contains(&p.mac.t))
                    .collect();

                // Filter the [`KvacPreCoin`]s and get only the ones that were issued a [`MAC`]
                let pre_coins: Vec<KvacPreCoin> = pre_coins
                    .into_iter()
                    .filter(|p| issued_tags.contains(&p.t_tag))
                    .collect();

                // Extract amount commitments
                let amount_commitments = pre_coins
                    .iter()
                    .map(|p| p.attributes.0.commitment())
                    .collect::<Vec<GroupElement>>();

                // Exctract blinding factors
                let blinding_factors = pre_coins
                    .iter()
                    .map(|p| p.attributes.0.r.clone())
                    .collect::<Vec<Scalar>>();

                // Recover the amounts
                let amounts = recover_amounts(
                    &amount_commitments,
                    &blinding_factors,
                    expected_maximum_amount,
                );

                // Filter out any [`KvacPreCoin`] for which amount wasn't found
                let filtered: Vec<(Option<u64>, (KvacPreCoin, KvacIssuedMac))> = amounts
                    .into_iter()
                    .zip(pre_coins.into_iter().zip(issued_macs))
                    .filter(|(amount, k)| {
                        if amount.is_some() {
                            true
                        } else {
                            tracing::error!(
                                "Amount was not found for KvacPreCoin with tag: {:?}",
                                k.0.t_tag
                            );
                            false
                        }
                    })
                    .collect();

                // Construct coins
                let coins: Vec<KvacCoin> = filtered
                    .into_iter()
                    .map(|(amount, (pre_coin, issued_macs))| KvacCoin {
                        keyset_id: keyset.id,
                        amount: Amount::from(amount.expect("amount is not None")),
                        script: None,
                        unit: keyset.unit.clone(),
                        coin: Coin::new(
                            pre_coin.attributes.0,
                            Some(pre_coin.attributes.1),
                            issued_macs.mac,
                        ),
                    })
                    .collect();

                tracing::debug!("Restored {} coins", coins.len());

                self.localstore
                    .increment_kvac_keyset_counter(&keyset.id, coins.len() as u32)
                    .await?;

                let states = self.check_coins_spent(coins.clone()).await?;

                // Get the unspent ones
                let unspent_coins: Vec<KvacCoin> = coins
                    .iter()
                    .zip(states)
                    .filter(|(_, state)| !state.state.eq(&State::Spent))
                    .map(|(p, _)| p)
                    .cloned()
                    .collect();

                let restored_value = unspent_coins
                    .iter()
                    .fold(Amount::ZERO, |acc, c| acc + c.amount);

                tracing::debug!(
                    "Recovered value for keyset {}: {}",
                    keyset.id,
                    restored_value.0
                );
                keyset_recovered_map.insert(keyset.id, restored_value);

                // Add metadata for DB insertion
                let unspent_coins = unspent_coins
                    .into_iter()
                    .map(|coin| KvacCoinInfo {
                        coin,
                        mint_url: self.mint_url.clone(),
                        state: State::Unspent,
                    })
                    .collect::<Vec<KvacCoinInfo>>();

                // Insert into DB
                self.localstore
                    .update_kvac_coins(unspent_coins, vec![])
                    .await?;

                // Next batch
                empty_batch = 0;
                start_counter += 100;
            }
        }

        Ok(keyset_recovered_map)
    }
}
