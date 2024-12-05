use std::collections::HashSet;
use std::str::FromStr;

use anyhow::bail;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;
use uuid::Uuid;

use super::{
    CurrencyUnit, MeltBolt11Request, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    Mint, PaymentMethod, PublicKey, State,
};
use crate::amount::to_unit;
use crate::cdk_lightning::{MintLightning, PayInvoiceResponse};
use crate::mint::SigFlag;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut11::{enforce_sig_flag, EnforceSigFlag};
use crate::nuts::{Id, MeltQuoteState};
use crate::types::LnKey;
use crate::util::unix_time;
use crate::{cdk_lightning, Amount, Error};

impl Mint {
    fn check_melt_request_acceptable(
        &self,
        amount: Amount,
        unit: CurrencyUnit,
        method: PaymentMethod,
    ) -> Result<(), Error> {
        let nut05 = &self.mint_info.nuts.nut05;

        if nut05.disabled {
            return Err(Error::MeltingDisabled);
        }

        let settings = nut05
            .get_settings(&unit, &method)
            .ok_or(Error::UnitUnsupported)?;

        let is_above_max = matches!(settings.max_amount, Some(max) if amount > max);
        let is_below_min = matches!(settings.min_amount, Some(min) if amount < min);
        match is_above_max || is_below_min {
            true => Err(Error::AmountOutofLimitRange(
                settings.min_amount.unwrap_or_default(),
                settings.max_amount.unwrap_or_default(),
                amount,
            )),
            false => Ok(()),
        }
    }

