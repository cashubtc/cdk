use crate::{Mint, Error};
use anyhow::bail;
use cashu_kvac::{secp::{GroupElement, TweakKind}, transcript::CashuTranscript};
use cdk_common::{
    amount::to_unit,
    common::LnKey,
    kvac::{
        KvacIssuedMac,
        KvacMeltBolt11Request,
        KvacMeltBolt11Response,
        KvacNullifier,
        KvacRandomizedCoin
    },
    lightning::{
        MintLightning,
        PayInvoiceResponse
    },
    mint::MeltQuote,
    Amount,
    MeltQuoteState,
    MintQuoteState,
    PaymentMethod,
    State
};
use tracing::instrument;
use uuid::Uuid;

impl Mint {

    /// Process unpaid melt request
    /// In the event that a melt request fails and the lighthing payment is not
    /// made The [`KvacCoin`]s should be returned to an unspent state and the
    /// quote should be unpaid
    #[instrument(skip_all)]
    pub async fn process_unpaid_kvac_melt(
        &self,
        inputs: &Vec<KvacRandomizedCoin>,
        quote_id: &Uuid,
    ) -> Result<(), Error> {
        // Collect nullifiers
        let nullifiers = inputs
            .iter()
            .map(|i| KvacNullifier::from(i).nullifier)
            .collect::<Vec<GroupElement>>();

        self.localstore
            .update_kvac_nullifiers_states(&nullifiers, State::Unspent)
            .await?;

        self.localstore
            .update_melt_quote_state(quote_id, MeltQuoteState::Unpaid)
            .await?;

        if let Ok(Some(quote)) = self.localstore.get_melt_quote(quote_id).await {
            self.pubsub_manager
                .melt_quote_status(&quote, None, None, MeltQuoteState::Unpaid);
        }

        // TODO: pubsub manager for nullifiers states
        /*
        for public_key in input_ys {
            self.pubsub_manager
                .proof_state((public_key, State::Unspent));
        }
        */

        Ok(())
    }

    /// Verify melt request is valid
    /// Check to see if there is a corresponding mint quote for a melt.
    /// In this case the mint can settle the payment internally and no ln payment is
    /// needed
    #[instrument(skip_all)]
    pub async fn handle_internal_kvac_melt_mint(
        &self,
        melt_quote: &MeltQuote,
    ) -> Result<Option<Amount>, Error> {
        let mint_quote = match self
            .localstore
            .get_mint_quote_by_request(&melt_quote.request)
            .await
        {
            Ok(Some(mint_quote)) => mint_quote,
            // Not an internal melt -> mint
            Ok(None) => return Ok(None),
            Err(err) => {
                tracing::debug!("Error attempting to get mint quote: {}", err);
                return Err(Error::Internal);
            }
        };

        // Mint quote has already been settled, proofs should not be burned or held.
        if mint_quote.state == MintQuoteState::Issued || mint_quote.state == MintQuoteState::Paid {
            return Err(Error::RequestAlreadyPaid);
        }

        let mut mint_quote = mint_quote;

        if mint_quote.amount > melt_quote.amount {
            tracing::debug!(
                "Melt quote is not enough to cover mint quote: {} needed {}",
                melt_quote.amount,
                mint_quote.amount
            );
            return Err(Error::InsufficientFunds);
        }

        mint_quote.state = MintQuoteState::Paid;

        let amount = melt_quote.amount;

        self.update_mint_quote(mint_quote).await?;

        Ok(Some(amount))
    }

    /// Process mint request
    #[instrument(skip_all)]
    pub async fn process_kvac_melt_request(
        &self,
        melt_request: KvacMeltBolt11Request<Uuid>,
    ) -> Result<KvacMeltBolt11Response, Error> {

        // Define helper to check outgoing payments
        use std::sync::Arc;
        async fn check_payment_state(
            ln: Arc<dyn MintLightning<Err = crate::cdk_lightning::Error> + Send + Sync>,
            melt_quote: &MeltQuote,
        ) -> anyhow::Result<PayInvoiceResponse> {
            match ln
                .check_outgoing_payment(&melt_quote.request_lookup_id)
                .await
            {
                Ok(response) => Ok(response),
                Err(check_err) => {
                    // If we cannot check the status of the payment we keep the proofs stuck as pending.
                    tracing::error!(
                        "Could not check the status of payment for {},. Proofs stuck as pending",
                        melt_quote.id
                    );
                    tracing::error!("Checking payment error: {}", check_err);
                    bail!("Could not check payment status")
                }
            }
        }

        tracing::info!("KVAC melt has been called");

        // Get the melt quote
        let melt_quote =
            if let Some(mint_quote) = self.localstore.get_melt_quote(&melt_request.quote).await? {
                mint_quote
            } else {
                return Err(Error::UnknownQuote);
            };
        
        // Get the previous state of the melt quote
        // while simoultaneously setting it to PENDING
        let state = self
            .localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Pending)
            .await?;

