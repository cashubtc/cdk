use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::PaymentMethod;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;

use crate::amount::to_unit;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CurrencyUnit, MeltOptions, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest,
    PreMintSecrets, Proofs, State,
};
use crate::types::{Melted, ProofInfo};
use crate::util::unix_time;
use crate::wallet::send::split_proofs_for_send;
use crate::wallet::MeltQuote;
use crate::{ensure_cdk, Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote
    /// # Synopsis
    /// ```rust,no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::Wallet;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
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
            request: invoice.clone(),
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
            payment_method: PaymentMethod::Bolt11,
        };

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.add_melt_quote(quote.clone()).await?;
        tx.commit().await?;

        Ok(quote)
    }

    /// Melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let response = self.client.get_melt_quote_status(quote_id).await?;

        let mut tx = self.localstore.begin_db_transaction().await?;

        match tx.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                if let Err(e) = self
                    .add_transaction_for_pending_melt(&mut tx, &quote, &response)
                    .await
                {
                    tracing::error!("Failed to add transaction for pending melt: {}", e);
                }

                quote.state = response.state;
                tx.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        tx.commit().await?;

        Ok(response)
    }

    /// Melt specific proofs
    #[instrument(skip(self, proofs))]
    pub async fn melt_proofs(&self, quote_id: &str, proofs: Proofs) -> Result<Melted, Error> {
        self.melt_proofs_with_metadata(quote_id, proofs, HashMap::new())
            .await
    }

    /// Melt specific proofs
    #[instrument(skip(self, proofs))]
    pub async fn melt_proofs_with_metadata(
        &self,
        quote_id: &str,
        proofs: Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<Melted, Error> {
        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let mut tx = self.localstore.begin_db_transaction().await?;
        let mut quote_info = tx
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

        // Since the proofs may be external (not in our database), add them first
        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| ProofInfo::new(p, self.mint_url.clone(), State::Pending, self.unit.clone()))
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        tx.update_proofs(proofs_info, vec![]).await?;

        // Calculate change accounting for input fees
        // The mint deducts input fees from available funds before calculating change
        let input_fee = self.get_proofs_fee(&proofs).await?.total;
        let change_amount = proofs_total - quote_info.amount - input_fee;

        let premint_secrets = if change_amount <= Amount::ZERO {
            PreMintSecrets::new(active_keyset_id)
        } else {
            // TODO: consolidate this calculation with from_seed_blank into a shared function
            // Calculate how many secrets will be needed using the same logic as from_seed_blank
            let num_secrets =
                ((u64::from(change_amount) as f64).log2().ceil() as u64).max(1) as u32;

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

            PreMintSecrets::from_seed_blank(active_keyset_id, count, &self.seed, change_amount)?
        };

        let request = MeltRequest::new(
            quote_id.to_string(),
            proofs.clone(),
            Some(premint_secrets.blinded_messages()),
        );

        tx.commit().await?;

        let melt_response = match quote_info.payment_method {
            cdk_common::PaymentMethod::Bolt11 => {
                self.try_proof_operation_or_reclaim(
                    request.inputs().clone(),
                    self.client.post_melt(request),
                )
                .await?
            }
            cdk_common::PaymentMethod::Bolt12 => {
                self.try_proof_operation_or_reclaim(
                    request.inputs().clone(),
                    self.client.post_melt_bolt12(request),
                )
                .await?
            }
            cdk_common::PaymentMethod::Custom(_) => {
                return Err(Error::UnsupportedPaymentMethod);
            }
        };

        let active_keys = self.load_keyset_keys(active_keyset_id).await?;

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

        let payment_preimage = melt_response.payment_preimage.clone();

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

        let mut tx = self.localstore.begin_db_transaction().await?;

        quote_info.state = cdk_common::MeltQuoteState::Paid;

        let payment_request = quote_info.request.clone();
        let payment_method = quote_info.payment_method.clone();
        tx.add_melt_quote(quote_info).await?;

        let deleted_ys = proofs.ys()?;

        tx.update_proofs(change_proof_infos, deleted_ys).await?;

        // Add transaction to store
        tx.add_transaction(Transaction {
            mint_url: self.mint_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: melted.amount,
            fee: melted.fee_paid,
            unit: self.unit.clone(),
            ys: proofs.ys()?,
            timestamp: unix_time(),
            memo: None,
            metadata,
            quote_id: Some(quote_id.to_string()),
            payment_request: Some(payment_request),
            payment_proof: payment_preimage,
            payment_method: Some(payment_method),
        })
        .await?;

        tx.commit().await?;

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
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://fake.thesimplekid.dev";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
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
        self.melt_with_metadata(quote_id, HashMap::new()).await
    }

    /// Melt with additional metadata to be saved locally with the transaction
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
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://fake.thesimplekid.dev";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let bolt11 = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq".to_string();
    ///  let quote = wallet.melt_quote(bolt11, None).await?;
    ///  let quote_id = quote.id;
    ///
    ///  let mut metadata = std::collections::HashMap::new();
    ///  metadata.insert("my key".to_string(), "my value".to_string());
    ///
    ///  let _ = wallet.melt_with_metadata(&quote_id, metadata).await?;
    ///
    ///  Ok(())
    /// }
    #[instrument(skip(self))]
    pub async fn melt_with_metadata(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<Melted, Error> {
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

        let active_keyset_ids = self
            .get_mint_keysets()
            .await?
            .into_iter()
            .map(|k| k.id)
            .collect();
        let keyset_fees_and_amounts = self.get_keyset_fees_and_amounts().await?;

        let available_proofs = self.get_unspent_proofs().await?;

        // Two-step proof selection for melt:
        // Step 1: Try to select proofs that exactly match inputs_needed_amount.
        //         If successful, no swap is required and we avoid paying swap fees.
        // Step 2: If exact match not possible, we need to swap to get optimal denominations.
        //         In this case, we must select more proofs to cover the additional swap fees.
        {
            let input_proofs = Wallet::select_proofs(
                inputs_needed_amount,
                available_proofs.clone(),
                &active_keyset_ids,
                &keyset_fees_and_amounts,
                true,
            )?;
            let proofs_total = input_proofs.total_amount()?;

            // If exact match, use proofs directly without swap
            if proofs_total == inputs_needed_amount {
                return self
                    .melt_proofs_with_metadata(quote_id, input_proofs, metadata)
                    .await;
            }
        }

        let active_keyset_id = self.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        // Calculate optimal denomination split and the fee for those proofs
        // First estimate based on inputs_needed_amount to get target_fee
        let initial_split = inputs_needed_amount.split(&fee_and_amounts);
        let target_fee = self
            .get_proofs_fee_by_count(
                vec![(active_keyset_id, initial_split.len() as u64)]
                    .into_iter()
                    .collect(),
            )
            .await?
            .total;

        // Since we could not select the correct inputs amount needed for melting,
        // we select again this time including the amount we will now have to pay as a fee for the swap.
        let inputs_total_needed = inputs_needed_amount + target_fee;

        // Recalculate target amounts based on the actual total we need (including fee)
        let target_amounts = inputs_total_needed.split(&fee_and_amounts);
        let input_proofs = Wallet::select_proofs(
            inputs_total_needed,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees_and_amounts,
            true,
        )?;
        let proofs_total = input_proofs.total_amount()?;

        // Need to swap to get exact denominations
        tracing::debug!(
            "Proofs total {} != inputs needed {}, swapping to get exact amount",
            proofs_total,
            inputs_total_needed
        );

        let keyset_fees: HashMap<cdk_common::Id, u64> = keyset_fees_and_amounts
            .iter()
            .map(|(key, values)| (*key, values.fee()))
            .collect();

        let split_result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            inputs_total_needed,
            target_fee,
            &keyset_fees,
            false,
            false,
        )?;

        let mut final_proofs = split_result.proofs_to_send;

        if !split_result.proofs_to_swap.is_empty() {
            let swap_amount = inputs_total_needed
                .checked_sub(final_proofs.total_amount()?)
                .ok_or(Error::AmountOverflow)?;

            tracing::debug!(
                "Swapping {} proofs to get {} sats (swap fee: {} sats)",
                split_result.proofs_to_swap.len(),
                swap_amount,
                split_result.swap_fee
            );

            if let Some(swapped) = self
                .try_proof_operation_or_reclaim(
                    split_result.proofs_to_swap.clone(),
                    self.swap(
                        Some(swap_amount),
                        SplitTarget::None,
                        split_result.proofs_to_swap,
                        None,
                        false, // fees already accounted for in inputs_total_needed
                    ),
                )
                .await?
            {
                final_proofs.extend(swapped);
            }
        }

        self.melt_proofs_with_metadata(quote_id, final_proofs, metadata)
            .await
    }
}