    /// Get melt bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_melt_bolt11_quote(
        &self,
        melt_request: &MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<Uuid>, Error> {
        let MeltQuoteBolt11Request {
            request,
            unit,
            options: _,
        } = melt_request;

        let amount = match melt_request.options {
            Some(mpp_amount) => mpp_amount.amount,
            None => {
                let amount_msat = request
                    .amount_milli_satoshis()
                    .ok_or(Error::InvoiceAmountUndefined)?;

                to_unit(amount_msat, &CurrencyUnit::Msat, unit)
                    .map_err(|_err| Error::UnsupportedUnit)?
            }
        };

        self.check_melt_request_acceptable(amount, unit.clone(), PaymentMethod::Bolt11)?;

        let ln = self
            .ln
            .get(&LnKey::new(unit.clone(), PaymentMethod::Bolt11))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt11 ", unit);

                Error::UnitUnsupported
            })?;

        let payment_quote = ln.get_payment_quote(melt_request).await.map_err(|err| {
            tracing::error!(
                "Could not get payment quote for mint quote, {} bolt11, {}",
                unit,
                err
            );

            Error::UnitUnsupported
        })?;

        let quote = MeltQuote::new(
            request.to_string(),
            unit.clone(),
            payment_quote.amount,
            payment_quote.fee,
            unix_time() + self.quote_ttl.melt_ttl,
            payment_quote.request_lookup_id.clone(),
        );

        tracing::debug!(
            "New melt quote {} for {} {} with request id {}",
            quote.id,
            amount,
            unit,
            payment_quote.request_lookup_id
        );

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote.into())
    }

    /// Check melt quote status
    #[instrument(skip(self))]
    pub async fn check_melt_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<MeltQuoteBolt11Response<Uuid>, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let blind_signatures = self
            .localstore
            .get_blind_signatures_for_quote(quote_id)
            .await?;

        let change = (!blind_signatures.is_empty()).then_some(blind_signatures);

        Ok(MeltQuoteBolt11Response {
            quote: quote.id,
            paid: Some(quote.state == MeltQuoteState::Paid),
            state: quote.state,
            expiry: quote.expiry,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
            payment_preimage: quote.payment_preimage,
            change,
        })
    }

    /// Update melt quote
    #[instrument(skip_all)]
    pub async fn update_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.localstore.add_melt_quote(quote).await?;
        Ok(())
    }

    /// Get melt quotes
    #[instrument(skip_all)]
    pub async fn melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes)
    }

    /// Remove melt quote
    #[instrument(skip(self))]
    pub async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Error> {
        self.localstore.remove_melt_quote(quote_id).await?;

        Ok(())
    }

    /// Check melt has expected fees
    #[instrument(skip_all)]
    pub async fn check_melt_expected_ln_fees(
        &self,
        melt_quote: &MeltQuote,
        melt_request: &MeltBolt11Request<Uuid>,
    ) -> Result<Option<Amount>, Error> {
        let invoice = Bolt11Invoice::from_str(&melt_quote.request)?;

        let quote_msats = to_unit(melt_quote.amount, &melt_quote.unit, &CurrencyUnit::Msat)
            .expect("Quote unit is checked above that it can convert to msat");

        let invoice_amount_msats: Amount = invoice
            .amount_milli_satoshis()
            .ok_or(Error::InvoiceAmountUndefined)?
            .into();

        let partial_amount = match invoice_amount_msats > quote_msats {
            true => {
                let partial_msats = invoice_amount_msats - quote_msats;

                Some(
                    to_unit(partial_msats, &CurrencyUnit::Msat, &melt_quote.unit)
                        .map_err(|_| Error::UnitUnsupported)?,
                )
            }
            false => None,
        };

        let amount_to_pay = match partial_amount {
            Some(amount_to_pay) => amount_to_pay,
            None => to_unit(invoice_amount_msats, &CurrencyUnit::Msat, &melt_quote.unit)
                .map_err(|_| Error::UnitUnsupported)?,
        };

        let inputs_amount_quote_unit = melt_request.proofs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        if amount_to_pay + melt_quote.fee_reserve > inputs_amount_quote_unit {
            tracing::debug!(
                "Not enough inputs provided: {} msats needed {} msats",
                inputs_amount_quote_unit,
                amount_to_pay
            );

            return Err(Error::TransactionUnbalanced(
                inputs_amount_quote_unit.into(),
                amount_to_pay.into(),
                melt_quote.fee_reserve.into(),
            ));
        }

        Ok(partial_amount)
    }

    /// Verify melt request is valid
    #[instrument(skip_all)]
    pub async fn verify_melt_request(
        &self,
        melt_request: &MeltBolt11Request<Uuid>,
    ) -> Result<MeltQuote, Error> {
        let state = self
            .localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Pending)
            .await?;

        match state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => Ok(()),
            MeltQuoteState::Pending => Err(Error::PendingQuote),
            MeltQuoteState::Paid => Err(Error::PaidQuote),
            MeltQuoteState::Unknown => Err(Error::UnknownPaymentState),
        }?;

        let ys = melt_request.inputs.ys()?;

        // Ensure proofs are unique and not being double spent
        if melt_request.inputs.len() != ys.iter().collect::<HashSet<_>>().len() {
            return Err(Error::DuplicateProofs);
        }

        self.localstore
            .add_proofs(melt_request.inputs.clone(), Some(melt_request.quote))
            .await?;
        self.check_ys_spendable(&ys, State::Pending).await?;

        for proof in &melt_request.inputs {
            self.verify_proof(proof).await?;
        }

        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let proofs_total = melt_request.proofs_amount()?;

        let fee = self.get_proofs_fee(&melt_request.inputs).await?;

        let required_total = quote.amount + quote.fee_reserve + fee;

        // Check that the inputs proofs are greater then total.
        // Transaction does not need to be balanced as wallet may not want change.
        if proofs_total < required_total {
            tracing::info!(
                "Swap request unbalanced: {}, outputs {}, fee {}",
                proofs_total,
                quote.amount,
                fee
            );
            return Err(Error::TransactionUnbalanced(
                proofs_total.into(),
                quote.amount.into(),
                (fee + quote.fee_reserve).into(),
            ));
        }

        let input_keyset_ids: HashSet<Id> =
            melt_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(melt_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            return Err(Error::SigAllUsedInMelt);
        }

        if let Some(outputs) = &melt_request.outputs {
            let output_keysets_ids: HashSet<Id> = outputs.iter().map(|b| b.keyset_id).collect();
            for id in output_keysets_ids {
                let keyset = self
                    .localstore
                    .get_keyset_info(&id)
                    .await?
                    .ok_or(Error::UnknownKeySet)?;

                // Get the active keyset for the unit
                let active_keyset_id = self
                    .localstore
                    .get_active_keyset_id(&keyset.unit)
                    .await?
                    .ok_or(Error::InactiveKeyset)?;

                // Check output is for current active keyset
                if id.ne(&active_keyset_id) {
                    return Err(Error::InactiveKeyset);
                }
                keyset_units.insert(keyset.unit);
            }
        }

        // Check that all input and output proofs are the same unit
        if keyset_units.len().gt(&1) {
            return Err(Error::MultipleUnits);
        }

        tracing::debug!("Verified melt quote: {}", melt_request.quote);
        Ok(quote)
    }

    /// Process unpaid melt request
    /// In the event that a melt request fails and the lighthing payment is not
    /// made The [`Proofs`] should be returned to an unspent state and the
    /// quote should be unpaid
    #[instrument(skip_all)]
    pub async fn process_unpaid_melt(
        &self,
        melt_request: &MeltBolt11Request<Uuid>,
    ) -> Result<(), Error> {
        let input_ys = melt_request.inputs.ys()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Unspent)
            .await?;

        self.localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Unpaid)
            .await?;

        if let Ok(Some(quote)) = self.localstore.get_melt_quote(&melt_request.quote).await {
            self.pubsub_manager
                .melt_quote_status(&quote, None, None, MeltQuoteState::Unpaid);
        }

        for public_key in input_ys {
            self.pubsub_manager
                .proof_state((public_key, State::Unspent));
        }

        Ok(())
    }

    /// Melt Bolt11
    #[instrument(skip_all)]
    pub async fn melt_bolt11(
        &self,
        melt_request: &MeltBolt11Request<Uuid>,
    ) -> Result<MeltQuoteBolt11Response<Uuid>, Error> {
        use std::sync::Arc;
        async fn check_payment_state(
            ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
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

        let quote = match self.verify_melt_request(melt_request).await {
            Ok(quote) => quote,
            Err(err) => {
                tracing::debug!("Error attempting to verify melt quote: {}", err);

                if let Err(err) = self.process_unpaid_melt(melt_request).await {
                    tracing::error!(
                        "Could not reset melt quote {} state: {}",
                        melt_request.quote,
                        err
                    );
                }
                return Err(err);
            }
        };

        let settled_internally_amount =
            match self.handle_internal_melt_mint(&quote, melt_request).await {
                Ok(amount) => amount,
                Err(err) => {
                    tracing::error!("Attempting to settle internally failed");
                    if let Err(err) = self.process_unpaid_melt(melt_request).await {
                        tracing::error!(
                            "Could not reset melt quote {} state: {}",
                            melt_request.quote,
                            err
                        );
                    }
                    return Err(err);
                }
            };

        let (preimage, amount_spent_quote_unit) = match settled_internally_amount {
            Some(amount_spent) => (None, amount_spent),
            None => {
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
                let ln = match self
                    .ln
                    .get(&LnKey::new(quote.unit.clone(), PaymentMethod::Bolt11))
                {
                    Some(ln) => ln,
                    None => {
                        tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                        if let Err(err) = self.process_unpaid_melt(melt_request).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }

                        return Err(Error::UnitUnsupported);
                    }
                };

                let pre = match ln
                    .pay_invoice(quote.clone(), partial_amount, Some(quote.fee_reserve))
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
                        if matches!(err, cdk_lightning::Error::InvoiceAlreadyPaid) {
                            tracing::debug!("Invoice already paid, resetting melt quote");
                            if let Err(err) = self.process_unpaid_melt(melt_request).await {
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
                        if let Err(err) = self.process_unpaid_melt(melt_request).await {
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

                    let mut melt_quote = quote;
                    melt_quote.request_lookup_id = payment_lookup_id;

                    if let Err(err) = self.localstore.add_melt_quote(melt_quote).await {
                        tracing::warn!("Could not update payment lookup id: {}", err);
                    }
                }

                (pre.payment_preimage, amount_spent)
            }
        };

        // If we made it here the payment has been made.
        // We process the melt burning the inputs and returning change
        let res = self
            .process_melt_request(melt_request, preimage, amount_spent_quote_unit)
            .await
            .map_err(|err| {
                tracing::error!("Could not process melt request: {}", err);
                err
            })?;

        Ok(res)
    }

    /// Process melt request marking [`Proofs`] as spent
    /// The melt request must be verifyed using [`Self::verify_melt_request`]
    /// before calling [`Self::process_melt_request`]
    #[instrument(skip_all)]
    pub async fn process_melt_request(
        &self,
        melt_request: &MeltBolt11Request<Uuid>,
        payment_preimage: Option<String>,
        total_spent: Amount,
    ) -> Result<MeltQuoteBolt11Response<Uuid>, Error> {
        tracing::debug!("Processing melt quote: {}", melt_request.quote);

        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let input_ys = melt_request.inputs.ys()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        self.localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Paid)
            .await?;

        self.pubsub_manager.melt_quote_status(
            &quote,
            payment_preimage.clone(),
            None,
            MeltQuoteState::Paid,
        );

        for public_key in input_ys {
            self.pubsub_manager.proof_state((public_key, State::Spent));
        }

        let mut change = None;

        // Check if there is change to return
        if melt_request.proofs_amount()? > total_spent {
            // Check if wallet provided change outputs
            if let Some(outputs) = melt_request.outputs.clone() {
                let blinded_messages: Vec<PublicKey> =
                    outputs.iter().map(|b| b.blinded_secret).collect();

                if self
                    .localstore
                    .get_blind_signatures(&blinded_messages)
                    .await?
                    .iter()
                    .flatten()
                    .next()
                    .is_some()
                {
                    tracing::info!("Output has already been signed");

                    return Err(Error::BlindedMessageAlreadySigned);
                }

                let change_target = melt_request.proofs_amount()? - total_spent;
                let mut amounts = change_target.split();
                let mut change_sigs = Vec::with_capacity(amounts.len());

                if outputs.len().lt(&amounts.len()) {
                    tracing::debug!(
                        "Providing change requires {} blinded messages, but only {} provided",
                        amounts.len(),
                        outputs.len()
                    );

                    // In the case that not enough outputs are provided to return all change
                    // Reverse sort the amounts so that the most amount of change possible is
                    // returned. The rest is burnt
                    amounts.sort_by(|a, b| b.cmp(a));
                }

                let mut outputs = outputs;

                for (amount, blinded_message) in amounts.iter().zip(&mut outputs) {
                    blinded_message.amount = *amount;

                    let blinded_signature = self.blind_sign(blinded_message).await?;
                    change_sigs.push(blinded_signature)
                }

                self.localstore
                    .add_blind_signatures(
                        &outputs[0..change_sigs.len()]
                            .iter()
                            .map(|o| o.blinded_secret)
                            .collect::<Vec<PublicKey>>(),
                        &change_sigs,
                        Some(quote.id),
                    )
                    .await?;

                change = Some(change_sigs);
            }
        }

        Ok(MeltQuoteBolt11Response {
            amount: quote.amount,
            paid: Some(true),
            payment_preimage,
            change,
            quote: quote.id,
            fee_reserve: quote.fee_reserve,
            state: MeltQuoteState::Paid,
            expiry: quote.expiry,
        })
    }
}
