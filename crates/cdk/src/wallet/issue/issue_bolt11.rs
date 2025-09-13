use std::collections::HashMap;

use cdk_common::nut04::MintMethodOptions;
use cdk_common::wallet::{MintQuote, Transaction, TransactionDirection};
use cdk_common::PaymentMethod;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, MintQuoteBolt11Request, MintQuoteBolt11Response, MintRequest, PreMintSecrets, Proofs,
    SecretKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuoteState;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use cdk::amount::Amount;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        self.refresh_keysets().await?;

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let settings = self
                .localstore
                .get_mint(mint_url.clone())
                .await?
                .ok_or(Error::IncorrectMint)?
                .nuts
                .nut04
                .get_settings(&unit, &crate::nuts::PaymentMethod::Bolt11)
                .ok_or(Error::UnsupportedUnit)?;

            match settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let request = MintQuoteBolt11Request {
            amount,
            unit: unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
        };

        let quote_res = self.client.post_mint_quote(request).await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Bolt11,
            Some(amount),
            unit,
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let response = self.client.get_mint_quote_status(quote_id).await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.state = response.state;
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Check status of pending mint quotes
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            let mint_quote_response = self.mint_quote_state(&mint_quote.id).await?;

            if mint_quote_response.state == MintQuoteState::Paid {
                let proofs = self
                    .mint(&mint_quote.id, SplitTarget::default(), None)
                    .await?;
                total_amount += proofs.total_amount()?;
            } else if mint_quote.expiry.le(&unix_time()) {
                self.localstore.remove_mint_quote(&mint_quote.id).await?;
            }
        }
        Ok(total_amount)
    }

    /// Get active mint quotes
    /// Returns mint quotes that are not expired and not yet issued.
    #[instrument(skip(self))]
    pub async fn get_active_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mut mint_quotes = self.localstore.get_mint_quotes().await?;
        let unix_time = unix_time();
        mint_quotes.retain(|quote| {
            quote.mint_url == self.mint_url
                && quote.state != MintQuoteState::Issued
                && quote.expiry > unix_time
        });
        Ok(mint_quotes)
    }

    /// Mint
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use anyhow::Result;
    /// use cdk::amount::{Amount, SplitTarget};
    /// use cdk::nuts::nut00::ProofsMethods;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     let quote_id = quote.id;
    ///     // To be called after quote request is paid
    ///     let minted_proofs = wallet.mint(&quote_id, SplitTarget::default(), None).await?;
    ///     let minted_amount = minted_proofs.total_amount()?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint(
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

        if quote_info.payment_method != PaymentMethod::Bolt11 {
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
            })
            .await?;

        Ok(proofs)
    }
}
