use cdk_common::{BlindSignature, CurrencyUnit};
use tracing::{instrument, warn};

use super::MintQuote;
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, Id, MintBolt11Request, MintQuoteBolt11Request, MintQuoteBolt11Response, PreMintSecrets,
    Proofs, SecretKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuoteState;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote
    /// # Synopsis
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use cdk::amount::Amount;
    /// use cdk::cdk_database::WalletMemoryDatabase;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use rand::Rng;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = rand::thread_rng().gen::<[u8; 32]>();
    ///     let mint_url = "https://testnut.cashu.space";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = WalletMemoryDatabase::default();
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;
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

        let secret_key = SecretKey::generate();

        let request = MintQuoteBolt11Request {
            amount,
            unit: unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
        };

        let quote_res = self.client.post_mint_quote(request).await?;

        let quote = MintQuote {
            mint_url,
            id: quote_res.quote,
            amount,
            unit: unit.clone(),
            request: quote_res.request,
            state: quote_res.state,
            expiry: quote_res.expiry.unwrap_or(0),
            secret_key: Some(secret_key),
        };

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
                // TODO: Need to pass in keys here
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

    /// Mint
    /// # Synopsis
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use anyhow::Result;
    /// use cdk::amount::{Amount, SplitTarget};
    /// use cdk::cdk_database::WalletMemoryDatabase;
    /// use cdk::nuts::nut00::ProofsMethods;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use rand::Rng;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let seed = rand::thread_rng().gen::<[u8; 32]>();
    ///     let mint_url = "https://testnut.cashu.space";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = WalletMemoryDatabase::default();
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     let quote_id = quote.id;
    ///     // To be called after quote request is paid
    ///     let minted_proofs = wallet.mint(&quote_id, SplitTarget::default(), None).await?;
    ///     let minted_amount = minted_proofs.total_amount()?;
    ///
    ///     Ok(())

    /// ```
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
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

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                quote_info.amount,
                &amount_split_target,
                spending_conditions,
            )?,
            None => PreMintSecrets::from_xpriv(
                active_keyset_id,
                count,
                self.xpriv,
                quote_info.amount,
                &amount_split_target,
            )?,
        };

        let mut request = MintBolt11Request {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = quote_info.secret_key {
            request.sign(secret_key)?;
        }

        let mint_res = self.client.post_mint(request).await?;

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

        // Remove filled quote from store
        self.localstore.remove_mint_quote(&quote_info.id).await?;

        if spending_conditions.is_none() {
            tracing::debug!(
                "Incrementing keyset {} counter by {}",
                active_keyset_id,
                proofs.len()
            );

            // Update counter for keyset
            self.localstore
                .increment_keyset_counter(&active_keyset_id, proofs.len() as u32)
                .await?;
        }

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

        Ok(proofs)
    }

    fn generate_premint_secrets(
        &self,
        active_keyset_id: Id,
        quote_info_amount: Amount,
        amount_split_target: &SplitTarget,
        spending_conditions: Option<&SpendingConditions>,
        count: u32,
    ) -> Result<PreMintSecrets, Error> {
        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                quote_info_amount,
                amount_split_target,
                spending_conditions,
            )?,
            None => PreMintSecrets::from_xpriv(
                active_keyset_id,
                count,
                self.xpriv,
                quote_info_amount,
                amount_split_target,
            )?,
        };

        Ok(premint_secrets)
    }

    /// Creates and stores pre-mint secrets for a given quote.
    ///
    /// This function checks whether a mint quote already exists, creates a new quote if necessary,
    /// generates pre-mint secrets, and stores them in the local wallet store.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount for which pre-mint secrets should be generated.
    /// * `quote_id` - A unique identifier for the mint quote.
    /// * `mint_url` - The URL of the mint.
    /// * `currency_unit` - nut00 currency unit
    ///
    /// # Returns
    ///
    /// Returns a `PreMintSecrets` object containing the generated secrets.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The quote ID already exists (`Error::PaidQuote`).
    /// - Adding the quote or pre-mint secrets fails.
    pub async fn create_premint_secrets(
        &self,
        amount: u64,
        quote_id: &str,
        mint_url: &str,
        currency_unit: CurrencyUnit,
    ) -> Result<PreMintSecrets, Error> {
        // Ensure the quote does not already exist
        if self.localstore.get_mint_quote(quote_id).await?.is_some() {
            return Err(Error::PaidQuote);
        }

        // Create and store a new mint quote
        let mint_quote = MintQuote {
            id: quote_id.to_string(),
            mint_url: mint_url.parse()?,
            amount: Amount::from(amount),
            unit: currency_unit,
            request: "todo".to_string(), // TODO: what does request do?
            state: MintQuoteState::Paid,
            expiry: u64::MAX, // TODO: expiry param?
            secret_key: None,
        };

        self.localstore.add_mint_quote(mint_quote).await?;

        let active_keyset_id = self.get_active_mint_keyset_local().await?.id;

        // Retrieve the keyset counter, defaulting to 0 if not found
        // TODO do we need to update the counter if it was not returned?
        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?
            .unwrap_or(0)
            + 1;

        let premint_secrets = self.generate_premint_secrets(
            active_keyset_id,
            Amount::from(amount),
            &SplitTarget::None,
            None,
            count,
        )?;

        self.localstore
            .add_premint_secrets(quote_id, &premint_secrets)
            .await?;

        Ok(premint_secrets)
    }

    /// Verifies signatures, constructs proofs, and stores them in the wallet.
    ///
    /// This function:
    /// - Ensures that the quote exists and the keyset matches.
    /// - Verifies each provided signature against the pre-mint secrets.
    /// - Constructs proofs based on verified signatures.
    /// - Updates the keyset counter and stores the proofs.
    /// - Removes the corresponding mint quote, logging a warning if removal fails.
    ///
    /// # Arguments
    ///
    /// * `signatures` - An array of optional `BlindSignature`s, each corresponding to a pre-minted secret.
    /// * `quote_id` - The quote identifier associated with the signatures.
    ///
    /// # Returns
    ///
    /// Returns the total minted amount.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The quote is unknown or has an invalid keyset (`Error::UnknownQuote` or `Error::UnknownKeySet`).
    /// - Signature verification fails (`Error::CouldNotVerifyDleq`).
    /// - Constructing proofs fails.
    pub async fn verify_and_store_proofs(
        &self,
        signatures: [Option<BlindSignature>; 64],
        quote_id: &str,
    ) -> Result<Amount, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .expect("No quote found");

        // TODO pass this in, it will break if the keyset changes before getting proofs
        let active_keyset_id = self.get_active_mint_keyset_local().await?.id;
        let keys = self.get_keyset_keys(active_keyset_id).await?;

        let premint_secrets = self
            .localstore
            .get_premint_secrets(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if premint_secrets.keyset_id != active_keyset_id {
            return Err(Error::UnknownKeySet);
        }

        let mut verified_signatures = Vec::new();
        let mut premint_rs = Vec::new();
        let mut premint_secrets_vec = Vec::new();

        // verify each signature
        for sig in signatures.iter().flatten() {
            let matching_secret = premint_secrets
                .secrets
                .iter()
                .find(|secret| secret.amount == sig.amount)
                .ok_or_else(|| Error::Custom("Secret not found".to_string()))?;

            let keys = self.get_keyset_keys(sig.keyset_id).await?;
            let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;

            match sig.verify_dleq(key, matching_secret.blinded_message.blinded_secret) {
                Ok(_) | Err(nut12::Error::MissingDleqProof) => {
                    // TODO can we clean up these clones?
                    verified_signatures.push(sig.clone());
                    premint_rs.push(matching_secret.r.clone());
                    premint_secrets_vec.push(matching_secret.secret.clone());
                }
                Err(_) => return Err(Error::CouldNotVerifyDleq),
            }
        }

        let proofs = construct_proofs(verified_signatures, premint_rs, premint_secrets_vec, &keys)?;
        let minted_amount = proofs.total_amount()?;

        self.localstore
            .increment_keyset_counter(&active_keyset_id, proofs.len() as u32)
            .await?;

        let proofs: Vec<ProofInfo> = proofs
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    self.mint_url.clone(),
                    State::Unspent,
                    quote.unit.clone(),
                )
            })
            .collect::<Result<_, _>>()?;

        // store new proofs in wallet
        self.localstore
            .update_proofs(proofs.clone(), vec![])
            .await?;

        // Remove Quote and warn log any error
        if let Err(err) = self.localstore.remove_mint_quote(quote_id).await {
            warn!("Failed to remove mint quote {}: {}", quote_id, err);
        }

        Ok(minted_amount)
    }
}
