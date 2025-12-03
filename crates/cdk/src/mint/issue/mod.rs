use std::{collections::HashSet, str::FromStr};

use cdk_common::mint::{BatchMintRequest, MintQuote, Operation};
use cdk_common::payment::{
    Bolt11IncomingPaymentOptions, Bolt11Settings, Bolt12IncomingPaymentOptions,
    IncomingPaymentOptions, WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::util::unix_time;
use cdk_common::{
    database, ensure_cdk, Amount, CurrencyUnit, Error, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteBolt12Request, MintQuoteBolt12Response, MintQuoteState,
    MintRequest, MintResponse, NotificationPayload, PaymentMethod, PublicKey,
};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use tracing::instrument;

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

impl MintQuoteRequest {
    /// Get the amount from the mint quote request
    ///
    /// For Bolt11 requests, this returns `Some(amount)` as the amount is required.
    /// For Bolt12 requests, this returns the optional amount.
    pub fn amount(&self) -> Option<Amount> {
        match self {
            MintQuoteRequest::Bolt11(request) => Some(request.amount),
            MintQuoteRequest::Bolt12(request) => request.amount,
        }
    }

    /// Get the currency unit from the mint quote request
    pub fn unit(&self) -> CurrencyUnit {
        match self {
            MintQuoteRequest::Bolt11(request) => request.unit.clone(),
            MintQuoteRequest::Bolt12(request) => request.unit.clone(),
        }
    }

    /// Get the payment method for the mint quote request
    pub fn payment_method(&self) -> PaymentMethod {
        match self {
            MintQuoteRequest::Bolt11(_) => PaymentMethod::Bolt11,
            MintQuoteRequest::Bolt12(_) => PaymentMethod::Bolt12,
        }
    }

    /// Get the pubkey from the mint quote request
    ///
    /// For Bolt11 requests, this returns the optional pubkey.
    /// For Bolt12 requests, this returns `Some(pubkey)` as the pubkey is required.
    pub fn pubkey(&self) -> Option<PublicKey> {
        match self {
            MintQuoteRequest::Bolt11(request) => request.pubkey,
            MintQuoteRequest::Bolt12(request) => Some(request.pubkey),
        }
    }
}

/// Response for a mint quote request
///
/// This enum represents the different types of payment responses that can be returned
/// when creating a mint quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintQuoteResponse {
    /// Lightning Network BOLT11 invoice response
    Bolt11(MintQuoteBolt11Response<QuoteId>),
    /// Lightning Network BOLT12 offer response
    Bolt12(MintQuoteBolt12Response<QuoteId>),
}

impl TryFrom<MintQuoteResponse> for MintQuoteBolt11Response<QuoteId> {
    type Error = Error;

