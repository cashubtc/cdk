use std::str::FromStr;

use cdk_common::amount::amount_for_offer;
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

use super::{
    CurrencyUnit, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, Mint,
    PaymentMethod,
};
use crate::amount::to_unit;
use crate::nuts::MeltQuoteState;
use crate::types::PaymentProcessorKey;
use crate::util::unix_time;
use crate::{ensure_cdk, Amount, Error};

mod melt_saga;
pub(super) mod shared;

use melt_saga::MeltSaga;

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

    /// Melt
    ///
    /// Uses MeltSaga typestate pattern for atomic transaction handling with automatic rollback on failure.
    #[instrument(skip_all)]
    pub async fn melt(
        &self,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        let verification = self.verify_inputs(melt_request.inputs()).await?;

        let init_saga = MeltSaga::new(
            std::sync::Arc::new(self.clone()),
            self.localstore.clone(),
            std::sync::Arc::clone(&self.pubsub_manager),
        );

        // Step 1: Setup (TX1 - reserves inputs and outputs)
        let setup_saga = init_saga.setup_melt(melt_request, verification).await?;

        // Step 2: Attempt internal settlement (returns saga + SettlementDecision)
        // Note: Compensation is handled internally if this fails
        let (setup_saga, settlement) = setup_saga.attempt_internal_settlement(melt_request).await?;

        // Step 3: Make payment (internal or external)
        let payment_saga = setup_saga.make_payment(settlement).await?;

        // Step 4: Finalize (TX2 - marks spent, issues change)
        payment_saga.finalize().await
    }
}
