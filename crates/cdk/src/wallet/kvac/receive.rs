//! Send coins

use cdk_common::common::KvacCoinInfo;
use cdk_common::kvac::{KvacCoin, KvacRandomizedCoin};
use cdk_common::{Amount, State};

use crate::{Error, Wallet};

impl Wallet {
    /// Receive KVAC coins into the wallet and return the new balance coin
    pub async fn kvac_receive_coins(&self, coins: Vec<KvacCoin>) -> Result<Vec<KvacCoin>, Error> {
        let mint_url = &self.mint_url;
        let active_keyset_id = self.get_active_mint_kvac_keyset().await?.id;
        let mut localcoins = self.get_unspent_kvac_coins().await?;

        // Filter only those with same keyset as coins
        let allowed_unit = coins
            .first()
            .expect("at least 1 coin to receive")
            .unit
            .clone();
        localcoins = localcoins
            .into_iter()
            .filter(|c| c.unit == allowed_unit)
            .collect::<Vec<KvacCoin>>();

        // Receive amount
        let receive_amount = coins.iter().fold(Amount::ZERO, |acc, c| c.amount + acc);

        // Get the coin encoding the most balance
        localcoins.sort_by(|a, b| b.amount.cmp(&a.amount));
        let balance_coin = localcoins.swap_remove(0);
        let unit_balance = balance_coin.amount.clone();

        // Create inputs
        let mut inputs = coins;
        inputs.push(balance_coin.clone());

        // Get fee
        let fee = self.get_kvac_coins_fee(&inputs).await?;

        // If the received amount is not even enough to cover the fees
        // don't even bother receiving
        if receive_amount <= fee {
            return Err(Error::InsufficientFunds);
        }

        // Create outputs
        // IMPORTANT: THE BALANCE AMOUNT ALWAYS LAST
        let outputs = self
            .create_kvac_outputs(vec![Amount::ZERO, unit_balance + receive_amount - fee])
            .await?;

        // Set selected balance coin as pending
        let balance_coin_nullifier = vec![KvacRandomizedCoin::from(&balance_coin).get_nullifier()];
        self.localstore
            .set_pending_kvac_coins(&balance_coin_nullifier)
            .await?;

        let result = self.kvac_swap(&inputs, &outputs).await;

        match result {
            Err(e) => {
                tracing::error!("Send has failed");
                // Mark coins as spendable
                self.localstore
                    .set_unspent_kvac_coins(&balance_coin_nullifier)
                    .await?;
                Err(e)
            }
            Ok(new_coins) => {
                // Increase keyset counter
                self.localstore
                    .increment_kvac_keyset_counter(&active_keyset_id, outputs.len() as u32)
                    .await?;

                let coins_infos = new_coins
                    .iter()
                    .map(|c| KvacCoinInfo {
                        coin: c.clone(),
                        mint_url: mint_url.clone(),
                        state: State::Unspent,
                    })
                    .collect::<Vec<KvacCoinInfo>>();

                // Store the coin encoding the kept balance
                self.localstore
                    .update_kvac_coins(coins_infos, balance_coin_nullifier)
                    .await?;

                tracing::info!("Send succeeded");
                Ok(new_coins)
            }
        }
    }
}
