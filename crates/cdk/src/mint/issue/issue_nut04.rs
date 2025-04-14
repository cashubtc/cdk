use cdk_common::payment::{Bolt11Settings, WaitPaymentResponse};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::{
    CurrencyUnit, MintBolt11Request, MintBolt11Response, MintQuote, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteState, NotificationPayload, PublicKey, Verification,
};
use crate::nuts::PaymentMethod;
use crate::util::unix_time;
use crate::{ensure_cdk, Amount, Error, Mint};

impl Mint {
    /// Checks that minting is enabled, request is supported unit and within range
    async fn check_mint_request_acceptable(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
    ) -> Result<(), Error> {
        let mint_info = self.localstore.get_mint_info().await?;
        let nut04 = &mint_info.nuts.nut04;

        ensure_cdk!(!nut04.disabled, Error::MintingDisabled);

        let settings = nut04
            .get_settings(unit, &PaymentMethod::Bolt11)
            .ok_or(Error::UnsupportedUnit)?;

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
            pubkey,
        } = mint_quote_request;

        self.check_mint_request_acceptable(amount, &unit).await?;

        let ln = self.get_payment_processor(unit.clone(), PaymentMethod::Bolt11)?;

        let mint_ttl = self.localstore.get_quote_ttl().await?.mint_ttl;

        let quote_expiry = unix_time() + mint_ttl;

        let settings = ln.get_settings().await?;
        let settings: Bolt11Settings = serde_json::from_value(settings)?;

        if description.is_some() && !settings.invoice_description {
            tracing::error!("Backend does not support invoice description");
            return Err(Error::InvoiceDescriptionUnsupported);
        }

        let create_invoice_response = ln
            .create_incoming_payment_request(
                amount,
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
            Some(amount),
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
            pubkey,
            Amount::ZERO,
            Amount::ZERO,
            true,
            vec![],
            PaymentMethod::Bolt11,
            false,
            unix_time(),
            None,
            None,
        );

