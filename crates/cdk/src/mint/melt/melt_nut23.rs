//! Melt Bolt12
//!
//!

use std::str::FromStr;

use cdk_common::amount::{amount_for_offer, to_unit};
use cdk_common::common::PaymentProcessorKey;
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};
use cdk_common::payment::PaymentQuoteOptions;
use cdk_common::util::unix_time;
use cdk_common::{
    CurrencyUnit, MeltOptions, MeltQuoteBolt11Response, MeltQuoteBolt12Request, PaymentMethod,
};
use lightning::offers::offer::Offer;
use tracing::instrument;
use uuid::Uuid;

use crate::{Error, Mint};

impl Mint {
    /// Get melt bolt12 quote
    #[instrument(skip_all)]
    pub async fn get_melt_bolt12_quote(
        &self,
        melt_request: &MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<Uuid>, Error> {
        let MeltQuoteBolt12Request {
            request,
            unit,
            options,
        } = melt_request;

        let offer = Offer::from_str(request).unwrap();

        let amount = match options {
            Some(options) => match options {
                MeltOptions::Amountless { amountless } => {
                    to_unit(amountless.amount_msat, &CurrencyUnit::Msat, unit)?
                }
                _ => return Err(Error::UnsupportedUnit),
            },
            None => amount_for_offer(&offer, unit).map_err(|_| Error::UnsupportedUnit)?,
        };

        self.check_melt_request_acceptable(
            amount,
            unit.clone(),
            PaymentMethod::Bolt12,
            request,
            *options,
        )
        .await?;

        let ln = self
            .ln
            .get(&PaymentProcessorKey::new(
                unit.clone(),
                PaymentMethod::Bolt12,
            ))
            .ok_or_else(|| {
                tracing::info!("Could not get ln backend for {}, bolt11 ", unit);

                Error::UnsupportedUnit
            })?;

        let payment_quote = ln
            .get_payment_quote(
                &melt_request.request.to_string(),
                &melt_request.unit,
                melt_request.options,
            )
            .await
            .map_err(|err| {
                tracing::error!(
                    "Could not get payment quote for mint quote, {} bolt11, {}",
                    unit,
                    err
                );

                Error::UnsupportedUnit
            })?;

        let invoice = payment_quote
            .options
            .ok_or_else(|| {
                tracing::error!("Payment backend did not return invoice");
                Error::InvoiceMissing
            })
            .map(|options| match options {
                PaymentQuoteOptions::Bolt12 { invoice } => invoice,
            })?;

        let payment_request = MeltPaymentRequest::Bolt12 {
            offer: Box::new(offer),
            invoice,
        };

        let quote = MeltQuote::new(
            payment_request,
            unit.clone(),
            payment_quote.amount,
            payment_quote.fee,
            unix_time() + self.quote_ttl().await?.melt_ttl,
            payment_quote.request_lookup_id.clone(),
            options.map(|_| amount),
            PaymentMethod::Bolt12,
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
}
