use std::collections::HashMap;

use cdk_common::mint::{BatchMintRequest, BatchQuoteStatusRequest};
use cdk_common::wallet::MintQuote as WalletMintQuote;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{nut12, CurrencyUnit, PreMintSecrets, Proofs, SpendingConditions};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuoteState;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint batch of proofs from multiple quotes
    ///
    /// Per NUT-XX specification (https://github.com/cashubtc/nuts/issues/XX), batch minting allows
    /// minting multiple quotes in a single atomic operation. All quotes MUST be from the same
    /// payment method and currency unit.
    ///
    /// # Arguments
    /// * `quote_ids` - List of quote IDs to mint from (max 100)
    /// * `amount_split_target` - Target split for the amount
    /// * `spending_conditions` - Optional spending conditions (not yet supported for batches)
    ///
    /// # Returns
    /// * Vector of minted proofs in deterministic order
    ///
    /// # Errors
    /// * Returns error if quotes are from different mints
    /// * Returns error if quotes are from different payment methods (NUT-XX requirement)
    /// * Returns error if quotes are from different currency units (NUT-XX requirement)
    /// * Returns error if any quote is unknown
    /// * Returns error if any quote is not in PAID state
    /// * Returns error if batch exceeds 100 quote limit
    #[instrument(skip(self, spending_conditions), fields(quote_count = quote_ids.len()))]
    pub async fn mint_batch(
        &self,
        quote_ids: Vec<String>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        payment_method: crate::nuts::PaymentMethod,
    ) -> Result<Proofs, Error> {
        if quote_ids.is_empty() {
            return Err(Error::BatchEmpty);
        }

        if quote_ids.len() > 100 {
            return Err(Error::BatchSizeExceeded);
        }

        // Fetch all quote details
        let mut quote_infos = Vec::new();
        for quote_id in &quote_ids {
            let quote_info = self
                .localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            quote_infos.push(quote_info);
        }

        // Validate all quotes are from same payment method
        let quote_payment_method = &quote_infos[0].payment_method;
        for quote_info in &quote_infos {
            if &quote_info.payment_method != quote_payment_method {
                return Err(Error::BatchPaymentMethodMismatch);
            }
        }

        // Validate quotes match the endpoint payment method
        if *quote_payment_method != payment_method {
            return Err(Error::BatchPaymentMethodEndpointMismatch);
        }

        // Validate all quotes have same unit
        let unit = quote_infos[0].unit.clone();
        for quote_info in &quote_infos {
            if quote_info.unit != unit {
                return Err(Error::BatchCurrencyUnitMismatch);
            }
        }

        // Validate all quotes are in PAID state
        for quote_info in &quote_infos {
            if quote_info.state != MintQuoteState::Paid {
                return Err(Error::UnpaidQuote);
            }
        }

        if payment_method == crate::nuts::PaymentMethod::Bolt12 {
            return self
                .mint_batch_bolt12(
                    &quote_ids,
                    &quote_infos,
                    unit,
                    amount_split_target,
                    spending_conditions,
                )
                .await;
        }

        if payment_method != crate::nuts::PaymentMethod::Bolt11 {
            return Err(Error::UnsupportedPaymentMethod);
        }

        self.mint_batch_bolt11(
            &quote_ids,
            &quote_infos,
            unit,
            amount_split_target,
            spending_conditions,
        )
        .await
    }

    async fn mint_batch_bolt11(
        &self,
        quote_ids: &[String],
        quote_infos: &[WalletMintQuote],
        unit: CurrencyUnit,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let mut total_amount = Amount::ZERO;
        for quote in quote_infos {
            let amount = quote.amount_mintable();
            if amount == Amount::ZERO {
                return Err(Error::AmountUndefined);
            }
            total_amount += amount;
        }

        if total_amount == Amount::ZERO {
            return Err(Error::AmountUndefined);
        }

        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let premint_secrets = match &spending_conditions {
            Some(conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                total_amount,
                &amount_split_target,
                conditions,
                &fee_and_amounts,
            )?,
            None => {
                let amount_split =
                    total_amount.split_targeted(&amount_split_target, &fee_and_amounts)?;
                let num_secrets = amount_split.len() as u32;

                tracing::debug!(
                    "Incrementing keyset {} counter by {} for batch mint",
                    active_keyset_id,
                    num_secrets
                );

                // Atomically get the counter range we need while holding a transaction
                let mut tx = self.localstore.begin_db_transaction().await?;
                let new_counter = tx
                    .increment_keyset_counter(&active_keyset_id, num_secrets)
                    .await?;
                tx.commit().await?;

                let count = new_counter - num_secrets;

                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    total_amount,
                    &amount_split_target,
                    &fee_and_amounts,
                )?
            }
        };

        let blinded_messages = premint_secrets.blinded_messages();
        let mut batch_signatures: Option<Vec<Option<String>>> = None;
        if quote_infos.iter().any(|q| q.secret_key.is_some()) {
            let mut signatures = Vec::with_capacity(quote_infos.len());
            for (idx, quote) in quote_infos.iter().enumerate() {
                if let Some(secret_key) = &quote.secret_key {
                    let mut mint_req = cdk_common::nuts::MintRequest {
                        quote: quote_ids[idx].clone(),
                        outputs: blinded_messages.clone(),
                        signature: None,
                    };
                    mint_req
                        .sign(secret_key.clone())
                        .map_err(|_| Error::SignatureMissingOrInvalid)?;
                    signatures.push(mint_req.signature);
                } else {
                    signatures.push(None);
                }
            }
            batch_signatures = Some(signatures);
        }

        let batch_request = BatchMintRequest {
            quote: quote_ids.to_vec(),
            outputs: blinded_messages,
            signature: batch_signatures,
        };

        let proofs = self
            .execute_batch_mint(
                quote_ids,
                quote_infos,
                unit,
                crate::nuts::PaymentMethod::Bolt11,
                premint_secrets,
                batch_request,
            )
            .await?;

        for quote_id in quote_ids {
            if let Err(e) = self.localstore.remove_mint_quote(quote_id).await {
                tracing::warn!(
                    "Failed to remove quote {} from storage after batch mint: {}",
                    quote_id,
                    e
                );
            }
        }

        Ok(proofs)
    }

    async fn mint_batch_bolt12(
        &self,
        quote_ids: &[String],
        quote_infos: &[WalletMintQuote],
        unit: CurrencyUnit,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let spending_conditions =
            spending_conditions.ok_or(Error::BatchBolt12RequiresSpendingConditions)?;

        let mut mintable_amounts = Vec::with_capacity(quote_infos.len());
        let mut total_amount = Amount::ZERO;
        for quote in quote_infos {
            let amount = quote.amount_mintable();
            if amount == Amount::ZERO {
                return Err(Error::AmountUndefined);
            }
            mintable_amounts.push(amount);
            total_amount += amount;
        }

        if total_amount == Amount::ZERO {
            return Err(Error::AmountUndefined);
        }

        if quote_infos.iter().any(|quote| quote.secret_key.is_none()) {
            return Err(Error::BatchBolt12MissingSecretKey);
        }

        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let premint_secrets = PreMintSecrets::with_conditions(
            active_keyset_id,
            total_amount,
            &amount_split_target,
            &spending_conditions,
            &fee_and_amounts,
        )?;

        let blinded_messages = premint_secrets.blinded_messages();
        let signatures =
            self.prepare_bolt12_batch_signatures(quote_ids, quote_infos, &blinded_messages)?;

        let batch_request = BatchMintRequest {
            quote: quote_ids.to_vec(),
            outputs: blinded_messages,
            signature: Some(signatures),
        };

        let proofs = self
            .execute_batch_mint(
                quote_ids,
                quote_infos,
                unit,
                crate::nuts::PaymentMethod::Bolt12,
                premint_secrets,
                batch_request,
            )
            .await?;

        self.finalize_bolt12_quotes(quote_infos, &mintable_amounts)
            .await?;

        Ok(proofs)
    }

    async fn execute_batch_mint(
        &self,
        quote_ids: &[String],
        quote_infos: &[WalletMintQuote],
        unit: CurrencyUnit,
        payment_method: crate::nuts::PaymentMethod,
        premint_secrets: PreMintSecrets,
        batch_request: BatchMintRequest,
    ) -> Result<Proofs, Error> {
        if matches!(
            payment_method,
            crate::nuts::PaymentMethod::Bolt11 | crate::nuts::PaymentMethod::Bolt12
        ) {
            let batch_status_request = BatchQuoteStatusRequest {
                quote: quote_ids.to_vec(),
            };

            let batch_status = self
                .client
                .post_mint_batch_quote_status(batch_status_request)
                .await?;

            for status in &batch_status.0 {
                match status.state {
                    MintQuoteState::Paid => (),
                    MintQuoteState::Unpaid => return Err(Error::UnpaidQuote),
                    MintQuoteState::Issued => (),
                }
            }
        }

        let mint_res = self
            .client
            .post_mint_batch(batch_request, payment_method.clone())
            .await?;

        for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
            let keys = self.load_keyset_keys(sig.keyset_id).await?;
            let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
            match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                Err(_) => return Err(Error::CouldNotVerifyDleq),
            }
        }

        let active_keys = self.load_keyset_keys(premint_secrets.keyset_id).await?;
        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &active_keys,
        )?;

        let mut tx = self.localstore.begin_db_transaction().await?;

        // Remove all filled quotes from store (best-effort cleanup)
        for quote_id in quote_ids.iter() {
            if let Err(e) = tx.remove_mint_quote(quote_id).await {
                tracing::warn!("Failed to remove quote {} from storage: {}", quote_id, e);
            }
        }

        let proof_infos = proofs
            .iter()
            .map(|proof| {
                ProofInfo::new(
                    proof.clone(),
                    self.mint_url.clone(),
                    crate::nuts::State::Unspent,
                    unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Add new proofs to store within the same transaction
        tx.update_proofs(proof_infos, vec![]).await?;

        let batch_ids = quote_ids.join(",");
        let first_request = quote_infos.first().map(|quote| quote.request.clone());
        tx.add_transaction(crate::wallet::types::Transaction {
            mint_url: self.mint_url.clone(),
            direction: crate::wallet::types::TransactionDirection::Incoming,
            amount: proofs.total_amount()?,
            fee: Amount::ZERO,
            unit: self.unit.clone(),
            ys: proofs.ys()?,
            timestamp: unix_time(),
            memo: None,
            metadata: HashMap::new(),
            quote_id: Some(batch_ids),
            payment_request: first_request,
            payment_proof: None,
        })
        .await?;

        tx.commit().await?;

        Ok(proofs)
    }

    fn prepare_bolt12_batch_signatures(
        &self,
        quote_ids: &[String],
        quote_infos: &[WalletMintQuote],
        blinded_messages: &[crate::nuts::nut00::BlindedMessage],
    ) -> Result<Vec<Option<String>>, Error> {
        let mut signatures = Vec::with_capacity(quote_infos.len());
        for (idx, quote) in quote_infos.iter().enumerate() {
            let secret_key = quote
                .secret_key
                .clone()
                .ok_or(Error::BatchBolt12MissingSecretKey)?;
            let mut mint_req = cdk_common::nuts::MintRequest {
                quote: quote_ids[idx].clone(),
                outputs: blinded_messages.to_vec(),
                signature: None,
            };
            mint_req
                .sign(secret_key)
                .map_err(|_| Error::SignatureMissingOrInvalid)?;
            signatures.push(mint_req.signature);
        }
        Ok(signatures)
    }

    async fn finalize_bolt12_quotes(
        &self,
        quote_infos: &[WalletMintQuote],
        minted_amounts: &[Amount],
    ) -> Result<(), Error> {
        debug_assert_eq!(quote_infos.len(), minted_amounts.len());

        for (quote, minted_amount) in quote_infos.iter().zip(minted_amounts.iter()) {
            let mut updated_quote = quote.clone();
            updated_quote.amount_issued += *minted_amount;
            if updated_quote.amount_paid > Amount::ZERO
                && updated_quote.amount_paid == updated_quote.amount_issued
            {
                updated_quote.state = MintQuoteState::Issued;
            }
            self.localstore.add_mint_quote(updated_quote).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdk_sqlite::wallet::memory;
    use rand::random;
    use std::sync::Arc;

    fn build_test_quote(amount_paid: Amount, amount_issued: Amount) -> WalletMintQuote {
        let mut quote = WalletMintQuote::new(
            "test-quote".to_string(),
            "https://example.com".parse().expect("url"),
            crate::nuts::PaymentMethod::Bolt12,
            Some(amount_paid),
            CurrencyUnit::Sat,
            "lnbc1".to_string(),
            1000,
            None,
        );
        quote.state = MintQuoteState::Paid;
        quote.amount_paid = amount_paid;
        quote.amount_issued = amount_issued;
        quote
    }

    #[tokio::test]
    async fn finalize_bolt12_quotes_marks_issued_when_fully_redeemed() -> Result<(), Error> {
        let seed = random::<[u8; 64]>();
        let localstore = memory::empty().await.expect("store");
        let wallet = Wallet::new(
            "https://example.com",
            CurrencyUnit::Sat,
            Arc::new(localstore),
            seed,
            None,
        )?;

        let quote = build_test_quote(Amount::from(200), Amount::from(100));
        wallet
            .localstore
            .add_mint_quote(quote.clone())
            .await
            .expect("store quote");

        wallet
            .finalize_bolt12_quotes(&[quote.clone()], &[Amount::from(100)])
            .await?;

        let stored = wallet
            .localstore
            .get_mint_quote(&quote.id)
            .await
            .expect("lookup")
            .expect("quote");
        assert_eq!(stored.amount_issued, Amount::from(200));
        assert_eq!(stored.state, MintQuoteState::Issued);
        Ok(())
    }

    #[tokio::test]
    async fn finalize_bolt12_quotes_keeps_paid_when_amount_remaining() -> Result<(), Error> {
        let seed = random::<[u8; 64]>();
        let localstore = memory::empty().await.expect("store");
        let wallet = Wallet::new(
            "https://example.com",
            CurrencyUnit::Sat,
            Arc::new(localstore),
            seed,
            None,
        )?;

        let quote = build_test_quote(Amount::from(200), Amount::from(50));
        wallet
            .localstore
            .add_mint_quote(quote.clone())
            .await
            .expect("store quote");

        wallet
            .finalize_bolt12_quotes(&[quote.clone()], &[Amount::from(100)])
            .await?;

        let stored = wallet
            .localstore
            .get_mint_quote(&quote.id)
            .await
            .expect("lookup")
            .expect("quote");
        assert_eq!(stored.amount_issued, Amount::from(150));
        assert_eq!(stored.state, MintQuoteState::Paid);
        Ok(())
    }
}