    fn try_from(response: MintQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MintQuoteResponse::Bolt11(bolt11_response) => Ok(bolt11_response),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MintQuoteResponse> for MintQuoteBolt12Response<QuoteId> {
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
                let bolt11_response: MintQuoteBolt11Response<QuoteId> = quote.into();
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
    /// # Returns
    /// * `Ok(())` if the request is acceptable
    /// * `Error` if any validation fails
    pub async fn check_mint_request_acceptable(
        &self,
        mint_quote_request: &MintQuoteRequest,
    ) -> Result<(), Error> {
        let mint_info = self.mint_info().await?;

        let unit = mint_quote_request.unit();
        let amount = mint_quote_request.amount();
        let payment_method = mint_quote_request.payment_method();

        let nut04 = &mint_info.nuts.nut04;
        ensure_cdk!(!nut04.disabled, Error::MintingDisabled);

        let disabled = nut04.disabled;

        ensure_cdk!(!disabled, Error::MintingDisabled);

        let settings = nut04
            .get_settings(&unit, &payment_method)
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
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_mint_quote");

        let result = async {
            // Use the new getters for cleaner code
            let unit = mint_quote_request.unit();
            let amount = mint_quote_request.amount();
            let payment_method = mint_quote_request.payment_method();

            // Validate the request before processing
            self.check_mint_request_acceptable(&mint_quote_request)
                .await?;

            // Extract pubkey using the getter
            let pubkey = mint_quote_request.pubkey();

            let ln = self.get_payment_processor(unit.clone(), payment_method.clone())?;

            let payment_options = match mint_quote_request {
                MintQuoteRequest::Bolt11(bolt11_request) => {
                    let mint_ttl = self.quote_ttl().await?.mint_ttl;

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

                    IncomingPaymentOptions::Bolt11(bolt11_options)
                }
                MintQuoteRequest::Bolt12(bolt12_request) => {
                    let description = bolt12_request.description;

                    let bolt12_options = Bolt12IncomingPaymentOptions {
                        description,
                        amount,
                        unix_expiry: None,
                    };

                    IncomingPaymentOptions::Bolt12(Box::new(bolt12_options))
                }
            };

            let create_invoice_response = ln
                .create_incoming_payment_request(&unit, payment_options)
                .await
                .map_err(|err| {
                    tracing::error!("Could not create invoice: {}", err);
                    Error::InvalidPaymentRequest
                })?;

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
                    let res: MintQuoteBolt11Response<QuoteId> = quote.clone().into();
                    self.pubsub_manager
                        .publish(NotificationPayload::MintQuoteBolt11Response(res));
                }
                PaymentMethod::Bolt12 => {
                    let res: MintQuoteBolt12Response<QuoteId> = quote.clone().try_into()?;
                    self.pubsub_manager
                        .publish(NotificationPayload::MintQuoteBolt12Response(res));
                }
                PaymentMethod::Custom(_) => {}
            }

            quote.try_into()
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("get_mint_quote");
            METRICS.record_mint_operation("get_mint_quote", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
    }

    /// Retrieves all mint quotes from the database
    ///
    /// # Returns
    /// * `Vec<MintQuote>` - List of all mint quotes
    /// * `Error` if database access fails
    #[instrument(skip_all)]
    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("mint_quotes");

        let result = async {
            let quotes = self.localstore.get_mint_quotes().await?;
            Ok(quotes)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("mint_quotes");
            METRICS.record_mint_operation("mint_quotes", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
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
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("pay_mint_quote_for_request_id");
        let result = async {
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
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("pay_mint_quote_for_request_id");
            METRICS.record_mint_operation("pay_mint_quote_for_request_id", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
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
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("pay_mint_quote");

        let result = async {
            Self::handle_mint_quote_payment(
                tx,
                mint_quote,
                wait_payment_response,
                &self.pubsub_manager,
            )
            .await
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("pay_mint_quote");
            METRICS.record_mint_operation("pay_mint_quote", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
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
    pub async fn check_mint_quote(&self, quote_id: &QuoteId) -> Result<MintQuoteResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("check_mint_quote");
        let result = async {
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
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("check_mint_quote");
            METRICS.record_mint_operation("check_mint_quote", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
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
        mint_request: MintRequest<QuoteId>,
    ) -> Result<MintResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_mint_request");
        let result = async {
            let MintRequest {
                quote,
                outputs,
                signature,
            } = mint_request;

            let normalized_request = BatchMintRequest {
                quote: vec![quote.to_string()],
                outputs,
                signature: signature.map(|sig| vec![Some(sig)]),
            };

            self.process_mint_workload(
                normalized_request,
                PaymentMethod::Bolt11,
                MintRequestOrigin::Single,
            )
            .await
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("process_mint_request");
            METRICS.record_mint_operation("process_mint_request", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }
        result
    }

    /// Process batch mint request for multiple quotes
    ///
    /// Creates blind signatures for multiple quotes in a single atomic operation.
    /// All quotes must belong to the same payment method (architectural constraint).
    ///
    /// Per NUT-XX specification (https://github.com/cashubtc/nuts/issues/XX):
    /// - All quotes MUST be from the same payment method
    /// - All quotes MUST use the same currency unit
    /// - All quotes MUST be unique (no duplicate quote IDs)
    /// - Quote payment methods MUST match the URL path method (bolt11 vs bolt12)
    ///
    /// # Arguments
    /// * `batch_request` - Batch mint request containing quote IDs and outputs
    ///
    /// # Returns
    /// * `MintResponse` - Response containing blind signatures in order
    /// * `Error` if validation fails, quotes are not found, or signing fails
    #[instrument(skip_all, fields(quote_count = batch_request.quote.len()))]
    pub async fn process_batch_mint_request(
        &self,
        batch_request: BatchMintRequest,
        endpoint_payment_method: PaymentMethod,
    ) -> Result<MintResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_batch_mint_request");
        let result = async {
            self.process_mint_workload(
                batch_request,
                endpoint_payment_method,
                MintRequestOrigin::Batch,
            )
            .await
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("process_batch_mint_request");
            METRICS.record_mint_operation("process_batch_mint_request", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }
        result
    }

    #[instrument(skip_all, fields(origin = ?origin, quote_count = batch_request.quote.len()))]
    async fn process_mint_workload(
        &self,
        batch_request: BatchMintRequest,
        endpoint_payment_method: PaymentMethod,
        origin: MintRequestOrigin,
    ) -> Result<MintResponse, Error> {
        const BATCH_SIZE_LIMIT: usize = 100;

        if batch_request.quote.is_empty() {
            return Err(Error::BatchEmpty);
        }

        if origin == MintRequestOrigin::Batch && batch_request.quote.len() > BATCH_SIZE_LIMIT {
            return Err(Error::BatchSizeExceeded);
        }

        if origin == MintRequestOrigin::Batch {
            let mut seen = HashSet::new();
            for quote_id_str in &batch_request.quote {
                if !seen.insert(quote_id_str) {
                    return Err(Error::DuplicateQuoteIdInBatch);
                }
            }
        }

        let mut quote_ids = Vec::with_capacity(batch_request.quote.len());
        for quote_id_str in &batch_request.quote {
            let quote_id = QuoteId::from_str(quote_id_str).map_err(|_| Error::UnknownQuote)?;
            quote_ids.push(quote_id);
        }

        let blind_signatures = self.blind_sign(batch_request.outputs.clone()).await?;
        let mut tx = self.localstore.begin_transaction().await?;

        let mut quotes = Vec::with_capacity(quote_ids.len());
        for quote_id in &quote_ids {
            let mut quote = tx
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;

            if quote.payment_method == PaymentMethod::Bolt11 {
                self.check_mint_quote_paid(&mut quote).await?;
            }

            if origin == MintRequestOrigin::Single
                && quote.payment_method == PaymentMethod::Bolt12
                && quote.pubkey.is_none()
            {
                tracing::warn!("Bolt12 mint quote created without pubkey");
                return Err(Error::SignatureMissingOrInvalid);
            }

            quotes.push(quote);
        }

        let payment_method = quotes
            .first()
            .map(|q| q.payment_method.clone())
            .ok_or(Error::BatchEmpty)?;

        if !quotes.iter().all(|q| q.payment_method == payment_method) {
            return Err(Error::BatchPaymentMethodMismatch);
        }

        if payment_method != endpoint_payment_method {
            return Err(Error::BatchPaymentMethodEndpointMismatch);
        }

        let unit = quotes
            .first()
            .map(|q| q.unit.clone())
            .ok_or(Error::UnknownQuote)?;
        if !quotes.iter().all(|q| q.unit == unit) {
            return Err(Error::BatchCurrencyUnitMismatch);
        }

        if let Some(signatures) = &batch_request.signature {
            if signatures.len() != quotes.len() {
                return Err(Error::BatchSignatureCountMismatch);
            }

            for (i, (quote, signature)) in quotes.iter().zip(signatures.iter()).enumerate() {
                if let Some(sig_str) = signature {
                    if let Some(pubkey) = &quote.pubkey {
                        let mint_req = cdk_common::nuts::MintRequest {
                            quote: batch_request.quote[i].clone(),
                            outputs: batch_request.outputs.clone(),
                            signature: Some(sig_str.clone()),
                        };

                        mint_req
                            .verify_signature(pubkey.clone())
                            .map_err(|_| Error::BatchInvalidSignature)?;
                    } else {
                        return Err(Error::BatchUnexpectedSignature);
                    }
                } else if quote.pubkey.is_some() && origin == MintRequestOrigin::Single {
                    return Err(Error::SignatureMissingOrInvalid);
                }
            }
        } else if origin == MintRequestOrigin::Single
            && quotes.iter().any(|quote| quote.pubkey.is_some())
        {
            return Err(Error::SignatureMissingOrInvalid);
        }

        for quote in &quotes {
            match quote.state() {
                MintQuoteState::Unpaid => {
                    return Err(Error::UnpaidQuote);
                }
                MintQuoteState::Issued => {
                    if quote.payment_method == PaymentMethod::Bolt12
                        && quote.amount_paid() > quote.amount_issued()
                    {
                        tracing::warn!("Mint quote should state should have been set to issued upon new payment. Something isn't right. Stopping mint");
                    }

                    return Err(Error::IssuedQuote);
                }
                MintQuoteState::Paid => (),
            }
        }

        let mut total_mint_amount = Amount::ZERO;
        let mut per_quote_amounts = Vec::with_capacity(quotes.len());
        for quote in &quotes {
            let quote_amount = match payment_method {
                PaymentMethod::Bolt11 => {
                    let amt = quote.amount.ok_or(Error::AmountUndefined)?;
                    if amt != quote.amount_mintable() {
                        tracing::error!(
                            "The quote amount {} does not equal the amount paid {}.",
                            amt,
                            quote.amount_mintable()
                        );
                        return Err(Error::IncorrectQuoteAmount);
                    }
                    amt
                }
                PaymentMethod::Bolt12 => {
                    if quote.amount_mintable() == Amount::ZERO {
                        tracing::error!(
                            "Quote state should not be issued if issued {} is => paid {}.",
                            quote.amount_issued(),
                            quote.amount_paid()
                        );
                        return Err(Error::UnpaidQuote);
                    }
                    quote.amount_mintable()
                }
                _ => return Err(Error::UnsupportedPaymentMethod),
            };

            total_mint_amount = total_mint_amount
                .checked_add(quote_amount)
                .ok_or(Error::AmountOverflow)?;
            per_quote_amounts.push(quote_amount);
        }

        let Verification {
            amount: outputs_amount,
            unit: output_unit,
        } = match self.verify_outputs(&mut tx, &batch_request.outputs).await {
            Ok(verification) => verification,
            Err(err) => {
                tracing::debug!("Could not verify mint outputs");
                return Err(err);
            }
        };

        if payment_method == PaymentMethod::Bolt11 {
            if outputs_amount != total_mint_amount {
                return Err(Error::TransactionUnbalanced(
                    total_mint_amount.into(),
                    batch_request.total_amount()?.into(),
                    0,
                ));
            }
        } else if outputs_amount > total_mint_amount {
            return Err(Error::TransactionUnbalanced(
                total_mint_amount.into(),
                batch_request.total_amount()?.into(),
                0,
            ));
        }

        let output_unit = output_unit.ok_or(Error::UnsupportedUnit)?;
        ensure_cdk!(output_unit == unit, Error::UnsupportedUnit);

        let operation = Operation::new_mint();
        let associated_quote = if origin == MintRequestOrigin::Single {
            quote_ids.first().cloned()
        } else {
            None
        };

        tx.add_blinded_messages(
            associated_quote.as_ref(),
            &batch_request.outputs,
            &operation,
        )
        .await?;

        tx.add_blind_signatures(
            &batch_request
                .outputs
                .iter()
                .map(|p| p.blinded_secret)
                .collect::<Vec<PublicKey>>(),
            &blind_signatures,
            associated_quote.clone(),
        )
        .await?;

        let mut total_issued_per_quote = Vec::with_capacity(quote_ids.len());
        for (i, quote_id) in quote_ids.iter().enumerate() {
            let total_issued = tx
                .increment_mint_quote_amount_issued(quote_id, per_quote_amounts[i])
                .await?;
            total_issued_per_quote.push(total_issued);
        }

        tx.commit().await?;

        for (quote, total_issued) in quotes.iter().zip(total_issued_per_quote.iter()) {
            self.pubsub_manager.mint_quote_issue(quote, *total_issued);
        }

        Ok(MintResponse {
            signatures: blind_signatures,
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum MintRequestOrigin {
    Single,
    Batch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::mint::{create_test_blinded_messages, create_test_mint};
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn single_origin_helper_path_processes_quote() {
        let mint = create_test_mint().await.unwrap();
        let amount = Amount::from(64u64);

        let MintQuoteResponse::Bolt11(quote) = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount,
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
        else {
            unreachable!("expected bolt11 quote");
        };

        let quote_id = quote.quote.clone();

        loop {
            if let MintQuoteResponse::Bolt11(status) =
                mint.check_mint_quote(&quote_id).await.unwrap()
            {
                if status.state == MintQuoteState::Paid {
                    break;
                }
            }
            sleep(Duration::from_millis(50)).await;
        }

        let (outputs, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

        let batch_request = BatchMintRequest {
            quote: vec![quote_id.to_string()],
            outputs: outputs.clone(),
            signature: None,
        };

        let response = mint
            .process_mint_workload(
                batch_request,
                PaymentMethod::Bolt11,
                MintRequestOrigin::Single,
            )
            .await
            .unwrap();

        assert_eq!(response.signatures.len(), outputs.len());

        if let MintQuoteResponse::Bolt11(updated) = mint.check_mint_quote(&quote_id).await.unwrap()
        {
            assert_eq!(updated.state, MintQuoteState::Issued);
        } else {
            panic!("expected bolt11 quote response");
        }
    }
}
