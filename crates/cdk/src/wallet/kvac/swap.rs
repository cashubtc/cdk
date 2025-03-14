//! KVAC Swap request
use std::collections::HashSet;

use cashu_kvac::kvac::{BalanceProof, IParamsProof, MacProof, RangeProof};
use cashu_kvac::models::{AmountAttribute, Coin};
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::Error::DifferentScriptsError;
use cdk_common::kvac::{
    KvacCoin, KvacCoinMessage, KvacPreCoin, KvacRandomizedCoin, KvacSwapRequest,
};
use cdk_common::Amount;
use tracing::instrument;

use crate::{Error, Wallet};

impl Wallet {
    /// Compute the necessary proofs and perform a KVAC swap
    #[instrument(skip(self, inputs, outputs))]
    pub async fn kvac_swap(
        &self,
        inputs: &[KvacCoin],
        outputs: &[KvacPreCoin],
    ) -> Result<Vec<KvacCoin>, Error> {
        let mut proving_transcript = CashuTranscript::new();
        let mut verifying_transcript = CashuTranscript::new();

        // BalanceProof
        let input_attributes: Vec<AmountAttribute> = inputs
            .iter()
            .map(|i| i.coin.amount_attribute.clone())
            .collect();
        let output_attributes: Vec<AmountAttribute> =
            outputs.iter().map(|o| o.attributes.0.clone()).collect();

        /*
        let delta_amount = inputs.iter().fold(0, |acc, i| acc + i.amount.0)
            - outputs.iter().fold(0, |acc, o| acc + o.amount.0);
        */

        // Create balance proof
        let balance_proof = BalanceProof::create(
            &input_attributes,
            &output_attributes,
            &mut proving_transcript,
        );

        let mut mac_proofs = vec![];
        let mut input_randomized_coins = vec![];
        let mut output_coin_messages = vec![];
        let mut scripts_set = HashSet::new();
        for input in inputs.iter() {
            let randomized_coin = KvacRandomizedCoin::from(input);
            let keys = self.get_kvac_keyset_keys(input.keyset_id).await?;
            let proof = MacProof::create(
                &keys.0,
                &input.coin,
                &randomized_coin.randomized_coin,
                &mut proving_transcript,
            );
            mac_proofs.push(proof);
            input_randomized_coins.push(randomized_coin);
            scripts_set.insert(&input.script);
        }
        for output in outputs.iter() {
            let coin_message = KvacCoinMessage::from(output);
            output_coin_messages.push(coin_message);
            scripts_set.insert(&output.script);
        }

        if scripts_set.len() > 1 {
            return Err(Error::from(DifferentScriptsError));
        }

        // Debug: print the state of the transcript
        //let test = proving_transcript.get_challenge(b"test");
        //println!("test challenge: {}", String::from(&test));

        // Create range proof
        let range_proof =
            RangeProof::create_bulletproof(&mut proving_transcript, &output_attributes);

        // Assemble Swap Request
        let request = KvacSwapRequest {
            inputs: input_randomized_coins,
            outputs: output_coin_messages,
            balance_proof,
            mac_proofs,
            range_proof,
            script: inputs.iter().next().unwrap().script.clone(),
        };

        let response = self.client.post_kvac_swap(request).await?;

        // Assemble new coins
        let mut new_coins = vec![];
        for (mac, coin) in response.macs.into_iter().zip(outputs.iter()) {
            let coin = KvacCoin {
                keyset_id: coin.keyset_id,
                amount: coin.amount,
                script: coin.script.clone(),
                unit: coin.unit.clone(),
                coin: Coin::new(
                    coin.attributes.0.clone(),
                    Some(coin.attributes.1.clone()),
                    mac,
                ),
            };
            new_coins.push(coin);
        }

        // Verify each MAC issuance
        for (new_coin, proof) in new_coins.iter().zip(response.proofs.into_iter()) {
            let keys = self.get_kvac_keyset_keys(new_coin.keyset_id).await?;
            if !IParamsProof::verify(&keys.0, &new_coin.coin, proof, &mut verifying_transcript) {
                println!("couldn't verify MAC issuance! the mint is probably tagging!");
                println!(
                    "suspected MAC:\nt = {}\nV = {}",
                    serde_json::to_string(&new_coin.coin.mac.t).unwrap(),
                    serde_json::to_string(&new_coin.coin.mac.V).unwrap()
                );
            }
        }

        Ok(new_coins)
    }

    /// Create outputs with deterministic secrets for KVAC requests
    #[instrument(skip(self, amounts))]
    pub async fn create_kvac_outputs(
        &self,
        amounts: Vec<Amount>,
    ) -> Result<Vec<KvacPreCoin>, Error> {
        let keyset = self.get_active_mint_kvac_keyset().await?;
        let unit = keyset.unit;
        let id = keyset.id;
        let counter = self
            .localstore
            .get_kvac_keyset_counter(&id)
            .await?
            .unwrap_or(0);

        let pre_coins = amounts
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                KvacPreCoin::from_xpriv(id, a, unit.clone(), None, counter + (i as u32), self.xpriv)
                    .map_err(Error::from)
            })
            .collect::<Result<Vec<KvacPreCoin>, Error>>()?;

        Ok(pre_coins)
    }
}
