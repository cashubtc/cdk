use std::str::FromStr;

use cdk_common::amount::amount_for_offer;
use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::{self, MintTransaction};
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::MeltPaymentRequest;
use cdk_common::nut05::MeltMethodOptions;
use cdk_common::payment::{
    Bolt11OutgoingPaymentOptions, Bolt12OutgoingPaymentOptions, OutgoingPaymentOptions,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::{MeltOptions, MeltQuoteBolt12Request};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use lightning::offers::offer::Offer;
use tracing::instrument;

mod change_processor;
mod external_melt_executor;
mod internal_melt_executor;
mod payment_executor;

pub use change_processor::ChangeProcessor;
use external_melt_executor::ExternalMeltExecutor;
use internal_melt_executor::InternalMeltExecutor;

use super::{
    CurrencyUnit, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, Mint,
    PaymentMethod, State,
};
use crate::amount::to_unit;
use crate::mint::proof_writer::ProofWriter;
use crate::mint::verification::Verification;
use crate::mint::SigFlag;
use crate::nuts::nut11::{enforce_sig_flag, EnforceSigFlag};
use crate::nuts::MeltQuoteState;
use crate::types::PaymentProcessorKey;
use crate::util::unix_time;
use crate::{ensure_cdk, Amount, Error};

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
                if (self.localstore.get_mint_quote_by_request(&request).await?).is_some() {
                    return Err(Error::InternalMultiPartMeltQuote);
                }
                if !nut15
                    .methods
                    .into_iter()
                    .any(|m| m.method == method && m.unit == unit)
                {
                    return Err(Error::MppUnitMethodNotSupported(unit, method));
                }
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

    /// Prepare melt request - verify inputs and setup transaction
    #[instrument(skip_all)]
    async fn prepare_melt_request(
        &self,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<
        (
            ProofWriter,
            MeltQuote,
            Box<dyn MintTransaction<'_, database::Error> + Send + Sync + '_>,
        ),
        Error,
    > {
        let verification = self.verify_inputs(melt_request.inputs()).await?;

        let mut tx = self.localstore.begin_transaction().await?;

        let (proof_writer, quote) = self
            .verify_melt_request(&mut tx, verification, melt_request)
            .await?;

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

        Ok((proof_writer, quote, tx))
    }

    /// Execute internal melt - settle with matching mint quote
    /// Returns (preimage, amount_spent, quote)
    #[instrument(skip_all)]
    async fn execute_internal_melt(
        &self,
        quote: &MeltQuote,
        mint_quote_id: &QuoteId,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        let executor = InternalMeltExecutor::new(self);
        executor.execute(quote, mint_quote_id).await
    }

    /// Execute external melt - make payment via payment processor
    /// Returns (preimage, amount_spent, quote)
    #[instrument(skip_all)]
    async fn execute_external_melt(
        &self,
        quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        let executor = ExternalMeltExecutor::new(self, self.payment_processors.clone());
        executor.execute(quote, melt_request).await
    }

    /// Execute melt payment - either internal or external
    /// Returns (preimage, amount_spent, quote)
    #[instrument(skip_all)]
    async fn execute_melt_payment<'a>(
        &'a self,
        mut tx: Box<dyn MintTransaction<'a, database::Error> + Send + Sync + 'a>,
        quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        let internal_executor = InternalMeltExecutor::new(self);
        let mint_quote_id = internal_executor
            .is_internal_settlement(&mut tx, quote, melt_request)
            .await?;

        tx.commit().await?;

        match mint_quote_id {
            Some(mint_quote_id) => {
                let (preimage, amount_spent, quote) =
                    self.execute_internal_melt(quote, &mint_quote_id).await?;
                Ok((preimage, amount_spent, quote))
            }
            None => {
                let (preimage, amount_spent, quote) =
                    self.execute_external_melt(quote, melt_request).await?;
                Ok((preimage, amount_spent, quote))
            }
        }
    }

    /// Finalize melt - burn inputs and return change
    #[instrument(skip_all)]
    async fn finalize_melt(
        &self,
        mut proof_writer: ProofWriter,
        quote: MeltQuote,
        payment_preimage: Option<String>,
        total_spent: Amount,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        let mut tx = self.localstore.begin_transaction().await?;

        let input_ys: Vec<_> = tx.get_proof_ys_by_quote_id(&quote.id).await?;

        if input_ys.is_empty() {
            tracing::error!("No input proofs found for quote {}", quote.id);
            return Err(Error::UnknownQuote);
        }

        tracing::debug!(
            "Updating {} proof states to Spent for quote {}",
            input_ys.len(),
            quote.id
        );

        proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await?;

        tracing::debug!("Successfully updated proof states to Spent");

        tx.update_melt_quote_state(&quote.id, MeltQuoteState::Paid, payment_preimage.clone())
            .await?;

        let MeltRequestInfo {
            inputs_amount,
            inputs_fee,
            change_outputs,
        } = tx
            .get_melt_request_and_blinded_messages(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let change = if inputs_amount > total_spent && !change_outputs.is_empty() {
            let change_processor = ChangeProcessor::new(self);
            change_processor
                .calculate_and_sign_change(
                    tx,
                    &quote,
                    inputs_amount,
                    inputs_fee,
                    total_spent,
                    change_outputs,
                )
                .await?
        } else {
            if inputs_amount > total_spent {
                tracing::info!(
                    "Inputs for {} {} greater than spent on melt {} but change outputs not provided.",
                    quote.id,
                    inputs_amount,
                    total_spent
                );
            } else {
                tracing::debug!("No change required for melt {}", quote.id);
            }
            tx.commit().await?;
            None
        };

        proof_writer.commit();

        // Clean up melt_request and associated blinded_messages (change was already signed)
        let mut tx = self.localstore.begin_transaction().await?;
        tx.delete_melt_request(&quote.id).await?;
        tx.commit().await?;

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

        Ok(MeltQuoteBolt11Response {
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
        })
    }

    /// Melt Bolt11
    #[instrument(skip_all)]
    pub async fn melt(
        &self,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("melt_bolt11");

        let result = async {
            let (proof_writer, quote, tx) = self.prepare_melt_request(melt_request).await?;

            let (preimage, amount_spent, quote) =
                match self.execute_melt_payment(tx, &quote, melt_request).await {
                    Ok(result) => result,
                    Err(err) => {
                        if matches!(err, Error::PaymentFailed | Error::RequestAlreadyPaid) {
                            tracing::info!(
                                "Payment for quote {} failed: {}. Resetting quote state.",
                                melt_request.quote(),
                                err
                            );

                            proof_writer.rollback().await?;

                            let mut tx = self.localstore.begin_transaction().await?;

                            // Reset quote to unpaid state
                            tx.update_melt_quote_state(
                                melt_request.quote(),
                                MeltQuoteState::Unpaid,
                                None,
                            )
                            .await?;

                            // Delete melt_request and associated blinded_messages
                            tx.delete_melt_request(melt_request.quote_id()).await?;

                            tx.commit().await?;

                            self.pubsub_manager.melt_quote_status(
                                &quote,
                                None,
                                None,
                                MeltQuoteState::Unpaid,
                            );
                        } else if matches!(err, Error::PendingQuote | Error::Internal) {
                            proof_writer.commit();
                        }

                        #[cfg(feature = "prometheus")]
                        {
                            METRICS.dec_in_flight_requests("melt_bolt11");
                            METRICS.record_mint_operation("melt_bolt11", false);
                            METRICS.record_error();
                        }
                        return Err(err);
                    }
                };

            self.finalize_melt(proof_writer, quote, preimage, amount_spent)
                .await
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("melt_bolt11");
            METRICS.record_mint_operation("melt_bolt11", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
    }
}
