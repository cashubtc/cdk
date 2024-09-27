use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::bail;
use lightning::offers::offer::Offer;
use tracing::instrument;

use crate::cdk_lightning;
use crate::cdk_lightning::MintLightning;
use crate::cdk_lightning::PayInvoiceResponse;
use crate::dhke::hash_to_curve;
use crate::nuts::nut11::enforce_sig_flag;
use crate::nuts::nut11::EnforceSigFlag;
use crate::{
    amount::to_unit, mint::SigFlag, nuts::Id, nuts::MeltQuoteState, types::LnKey, util::unix_time,
    Amount, Error,
};

use super::nut05::MeltRequestTrait;
use super::BlindSignature;
use super::MeltQuoteBolt12Request;
use super::{
    CurrencyUnit, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response, Mint, PaymentMethod,
    PaymentRequest, PublicKey, State,
};

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

        match nut05.get_settings(&unit, &method) {
            Some(settings) => {
                if settings
                    .max_amount
                    .map_or(false, |max_amount| amount > max_amount)
                {
                    return Err(Error::AmountOutofLimitRange(
                        settings.min_amount.unwrap_or_default(),
                        settings.max_amount.unwrap_or_default(),
                        amount,
                    ));
                }

                if settings
                    .min_amount
                    .map_or(false, |min_amount| amount < min_amount)
                {
                    return Err(Error::AmountOutofLimitRange(
                        settings.min_amount.unwrap_or_default(),
                        settings.max_amount.unwrap_or_default(),
                        amount,
                    ));
                }
            }
            None => {
                return Err(Error::UnitUnsupported);
            }
        }

        Ok(())
    }

    /// Get melt bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_melt_bolt11_quote(
        &self,
        melt_request: &MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response, Error> {
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

        self.check_melt_request_acceptable(amount, *unit, PaymentMethod::Bolt11)?;

        let ln = self
            .ln
            .get(&LnKey::new(*unit, PaymentMethod::Bolt11))
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

        let request = PaymentRequest::Bolt11 {
            bolt11: request.clone(),
        };

        let quote = MeltQuote::new(
            request,
            *unit,
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

    /// Get melt bolt12 quote
    #[instrument(skip_all)]
    pub async fn get_melt_bolt12_quote(
        &self,
        melt_request: &MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let MeltQuoteBolt12Request {
            request,
            unit,
            amount,
        } = melt_request;

        let offer = Offer::from_str(request).unwrap();

        let amount = match amount {
            Some(amount) => *amount,
            None => {
                let offer_amount = offer.amount().ok_or(Error::InvoiceAmountUndefined)?;

                let (amount, currency) = match offer_amount {
                    lightning::offers::offer::Amount::Bitcoin { amount_msats } => {
                        (amount_msats, CurrencyUnit::Msat)
                    }
                    lightning::offers::offer::Amount::Currency {
                        iso4217_code,
                        amount,
                    } => (
                        amount,
                        CurrencyUnit::from_str(&String::from_utf8(iso4217_code.to_vec())?)?,
                    ),
                };

                to_unit(amount, &currency, unit).map_err(|_err| Error::UnsupportedUnit)?
            }
        };

        self.check_melt_request_acceptable(amount, *unit, PaymentMethod::Bolt12)?;

        let ln = self
            .ln
            .get(&LnKey::new(*unit, PaymentMethod::Bolt12))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt11 ", unit);

                Error::UnitUnsupported
            })?;

        let payment_quote = ln
            .get_bolt12_payment_quote(melt_request)
            .await
            .map_err(|err| {
                tracing::error!(
                    "Could not get payment quote for mint quote, {} bolt11, {}",
                    unit,
                    err
                );

                Error::UnitUnsupported
            })?;

        let offer = Offer::from_str(request)?;

        let payment_request = PaymentRequest::Bolt12 {
            offer: Box::new(offer),
            invoice: None,
        };

        let quote = MeltQuote::new(
            payment_request,
            *unit,
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
    pub async fn check_melt_quote(&self, quote_id: &str) -> Result<MeltQuoteBolt11Response, Error> {
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
    pub async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_melt_quote(quote_id).await?;

        Ok(())
    }

    /// Check melt has expected fees
    #[instrument(skip_all)]
    pub async fn check_melt_expected_ln_fees<R>(
        &self,
        melt_quote: &MeltQuote,
        melt_request: &R,
    ) -> Result<Option<Amount>, Error>
    where
        R: MeltRequestTrait,
    {
        let quote_amount = melt_quote.amount;

        let request_amount = match &melt_quote.request {
            PaymentRequest::Bolt11 { bolt11 } => match bolt11.amount_milli_satoshis() {
                Some(amount) => Some(
                    to_unit(amount, &CurrencyUnit::Msat, &melt_quote.unit)
                        .map_err(|_| Error::UnitUnsupported)?,
                ),
                None => None,
            },
            PaymentRequest::Bolt12 { offer, invoice: _ } => match offer.amount() {
                Some(amount) => {
                    let (amount, currency) = match amount {
                        lightning::offers::offer::Amount::Bitcoin { amount_msats } => {
                            (amount_msats, CurrencyUnit::Msat)
                        }
                        lightning::offers::offer::Amount::Currency {
                            iso4217_code,
                            amount,
                        } => (
                            amount,
                            CurrencyUnit::from_str(&String::from_utf8(iso4217_code.to_vec())?)?,
                        ),
                    };

                    Some(
                        to_unit(amount, &currency, &melt_quote.unit)
                            .map_err(|_err| Error::UnsupportedUnit)?,
                    )
                }
                None => None,
            },
        };

        let amount_to_pay = request_amount.unwrap_or(quote_amount);

        let inputs_amount = melt_request
            .inputs_amount()
            .map_err(|_| Error::AmountOverflow)?;

        if amount_to_pay + melt_quote.fee_reserve > inputs_amount {
            tracing::debug!(
                "Not enough inputs provided: {} msats needed {} msats",
                inputs_amount,
                amount_to_pay
            );

            return Err(Error::TransactionUnbalanced(
                inputs_amount.into(),
                amount_to_pay.into(),
                melt_quote.fee_reserve.into(),
            ));
        }

        Ok(Some(amount_to_pay))
    }

    /// Verify melt request is valid
    #[instrument(skip_all)]
    pub async fn verify_melt_request<R>(&self, melt_request: &R) -> Result<MeltQuote, Error>
    where
        R: MeltRequestTrait,
    {
        let quote_id = melt_request.get_quote_id();
        let state = self
            .localstore
            .update_melt_quote_state(quote_id, MeltQuoteState::Pending)
            .await?;

        match state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => (),
            MeltQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MeltQuoteState::Paid => {
                return Err(Error::PaidQuote);
            }
            MeltQuoteState::Unknown => {
                return Err(Error::UnknownPaymentState);
            }
        }

        let inputs = melt_request.get_inputs();

        let ys = inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        // Ensure proofs are unique and not being double spent
        if inputs.len() != ys.iter().collect::<HashSet<_>>().len() {
            return Err(Error::DuplicateProofs);
        }

        self.localstore
            .add_proofs(inputs.clone(), Some(quote_id.to_string()))
            .await?;
        self.check_ys_spendable(&ys, State::Pending).await?;

        for proof in inputs.iter() {
            self.verify_proof(proof).await?;
        }

        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let proofs_total = Amount::try_sum(inputs.iter().map(|p| p.amount))?;

        let fee = self.get_proofs_fee(inputs).await?;

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

        let input_keyset_ids: HashSet<Id> = inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            return Err(Error::SigAllUsedInMelt);
        }

        let outputs = melt_request.get_outputs();

        if let Some(outputs) = outputs {
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

        tracing::debug!("Verified melt quote: {}", quote_id);
        Ok(quote)
    }

    /// Process unpaid melt request
    /// In the event that a melt request fails and the lighthing payment is not
    /// made The [`Proofs`] should be returned to an unspent state and the
    /// quote should be unpaid
    #[instrument(skip_all)]
    pub async fn process_unpaid_melt<R>(&self, melt_request: &R) -> Result<(), Error>
    where
        R: MeltRequestTrait,
    {
        let inputs = melt_request.get_inputs();
        let input_ys = inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Unspent)
            .await?;

        self.localstore
            .update_melt_quote_state(melt_request.get_quote_id(), MeltQuoteState::Unpaid)
            .await?;

        Ok(())
    }

    async fn check_payment_state(
        ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        request_lookup_id: &str,
    ) -> anyhow::Result<PayInvoiceResponse> {
        match ln.check_outgoing_payment(request_lookup_id).await {
            Ok(response) => Ok(response),
            Err(check_err) => {
                // If we cannot check the status of the payment we keep the proofs stuck as pending.
                tracing::error!(
                    "Could not check the status of payment for {},. Proofs stuck as pending",
                    request_lookup_id
                );
                tracing::error!("Checking payment error: {}", check_err);
                bail!("Could not check payment status")
            }
        }
    }

    /// Melt Bolt11
    #[instrument(skip_all)]
    pub async fn melt<R>(&self, melt_request: &R) -> Result<MeltQuoteBolt11Response, Error>
    where
        R: MeltRequestTrait,
    {
        let quote = match self.verify_melt_request(melt_request).await {
            Ok(quote) => quote,
            Err(err) => {
                tracing::debug!("Error attempting to verify melt quote: {}", err);

                if let Err(err) = self.process_unpaid_melt(melt_request).await {
                    tracing::error!(
                        "Could not reset melt quote {} state: {}",
                        melt_request.get_quote_id(),
                        err
                    );
                }
                return Err(err);
            }
        };

        let inputs_amount = melt_request
            .inputs_amount()
            .map_err(|_err| Error::AmountOverflow)?;

        let settled_internally_amount =
            match self.handle_internal_melt_mint(&quote, inputs_amount).await {
                Ok(amount) => amount,
                Err(err) => {
                    tracing::error!("Attempting to settle internally failed");
                    if let Err(err) = self.process_unpaid_melt(melt_request).await {
                        tracing::error!(
                            "Could not reset melt quote {} state: {}",
                            melt_request.get_quote_id(),
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

                let ln = match self.ln.get(&LnKey::new(quote.unit, PaymentMethod::Bolt11)) {
                    Some(ln) => ln,
                    None => {
                        tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                        if let Err(err) = self.process_unpaid_melt(melt_request).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }

                        return Err(Error::UnitUnsupported);
                    }
                };

                let attempt_to_pay = match melt_request.get_payment_method() {
                    PaymentMethod::Bolt11 => {
                        ln.pay_invoice(quote.clone(), partial_amount, Some(quote.fee_reserve))
                            .await
                    }
                    PaymentMethod::Bolt12 => {
                        ln.pay_bolt12_offer(quote.clone(), partial_amount, Some(quote.fee_reserve))
                            .await
                    }
                };

                let pre = match attempt_to_pay {
                    Ok(pay)
                        if pay.status == MeltQuoteState::Unknown
                            || pay.status == MeltQuoteState::Failed =>
                    {
                        let check_response =
                            Self::check_payment_state(Arc::clone(ln), &quote.request_lookup_id)
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

                        let check_response =
                            Self::check_payment_state(Arc::clone(ln), &quote.request_lookup_id)
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
                            melt_request.get_quote_id()
                        );
                        if let Err(err) = self.process_unpaid_melt(melt_request).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }
                        return Err(Error::PaymentFailed);
                    }
                    MeltQuoteState::Pending => {
                        tracing::warn!(
                            "LN payment pending, proofs are stuck as pending for quote: {}",
                            melt_request.get_quote_id()
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

        // If we made it here the payment has been made.
        // We process the melt burning the inputs and returning change
        let change = self
            .process_melt_request(melt_request, amount_spent_quote_unit)
            .await
            .map_err(|err| {
                tracing::error!("Could not process melt request: {}", err);
                err
            })?;

        Ok(MeltQuoteBolt11Response {
            paid: Some(true),
            payment_preimage: preimage,
            change,
            quote: quote.id,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
            state: MeltQuoteState::Paid,
            expiry: quote.expiry,
        })
    }

    /// Process melt request marking [`Proofs`] as spent
    /// The melt request must be verifyed using [`Self::verify_melt_request`]
    /// before calling [`Self::process_melt_request`]
    #[instrument(skip_all)]
    pub async fn process_melt_request<R>(
        &self,
        melt_request: &R,
        total_spent: Amount,
    ) -> Result<Option<Vec<BlindSignature>>, Error>
    where
        R: MeltRequestTrait,
    {
        let quote_id = melt_request.get_quote_id();
        tracing::debug!("Processing melt quote: {}", quote_id);

        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let inputs = melt_request.get_inputs();

        let input_ys = inputs
            .iter()
            .map(|p| hash_to_curve(&p.secret.to_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        self.localstore
            .update_melt_quote_state(quote_id, MeltQuoteState::Paid)
            .await?;

        let mut change = None;

        let inputs_amount = Amount::try_sum(inputs.iter().map(|p| p.amount))?;

        let outputs = melt_request.get_outputs();

        // Check if there is change to return
        if inputs_amount > total_spent {
            // Check if wallet provided change outputs
            if let Some(outputs) = outputs {
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

                let change_target = inputs_amount - total_spent;
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

                let mut outputs = outputs.clone();

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
                        Some(quote.id.clone()),
                    )
                    .await?;

                change = Some(change_sigs);
            }
        }

        Ok(change)
    }
}