        tracing::debug!(
            "New mint quote {} for {} {} with request id {:?}",
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

        let state = match quote.state() {
            MintQuoteState::Unpaid => {
                tracing::debug!("Check called on unpaid mint quote. Checking ...");
                self.check_mint_quote_paid(quote_id).await?
            }
            s => s,
        };

        Ok(MintQuoteBolt11Response {
            quote: quote.id,
            request: quote.request,
            state,
            expiry: Some(quote.expiry),
            pubkey: quote.pubkey,
            amount: quote.amount,
            unit: Some(quote.unit.clone()),
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
        let mint_quotes = self
            .localstore
            .get_mint_quotes()
            .await?
            .into_iter()
            .filter(|p| p.pending())
            .collect();

        Ok(mint_quotes)
    }

    /// Get pending mint quotes
    #[instrument(skip_all)]
    pub async fn get_unpaid_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self
            .localstore
            .get_mint_quotes_with_state(MintQuoteState::Unpaid)
            .await?;

        Ok(mint_quotes)
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
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        if let Ok(Some(mint_quote)) = self
            .localstore
            .get_mint_quote_by_request_lookup_id(&wait_payment_response.payment_identifier)
            .await
        {
            self.pay_mint_quote(&mint_quote, wait_payment_response)
                .await?;
        }
        Ok(())
    }

    /// Mark mint quote as paid
    #[instrument(skip_all)]
    pub async fn pay_mint_quote(
        &self,
        mint_quote: &MintQuote,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        tracing::debug!(
            "Received payment notification of {} for mint quote {} with payment id {}",
            wait_payment_response.payment_amount,
            mint_quote.id,
            wait_payment_response.payment_id
        );

        println!("{}", mint_quote.state());

        let quote_state = mint_quote.state();
        if !mint_quote
            .payment_ids
            .contains(&wait_payment_response.payment_id)
        {
            if mint_quote.payment_method == PaymentMethod::Bolt11
                && (quote_state == MintQuoteState::Issued || quote_state == MintQuoteState::Paid)
            {
                tracing::info!("Received payment notification for already seen payment.");
            } else {
                self.localstore
                    .increment_mint_quote_amount_paid(
                        &mint_quote.id,
                        wait_payment_response.payment_amount,
                        wait_payment_response.payment_id,
                    )
                    .await?;

                self.pubsub_manager
                    .mint_quote_bolt11_status(mint_quote.clone(), MintQuoteState::Paid);
            }
        } else {
            tracing::info!("Received payment notification for already seen payment.");
        }

        Ok(())
    }

    /// Process mint request
    #[instrument(skip_all)]
    pub async fn process_mint_request(
        &self,
        mint_request: MintBolt11Request<Uuid>,
    ) -> Result<MintBolt11Response, Error> {
        let mint_quote = self
            .localstore
            .get_mint_quote(&mint_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if let Err(err) = self
            .localstore
            .set_mint_quote_pending(&mint_request.quote)
            .await
        {
            tracing::warn!("Attempt to mint pending quote: {}", err);
            return Err(Error::PendingQuote);
        }

        let state = if mint_quote.state() == MintQuoteState::Unpaid {
            self.check_mint_quote_paid(&mint_quote.id).await?
        } else {
            mint_quote.state()
        };

        match state {
            MintQuoteState::Unpaid => {
                self.localstore
                    .unset_mint_quote_pending(&mint_request.quote)
                    .await?;
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Issued => {
                if mint_quote.payment_method == PaymentMethod::Bolt12
                    && mint_quote.amount_paid() > mint_quote.amount_issued()
                {
                    tracing::warn!("Mint quote should state should have been set to issued upon new payment. Something isn't right. Stopping mint");
                }

                self.localstore
                    .unset_mint_quote_pending(&mint_request.quote)
                    .await?;
                return Err(Error::IssuedQuote);
            }
            MintQuoteState::Paid => (),
        }

        if mint_quote.payment_method == PaymentMethod::Bolt12 && mint_quote.pubkey.is_none() {
            tracing::warn!("Bolt12 mint quote created without pubkey");
            return Err(Error::SignatureMissingOrInvalid);
        }

        let mint_amount = match mint_quote.payment_method {
            PaymentMethod::Bolt11 => mint_quote.amount.ok_or(Error::AmountUndefined)?,
            PaymentMethod::Bolt12 => {
                if mint_quote.amount_issued() > mint_quote.amount_paid() {
                    tracing::error!(
                        "Quote state should not be issued if issued {} is > paid {}.",
                        mint_quote.amount_issued(),
                        mint_quote.amount_paid()
                    );
                    return Err(Error::UnpaidQuote);
                }
                mint_quote.amount_paid() - mint_quote.amount_issued()
            }
            _ => return Err(Error::UnsupportedPaymentMethod),
        };

        // If the there is a public key provoided in mint quote request
        // verify the signature is provided for the mint request
        if let Some(pubkey) = mint_quote.pubkey {
            mint_request.verify_signature(pubkey)?;
        }

        let Verification { amount, unit } = match self.verify_outputs(&mint_request.outputs).await {
            Ok(verification) => verification,
            Err(err) => {
                tracing::debug!("Could not verify mint outputs");
                self.localstore
                    .unset_mint_quote_pending(&mint_request.quote)
                    .await?;

                return Err(err);
            }
        };

        // We check the total value of blinded messages == mint quote
        if amount != mint_amount {
            return Err(Error::TransactionUnbalanced(
                mint_amount.into(),
                mint_request.total_amount()?.into(),
                0,
            ));
        }

        let unit = unit.ok_or(Error::UnsupportedUnit).unwrap();
        println!("{}", unit);
        ensure_cdk!(unit == mint_quote.unit, Error::UnsupportedUnit);

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
            .increment_mint_quote_amount_issued(&mint_request.quote, mint_request.total_amount()?)
            .await?;

        self.localstore
            .unset_mint_quote_pending(&mint_request.quote)
            .await?;

        // TODO: bolt 12
        self.pubsub_manager
            .mint_quote_bolt11_status(mint_quote, MintQuoteState::Issued);

        Ok(MintBolt11Response {
            signatures: blind_signatures,
        })
    }
}
