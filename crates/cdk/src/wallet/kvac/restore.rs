//! Restore coins from seed

use std::collections::HashMap;

use crate::{Error, Wallet};
use cashu_kvac::{
    models::{AmountAttribute, Coin},
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

                // Generate the pre-coins for this batch
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
            
                //println!("restore pre_coins: {}", serde_json::to_string(&pre_coins).unwrap());

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

                let issued_macs_map: HashMap<Scalar, KvacIssuedMac> = response
                    .issued_macs
                    .into_iter()
                    .map(|issued| (issued.mac.t.clone(), issued))
                    .collect();

                // Filter the [`KvacPreCoin`]s and get only the ones that were issued a [`MAC`]
                let coins: Vec<(KvacPreCoin, KvacIssuedMac)> = pre_coins
                    .into_iter()
                    .filter(|p| issued_macs_map.contains_key(&p.t_tag))
                    .map(|p| (p.clone(), issued_macs_map.get(&p.t_tag).expect("issued macs contains the key").clone()))
                    .collect();

                // Extract amount commitments
                let amount_commitments = coins
                    .iter()
                    .map(|(_, issued_mac)| issued_mac.commitments.0.clone())
                    .collect::<Vec<GroupElement>>();

                // println!("amount_commitments: {:?}", amount_commitments);

                // Exctract blinding factors
                let blinding_factors = coins
                    .iter()
                    .map(|(pre_coin, _)| pre_coin.attributes.0.r.clone())
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
                    .zip(coins.into_iter())
                    .filter(|(amount, (pre_coin, _))| {
                        if amount.is_some() {
                            true
                        } else {
                            tracing::error!(
                                "Amount was not found for KvacPreCoin with tag: {:?}",
                                pre_coin.t_tag
                            );
                            false
                        }
                    })
                    .collect();

                // Construct coins
                let coins: Vec<KvacCoin> = filtered
                    .into_iter()
                    .map(|(amount, (pre_coin, issued_macs))| {
                            let bytes_blinding_factor = pre_coin.attributes.0.r.to_bytes();
                            let amount = amount.expect("amount is not None");
                            KvacCoin {
                                keyset_id: keyset.id,
                                amount: Amount::from(amount),
                                script: None,   // TODO: FIX THIS ONCE SCRIPTS ARE USED/AVAILABLE
                                unit: keyset.unit.clone(),
                                coin: Coin::new(
                                    AmountAttribute::new(amount, Some(&bytes_blinding_factor)),
                                    Some(pre_coin.attributes.1),
                                    issued_macs.mac,
                                ),
                            }
                        }
                    )
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

                println!("unspent_coins: {}", serde_json::to_string_pretty(&unspent_coins).unwrap());

                // Fold the amount in each coin and calculate a total for this keyset
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
