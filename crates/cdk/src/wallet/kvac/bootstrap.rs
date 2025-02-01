//! KVAC Bootstrap interactions

use cashu_kvac::kvac::{BootstrapProof, IParamsProof};
use cashu_kvac::models::Coin;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::common::KvacCoinInfo;
use cdk_common::error::Error;
use cdk_common::kvac::{BootstrapRequest, KvacCoin, KvacCoinMessage, KvacPreCoin};
use cdk_common::{Amount, State};
use tracing::instrument;

use crate::Wallet;

impl Wallet {
    /// Request the Mint for a MAC on zero-valued coins
    ///
    /// Use this to obtain initial inputs for further KVAC requests
    #[instrument(skip(self))]
    pub async fn bootstrap(
        &self,
        n: usize,
        script: Option<String>,
    ) -> Result<Vec<KvacCoin>, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let active_keyset_id = self.get_active_mint_kvac_keyset().await?.id;

        let mut pre_coins = vec![];
        let mut coin_messages = vec![];
        let mut bootstrap_proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for _ in 0..n {
            let pre_coin = KvacPreCoin::new(
                active_keyset_id.clone(),
                Amount::from(0),
                self.unit.clone(),
                script.clone(),
            );
            bootstrap_proofs.push(BootstrapProof::create(
                &pre_coin.attributes.0,
                &mut proving_transcript,
            ));
            coin_messages.push(KvacCoinMessage::from(&pre_coin));
            pre_coins.push(pre_coin);
        }

        let request = BootstrapRequest {
            outputs: coin_messages,
            proofs: bootstrap_proofs,
        };

        let response = self.client.post_bootstrap(request).await?;

        // Verify IParams Proofs and construct coins
        let mut coins = vec![];
        let mint_keys = self.get_kvac_keyset_keys(active_keyset_id).await?;
        let mut verifying_transcript = CashuTranscript::new();
        for (i, pre_coin) in pre_coins.into_iter().enumerate() {
            let proof = response.proofs.get(i).ok_or(Error::OutOfBounds)?;
            let mac = response.macs.get(i).ok_or(Error::OutOfBounds)?;
            let inner_coin = Coin::new(
                pre_coin.attributes.0,
                Some(pre_coin.attributes.1),
                mac.clone(),
            );

            if !IParamsProof::verify(
                &mint_keys.0,
                &inner_coin,
                proof.clone(),
                &mut verifying_transcript,
            ) {
                return Err(Error::IParamsVerificationError);
            }

            // Construct coin
            coins.push(KvacCoin {
                keyset_id: active_keyset_id,
                amount: pre_coin.amount,
                script: pre_coin.script,
                unit: pre_coin.unit,
                coin: inner_coin,
            })
        }

        let coins_infos: Vec<KvacCoinInfo> = coins
            .iter()
            .map(|coin| KvacCoinInfo {
                coin: coin.clone(),
                mint_url: self.mint_url.clone(),
                state: State::Unspent,
            })
            .collect::<Vec<KvacCoinInfo>>();

        // Add new proofs to store
        self.localstore
            .update_kvac_coins(coins_infos, vec![])
            .await?;

        Ok(coins)
    }
}
