//! Send coins

use cashu_kvac::secp::Scalar;
use cdk_common::{common::KvacCoinInfo, error::Error, kvac::KvacCoin, Amount, State};

use crate::Wallet;

impl Wallet {
    /// Send `send_amount` from the current balance
    pub async fn kvac_send(
        &self,
        send_amount: Amount,
    ) -> Result<KvacCoin, Error> {
        let mint_url = &self.mint_url;
        let active_keyset_id = self.get_active_mint_kvac_keyset().await?.id;

        let coins = self.get_unspent_kvac_coins().await?;

        // Find a coin >= amount
        let mut coin: KvacCoin = coins
            .iter()
            .filter_map(|c| {
                if c.amount >= send_amount {
                    Some(c.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .first()
            .ok_or(Error::InsufficientFunds)?
            .clone();

        // Find a zero-valued coin
        let zero_coin: KvacCoin = coins
            .iter()
            .filter_map(|c| {
                if c.amount == Amount::from(0) {
                    Some(c.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .first()
            .ok_or(Error::NoZeroValueCoins)?
            .clone();

        // Create outputs [balance, 0]
        let inputs = vec![coin.clone(), zero_coin.clone()];
        
        // Get fee
        let fee = self.get_kvac_coins_fee(&inputs).await?;
        if coin.amount < send_amount + fee {
            // Try and look for some other coin >= send_mount + fee
            coin = coins
                .into_iter()
                .filter(|c| c.amount >= send_amount + fee)
                .collect::<Vec<_>>()
                .first()
                .cloned()
                .ok_or(Error::InsufficientFunds)?;
        }
        // Calculate change
        let keep_amount = coin.amount - send_amount - fee;

        // Create outputs
        let outputs = self.create_kvac_outputs(vec![send_amount, keep_amount]).await?;

        // Set selected inputs as pending
        let ts: Vec<Scalar> = inputs[..1].iter().map(|i| i.coin.mac.t.clone()).collect();
        self.localstore.set_pending_kvac_coins(&ts).await?;

        let result = self.kvac_swap(&inputs, &outputs).await;

        match result {
            Err(e) => {
                tracing::error!("Send has failed");
                // Mark coins as spendable
                self.localstore.set_unspent_kvac_coins(&ts).await?;
                Err(e)
            },
            Ok(new_coins) => {
                let sent = new_coins.get(0).expect("always two outputs").clone();
                let kept = new_coins.get(1).expect("always two outputs").clone();

                // Store the coin encoding the kept balance
                self.localstore.update_kvac_coins(
                    vec![KvacCoinInfo{
                        coin: kept,
                        mint_url: mint_url.clone(),
                        state: State::Unspent,
                    }],
                    ts,
                ).await?;

                // Increase keyset counter
                self.localstore.increment_kvac_keyset_counter(&active_keyset_id, outputs.len() as u32).await?;

                tracing::info!("Send succeeded");
                Ok(sent)
            }
        }
        
    }
}