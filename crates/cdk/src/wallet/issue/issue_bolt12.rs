use std::collections::HashMap;

use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut25::MintQuoteBolt12Request;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::{Proofs, SecretKey};
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, MintQuoteBolt12Response, MintRequest, PaymentMethod, PreMintSecrets, SpendingConditions,
    State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuote;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Bolt12
    #[instrument(skip(self))]
    pub async fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_info = self.load_mint_info().await?;

        let mint_url = self.mint_url.clone();
        let unit = &self.unit;

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let mint_method_settings = mint_info
                .nuts
                .nut04
                .get_settings(unit, &crate::nuts::PaymentMethod::Bolt12)
                .ok_or(Error::UnsupportedUnit)?;

            match mint_method_settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let mint_request = MintQuoteBolt12Request {
            amount,
            unit: self.unit.clone(),
            description,
            pubkey: secret_key.public_key(),
        };

        let quote_res = self.client.post_mint_bolt12_quote(mint_request).await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Bolt12,
            amount,
            unit.clone(),
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.add_mint_quote(quote.clone()).await?;
        tx.commit().await?;

        Ok(quote)
    }

    /// Mint bolt12
    #[instrument(skip(self))]
    pub async fn mint_bolt12(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let mut tx = self.localstore.begin_db_transaction().await?;
        let quote_info = tx.get_mint_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) && quote.expiry.ne(&0) {
                tracing::info!("Attempting to mint expired quote.");
            }

            quote.clone()
        } else {
            return Err(Error::UnknownQuote);
        };

        let (mut tx, quote_info, amount) = match amount {
            Some(amount) => (tx, quote_info, amount),
            None => {
                // If an amount it not supplied with check the status of the quote
                // The mint will tell us how much can be minted
                tx.commit().await?;
                let state = self.mint_bolt12_quote_state(quote_id).await?;

                let mut tx = self.localstore.begin_db_transaction().await?;
                let quote_info = tx
                    .get_mint_quote(quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;

                (tx, quote_info, state.amount_paid - state.amount_issued)
            }
        };

        if amount == Amount::ZERO {
            tracing::error!("Cannot mint zero amount.");
            return Err(Error::UnpaidQuote);
        }

        let split_target = match amount_split_target {
            SplitTarget::None => {
                self.determine_split_target_values(&mut tx, amount, &fee_and_amounts)
                    .await?
            }
            s => s,
        };

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                amount,
                &split_target,
                spending_conditions,
                &fee_and_amounts,
            )?,
            None => {
                let amount_split = amount.split_targeted(&split_target, &fee_and_amounts)?;
                let num_secrets = amount_split.len() as u32;

                tracing::debug!(
                    "Incrementing keyset {} counter by {}",
                    active_keyset_id,
                    num_secrets
                );

                // Atomically get the counter range we need
                let new_counter = tx
                    .increment_keyset_counter(&active_keyset_id, num_secrets)
                    .await?;

                let count = new_counter - num_secrets;

                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    amount,
                    &split_target,
                    &fee_and_amounts,
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
            tracing::error!("Signature is required for bolt12.");
            return Err(Error::SignatureMissingOrInvalid);
        }

        tx.commit().await?;

        let mint_res = self.client.post_mint(request).await?;

        let mut tx = self.localstore.begin_db_transaction().await?;

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

        // Update quote with issued amount
        let mut quote_info = tx
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnpaidQuote)?;
        quote_info.amount_issued += proofs.total_amount()?;

        tx.add_mint_quote(quote_info.clone()).await?;

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
        tx.update_proofs(proof_infos, vec![]).await?;

        // Add transaction to store
        tx.add_transaction(Transaction {
            mint_url: self.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: proofs.total_amount()?,
            fee: Amount::ZERO,
            unit: self.unit.clone(),
            ys: proofs.ys()?,
            timestamp: unix_time(),
            memo: None,
            metadata: HashMap::new(),
            quote_id: Some(quote_id.to_string()),
            payment_request: Some(quote_info.request),
            payment_proof: None,
            payment_method: Some(quote_info.payment_method),
        })
        .await?;

        tx.commit().await?;

        Ok(proofs)
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_bolt12_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let response = self.client.get_mint_quote_bolt12_status(quote_id).await?;

        let mut tx = self.localstore.begin_db_transaction().await?;

        match tx.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;
                quote.amount_issued = response.amount_issued;
                quote.amount_paid = response.amount_paid;

                tx.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        tx.commit().await?;

        Ok(response)
    }
}
