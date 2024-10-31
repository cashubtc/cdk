use tracing::instrument;

use super::MintQuote;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut19::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use crate::nuts::PaymentMethod;
use crate::{
    amount::SplitTarget,
    dhke::construct_proofs,
    nuts::{nut12, PreMintSecrets, SpendingConditions, State},
    types::ProofInfo,
    util::unix_time,
    Amount, Error, Wallet,
};

impl Wallet {
    /// Mint Bolt12
    #[instrument(skip(self))]
    pub async fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
        single_use: bool,
        expiry: Option<u64>,
    ) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = self.unit;

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let mint_method_settings = self
                .localstore
                .get_mint(mint_url.clone())
                .await?
                .ok_or(Error::IncorrectMint)?
                .nuts
                .nut04
                .get_settings(&unit, &crate::nuts::PaymentMethod::Bolt11)
                .ok_or(Error::UnsupportedUnit)?;

            if !mint_method_settings.description {
                return Err(Error::InvoiceDescriptionUnsupported);
            }
        }

        let mint_request = MintQuoteBolt12Request {
            amount,
            unit,
            description,
            single_use,
            expiry,
        };

        let quote_res = self
            .client
            .post_mint_bolt12_quote(mint_url.clone(), mint_request)
            .await?;

        let quote = MintQuote {
            mint_url,
            id: quote_res.quote.clone(),
            payment_method: PaymentMethod::Bolt12,
            amount: amount.unwrap_or(Amount::ZERO),
            unit,
            request: quote_res.request,
            state: crate::nuts::MintQuoteState::Unpaid,
            expiry: quote_res.expiry.unwrap_or(0),
            amount_minted: Amount::ZERO,
            amount_paid: Amount::ZERO,
        };

        self.localstore.add_mint_quote(quote.clone()).await?;

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
    ) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let quote_info = self.localstore.get_mint_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) && quote.expiry.ne(&0) {
                return Err(Error::ExpiredQuote(quote.expiry, unix_time()));
            }

            quote.clone()
        } else {
            return Err(Error::UnknownQuote);
        };

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let amount = match amount {
            Some(amount) => amount,
            None => {
                // If an amount it not supplied with check the status of the quote
                // The mint will tell us how much can be minted
                let state = self.mint_bolt12_quote_state(quote_id).await?;

                state.amount_paid - state.amount_issued
            }
        };

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                amount,
                &amount_split_target,
                spending_conditions,
            )?,
            None => PreMintSecrets::from_xpriv(
                active_keyset_id,
                count,
                self.xpriv,
                amount,
                &amount_split_target,
            )?,
        };

        let mint_res = self
            .client
            .post_mint(self.mint_url.clone(), quote_id, premint_secrets.clone())
            .await?;

        let keys = self.get_keyset_keys(active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.get_keyset_keys(sig.keyset_id).await?;
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

        let minted_amount = proofs.total_amount()?;

        // Remove filled quote from store
        //self.localstore.remove_mint_quote(&quote_info.id).await?;

        if spending_conditions.is_none() {
            // Update counter for keyset
            self.localstore
                .increment_keyset_counter(&active_keyset_id, proofs.len() as u32)
                .await?;
        }

        let proofs = proofs
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    self.mint_url.clone(),
                    State::Unspent,
                    quote_info.unit,
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Add new proofs to store
        self.localstore.update_proofs(proofs, vec![]).await?;

        Ok(minted_amount)
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_bolt12_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response, Error> {
        let response = self
            .client
            .get_mint_bolt12_quote_status(self.mint_url.clone(), quote_id)
            .await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;
                quote.amount_minted = response.amount_issued;
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
