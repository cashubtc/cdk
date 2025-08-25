use std::collections::HashMap;

use cdk_common::nut26::MintQuoteOnchainRequest;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::{Proofs, SecretKey};
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, MintQuoteOnchainResponse, MintRequest, PaymentMethod, PreMintSecrets, PublicKey,
    SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuote;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Onchain Quote
    #[instrument(skip(self))]
    pub async fn mint_onchain_quote(&self, pubkey: PublicKey) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = &self.unit;

        self.refresh_keysets().await?;

        let secret_key = SecretKey::generate();

        let mint_request = MintQuoteOnchainRequest {
            unit: self.unit.clone(),
            pubkey,
        };

        let quote_res = self.client.post_mint_onchain_quote(mint_request).await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Onchain,
            None, // Onchain quotes don't have a predefined amount
            unit.clone(),
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint onchain
    #[instrument(skip(self))]
    pub async fn mint_onchain(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.refresh_keysets().await?;

        let quote_info = self.localstore.get_mint_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) && quote.expiry.ne(&0) {
                tracing::info!("Minting after expiry");
            }

            quote.clone()
        } else {
            return Err(Error::UnknownQuote);
        };

        let active_keyset_id = self.fetch_active_keyset().await?.id;

        let amount = match amount {
            Some(amount) => amount,
            None => {
                // If an amount is not supplied, check the status of the quote
                // The mint will tell us how much can be minted
                let state = self.mint_onchain_quote_state(quote_id).await?;

                state.amount_paid - state.amount_issued
            }
        };

        if amount == Amount::ZERO {
            tracing::error!("Cannot mint zero amount.");
            return Err(Error::InvoiceAmountUndefined);
        }

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                amount,
                &amount_split_target,
                spending_conditions,
            )?,
            None => {
                // Calculate how many secrets we'll need without generating them
                let amount_split = amount.split_targeted(&amount_split_target)?;
                let num_secrets = amount_split.len() as u32;

                tracing::debug!(
                    "Incrementing keyset {} counter by {}",
                    active_keyset_id,
                    num_secrets
                );

                // Atomically get the counter range we need
                let new_counter = self
                    .localstore
                    .increment_keyset_counter(&active_keyset_id, num_secrets)
                    .await?;

                let count = new_counter - num_secrets;

                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    amount,
                    &amount_split_target,
                )?
            }
        };

        let mut request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = quote_info.secret_key.clone() {
            request.sign(secret_key)?;
        } else {
            tracing::error!("Signature is required for onchain.");
            return Err(Error::SignatureMissingOrInvalid);
        }

        let mint_res = self.client.post_mint(request).await?;

        let keys = self.load_keyset_keys(active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.load_keyset_keys(sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
                match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                    Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
        }

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        // Remove filled quote from store
        let mut quote_info = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnpaidQuote)?;
        quote_info.amount_issued += proofs.total_amount()?;

        self.localstore.add_mint_quote(quote_info.clone()).await?;

        let proof_infos = proofs
            .iter()
            .map(|proof| {
                ProofInfo::new(
                    proof.clone(),
                    self.mint_url.clone(),
                    State::Unspent,
                    quote_info.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Add new proofs to store
        self.localstore.update_proofs(proof_infos, vec![]).await?;

        // Add transaction to store
        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: proofs.total_amount()?,
                fee: Amount::ZERO,
                unit: self.unit.clone(),
                ys: proofs.ys()?,
                timestamp: unix_time(),
                memo: None,
                metadata: HashMap::new(),
            })
            .await?;

        Ok(proofs)
    }

    /// Check mint onchain quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_onchain_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteOnchainResponse<String>, Error> {
        let response = self.client.get_mint_quote_onchain_status(quote_id).await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;
                quote.amount_issued = response.amount_issued;
                quote.amount_paid = response.amount_paid;

                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }
}
