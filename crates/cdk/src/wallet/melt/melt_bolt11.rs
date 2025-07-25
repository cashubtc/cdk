use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{Transaction, TransactionDirection};
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;

use crate::amount::to_unit;
use crate::dhke::construct_proofs;
use crate::nuts::{
    CurrencyUnit, MeltOptions, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest,
    PreMintSecrets, Proofs, ProofsMethods, State,
};
use crate::types::{Melted, ProofInfo};
use crate::util::unix_time;
use crate::wallet::MeltQuote;
use crate::{ensure_cdk, Error, Wallet};

impl Wallet {
    /// Melt Quote
    /// # Synopsis
    /// ```rust
    ///  use std::sync::Arc;
    ///
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::Wallet;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 32]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();
    ///     let bolt11 = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq".to_string();
    ///     let quote = wallet.melt_quote(bolt11, None).await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self, request))]
    pub async fn melt_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, Error> {
        let invoice = Bolt11Invoice::from_str(&request)?;

        let quote_request = MeltQuoteBolt11Request {
            request: Bolt11Invoice::from_str(&request)?,
            unit: self.unit.clone(),
            options,
        };

        let quote_res = self.client.post_melt_quote(quote_request).await?;

        if self.unit == CurrencyUnit::Msat || self.unit == CurrencyUnit::Sat {
            let amount_msat = options
                .map(|opt| opt.amount_msat().into())
                .or_else(|| invoice.amount_milli_satoshis())
                .ok_or(Error::InvoiceAmountUndefined)?;

            let amount_quote_unit = to_unit(amount_msat, &CurrencyUnit::Msat, &self.unit)?;

            if quote_res.amount != amount_quote_unit {
                tracing::warn!(
                    "Mint returned incorrect quote amount. Expected {}, got {}",
                    amount_quote_unit,
                    quote_res.amount
                );
                return Err(Error::IncorrectQuoteAmount);
            }
        }

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let response = self.client.get_melt_quote_status(quote_id).await?;

        match self.localstore.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                if let Err(e) = self
                    .add_transaction_for_pending_melt(&quote, &response)
                    .await
                {
                    tracing::error!("Failed to add transaction for pending melt: {}", e);
                }

                quote.state = response.state;
                self.localstore.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Melt specific proofs
    #[instrument(skip(self, proofs))]
    pub async fn melt_proofs(&self, quote_id: &str, proofs: Proofs) -> Result<Melted, Error> {
        let quote_info = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        ensure_cdk!(
            quote_info.expiry.gt(&unix_time()),
            Error::ExpiredQuote(quote_info.expiry, unix_time())
        );

        let proofs_total = proofs.total_amount()?;
        if proofs_total < quote_info.amount + quote_info.fee_reserve {
            return Err(Error::InsufficientFunds);
        }

        let ys = proofs.ys()?;
        self.localstore
            .update_proofs_state(ys, State::Pending)
            .await?;

        let active_keyset_id = self.fetch_active_keyset().await?.id;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let premint_secrets = PreMintSecrets::from_xpriv_blank(
            active_keyset_id,
            count,
            self.xpriv,
            proofs_total - quote_info.amount,
        )?;

        let request = MeltRequest::new(
            quote_id.to_string(),
            proofs.clone(),
            Some(premint_secrets.blinded_messages()),
        );

        let melt_response = self.client.post_melt(request).await;

        let melt_response = match melt_response {
            Ok(melt_response) => melt_response,
            Err(err) => {
                tracing::error!("Could not melt: {}", err);
                tracing::info!("Checking status of input proofs.");

                self.reclaim_unspent(proofs).await?;

                return Err(err);
            }
        };

        let active_keys = self
            .localstore
            .get_keys(&active_keyset_id)
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        let change_proofs = match melt_response.change {
            Some(change) => {
                let num_change_proof = change.len();

                let num_change_proof = match (
                    premint_secrets.len() < num_change_proof,
                    premint_secrets.secrets().len() < num_change_proof,
                ) {
                    (true, _) | (_, true) => {
                        tracing::error!("Mismatch in change promises to change");
                        premint_secrets.len()
                    }
                    _ => num_change_proof,
                };

                Some(construct_proofs(
                    change,
                    premint_secrets.rs()[..num_change_proof].to_vec(),
                    premint_secrets.secrets()[..num_change_proof].to_vec(),
                    &active_keys,
                )?)
            }
            None => None,
        };

        let melted = Melted::from_proofs(
            melt_response.state,
            melt_response.payment_preimage,
            quote_info.amount,
            proofs.clone(),
            change_proofs.clone(),
        )?;

        let change_proof_infos = match change_proofs {
            Some(change_proofs) => {
                tracing::debug!(
                    "Change amount returned from melt: {}",
                    change_proofs.total_amount()?
                );

                // Update counter for keyset
                self.localstore
                    .increment_keyset_counter(&active_keyset_id, change_proofs.len() as u32)
                    .await?;

                change_proofs
                    .into_iter()
                    .map(|proof| {
                        ProofInfo::new(
                            proof,
                            self.mint_url.clone(),
                            State::Unspent,
                            quote_info.unit.clone(),
                        )
                    })
                    .collect::<Result<Vec<ProofInfo>, _>>()?
            }
            None => Vec::new(),
        };

        self.localstore.remove_melt_quote(&quote_info.id).await?;

        let deleted_ys = proofs.ys()?;
        self.localstore
            .update_proofs(change_proof_infos, deleted_ys)
            .await?;

        // Add transaction to store
        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Outgoing,
                amount: melted.amount,
                fee: melted.fee_paid,
                unit: self.unit.clone(),
                ys: proofs.ys()?,
                timestamp: unix_time(),
                memo: None,
                metadata: HashMap::new(),
            })
            .await?;

        Ok(melted)
    }

    /// Melt
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::Wallet;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 32]>();
    ///  let mint_url = "https://fake.thesimplekid.dev";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();
    ///  let bolt11 = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq".to_string();
    ///  let quote = wallet.melt_quote(bolt11, None).await?;
    ///  let quote_id = quote.id;
    ///
    ///  let _ = wallet.melt(&quote_id).await?;
    ///
    ///  Ok(())
    /// }
    #[instrument(skip(self))]
    pub async fn melt(&self, quote_id: &str) -> Result<Melted, Error> {
        let quote_info = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        ensure_cdk!(
            quote_info.expiry.gt(&unix_time()),
            Error::ExpiredQuote(quote_info.expiry, unix_time())
        );

        let inputs_needed_amount = quote_info.amount + quote_info.fee_reserve;

        let available_proofs = self.get_unspent_proofs().await?;

        let active_keyset_ids = self
            .refresh_keysets()
            .await?
            .into_iter()
            .map(|k| k.id)
            .collect();
        let keyset_fees = self.get_keyset_fees().await?;
        let (mut input_proofs, mut exchange) = Wallet::select_exact_proofs(
            inputs_needed_amount,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees,
            true,
        )?;

        if let Some((proof, exact_amount)) = exchange.take() {
            let new_proofs = self
                .swap(
                    Some(exact_amount),
                    SplitTarget::None,
                    vec![proof.clone()],
                    None,
                    false,
                )
                .await?
                .ok_or_else(|| {
                    tracing::error!("Received empty proofs");
                    Error::Internal
                })?;

            input_proofs.extend_from_slice(&new_proofs);
        }

        self.melt_proofs(quote_id, input_proofs).await
    }
}
