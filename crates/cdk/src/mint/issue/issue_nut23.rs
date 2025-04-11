use cdk_common::common::PaymentProcessorKey;
use cdk_common::mint::MintQuote;
use cdk_common::{CurrencyUnit, PaymentMethod};
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::nut23::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use crate::util::unix_time;
use crate::{ensure_cdk, Amount, Error, Mint};

impl Mint {
    /// Checks that minting is enabled, request is supported unit and within range
    async fn check_bolt12_mint_request_acceptable(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
    ) -> Result<(), Error> {
        let mint_info = self.localstore.get_mint_info().await?;
        let nut23 = &mint_info
            .nuts
            .nut23
            .ok_or(Error::UnsupportedPaymentMethod)?;

        ensure_cdk!(!nut23.disabled, Error::MintingDisabled);

        let settings = nut23
            .get_settings(unit, &PaymentMethod::Bolt11)
            .ok_or(Error::UnsupportedUnit)?;
        if let Some(amount) = amount {
            let is_above_max = settings
                .max_amount
                .is_some_and(|max_amount| amount > max_amount);
            let is_below_min = settings
                .min_amount
                .is_some_and(|min_amount| amount < min_amount);
            let is_out_of_range = is_above_max || is_below_min;
            ensure_cdk!(
                !is_out_of_range,
                Error::AmountOutofLimitRange(
                    settings.min_amount.unwrap_or_default(),
                    settings.max_amount.unwrap_or_default(),
                    amount,
                )
            );
        }

        Ok(())
    }

    /// Create new mint bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_mint_bolt12_quote(
        &self,
        mint_quote_request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<Uuid>, Error> {
        let MintQuoteBolt12Request {
            amount,
            unit,
            description,
            single_use,
            expiry,
            pubkey,
        } = mint_quote_request;

        self.check_bolt12_mint_request_acceptable(amount, &unit)
            .await?;

        let ln = self
            .ln
            .get(&PaymentProcessorKey::new(
                unit.clone(),
                PaymentMethod::Bolt12,
            ))
            .ok_or_else(|| {
                tracing::info!("Bolt12 mint request for unsupported unit");

                Error::UnsupportedUnit
            })?;

        let mint_ttl = self.localstore.get_quote_ttl().await?.mint_ttl;
        let quote_expiry = match expiry {
            Some(expiry) => expiry,
            None => unix_time() + mint_ttl,
        };

        let create_invoice_response = ln
            .create_incoming_payment_request(
                // TODO: We need to make this an option on the trait
                amount.unwrap_or_default(),
                &unit,
                &PaymentMethod::Bolt11,
                description.unwrap_or("".to_string()),
                Some(quote_expiry),
            )
            .await
            .map_err(|err| {
                tracing::error!("Could not create invoice: {}", err);
                Error::InvalidPaymentRequest
            })?;

        let quote = MintQuote::new(
            None,
            create_invoice_response.request.to_string(),
            unit.clone(),
            // TODO: Should be option
            amount.unwrap_or_default(),
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
            Some(pubkey),
            Amount::ZERO,
            Amount::ZERO,
            single_use,
            vec![],
            PaymentMethod::Bolt12,
            false,
            unix_time(),
            None,
            None,
        );

        tracing::debug!(
            "New bolt12 mint quote {} for {} {} with request id {}",
            quote.id,
            amount.unwrap_or_default(),
            unit,
            create_invoice_response.request_lookup_id,
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        quote.try_into()
    }

    /// Check mint quote
    #[instrument(skip(self))]
    pub async fn check_mint_bolt12_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<MintQuoteBolt12Response<Uuid>, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        quote.try_into()
    }
}
