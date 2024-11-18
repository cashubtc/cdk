use tracing::instrument;

use super::nut20::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use super::{Mint, MintQuote, PaymentMethod};
use crate::util::unix_time;
use crate::{Amount, Error};

impl Mint {
    /// Create new mint bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_mint_bolt12_quote(
        &self,
        mint_quote_request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response, Error> {
        let MintQuoteBolt12Request {
            amount,
            unit,
            description,
            single_use,
            expiry,
            pubkey,
        } = mint_quote_request;

        let nut18 = &self
            .mint_info
            .nuts
            .nut18
            .as_ref()
            .ok_or(Error::UnsupportedUnit)?;

        if nut18.disabled {
            return Err(Error::MintingDisabled);
        }

        let ln = self.bolt12_backends.get(&unit).ok_or_else(|| {
            tracing::info!("Bolt11 mint request for unsupported unit");

            Error::UnitUnsupported
        })?;

        let quote_expiry = match expiry {
            Some(expiry) => expiry,
            None => unix_time() + self.quote_ttl.mint_ttl,
        };

        let create_invoice_response = ln
            .create_bolt12_offer(
                amount,
                &unit,
                description.unwrap_or("".to_string()),
                quote_expiry,
                single_use,
            )
            .await
            .map_err(|err| {
                tracing::error!("Could not create invoice: {}", err);
                Error::InvalidPaymentRequest
            })?;

        let quote = MintQuote::new(
            self.mint_url.clone(),
            create_invoice_response.request.to_string(),
            PaymentMethod::Bolt12,
            unit.clone(),
            amount,
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
            Amount::ZERO,
            Amount::ZERO,
            single_use,
            vec![],
            Some(pubkey),
        );

        tracing::debug!(
            "New bolt12 mint quote {} for {} {} with request id {}",
            quote.id,
            amount.unwrap_or_default(),
            unit,
            create_invoice_response.request_lookup_id,
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote.try_into()?)
    }

    /// Check mint quote
    #[instrument(skip(self))]
    pub async fn check_mint_bolt12_quote(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(quote.try_into()?)
    }
}
