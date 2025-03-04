use cashu_kvac::secp::GroupElement;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::{
    KvacIssuedMac, KvacMintBolt11Request, KvacMintBolt11Response, KvacNullifier,
};
use cdk_common::{MintQuoteState, State};
use tracing::instrument;
use uuid::Uuid;

use crate::{Error, Mint};

impl Mint {
    /// Process mint request
    #[instrument(skip_all)]
    pub async fn process_kvac_mint_request(
        &self,
        mint_request: KvacMintBolt11Request<Uuid>,
    ) -> Result<KvacMintBolt11Response, Error> {
        tracing::info!("KVAC mint has been called");

        let mint_quote =
            if let Some(mint_quote) = self.localstore.get_mint_quote(&mint_request.quote).await? {
                mint_quote
            } else {
                return Err(Error::UnknownQuote);
            };

        let state = self
            .localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Pending)
            .await?;

        match state {
            MintQuoteState::Unpaid => {
                self.localstore
                    .update_mint_quote_state(&mint_request.quote, state)
                    .await?;
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MintQuoteState::Issued => {
                self.localstore
                    .update_mint_quote_state(&mint_request.quote, state)
                    .await?;
                return Err(Error::IssuedQuote);
            }
            MintQuoteState::Paid => (),
        };

        // TODO: mint quote signature verification
        /*
        // If the there is a public key provoided in mint quote request
        // verify the signature is provided for the mint request
        if let Some(pubkey) = mint_quote.pubkey {
            mint_request.verify_signature(pubkey)?;
        }
        */

        // Peg-in should be negative, so we take the quote amount and negate it.
        let peg_in = -i64::try_from(mint_quote.amount)?;

        // Process the request
        if let Err(e) = self
            .verify_kvac_request(
                false,
                peg_in,
                &mint_request.inputs,
                &mint_request.outputs,
                mint_request.balance_proof,
                mint_request.mac_proofs,
                mint_request.script,
                mint_request.range_proof,
            )
            .await
        {
            tracing::error!("KVAC verification failed");
            self.localstore
                .update_mint_quote_state(&mint_request.quote, state)
                .await?;
            return Err(e);
        }

        // Extract the nullifiers. Equivalent to getting the `Y` of normal ecash.
        let nullifiers = mint_request
            .inputs
            .iter()
            .map(|i| KvacNullifier::from(i).nullifier)
            .collect::<Vec<GroupElement>>();

        // Issue MACs
        let mut issued_macs = vec![];
        let mut iparams_proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for output in mint_request.outputs.iter() {
            let result = self.issue_mac(output, &mut proving_transcript).await;
            // Set nullifiers unspent in case of error
            match result {
                Err(e) => {
                    tracing::error!("Failure to issue MACs");
                    self.localstore
                        .update_kvac_nullifiers_states(&nullifiers, State::Unspent)
                        .await?;
                    return Err(e);
                }
                Ok((mac, proof)) => {
                    issued_macs.push(KvacIssuedMac {
                        commitments: output.commitments.clone(),
                        mac,
                        keyset_id: output.keyset_id,
                        quote_id: None,
                    });
                    iparams_proofs.push(proof);
                }
            }
        }

        // Add issued macs
        self.localstore
            .add_kvac_issued_macs(&issued_macs, None)
            .await?;

        // Set nullifiers as spent
        self.localstore
            .update_kvac_nullifiers_states(&nullifiers, State::Spent)
            .await?;

        // Update mint quote state to issued
        self.localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Issued)
            .await?;

        tracing::debug!("KVAC mint request successful");

        Ok(KvacMintBolt11Response {
            outputs: mint_request.outputs,
            macs: issued_macs.into_iter().map(|m| m.mac).collect(),
            proofs: iparams_proofs,
        })
    }
}
