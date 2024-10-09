use tracing::instrument;

use crate::{types::LnKey, util::unix_time, Amount, Error};

use super::{
    CurrencyUnit, Mint, MintQuote, MintQuoteBolt11Request, MintQuoteBolt11Response, PaymentMethod,
};

impl Mint {
    /// Checks that minting is enabled, request is supported unit and within range
    fn check_bolt12_mint_request_acceptable(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
    ) -> Result<(), Error> {
        let nut18 = &self
            .mint_info
            .nuts
            .nut18
            .as_ref()
            .ok_or(Error::UnsupportedUnit)?;

        if nut18.disabled {
            return Err(Error::MintingDisabled);
        }

        println!("{:?}", nut18);

        match nut18.get_settings(unit, &PaymentMethod::Bolt12) {
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
                tracing::info!("Got bolt12 mint quote for unsupported unit: {}", unit);
                return Err(Error::UnitUnsupported);
            }
        }

        Ok(())
    }

    /// Create new mint bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_mint_bolt12_quote(
        &self,
        mint_quote_request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let MintQuoteBolt11Request {
            amount,
            unit,
            description,
        } = mint_quote_request;

        self.check_bolt12_mint_request_acceptable(amount, &unit)?;

        let ln = self
            .ln
            .get(&LnKey::new(unit, PaymentMethod::Bolt12))
            .ok_or_else(|| {
                tracing::info!("Bolt11 mint request for unsupported unit");

                Error::UnitUnsupported
            })?;

        let quote_expiry = unix_time() + self.quote_ttl.mint_ttl;

        if description.is_some() && !ln.get_settings().invoice_description {
            tracing::error!("Backend does not support invoice description");
            return Err(Error::InvoiceDescriptionUnsupported);
        }

        let create_invoice_response = ln
            .create_bolt12_offer(
                amount,
                &unit,
                description.unwrap_or("".to_string()),
                quote_expiry,
            )
            .await
            .map_err(|err| {
                tracing::error!("Could not create invoice: {}", err);
                Error::InvalidPaymentRequest
            })?;

        let quote = MintQuote::new(
            self.mint_url.clone(),
            create_invoice_response.request.to_string(),
            unit,
            amount,
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
        );

        tracing::debug!(
            "New bolt12 mint quote {} for {} {} with request id {}",
            quote.id,
            amount,
            unit,
            create_invoice_response.request_lookup_id,
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote.into())
    }
}