        // Check the previous state
        match state {
            MeltQuoteState::Pending => {
                return Err(Error::PendingQuote);
            },
            MeltQuoteState::Paid => {
                self
                    .localstore
                    .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Paid)
                    .await?;
                return Err(Error::PaidQuote)
            },
            MeltQuoteState::Unknown => {
                self
                    .localstore
                    .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Unknown)
                    .await?;
                return Err(Error::UnknownQuote)
            },
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => ()
        };

        // Peg-out should be the quote amount + lightning fee reserve.
        // "swap" fees are calculated inside verify_kvac_request
        let peg_out = i64::try_from(melt_quote.amount + melt_quote.fee_reserve)?;

        // Process the request
        if let Err(e) = self.verify_kvac_request(
            true, 
            peg_out, 
            &melt_request.inputs,
            &melt_request.outputs,
            melt_request.balance_proof,
            melt_request.mac_proofs,
            melt_request.script,
            melt_request.range_proof,
        ).await {
            tracing::error!("KVAC verification failed");
            self.process_unpaid_kvac_melt(&melt_request.inputs, &melt_request.quote).await?;
            return Err(e);
        }

        let settled_internally_amount =
            match self.handle_internal_kvac_melt_mint(&melt_quote).await {
                Ok(amount) => amount,
                Err(err) => {
                    tracing::error!("Attempting to settle internally failed");
                    if let Err(err) = self.process_unpaid_kvac_melt(&melt_request.inputs, &melt_request.quote).await {
                        tracing::error!(
                            "Could not reset melt quote {} state: {}",
                            melt_request.quote,
                            err
                        );
                    }
                    return Err(err);
                }
            };
            
        let quote = melt_quote;
        let (preimage, amount_spent_quote_unit) = match settled_internally_amount {
            Some(amount_spent) => (None, amount_spent),
            None => {
                /* TODO: partial amounts
                // If the quote unit is SAT or MSAT we can check that the expected fees are
                // provided. We also check if the quote is less then the invoice
                // amount in the case that it is a mmp However, if the quote is not
                // of a bitcoin unit we cannot do these checks as the mint
                // is unaware of a conversion rate. In this case it is assumed that the quote is
                // correct and the mint should pay the full invoice amount if inputs
                // > `then quote.amount` are included. This is checked in the
                // `verify_melt` method.
                let partial_amount = match quote.unit {
                    CurrencyUnit::Sat | CurrencyUnit::Msat => {
                        match self.check_melt_expected_ln_fees(&quote, melt_request).await {
                            Ok(amount) => amount,
                            Err(err) => {
                                tracing::error!("Fee is not expected: {}", err);
                                if let Err(err) = self.process_unpaid_melt(melt_request).await {
                                    tracing::error!("Could not reset melt quote state: {}", err);
                                }
                                return Err(Error::Internal);
                            }
                        }
                    }
                    _ => None,
                };
                */
                let ln = match self
                    .ln
                    .get(&LnKey::new(quote.unit.clone(), PaymentMethod::Bolt11))
                {
                    Some(ln) => ln,
                    None => {
                        tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                        if let Err(err) = self.process_unpaid_kvac_melt(
                            &melt_request.inputs,
                            &melt_request.quote
                        ).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }

                        return Err(Error::UnsupportedUnit);
                    }
                };

                let pre = match ln
                    .pay_invoice(quote.clone(), None, Some(quote.fee_reserve))
                    .await
                {
                    Ok(pay)
                        if pay.status == MeltQuoteState::Unknown
                            || pay.status == MeltQuoteState::Failed =>
                    {
                        let check_response = check_payment_state(Arc::clone(ln), &quote)
                            .await
                            .map_err(|_| Error::Internal)?;

                        if check_response.status == MeltQuoteState::Paid {
                            tracing::warn!("Pay invoice returned {} but check returned {}. Proofs stuck as pending", pay.status.to_string(), check_response.status.to_string());

                            return Err(Error::Internal);
                        }

                        check_response
                    }
                    Ok(pay) => pay,
                    Err(err) => {
                        // If the error is that the invoice was already paid we do not want to hold
                        // hold the proofs as pending to we reset them  and return an error.
                        if matches!(err, crate::cdk_lightning::Error::InvoiceAlreadyPaid) {
                            tracing::debug!("Invoice already paid, resetting melt quote");
                            if let Err(err) = self.process_unpaid_kvac_melt(
                                &melt_request.inputs,
                                &melt_request.quote,
                            ).await {
                                tracing::error!("Could not reset melt quote state: {}", err);
                            }
                            return Err(Error::RequestAlreadyPaid);
                        }

                        tracing::error!("Error returned attempting to pay: {} {}", quote.id, err);

                        let check_response = check_payment_state(Arc::clone(ln), &quote)
                            .await
                            .map_err(|_| Error::Internal)?;
                        // If there error is something else we want to check the status of the payment ensure it is not pending or has been made.
                        if check_response.status == MeltQuoteState::Paid {
                            tracing::warn!("Pay invoice returned an error but check returned {}. Proofs stuck as pending", check_response.status.to_string());

                            return Err(Error::Internal);
                        }
                        check_response
                    }
                };

                match pre.status {
                    MeltQuoteState::Paid => (),
                    MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => {
                        tracing::info!(
                            "Lightning payment for quote {} failed.",
                            melt_request.quote
                        );
                        if let Err(err) = self.process_unpaid_kvac_melt(
                            &melt_request.inputs,
                            &melt_request.quote
                        ).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }
                        return Err(Error::PaymentFailed);
                    }
                    MeltQuoteState::Pending => {
                        tracing::warn!(
                            "LN payment pending, proofs are stuck as pending for quote: {}",
                            melt_request.quote
                        );
                        return Err(Error::PendingQuote);
                    }
                }

                // Convert from unit of backend to quote unit
                // Note: this should never fail since these conversions happen earlier and would fail there.
                // Since it will not fail and even if it does the ln payment has already been paid, proofs should still be burned
                let amount_spent =
                    to_unit(pre.total_spent, &pre.unit, &quote.unit).unwrap_or_default();

                let payment_lookup_id = pre.payment_lookup_id;

                if payment_lookup_id != quote.request_lookup_id {
                    tracing::info!(
                        "Payment lookup id changed post payment from {} to {}",
                        quote.request_lookup_id,
                        payment_lookup_id
                    );

                    let mut melt_quote = quote.clone();
                    melt_quote.request_lookup_id = payment_lookup_id;

                    if let Err(err) = self.localstore.add_melt_quote(melt_quote).await {
                        tracing::warn!("Could not update payment lookup id: {}", err);
                    }
                }

                (pre.payment_preimage, amount_spent)
            }
        };
        
        // If we made thus far the payment has been bade

        // This should never underflow
        let amount_overpaid = (quote.amount + quote.fee_reserve) - amount_spent_quote_unit;
        let mut outputs = melt_request.outputs;

        tracing::debug!("amount_overpaid: {:?}", amount_overpaid);

        if amount_overpaid > Amount::from(0) {
            // Do this to check underflow anyway
            let overpaid = i64::try_from(amount_overpaid)?;

            // Ma' = Ma + o*G_amount
            outputs
                .get_mut(0)
                .expect("outputs have length == 2")
                .commitments.0
                .tweak(TweakKind::AMOUNT, overpaid as u64);
        }

        // Collect nullifiers
        let nullifiers = melt_request.inputs
            .iter()
            .map(|i| KvacNullifier::from(i).nullifier)
            .collect::<Vec<GroupElement>>();

        // Issue MACs
        let mut issued_macs = vec![];
        let mut iparams_proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for output in outputs.iter() {
            let result = self.issue_mac(output, &mut proving_transcript).await;
            // Set nullifiers unspent in case of error
            match result {
                Err(e) => {
                    self.process_unpaid_kvac_melt(
                        &melt_request.inputs,
                        &melt_request.quote,
                    ).await?;
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
        self.localstore.update_melt_quote_state(
            &melt_request.quote,
            MeltQuoteState::Paid
        ).await?;

        // Notify pubsub manager
        if let Ok(Some(quote)) = self.localstore.get_melt_quote(&melt_request.quote).await {
            self.pubsub_manager
                .melt_quote_status(&quote, preimage.clone(), None, MeltQuoteState::Paid);
        }

        tracing::debug!("KVAC melt request successful");

        Ok(KvacMeltBolt11Response {
            fee_return: amount_overpaid,
            preimage,
            macs: issued_macs.into_iter().map(|m| m.mac).collect(),
            proofs: iparams_proofs,
        })
    }   
}