use std::collections::HashMap;

use cdk_common::nut04::MintMethodOptions;
use cdk_common::wallet::{MintQuote, Transaction, TransactionDirection};
use cdk_common::{Proofs, SecretKey};
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, MintQuoteCustomRequest, MintRequest, PaymentMethod, PreMintSecrets, SpendingConditions,
    State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote for Custom Payment Method
    #[instrument(skip(self))]
    pub(super) async fn mint_quote_custom(
        &self,
        amount: Option<Amount>,
        method: &str,
        request: String,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = &self.unit;

        self.refresh_keysets().await?;

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let payment_method = PaymentMethod::Custom(method.to_string());
            let mint_method_settings = self
                .localstore
                .get_mint(mint_url.clone())
                .await?
                .ok_or(Error::IncorrectMint)?
                .nuts
                .nut04
                .get_settings(unit, &payment_method)
                .ok_or(Error::UnsupportedUnit)?;

            match mint_method_settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let amount = amount.ok_or(Error::AmountUndefined)?;

        let mint_request = MintQuoteCustomRequest {
            amount,
            unit: self.unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
            extra: serde_json::Value::Null,
        };

        let quote_res = self
            .client
            .post_mint_custom_quote(method, mint_request)
            .await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Custom(method.to_string()),
            Some(amount),
            unit.clone(),
            request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint with custom payment method
    /// This is used for all custom payment methods - delegates to existing mint logic
    #[instrument(skip(self))]
    pub(super) async fn mint_custom(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.refresh_keysets().await?;

        let quote_info = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Verify it's a custom payment method
        if !quote_info.payment_method.is_custom() {
            return Err(Error::UnsupportedPaymentMethod);
        }

        let amount_mintable = quote_info.amount_mintable();

        if amount_mintable == Amount::ZERO {
            tracing::debug!("Amount mintable 0.");
            return Err(Error::AmountUndefined);
        }

        let unix_time = unix_time();

        if quote_info.expiry > unix_time {
            tracing::warn!("Attempting to mint with expired quote.");
        }

        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                amount_mintable,
                &amount_split_target,
                spending_conditions,
                &fee_and_amounts,
            )?,
            None => {
                // Calculate how many secrets we'll need
                let amount_split =
                    amount_mintable.split_targeted(&amount_split_target, &fee_and_amounts)?;
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
                    amount_mintable,
                    &amount_split_target,
                    &fee_and_amounts,
                )?
            }
        };

        let mut request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = quote_info.secret_key {
            request.sign(secret_key)?;
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
        self.localstore.remove_mint_quote(&quote_info.id).await?;

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
                timestamp: unix_time,
                memo: None,
                metadata: HashMap::new(),
                quote_id: Some(quote_id.to_string()),
                payment_request: Some(quote_info.request),
                payment_proof: None,
            })
            .await?;

        Ok(proofs)
    }
}
