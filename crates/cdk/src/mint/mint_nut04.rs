use tracing::instrument;
use uuid::Uuid;

use super::{
    nut04, CurrencyUnit, Mint, MintQuote, MintQuoteBolt11Request, MintQuoteBolt11Response,
    NotificationPayload, PaymentMethod, PublicKey,
};
use crate::nuts::MintQuoteState;
use crate::types::LnKey;
use crate::util::unix_time;
use crate::{Amount, Error};

impl Mint {
    /// Checks that minting is enabled, request is supported unit and within range
    fn check_mint_request_acceptable(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
    ) -> Result<(), Error> {
        let nut04 = &self.mint_info.nuts.nut04;

        if nut04.disabled {
            return Err(Error::MintingDisabled);
        }

        match nut04.get_settings(unit, &PaymentMethod::Bolt11) {
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

    /// Create new mint bolt11 quote
    #[instrument(skip_all)]
    pub async fn get_mint_bolt11_quote(
        &self,
        mint_quote_request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<Uuid>, Error> {
        let MintQuoteBolt11Request {
            amount,
            unit,
            description,
        } = mint_quote_request;

        self.check_mint_request_acceptable(amount, &unit)?;

        let ln = self
            .ln
            .get(&LnKey::new(unit.clone(), PaymentMethod::Bolt11))
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
            .create_invoice(
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
            unit.clone(),
            amount,
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
        );

        tracing::debug!(
            "New mint quote {} for {} {} with request id {}",
            quote.id,
            amount,
            unit,
            create_invoice_response.request_lookup_id,
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        let quote: MintQuoteBolt11Response<Uuid> = quote.into();

        self.pubsub_manager
            .broadcast(NotificationPayload::MintQuoteBolt11Response(quote.clone()));

        Ok(quote)
    }

    /// Check mint quote
    #[instrument(skip(self))]
    pub async fn check_mint_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<MintQuoteBolt11Response<Uuid>, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Since the pending state is not part of the NUT it should not be part of the
        // response. In practice the wallet should not be checking the state of
        // a quote while waiting for the mint response.
        let state = match quote.state {
            MintQuoteState::Pending => MintQuoteState::Paid,
            s => s,
        };

        Ok(MintQuoteBolt11Response {
            quote: quote.id,
            request: quote.request,
            state,
            expiry: Some(quote.expiry),
        })
    }

    /// Update mint quote
    #[instrument(skip_all)]
    pub async fn update_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.localstore.add_mint_quote(quote).await?;
        Ok(())
    }

    /// Get mint quotes
    #[instrument(skip_all)]
    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.localstore.get_mint_quotes().await?;
        Ok(quotes)
    }

    /// Get pending mint quotes
    #[instrument(skip_all)]
    pub async fn get_pending_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;

        Ok(mint_quotes
            .into_iter()
            .filter(|p| p.state == MintQuoteState::Pending)
            .collect())
    }

    /// Get pending mint quotes
    #[instrument(skip_all)]
    pub async fn get_unpaid_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;

        Ok(mint_quotes
            .into_iter()
            .filter(|p| p.state == MintQuoteState::Unpaid)
            .collect())
    }

    /// Remove mint quote
    #[instrument(skip_all)]
    pub async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Error> {
        self.localstore.remove_mint_quote(quote_id).await?;

        Ok(())
    }

    /// Flag mint quote as paid
    #[instrument(skip_all)]
    pub async fn pay_mint_quote_for_request_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<(), Error> {
        if let Ok(Some(mint_quote)) = self
            .localstore
            .get_mint_quote_by_request_lookup_id(request_lookup_id)
            .await
        {
            tracing::debug!(
                "Received payment notification for mint quote {}",
                mint_quote.id
            );
            if mint_quote.state != MintQuoteState::Issued
                && mint_quote.state != MintQuoteState::Paid
            {
                let unix_time = unix_time();

                if mint_quote.expiry < unix_time {
                    tracing::warn!(
                        "Mint quote {} paid at {} expired at {}, leaving current state",
                        mint_quote.id,
                        mint_quote.expiry,
                        unix_time,
                    );
                    return Err(Error::ExpiredQuote(mint_quote.expiry, unix_time));
                }

                tracing::debug!(
                    "Marking quote {} paid by lookup id {}",
                    mint_quote.id,
                    request_lookup_id
                );

                self.localstore
                    .update_mint_quote_state(&mint_quote.id, MintQuoteState::Paid)
                    .await?;
            } else {
                tracing::debug!(
                    "{} Quote already {} continuing",
                    mint_quote.id,
                    mint_quote.state
                );
            }

            self.pubsub_manager
                .mint_quote_bolt11_status(mint_quote, MintQuoteState::Paid);
        }
        Ok(())
    }

    /// Process mint request
    #[instrument(skip_all)]
    pub async fn process_mint_request(
        &self,
        mint_request: nut04::MintBolt11Request<Uuid>,
    ) -> Result<nut04::MintBolt11Response, Error> {
        let mint_quote =
            if let Some(mint_quote) = self.localstore.get_mint_quote(&mint_request.quote).await? {
                mint_quote
            } else {
                return Err(Error::UnknownQuote);
            };

        let state = self
            .localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Pending)
            .await?;

        match state {
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MintQuoteState::Issued => {
                return Err(Error::IssuedQuote);
            }
            MintQuoteState::Paid => (),
        }

        let blinded_messages: Vec<PublicKey> = mint_request
            .outputs
            .iter()
            .map(|b| b.blinded_secret)
            .collect();

        if self
            .localstore
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed",);
            tracing::info!(
                "Mint {} did not succeed returning quote to Paid state",
                mint_request.quote
            );

            self.localstore
                .update_mint_quote_state(&mint_request.quote, MintQuoteState::Paid)
                .await
                .unwrap();

            return Err(Error::BlindedMessageAlreadySigned);
        }

        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs.iter() {
            let blind_signature = self.blind_sign(blinded_message).await?;
            blind_signatures.push(blind_signature);
        }

        self.localstore
            .add_blind_signatures(
                &mint_request
                    .outputs
                    .iter()
                    .map(|p| p.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &blind_signatures,
                Some(mint_request.quote),
            )
            .await?;

        self.localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Issued)
            .await?;

        self.pubsub_manager
            .mint_quote_bolt11_status(mint_quote, MintQuoteState::Issued);

        Ok(nut04::MintBolt11Response {
            signatures: blind_signatures,
        })
    }
}
