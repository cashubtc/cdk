//! Restore coins from seed
/*
use crate::{Error, Wallet};
use cashu_kvac::{recovery::recover_amounts, secp::Scalar};
use cdk_common::{
    kvac::{KvacPreCoin, KvacRestoreRequest},
    Amount,
};

impl Wallet {
    
    /// Restores the one-coin balance for each keyset of the wallet's Mint
    pub async fn kvac_restore(&self, expected_maximum_amount: u64) -> Result<(), Error> {
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
        let mut recovered_commitments = vec![];

        tracing::info!("Start Restore");
        for keyset in keysets {
            let mut batch = 0;
            let mut last_commitment;
            let mut last_blinding_factor;
            tracing::info!("Checking keyset {}", keyset.id);

            loop {
                let tags = (0..100).map(|i| KvacPreCoin::from_xpriv(
                        keyset.id,
                        Amount::from(0),
                        keyset.unit.clone(),
                        None,
                        batch+i,
                        self.xpriv.clone(),
                    )
                    .expect("RNG busted")
                    .t_tag
                )
                .collect::<Vec<Scalar>>();

                let request = KvacRestoreRequest {tags};

                let response = self.client.post_kvac_restore(request.clone()).await?;

                // response was empty
                if response.issued_macs.len() == 0 {
                    break;
                }

                (last_commitments, last_blinding_factor) = match response.issued_macs.iter().last() {
                    Some(c) => (c.commitments, requ
                    None => last_commitment,
                };

                self.localstore
                    .increment_keyset_counter(&keyset.id, response.issued_macs.len() as u32)
                    .await?;

                batch += 100;
            }

            // The last recovered commitment will encode the balance
            // also save the blinding factor for that commitment
            recovered_commitments.push((
                response.issued_macs.iter().last().unwrap().clone(),
                request.
            ),

        }

        // Now recover the amounts
        let amount = recover_amounts(
            recovered_commitments.iter().map(|c| c.commitments.0.clone()),
            , hypothesized_max_amount)

        Ok(())
    }
    
}
*/