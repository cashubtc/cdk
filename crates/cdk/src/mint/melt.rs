use std::str::FromStr;

use anyhow::bail;
use cdk_common::amount::amount_for_offer;
use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{self, MintTransaction};
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::MeltPaymentRequest;
use cdk_common::nut05::MeltMethodOptions;
use cdk_common::payment::{
    Bolt11OutgoingPaymentOptions, Bolt12OutgoingPaymentOptions, DynMintPayment,
    OutgoingPaymentOptions, PaymentIdentifier,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::{MeltOptions, MeltQuoteBolt12Request};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use lightning::offers::offer::Offer;
use tracing::instrument;

use super::{
    CurrencyUnit, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, Mint,
    PaymentMethod, PublicKey, State,
};
use crate::amount::to_unit;
use crate::cdk_payment::MakePaymentResponse;
use crate::mint::proof_writer::ProofWriter;
use crate::mint::verification::Verification;
use crate::mint::SigFlag;
use crate::nuts::nut11::{enforce_sig_flag, EnforceSigFlag};
use crate::nuts::MeltQuoteState;
use crate::types::PaymentProcessorKey;
use crate::util::unix_time;
use crate::{cdk_payment, ensure_cdk, Amount, Error};

impl Mint {
    #[instrument(skip_all)]
    async fn check_melt_request_acceptable(
        &self,
        amount: Amount,
        unit: CurrencyUnit,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<(), Error> {
        let mint_info = self.mint_info().await?;
        let nut05 = mint_info.nuts.nut05;

        ensure_cdk!(!nut05.disabled, Error::MeltingDisabled);

        let settings = nut05
            .get_settings(&unit, &method)
            .ok_or(Error::UnsupportedUnit)?;

        let amount = match options {
            Some(MeltOptions::Mpp { mpp: _ }) => {
                let nut15 = mint_info.nuts.nut15;
                // Verify there is no corresponding mint quote.
                // Otherwise a wallet is trying to pay someone internally, but
                // with a multi-part quote. And that's just not possible.
                if (self.localstore.get_mint_quote_by_request(&request).await?).is_some() {
                    return Err(Error::InternalMultiPartMeltQuote);
                }
                // Verify MPP is enabled for unit and method
                if !nut15
                    .methods
                    .into_iter()
                    .any(|m| m.method == method && m.unit == unit)
                {
                    return Err(Error::MppUnitMethodNotSupported(unit, method));
                }
                // Assign `amount`
                // because should have already been converted to the partial amount
                amount
            }
            Some(MeltOptions::Amountless { amountless: _ }) => {
                if method == PaymentMethod::Bolt11
                    && !matches!(
                        settings.options,
                        Some(MeltMethodOptions::Bolt11 { amountless: true })
                    )
                {
                    return Err(Error::AmountlessInvoiceNotSupported(unit, method));
                }

                amount
            }
            None => amount,
        };

        let is_above_max = matches!(settings.max_amount, Some(max) if amount > max);
        let is_below_min = matches!(settings.min_amount, Some(min) if amount < min);
        match is_above_max || is_below_min {
            true => {
                tracing::error!(
                    "Melt amount out of range: {} is not within {} and {}",
                    amount,
                    settings.min_amount.unwrap_or_default(),
                    settings.max_amount.unwrap_or_default(),
                );
                Err(Error::AmountOutofLimitRange(
                    settings.min_amount.unwrap_or_default(),
                    settings.max_amount.unwrap_or_default(),
                    amount,
                ))
            }
            false => Ok(()),
        }
    }

    /// Get melt quote for either BOLT11 or BOLT12
    ///
    /// This function accepts a `MeltQuoteRequest` enum and delegates to the
    /// appropriate handler based on the request type.
    #[instrument(skip_all)]
    pub async fn get_melt_quote(
        &self,
        melt_quote_request: MeltQuoteRequest,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        match melt_quote_request {
            MeltQuoteRequest::Bolt11(bolt11_request) => {
                self.get_melt_bolt11_quote_impl(&bolt11_request).await
            }
            MeltQuoteRequest::Bolt12(bolt12_request) => {
                self.get_melt_bolt12_quote_impl(&bolt12_request).await
            }
        }
    }

    /// Implementation of get_melt_bolt11_quote
    #[instrument(skip_all)]
    async fn get_melt_bolt11_quote_impl(
        &self,
        melt_request: &MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_melt_bolt11_quote");
        let MeltQuoteBolt11Request {
            request,
            unit,
            options,
            ..
        } = melt_request;

        let ln = self
            .payment_processors
            .get(&PaymentProcessorKey::new(
                unit.clone(),
                PaymentMethod::Bolt11,
            ))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt11 ", unit);

                Error::UnsupportedUnit
            })?;

        let bolt11 = Bolt11OutgoingPaymentOptions {
            bolt11: melt_request.request.clone(),
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: melt_request.options,
        };

        let payment_quote = ln
            .get_payment_quote(
                &melt_request.unit,
                OutgoingPaymentOptions::Bolt11(Box::new(bolt11)),
            )
            .await
            .map_err(|err| {
                tracing::error!(
                    "Could not get payment quote for mint quote, {} bolt11, {}",
                    unit,
                    err
                );

                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("get_melt_bolt11_quote");
                    METRICS.record_mint_operation("get_melt_bolt11_quote", false);
                    METRICS.record_error();
                }
                Error::UnsupportedUnit
            })?;

        if &payment_quote.unit != unit {
            return Err(Error::UnitMismatch);
        }

        // Validate using processor quote amount for currency conversion
        self.check_melt_request_acceptable(
            payment_quote.amount,
            unit.clone(),
            PaymentMethod::Bolt11,
            request.to_string(),
            *options,
        )
        .await?;

        let melt_ttl = self.quote_ttl().await?.melt_ttl;

        let quote = MeltQuote::new(
            MeltPaymentRequest::Bolt11 {
                bolt11: request.clone(),
            },
            unit.clone(),
            payment_quote.amount,
            payment_quote.fee,
            unix_time() + melt_ttl,
            payment_quote.request_lookup_id.clone(),
            *options,
            PaymentMethod::Bolt11,
        );

        tracing::debug!(
            "New {} melt quote {} for {} {} with request id {:?}",
            quote.payment_method,
            quote.id,
            payment_quote.amount,
            unit,
            payment_quote.request_lookup_id
        );

        let mut tx = self.localstore.begin_transaction().await?;
        tx.add_melt_quote(quote.clone()).await?;
        tx.commit().await?;

        Ok(quote.into())
    }

    /// Implementation of get_melt_bolt12_quote
    #[instrument(skip_all)]
    async fn get_melt_bolt12_quote_impl(
        &self,
        melt_request: &MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        let MeltQuoteBolt12Request {
            request,
            unit,
            options,
        } = melt_request;

        let offer = Offer::from_str(request).map_err(|_| Error::InvalidPaymentRequest)?;

        let amount = match options {
            Some(options) => match options {
                MeltOptions::Amountless { amountless } => {
                    to_unit(amountless.amount_msat, &CurrencyUnit::Msat, unit)?
                }
                _ => return Err(Error::UnsupportedUnit),
            },
            None => amount_for_offer(&offer, unit).map_err(|_| Error::UnsupportedUnit)?,
        };

        let ln = self
            .payment_processors
            .get(&PaymentProcessorKey::new(
                unit.clone(),
                PaymentMethod::Bolt12,
            ))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt12 ", unit);

                Error::UnsupportedUnit
            })?;

        let offer = Offer::from_str(&melt_request.request).map_err(|_| Error::Bolt12parse)?;

        let outgoing_payment_options = Bolt12OutgoingPaymentOptions {
            offer: offer.clone(),
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: *options,
        };

        let payment_quote = ln
            .get_payment_quote(
                &melt_request.unit,
                OutgoingPaymentOptions::Bolt12(Box::new(outgoing_payment_options)),
            )
            .await
            .map_err(|err| {
                tracing::error!(
                    "Could not get payment quote for mint quote, {} bolt12, {}",
                    unit,
                    err
                );

                Error::UnsupportedUnit
            })?;

        if &payment_quote.unit != unit {
            return Err(Error::UnitMismatch);
        }

        // Validate using processor quote amount for currency conversion
        self.check_melt_request_acceptable(
            payment_quote.amount,
            unit.clone(),
            PaymentMethod::Bolt12,
            request.clone(),
            *options,
        )
        .await?;

        let payment_request = MeltPaymentRequest::Bolt12 {
            offer: Box::new(offer),
        };

        let quote = MeltQuote::new(
            payment_request,
            unit.clone(),
            payment_quote.amount,
            payment_quote.fee,
            unix_time() + self.quote_ttl().await?.melt_ttl,
            payment_quote.request_lookup_id.clone(),
            *options,
            PaymentMethod::Bolt12,
        );

        tracing::debug!(
            "New {} melt quote {} for {} {} with request id {:?}",
            quote.payment_method,
            quote.id,
            amount,
            unit,
            payment_quote.request_lookup_id
        );

        let mut tx = self.localstore.begin_transaction().await?;
        tx.add_melt_quote(quote.clone()).await?;
        tx.commit().await?;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("get_melt_bolt11_quote");
            METRICS.record_mint_operation("get_melt_bolt11_quote", true);
        }

        Ok(quote.into())
    }

    /// Check melt quote status
    #[instrument(skip(self))]
    pub async fn check_melt_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("check_melt_quote");
        let quote = match self.localstore.get_melt_quote(quote_id).await {
            Ok(Some(quote)) => quote,
            Ok(None) => {
                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("check_melt_quote");
                    METRICS.record_mint_operation("check_melt_quote", false);
                    METRICS.record_error();
                }
                return Err(Error::UnknownQuote);
            }
            Err(err) => {
                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("check_melt_quote");
                    METRICS.record_mint_operation("check_melt_quote", false);
                    METRICS.record_error();
                }
                return Err(err.into());
            }
        };

        let blind_signatures = match self
            .localstore
            .get_blind_signatures_for_quote(quote_id)
            .await
        {
            Ok(signatures) => signatures,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("check_melt_quote");
                    METRICS.record_mint_operation("check_melt_quote", false);
                    METRICS.record_error();
                }
                return Err(err.into());
            }
        };

        let change = (!blind_signatures.is_empty()).then_some(blind_signatures);

        let response = MeltQuoteBolt11Response {
            quote: quote.id,
            paid: Some(quote.state == MeltQuoteState::Paid),
            state: quote.state,
            expiry: quote.expiry,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
            payment_preimage: quote.payment_preimage,
            change,
            request: Some(quote.request.to_string()),
            unit: Some(quote.unit.clone()),
        };

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("check_melt_quote");
            METRICS.record_mint_operation("check_melt_quote", true);
        }

        Ok(response)
    }

    /// Get melt quotes
    #[instrument(skip_all)]
    pub async fn melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes)
    }

    /// Check melt has expected fees
    #[instrument(skip_all)]
    pub async fn check_melt_expected_ln_fees(
        &self,
        melt_quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<Option<Amount>, Error> {
        let quote_msats = to_unit(melt_quote.amount, &melt_quote.unit, &CurrencyUnit::Msat)
            .expect("Quote unit is checked above that it can convert to msat");

        let invoice_amount_msats = match &melt_quote.request {
            MeltPaymentRequest::Bolt11 { bolt11 } => match bolt11.amount_milli_satoshis() {
                Some(amount) => amount.into(),
                None => melt_quote
                    .options
                    .ok_or(Error::InvoiceAmountUndefined)?
                    .amount_msat(),
            },
            MeltPaymentRequest::Bolt12 { offer } => match offer.amount() {
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

                    to_unit(amount, &currency, &CurrencyUnit::Msat)
                        .map_err(|_err| Error::UnsupportedUnit)?
                }
                None => melt_quote
                    .options
                    .ok_or(Error::InvoiceAmountUndefined)?
                    .amount_msat(),
            },
        };

        let partial_amount = match invoice_amount_msats > quote_msats {
            true => Some(
                to_unit(quote_msats, &CurrencyUnit::Msat, &melt_quote.unit)
                    .map_err(|_| Error::UnsupportedUnit)?,
            ),
            false => None,
        };

        let amount_to_pay = match partial_amount {
            Some(amount_to_pay) => amount_to_pay,
            None => to_unit(invoice_amount_msats, &CurrencyUnit::Msat, &melt_quote.unit)
                .map_err(|_| Error::UnsupportedUnit)?,
        };

        let inputs_amount_quote_unit = melt_request.inputs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        if amount_to_pay + melt_quote.fee_reserve > inputs_amount_quote_unit {
            tracing::debug!(
                "Not enough inputs provided: {} {} needed {} {}",
                inputs_amount_quote_unit,
                melt_quote.unit,
                amount_to_pay,
                melt_quote.unit
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
        tx: &mut Box<dyn MintTransaction<'_, database::Error> + Send + Sync + '_>,
        input_verification: Verification,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<(ProofWriter, MeltQuote), Error> {
        let Verification {
            amount: input_amount,
            unit: input_unit,
        } = input_verification;

        let mut proof_writer =
            ProofWriter::new(self.localstore.clone(), self.pubsub_manager.clone());

        proof_writer
            .add_proofs(
                tx,
                melt_request.inputs(),
                Some(melt_request.quote_id().to_owned()),
            )
            .await?;

        // Only after proof verification succeeds, proceed with quote state check
        let (state, quote) = tx
            .update_melt_quote_state(melt_request.quote(), MeltQuoteState::Pending, None)
            .await?;

        if input_unit != Some(quote.unit.clone()) {
            return Err(Error::UnitMismatch);
        }

        match state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => Ok(()),
            MeltQuoteState::Pending => Err(Error::PendingQuote),
            MeltQuoteState::Paid => Err(Error::PaidQuote),
            MeltQuoteState::Unknown => Err(Error::UnknownPaymentState),
        }?;

        self.pubsub_manager
            .melt_quote_status(&quote, None, None, MeltQuoteState::Pending);

        let fee = self.get_proofs_fee(melt_request.inputs()).await?;

        let required_total = quote.amount + quote.fee_reserve + fee;

        // Check that the inputs proofs are greater then total.
        // Transaction does not need to be balanced as wallet may not want change.
        if input_amount < required_total {
            tracing::info!(
                "Swap request unbalanced: {}, outputs {}, fee {}",
                input_amount,
                quote.amount,
                fee
            );
            return Err(Error::TransactionUnbalanced(
                input_amount.into(),
                quote.amount.into(),
                (fee + quote.fee_reserve).into(),
            ));
        }

        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(melt_request.inputs().clone());

        if sig_flag == SigFlag::SigAll {
            melt_request.verify_sig_all()?;
        }

        if let Some(outputs) = &melt_request.outputs() {
            if !outputs.is_empty() {
                let Verification {
                    amount: _,
                    unit: output_unit,
                } = self.verify_outputs(tx, outputs).await?;

                ensure_cdk!(input_unit == output_unit, Error::UnsupportedUnit);
            }
        }

        tracing::debug!("Verified melt quote: {}", melt_request.quote());
        Ok((proof_writer, quote))
    }

    /// Melt Bolt11
    #[instrument(skip_all)]
    pub async fn melt(
        &self,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("melt_bolt11");

        use std::sync::Arc;
        async fn check_payment_state(
            ln: DynMintPayment,
            lookup_id: &PaymentIdentifier,
        ) -> anyhow::Result<MakePaymentResponse> {
            match ln.check_outgoing_payment(lookup_id).await {
                Ok(response) => Ok(response),
                Err(check_err) => {
                    // If we cannot check the status of the payment we keep the proofs stuck as pending.
                    tracing::error!(
                        "Could not check the status of payment for {},. Proofs stuck as pending",
                        lookup_id
                    );
                    tracing::error!("Checking payment error: {}", check_err);
                    bail!("Could not check payment status")
                }
            }
        }

        let verification = self.verify_inputs(melt_request.inputs()).await?;

        let mut tx = self.localstore.begin_transaction().await?;

        let (proof_writer, quote) = match self
            .verify_melt_request(&mut tx, verification, melt_request)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                tracing::debug!("Error attempting to verify melt quote: {}", err);

                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("melt_bolt11");
                    METRICS.record_mint_operation("melt_bolt11", false);
                    METRICS.record_error();
                }

                return Err(err);
            }
        };

        let inputs_fee = self.get_proofs_fee(melt_request.inputs()).await?;

        tx.add_melt_request(
            melt_request.quote_id(),
            melt_request.inputs_amount()?,
            inputs_fee,
        )
        .await?;

        tx.add_blinded_messages(
            Some(melt_request.quote_id()),
            melt_request.outputs().as_ref().unwrap_or(&Vec::new()),
        )
        .await?;

        let settled_internally_amount = match self
            .handle_internal_melt_mint(&mut tx, &quote, melt_request)
            .await
        {
            Ok(amount) => amount,
            Err(err) => {
                tracing::error!("Attempting to settle internally failed: {}", err);

                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("melt_bolt11");
                    METRICS.record_mint_operation("melt_bolt11", false);
                    METRICS.record_error();
                }

                return Err(err);
            }
        };

        let (tx, preimage, amount_spent_quote_unit, quote) = match settled_internally_amount {
            Some(amount_spent) => (tx, None, amount_spent, quote),

            None => {
                // If the quote unit is SAT or MSAT we can check that the expected fees are
                // provided. We also check if the quote is less then the invoice
                // amount in the case that it is a mmp However, if the quote is not
                // of a bitcoin unit we cannot do these checks as the mint
                // is unaware of a conversion rate. In this case it is assumed that the quote is
                // correct and the mint should pay the full invoice amount if inputs
                // > `then quote.amount` are included. This is checked in the
                // `verify_melt` method.
                let _partial_amount = match quote.unit {
                    CurrencyUnit::Sat | CurrencyUnit::Msat => {
                        match self.check_melt_expected_ln_fees(&quote, melt_request).await {
                            Ok(amount) => amount,
                            Err(err) => {
                                tracing::error!("Fee is not expected: {}", err);
                                return Err(Error::Internal);
                            }
                        }
                    }
                    _ => None,
                };

                let ln = match self.payment_processors.get(&PaymentProcessorKey::new(
                    quote.unit.clone(),
                    quote.payment_method.clone(),
                )) {
                    Some(ln) => ln,
                    None => {
                        tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                        return Err(Error::UnsupportedUnit);
                    }
                };

                // Commit before talking to the external call
                tx.commit().await?;

                let pre = match ln
                    .make_payment(&quote.unit, quote.clone().try_into()?)
                    .await
                {
                    Ok(pay)
                        if pay.status == MeltQuoteState::Unknown
                            || pay.status == MeltQuoteState::Failed =>
                    {
                        tracing::warn!("Got {} status when paying melt quote {} for {} {}. Checking with backend...", pay.status, quote.id, quote.amount, quote.unit);
                        let check_response = if let Ok(ok) =
                            check_payment_state(Arc::clone(ln), &pay.payment_lookup_id).await
                        {
                            ok
                        } else {
                            return Err(Error::Internal);
                        };

                        if check_response.status == MeltQuoteState::Paid {
                            tracing::warn!("Pay invoice returned {} but check returned {}. Proofs stuck as pending", pay.status.to_string(), check_response.status.to_string());

                            proof_writer.commit();

                            return Err(Error::Internal);
                        }

                        check_response
                    }
                    Ok(pay) => pay,
                    Err(err) => {
                        // If the error is that the invoice was already paid we do not want to hold
                        // hold the proofs as pending to we reset them  and return an error.
                        if matches!(err, cdk_payment::Error::InvoiceAlreadyPaid) {
                            tracing::debug!("Invoice already paid, resetting melt quote");
                            return Err(Error::RequestAlreadyPaid);
                        }

                        tracing::error!("Error returned attempting to pay: {} {}", quote.id, err);

                        let lookup_id = quote.request_lookup_id.as_ref().ok_or_else(|| {
                            tracing::error!(
                                "No payment id could not lookup payment for {} after error.",
                                quote.id
                            );
                            Error::Internal
                        })?;

                        let check_response =
                            if let Ok(ok) = check_payment_state(Arc::clone(ln), lookup_id).await {
                                ok
                            } else {
                                proof_writer.commit();
                                return Err(Error::Internal);
                            };
                        // If there error is something else we want to check the status of the payment ensure it is not pending or has been made.
                        if check_response.status == MeltQuoteState::Paid {
                            tracing::warn!("Pay invoice returned an error but check returned {}. Proofs stuck as pending", check_response.status.to_string());
                            proof_writer.commit();
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
                            melt_request.quote()
                        );
                        proof_writer.rollback().await?;

                        #[cfg(feature = "prometheus")]
                        {
                            METRICS.dec_in_flight_requests("melt_bolt11");
                            METRICS.record_mint_operation("melt_bolt11", false);
                            METRICS.record_error();
                        }

                        return Err(Error::PaymentFailed);
                    }
                    MeltQuoteState::Pending => {
                        tracing::warn!(
                            "LN payment pending, proofs are stuck as pending for quote: {}",
                            melt_request.quote()
                        );
                        proof_writer.commit();
                        #[cfg(feature = "prometheus")]
                        {
                            METRICS.dec_in_flight_requests("melt_bolt11");
                            METRICS.record_mint_operation("melt_bolt11", false);
                            METRICS.record_error();
                        }

                        return Err(Error::PendingQuote);
                    }
                }

                // Convert from unit of backend to quote unit
                // Note: this should never fail since these conversions happen earlier and would fail there.
                // Since it will not fail and even if it does the ln payment has already been paid, proofs should still be burned
                let amount_spent =
                    to_unit(pre.total_spent, &pre.unit, &quote.unit).unwrap_or_default();

                let payment_lookup_id = pre.payment_lookup_id;
                let mut tx = self.localstore.begin_transaction().await?;

                if Some(payment_lookup_id.clone()).as_ref() != quote.request_lookup_id.as_ref() {
                    tracing::info!(
                        "Payment lookup id changed post payment from {:?} to {}",
                        &quote.request_lookup_id,
                        payment_lookup_id
                    );

                    let mut melt_quote = quote;
                    melt_quote.request_lookup_id = Some(payment_lookup_id.clone());

                    if let Err(err) = tx
                        .update_melt_quote_request_lookup_id(&melt_quote.id, &payment_lookup_id)
                        .await
                    {
                        tracing::warn!("Could not update payment lookup id: {}", err);
                    }

                    (tx, pre.payment_proof, amount_spent, melt_quote)
                } else {
                    (tx, pre.payment_proof, amount_spent, quote)
                }
            }
        };

        // If we made it here the payment has been made.
        // We process the melt burning the inputs and returning change
        let res = match self
            .process_melt_request(tx, proof_writer, quote, preimage, amount_spent_quote_unit)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::error!("Could not process melt request: {}", err);

                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("melt_bolt11");
                    METRICS.record_mint_operation("melt_bolt11", false);
                    METRICS.record_error();
                }

                return Err(err);
            }
        };

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("melt_bolt11");
            METRICS.record_mint_operation("melt_bolt11", true);
        }

        Ok(res)
    }

    /// Process melt request marking proofs as spent
    /// The melt request must be verified using [`Self::verify_melt_request`]
    /// before calling [`Self::process_melt_request`]
    #[instrument(skip_all)]
    pub async fn process_melt_request(
        &self,
        mut tx: Box<dyn MintTransaction<'_, database::Error> + Send + Sync + '_>,
        mut proof_writer: ProofWriter,
        quote: MeltQuote,
        payment_preimage: Option<String>,
        total_spent: Amount,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_melt_request");

        // Try to get input_ys from the stored melt request, fall back to original request if not found
        let input_ys: Vec<_> = tx.get_proof_ys_by_quote_id(&quote.id).await?;

        assert!(!input_ys.is_empty());

        tracing::debug!(
            "Updating {} proof states to Spent for quote {}",
            input_ys.len(),
            quote.id
        );

        let update_proof_states_result = proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await;

        if update_proof_states_result.is_err() {
            #[cfg(feature = "prometheus")]
            self.record_melt_quote_failure("process_melt_request");
            return Err(update_proof_states_result.err().unwrap());
        }
        tracing::debug!("Successfully updated proof states to Spent");

        tx.update_melt_quote_state(&quote.id, MeltQuoteState::Paid, payment_preimage.clone())
            .await?;

        let mut change = None;

        let MeltRequestInfo {
            inputs_amount,
            inputs_fee,
            change_outputs,
        } = tx
            .get_melt_request_and_blinded_messages(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Check if there is change to return
        if inputs_amount > total_spent {
            // Check if wallet provided change outputs
            if !change_outputs.is_empty() {
                let outputs = change_outputs;

                let blinded_messages: Vec<PublicKey> =
                    outputs.iter().map(|b| b.blinded_secret).collect();

                if tx
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

                let change_target = inputs_amount - total_spent - inputs_fee;

                let fee_and_amounts = self
                    .keysets
                    .load()
                    .iter()
                    .filter_map(|keyset| {
                        if keyset.active && Some(keyset.id) == outputs.first().map(|x| x.keyset_id)
                        {
                            Some((keyset.input_fee_ppk, keyset.amounts.clone()).into())
                        } else {
                            None
                        }
                    })
                    .next()
                    .unwrap_or_else(|| {
                        (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into()
                    });

                let mut amounts = change_target.split(&fee_and_amounts);

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

                let mut blinded_messages = vec![];

                for (amount, mut blinded_message) in amounts.iter().zip(outputs.clone()) {
                    blinded_message.amount = *amount;
                    blinded_messages.push(blinded_message);
                }

                // commit db transaction before calling the signatory
                tx.commit().await?;

                let change_sigs = self.blind_sign(blinded_messages).await?;

                let mut tx = self.localstore.begin_transaction().await?;

                tx.add_blind_signatures(
                    &outputs[0..change_sigs.len()]
                        .iter()
                        .map(|o| o.blinded_secret)
                        .collect::<Vec<PublicKey>>(),
                    &change_sigs,
                    Some(quote.id.clone()),
                )
                .await?;

                change = Some(change_sigs);

                proof_writer.commit();

                tx.delete_melt_request(&quote.id).await?;

                tx.commit().await?;
            } else {
                tracing::info!(
                    "Inputs for {} {} greater then spent on melt {} but change outputs not provided.",
                    quote.id,
                    inputs_amount,
                    total_spent
                );
                proof_writer.commit();
                tx.delete_melt_request(&quote.id).await?;
                tx.commit().await?;
            }
        } else {
            tracing::debug!("No change required for melt {}", quote.id);
            proof_writer.commit();
            tx.delete_melt_request(&quote.id).await?;
            tx.commit().await?;
        }

        self.pubsub_manager.melt_quote_status(
            &quote,
            payment_preimage.clone(),
            change.clone(),
            MeltQuoteState::Paid,
        );
        tracing::debug!(
            "Melt for quote {} completed total spent {}, total inputs: {}, change given: {}",
            quote.id,
            total_spent,
            inputs_amount,
            change
                .as_ref()
                .map(|c| Amount::try_sum(c.iter().map(|a| a.amount))
                    .expect("Change cannot overflow"))
                .unwrap_or_default()
        );
        let response = MeltQuoteBolt11Response {
            amount: quote.amount,
            paid: Some(true),
            payment_preimage,
            change,
            quote: quote.id,
            fee_reserve: quote.fee_reserve,
            state: MeltQuoteState::Paid,
            expiry: quote.expiry,
            request: Some(quote.request.to_string()),
            unit: Some(quote.unit.clone()),
        };

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("process_melt_request");
            METRICS.record_mint_operation("process_melt_request", true);
        }

        Ok(response)
    }

    #[cfg(feature = "prometheus")]
    fn record_melt_quote_failure(&self, operation: &str) {
        METRICS.dec_in_flight_requests(operation);
        METRICS.record_mint_operation(operation, false);
        METRICS.record_error();
    }
}
