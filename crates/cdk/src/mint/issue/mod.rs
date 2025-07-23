use cdk_common::mint::MintQuote;
use cdk_common::payment::{
    Bolt11IncomingPaymentOptions, Bolt11Settings, Bolt12IncomingPaymentOptions,
    IncomingPaymentOptions, WaitPaymentResponse,
};
use cdk_common::util::unix_time;
use cdk_common::{
    database, ensure_cdk, Amount, CurrencyUnit, Error, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteBolt12Request, MintQuoteBolt12Response, MintQuoteState,
    MintRequest, MintResponse, NotificationPayload, PaymentMethod, PublicKey,
};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::Verification;
use crate::Mint;

#[cfg(feature = "auth")]
mod auth;

/// Request for creating a mint quote
///
/// This enum represents the different types of payment requests that can be used
/// to create a mint quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintQuoteRequest {
    /// Lightning Network BOLT11 invoice request
    Bolt11(MintQuoteBolt11Request),
    /// Lightning Network BOLT12 offer request
    Bolt12(MintQuoteBolt12Request),
}

impl From<MintQuoteBolt11Request> for MintQuoteRequest {
    fn from(request: MintQuoteBolt11Request) -> Self {
        MintQuoteRequest::Bolt11(request)
    }
}

impl From<MintQuoteBolt12Request> for MintQuoteRequest {
    fn from(request: MintQuoteBolt12Request) -> Self {
        MintQuoteRequest::Bolt12(request)
    }
}

/// Response for a mint quote request
///
/// This enum represents the different types of payment responses that can be returned
/// when creating a mint quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintQuoteResponse {
    /// Lightning Network BOLT11 invoice response
    Bolt11(MintQuoteBolt11Response<Uuid>),
    /// Lightning Network BOLT12 offer response
    Bolt12(MintQuoteBolt12Response<Uuid>),
}

