//! KVAC mint request
use cashu_kvac::kvac::{BalanceProof, IParamsProof, MacProof, RangeProof};
use cashu_kvac::models::{AmountAttribute, Coin};
use cashu_kvac::secp::GroupElement;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::common::KvacCoinInfo;
use cdk_common::kvac::Error::{NoZeroValueCoins, NotEnoughCoins};
use cdk_common::kvac::{KvacCoin, KvacCoinMessage, KvacMeltBolt11Request, KvacRandomizedCoin};
use cdk_common::util::unix_time;
use cdk_common::{Amount, State};
use tracing::instrument;

use crate::{Error, Wallet};

impl Wallet {
    /// Compute the necessary proofs and perform a KVAC melt
    #[instrument(skip(self))]
    pub async fn kvac_melt(&self, quote_id: &str) -> Result<Vec<KvacCoin>, Error> {
        let mint_url = &self.mint_url;
        let active_keyset_id = self.get_active_mint_kvac_keyset().await?.id;
        let mut coins = self.get_unspent_kvac_coins().await?;

        // Cannot perform melt with less than 2 coins
        if coins.len() < 2 {
            return Err(Error::from(NotEnoughCoins));
        }

        let quote_info = self.localstore.get_melt_quote(quote_id).await?;
        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::ExpiredQuote(quote.expiry, unix_time()));
            }

            quote.clone()
        } else {
            return Err(Error::UnknownQuote);
        };

        // Peg-out amount
        let peg_out = quote_info.amount + quote_info.fee_reserve;

        // Find a coin >= peg_out and move it out
        let index = coins
            .iter()
            .position(|c| c.amount >= peg_out)
            .ok_or(Error::InsufficientFunds)?;

        let mut coin = coins.swap_remove(index);

        // Find a zero-valued coin and move it out
        let index = coins
            .iter()
            .position(|c| c.amount == Amount::from(0))
            .ok_or(Error::from(NoZeroValueCoins))?;

        let zero_coin = coins.swap_remove(index);

        // Select inputs
        let mut inputs = vec![coin.clone(), zero_coin.clone()];

        // Get fee
        let fee = self.get_kvac_coins_fee(&inputs).await?;
        if coin.amount < peg_out + fee {
            // Try and look for some other coin >= peg_out + fee
            let index = coins
                .iter()
                .position(|c| c.amount >= peg_out + fee)
                .ok_or(Error::InsufficientFunds)?;
            coin = coins.swap_remove(index);
            inputs = vec![coin.clone(), zero_coin.clone()]
        }

        // Calculate outputs amount
        let output_amount = coin.amount - peg_out - fee;

        // Create outputs
        // IMPORTANT: THE BALANCE AMOUNT ALWAYS LAST
        // SO THAT THE MINT CAN RETURN OVERSPENT FEES
        let mut outputs = self
            .create_kvac_outputs(vec![Amount::from(0), output_amount])
            .await?;

        let mut proving_transcript = CashuTranscript::new();
        let mut verifying_transcript = CashuTranscript::new();

        // Set selected inputs as pending
        let nullifiers: Vec<GroupElement> = inputs
            .iter()
            .map(|i| KvacRandomizedCoin::from(i).get_nullifier())
            .collect();
        self.localstore.set_pending_kvac_coins(&nullifiers).await?;

        // BalanceProof
        let input_attributes: Vec<AmountAttribute> = inputs
            .iter()
            .map(|i| i.coin.amount_attribute.clone())
            .collect();
        let output_attributes: Vec<AmountAttribute> =
            outputs.iter().map(|o| o.attributes.0.clone()).collect();

        // Create balance proof
        let balance_proof = BalanceProof::create(
            &input_attributes,
            &output_attributes,
            &mut proving_transcript,
        );

        // Compute MAC proofs
        let mut mac_proofs = vec![];
        let mut input_randomized_coins = vec![];
        let mut output_coin_messages = vec![];
        // let mut scripts_set = HashSet::new();
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
            // scripts_set.insert(&input.script);
        }
        for output in outputs.iter() {
            let coin_message = KvacCoinMessage::from(output);
            output_coin_messages.push(coin_message);
            // scripts_set.insert(&output.script);
        }

        // Create range proof
        let range_proof =
            RangeProof::create_bulletproof(&mut proving_transcript, &output_attributes);

        // Assemble Mint Request
        let request = KvacMeltBolt11Request {
            quote: quote_id.to_string(),
            inputs: input_randomized_coins,
            outputs: output_coin_messages,
            balance_proof,
            mac_proofs,
            range_proof,
            script: None,
        };

        let response = self.client.post_kvac_melt(request).await;

        match response {
            Err(e) => {
                tracing::error!("Mint has failed");
                // Mark coins as spendable
                self.localstore.set_unspent_kvac_coins(&nullifiers).await?;
                Err(e)
            }
            Ok(response) => {
                // Check for a fee return
                if response.fee_return > Amount::from(0) {
                    println!("received fee return: {}", response.fee_return);

                    // Apply a tweak to the last output (encoding the balance), adding the fee return
                    outputs.get_mut(1).expect("outputs has length 2").amount += response.fee_return;

                    outputs
                        .get_mut(1)
                        .expect("outputs has length 2")
                        .attributes
                        .0
                        .tweak_amount(response.fee_return.into());
                }

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
                    if !IParamsProof::verify(
                        &keys.0,
                        &new_coin.coin,
                        proof,
                        &mut verifying_transcript,
                    ) {
                        println!("couldn't verify MAC issuance! the mint is probably tagging!");
                        println!(
                            "suspected MAC:\nt = {}\nV = {}",
                            serde_json::to_string(&new_coin.coin.mac.t).unwrap(),
                            serde_json::to_string(&new_coin.coin.mac.V).unwrap()
                        );
                    }
                }

                // Store the coins
                self.localstore
                    .update_kvac_coins(
                        new_coins
                            .iter()
                            .map(|c| KvacCoinInfo {
                                coin: c.clone(),
                                mint_url: mint_url.clone(),
                                state: State::Unspent,
                            })
                            .collect(),
                        nullifiers,
                    )
                    .await?;

                // Increase keyset counter
                self.localstore
                    .increment_kvac_keyset_counter(&active_keyset_id, outputs.len() as u32)
                    .await?;

                tracing::info!("Melt succeeded");
                Ok(new_coins)
            }
        }
    }
}