impl TryFrom<MintQuoteResponse> for MintQuoteBolt11Response<Uuid> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Bolt11(bolt11_response) => Ok(bolt11_response),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MintQuoteResponse> for MintQuoteBolt12Response<Uuid> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Bolt12(bolt12_response) => Ok(bolt12_response),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MintQuote> for MintQuoteResponse {
    type Error = Error;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        match quote.payment_method {
            PaymentMethod::Bolt11 => {
                let bolt11_response: MintQuoteBolt11Response<Uuid> = quote.into();
                Ok(MintQuoteResponse::Bolt11(bolt11_response))
            }
            PaymentMethod::Bolt12 => {
                let bolt12_response = MintQuoteBolt12Response::try_from(quote)?;
                Ok(MintQuoteResponse::Bolt12(bolt12_response))
            }
            PaymentMethod::Custom(_) => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl From<MintQuoteResponse> for MintQuoteBolt11Response<String> {
    fn from(response: MintQuoteResponse) -> Self {
        match response {
            MintQuoteResponse::Bolt11(bolt11_response) => MintQuoteBolt11Response {
                quote: bolt11_response.quote.to_string(),
                state: bolt11_response.state,
                request: bolt11_response.request,
                expiry: bolt11_response.expiry,
                pubkey: bolt11_response.pubkey,
                amount: bolt11_response.amount,
                unit: bolt11_response.unit,
            },
            _ => panic!("Expected Bolt11 response"),
        }
    }
}

impl Mint {
    /// Validates that a mint request meets all requirements
    ///
    /// Checks that:
    /// - Minting is enabled for the requested payment method
    /// - The currency unit is supported
    /// - The amount (if provided) is within the allowed range for the payment method
    ///
    /// # Arguments
    /// * `amount` - Optional amount to validate
    /// * `unit` - Currency unit for the request
    /// * `payment_method` - Payment method (Bolt11, Bolt12, etc.)
    ///
    /// # Returns
    /// * `Ok(())` if the request is acceptable
    /// * `Error` if any validation fails
    pub async fn check_mint_request_acceptable(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
        payment_method: &PaymentMethod,
    ) -> Result<(), Error> {
        let mint_info = self.localstore.get_mint_info().await?;

        let nut04 = &mint_info.nuts.nut04;
        ensure_cdk!(!nut04.disabled, Error::MintingDisabled);

        let disabled = nut04.disabled;

        ensure_cdk!(!disabled, Error::MintingDisabled);

        let settings = nut04
            .get_settings(unit, payment_method)
            .ok_or(Error::UnsupportedUnit)?;

        let min_amount = settings.min_amount;
        let max_amount = settings.max_amount;

        // Check amount limits if an amount is provided
        if let Some(amount) = amount {
            let is_above_max = max_amount.is_some_and(|max_amount| amount > max_amount);
            let is_below_min = min_amount.is_some_and(|min_amount| amount < min_amount);
            let is_out_of_range = is_above_max || is_below_min;

            ensure_cdk!(
                !is_out_of_range,
                Error::AmountOutofLimitRange(
                    min_amount.unwrap_or_default(),
                    max_amount.unwrap_or_default(),
                    amount,
                )
            );
        }

        Ok(())
    }

    /// Creates a new mint quote for the specified payment request
    ///
    /// Handles both Bolt11 and Bolt12 payment requests by:
    /// 1. Validating the request parameters
    /// 2. Creating an appropriate payment request via the payment processor
    /// 3. Storing the quote in the database
    /// 4. Broadcasting a notification about the new quote
    ///
    /// # Arguments
    /// * `mint_quote_request` - The request containing payment details
    ///
    /// # Returns
    /// * `MintQuoteResponse` - Response with payment details if successful
    /// * `Error` - If the request is invalid or payment creation fails
    #[instrument(skip_all)]
    pub async fn get_mint_quote(
        &self,
        mint_quote_request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse, Error> {
        let unit: CurrencyUnit;
        let amount;
        let pubkey;
        let payment_method;

        let create_invoice_response = match mint_quote_request {
            MintQuoteRequest::Bolt11(bolt11_request) => {
                unit = bolt11_request.unit;
                amount = Some(bolt11_request.amount);
                pubkey = bolt11_request.pubkey;
                payment_method = PaymentMethod::Bolt11;

                self.check_mint_request_acceptable(
                    Some(bolt11_request.amount),
                    &unit,
                    &payment_method,
                )
                .await?;

                let ln = self.get_payment_processor(unit.clone(), PaymentMethod::Bolt11)?;

                let mint_ttl = self.localstore.get_quote_ttl().await?.mint_ttl;

                let quote_expiry = unix_time() + mint_ttl;

                let settings = ln.get_settings().await?;
                let settings: Bolt11Settings = serde_json::from_value(settings)?;

                let description = bolt11_request.description;

                if description.is_some() && !settings.invoice_description {
                    tracing::error!("Backend does not support invoice description");
                    return Err(Error::InvoiceDescriptionUnsupported);
                }

                let bolt11_options = Bolt11IncomingPaymentOptions {
                    description,
                    amount: bolt11_request.amount,
                    unix_expiry: Some(quote_expiry),
                };

                let incoming_options = IncomingPaymentOptions::Bolt11(bolt11_options);

                ln.create_incoming_payment_request(&unit, incoming_options)
                    .await
                    .map_err(|err| {
                        tracing::error!("Could not create invoice: {}", err);
                        Error::InvalidPaymentRequest
                    })?
            }
            MintQuoteRequest::Bolt12(bolt12_request) => {
                unit = bolt12_request.unit;
                amount = bolt12_request.amount;
                pubkey = Some(bolt12_request.pubkey);
                payment_method = PaymentMethod::Bolt12;

                self.check_mint_request_acceptable(amount, &unit, &payment_method)
                    .await?;

                let ln = self.get_payment_processor(unit.clone(), payment_method.clone())?;

                let description = bolt12_request.description;

                let bolt12_options = Bolt12IncomingPaymentOptions {
                    description,
                    amount,
                    unix_expiry: None,
                };

                let incoming_options = IncomingPaymentOptions::Bolt12(Box::new(bolt12_options));

                ln.create_incoming_payment_request(&unit, incoming_options)
                    .await
                    .map_err(|err| {
                        tracing::error!("Could not create invoice: {}", err);
                        Error::InvalidPaymentRequest
                    })?
            }
        };

        let quote = MintQuote::new(
            None,
            create_invoice_response.request.to_string(),
            unit.clone(),
            amount,
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id.clone(),
            pubkey,
            Amount::ZERO,
            Amount::ZERO,
            payment_method.clone(),
            unix_time(),
            vec![],
            vec![],
        );

        tracing::debug!(
            "New {} mint quote {} for {:?} {} with request id {:?}",
            payment_method,
            quote.id,
            amount,
            unit,
            create_invoice_response.request_lookup_id.to_string(),
        );

        let mut tx = self.localstore.begin_transaction().await?;
        tx.add_mint_quote(quote.clone()).await?;
        tx.commit().await?;

        match payment_method {
            PaymentMethod::Bolt11 => {
                let res: MintQuoteBolt11Response<Uuid> = quote.clone().into();
                self.pubsub_manager
                    .broadcast(NotificationPayload::MintQuoteBolt11Response(res));
            }
            PaymentMethod::Bolt12 => {
                let res: MintQuoteBolt12Response<Uuid> = quote.clone().try_into()?;
                self.pubsub_manager
                    .broadcast(NotificationPayload::MintQuoteBolt12Response(res));
            }
            PaymentMethod::Custom(_) => {}
        }

        quote.try_into()
    }

    /// Retrieves all mint quotes from the database
    ///
    /// # Returns
    /// * `Vec<MintQuote>` - List of all mint quotes
    /// * `Error` if database access fails
    #[instrument(skip_all)]
    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.localstore.get_mint_quotes().await?;
        Ok(quotes)
    }

    /// Removes a mint quote from the database
    ///
    /// # Arguments
    /// * `quote_id` - The UUID of the quote to remove
    ///
    /// # Returns
    /// * `Ok(())` if removal was successful
    /// * `Error` if the quote doesn't exist or removal fails
    #[instrument(skip_all)]
    pub async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Error> {
        let mut tx = self.localstore.begin_transaction().await?;
        tx.remove_mint_quote(quote_id).await?;
        tx.commit().await?;

        Ok(())
    }

    /// Marks a mint quote as paid based on the payment request ID
    ///
    /// Looks up the mint quote by the payment request ID and marks it as paid
    /// if found.
    ///
    /// # Arguments
    /// * `wait_payment_response` - Payment response containing payment details
    ///
    /// # Returns
    /// * `Ok(())` if the quote was found and updated
    /// * `Error` if the update fails
    #[instrument(skip_all)]
    pub async fn pay_mint_quote_for_request_id(
        &self,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        if wait_payment_response.payment_amount == Amount::ZERO {
            tracing::warn!(
                "Received payment response with 0 amount with payment id {}.",
                wait_payment_response.payment_id.to_string()
            );

            return Err(Error::AmountUndefined);
        }

        let mut tx = self.localstore.begin_transaction().await?;

        if let Ok(Some(mint_quote)) = tx
            .get_mint_quote_by_request_lookup_id(&wait_payment_response.payment_identifier)
            .await
        {
            self.pay_mint_quote(&mut tx, &mint_quote, wait_payment_response)
                .await?;
        } else {
            tracing::warn!(
                "Could not get request for request lookup id {:?}.",
                wait_payment_response.payment_identifier
            );
        }

        tx.commit().await?;

        Ok(())
    }

    /// Marks a specific mint quote as paid
    ///
    /// Updates the mint quote with payment information and broadcasts
    /// a notification about the payment status change.
    ///
    /// # Arguments
    /// * `mint_quote` - The mint quote to mark as paid
    /// * `wait_payment_response` - Payment response containing payment details
    ///
    /// # Returns
    /// * `Ok(())` if the update was successful
    /// * `Error` if the update fails
    #[instrument(skip_all)]
    pub async fn pay_mint_quote(
        &self,
        tx: &mut Box<dyn database::MintTransaction<'_, database::Error> + Send + Sync + '_>,
        mint_quote: &MintQuote,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        Self::handle_mint_quote_payment(tx, mint_quote, wait_payment_response, &self.pubsub_manager)
            .await
    }

    /// Checks the status of a mint quote and updates it if necessary
    ///
    /// If the quote is unpaid, this will check if payment has been received.
    /// Returns the current state of the quote.
    ///
    /// # Arguments
    /// * `quote_id` - The UUID of the quote to check
    ///
    /// # Returns
    /// * `MintQuoteResponse` - The current state of the quote
    /// * `Error` if the quote doesn't exist or checking fails
    #[instrument(skip(self))]
    pub async fn check_mint_quote(&self, quote_id: &Uuid) -> Result<MintQuoteResponse, Error> {
        let mut quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if quote.payment_method == PaymentMethod::Bolt11 {
            self.check_mint_quote_paid(&mut quote).await?;
        }

        quote.try_into()
    }

    /// Processes a mint request to issue new tokens
    ///
    /// This function:
    /// 1. Verifies the mint quote exists and is paid
    /// 2. Validates the request signature if a pubkey was provided
    /// 3. Verifies the outputs match the expected amount
    /// 4. Signs the blinded messages
    /// 5. Updates the quote status
    /// 6. Broadcasts a notification about the status change
    ///
    /// # Arguments
    /// * `mint_request` - The mint request containing blinded outputs to sign
    ///
    /// # Returns
    /// * `MintBolt11Response` - Response containing blind signatures
    /// * `Error` if validation fails or signing fails
    #[instrument(skip_all)]
    pub async fn process_mint_request(
        &self,
        mint_request: MintRequest<Uuid>,
    ) -> Result<MintResponse, Error> {
        let mut mint_quote = self
            .localstore
            .get_mint_quote(&mint_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if mint_quote.payment_method == PaymentMethod::Bolt11 {
            self.check_mint_quote_paid(&mut mint_quote).await?;
        }

        // get the blind signatures before having starting the db transaction, if there are any
        // rollbacks this blind_signatures will be lost, and the signature is stateless. It is not a
        // good idea to call an external service (which is really a trait, it could be anything
        // anywhere) while keeping a database transaction on-going
        let blind_signatures = self.blind_sign(mint_request.outputs.clone()).await?;

        let mut tx = self.localstore.begin_transaction().await?;

        let mint_quote = tx
            .get_mint_quote(&mint_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        match mint_quote.state() {
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Issued => {
                if mint_quote.payment_method == PaymentMethod::Bolt12
                    && mint_quote.amount_paid() > mint_quote.amount_issued()
                {
                    tracing::warn!("Mint quote should state should have been set to issued upon new payment. Something isn't right. Stopping mint");
                }

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

        let Verification {
            amount: outputs_amount,
            unit,
        } = match self.verify_outputs(&mut tx, &mint_request.outputs).await {
            Ok(verification) => verification,
            Err(err) => {
                tracing::debug!("Could not verify mint outputs");

                return Err(err);
            }
        };

        if mint_quote.payment_method == PaymentMethod::Bolt11 {
            // For bolt11 we enforce that mint amount == quote amount
            if outputs_amount != mint_amount {
                return Err(Error::TransactionUnbalanced(
                    mint_amount.into(),
                    mint_request.total_amount()?.into(),
                    0,
                ));
            }
        } else {
            // For other payments we just make sure outputs is not more then mint amount
            if outputs_amount > mint_amount {
                return Err(Error::TransactionUnbalanced(
                    mint_amount.into(),
                    mint_request.total_amount()?.into(),
                    0,
                ));
            }
        }

        let unit = unit.ok_or(Error::UnsupportedUnit).unwrap();
        ensure_cdk!(unit == mint_quote.unit, Error::UnsupportedUnit);

        tx.add_blind_signatures(
            &mint_request
                .outputs
                .iter()
                .map(|p| p.blinded_secret)
                .collect::<Vec<PublicKey>>(),
            &blind_signatures,
            Some(mint_request.quote),
        )
        .await?;

        let amount_issued = mint_request.total_amount()?;

        let total_issued = tx
            .increment_mint_quote_amount_issued(&mint_request.quote, amount_issued)
            .await?;

        tx.commit().await?;

        self.pubsub_manager
            .mint_quote_issue(&mint_quote, total_issued);

        Ok(MintResponse {
            signatures: blind_signatures,
        })
    }
}
