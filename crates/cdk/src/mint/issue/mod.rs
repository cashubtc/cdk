use std::sync::Arc;

use cdk_common::database::mint::Acquired;
use cdk_common::mint::{MintQuote, Operation};
use cdk_common::payment::{
    Bolt11IncomingPaymentOptions, Bolt12IncomingPaymentOptions, CustomIncomingPaymentOptions,
    IncomingPaymentOptions, WaitPaymentResponse,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::util::unix_time;
use cdk_common::{
    database, ensure_cdk, Amount, BatchMintRequest, BlindedMessage, CurrencyUnit, Error,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteBolt12Request,
    MintQuoteBolt12Response, MintQuoteCustomRequest, MintQuoteCustomResponse, MintQuoteState,
    MintRequest, MintResponse, NotificationPayload, PaymentMethod, PublicKey,
};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use tracing::instrument;

use crate::mint::verification::MAX_REQUEST_FIELD_LEN;
use crate::Mint;

mod auth;

use cdk_common::nut00::KnownMethod;

/// Input enum to handle both single and batch mint formats (internal to CDK, not spec)
#[derive(Debug, Clone)]
pub enum MintInput {
    /// Single quote (legacy NUT-04)
    Single(MintRequest<QuoteId>),
    /// Multiple quotes sharing outputs (NUT-29)
    Batch(BatchMintRequest<QuoteId>),
}

/// Internal representation for unified processing of both single and batch mints
#[derive(Debug, Clone)]
struct QuoteEntry {
    quote_id: QuoteId,
    signature: Option<String>,
    expected_amount: Option<u64>,
}

impl MintInput {
    /// Validates the structure of the mint input
    ///
    /// For single requests, this is a no-op. For batch requests, checks that:
    /// - The quotes list is non-empty
    /// - There are no duplicate quote IDs
    /// - The `quote_amounts` array (if present) has the same length as `quotes`
    /// - The `signatures` array (if present) has the same length as `quotes`
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            MintInput::Single(_) => Ok(()),
            MintInput::Batch(batch) => {
                if batch.quotes.is_empty() {
                    return Err(Error::UnknownQuote);
                }

                let unique_ids: std::collections::HashSet<_> = batch.quotes.iter().collect();
                if unique_ids.len() != batch.quotes.len() {
                    return Err(Error::DuplicateInputs);
                }

                if let Some(ref amounts) = batch.quote_amounts {
                    if amounts.len() != batch.quotes.len() {
                        return Err(Error::TransactionUnbalanced(0, 0, 0));
                    }
                }

                if let Some(ref sigs) = batch.signatures {
                    if sigs.len() != batch.quotes.len() {
                        return Err(Error::SignatureMissingOrInvalid);
                    }
                }

                Ok(())
            }
        }
    }

    fn quote_entries(&self) -> Vec<QuoteEntry> {
        match self {
            MintInput::Single(req) => {
                vec![QuoteEntry {
                    quote_id: req.quote.clone(),
                    signature: req.signature.clone(),
                    expected_amount: None,
                }]
            }
            MintInput::Batch(batch) => batch
                .quotes
                .iter()
                .enumerate()
                .map(|(i, quote_id)| QuoteEntry {
                    quote_id: quote_id.clone(),
                    signature: batch
                        .signatures
                        .as_ref()
                        .and_then(|sigs| sigs.get(i).cloned())
                        .flatten(),
                    expected_amount: batch
                        .quote_amounts
                        .as_ref()
                        .and_then(|a| a.get(i).map(|amt| u64::from(*amt))),
                })
                .collect(),
        }
    }

    /// Returns the list of quote IDs referenced by this mint input
    pub fn quote_ids(&self) -> Vec<QuoteId> {
        match self {
            MintInput::Single(req) => vec![req.quote.clone()],
            MintInput::Batch(batch) => batch.quotes.clone(),
        }
    }

    /// Returns a reference to the blinded messages (outputs) to be signed
    pub fn outputs(&self) -> &[BlindedMessage] {
        match self {
            MintInput::Single(req) => &req.outputs,
            MintInput::Batch(batch) => &batch.outputs,
        }
    }

    /// Returns `true` if this is a batch mint request (NUT-29)
    pub fn is_batch(&self) -> bool {
        matches!(self, MintInput::Batch(_))
    }
}

/// Unified request type for creating mint quotes across different payment methods
///
/// Wraps the protocol-specific request types (BOLT11, BOLT12, custom) into a
/// single enum so the mint can handle quote creation through a common interface.
#[derive(Debug)]
pub enum MintQuoteRequest {
    /// Lightning Network BOLT11 invoice request
    Bolt11(MintQuoteBolt11Request),
    /// Lightning Network BOLT12 offer request
    Bolt12(MintQuoteBolt12Request),
    /// Custom payment method request
    Custom {
        /// Payment method name (e.g., "paypal", "venmo")
        method: String,
        /// Generic request data
        request: MintQuoteCustomRequest,
    },
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
    pub fn amount(&self) -> Option<Amount> {
        match self {
            MintQuoteRequest::Bolt11(request) => Some(request.amount),
            MintQuoteRequest::Bolt12(request) => request.amount,
            MintQuoteRequest::Custom { request, .. } => Some(request.amount),
        }
    }

    /// Get the currency unit from the mint quote request
    pub fn unit(&self) -> CurrencyUnit {
        match self {
            MintQuoteRequest::Bolt11(request) => request.unit.clone(),
            MintQuoteRequest::Bolt12(request) => request.unit.clone(),
            MintQuoteRequest::Custom { request, .. } => request.unit.clone(),
        }
    }

    /// Get the payment method for the mint quote request
    pub fn payment_method(&self) -> PaymentMethod {
        match self {
            MintQuoteRequest::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteRequest::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            MintQuoteRequest::Custom { method, .. } => PaymentMethod::from(method.clone()),
        }
    }

    /// Get the pubkey from the mint quote request
    pub fn pubkey(&self) -> Option<PublicKey> {
        match self {
            MintQuoteRequest::Bolt11(request) => request.pubkey,
            MintQuoteRequest::Bolt12(request) => Some(request.pubkey),
            MintQuoteRequest::Custom { request, .. } => request.pubkey,
        }
    }
}

/// Response for a mint quote request
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintQuoteResponse {
    /// Lightning Network BOLT11 invoice response
    Bolt11(MintQuoteBolt11Response<QuoteId>),
    /// Lightning Network BOLT12 offer response
    Bolt12(MintQuoteBolt12Response<QuoteId>),
    /// Custom payment method response
    Custom {
        /// Payment method name
        method: String,
        /// Generic response data
        response: MintQuoteCustomResponse<QuoteId>,
    },
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
        if quote.payment_method.is_bolt11() {
            let bolt11_response: MintQuoteBolt11Response<QuoteId> = quote.into();
            Ok(MintQuoteResponse::Bolt11(bolt11_response))
        } else if quote.payment_method.is_bolt12() {
            let bolt12_response = MintQuoteBolt12Response::try_from(quote)?;
            Ok(MintQuoteResponse::Bolt12(bolt12_response))
        } else {
            let method = quote.payment_method.to_string();
            let custom_response = MintQuoteCustomResponse::try_from(quote)?;
            Ok(MintQuoteResponse::Custom {
                method,
                response: custom_response,
            })
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
                    if let Some(ref desc) = bolt11_request.description {
                        if desc.len() > MAX_REQUEST_FIELD_LEN {
                            return Err(Error::RequestFieldTooLarge {
                                field: "description".to_string(),
                                actual: desc.len(),
                                max: MAX_REQUEST_FIELD_LEN,
                            });
                        }
                    }

                    let mint_ttl = self.quote_ttl().await?.mint_ttl;

                    let quote_expiry = unix_time() + mint_ttl;

                    let settings = ln.get_settings().await?;

                    let description = bolt11_request.description;

                    if let Some(ref bolt11_settings) = settings.bolt11 {
                        if description.is_some() && !bolt11_settings.invoice_description {
                            tracing::error!("Backend does not support invoice description");
                            return Err(Error::InvoiceDescriptionUnsupported);
                        }
                    }

                    let bolt11_options = Bolt11IncomingPaymentOptions {
                        description,
                        amount: bolt11_request.amount.with_unit(unit.clone()),
                        unix_expiry: Some(quote_expiry),
                    };

                    IncomingPaymentOptions::Bolt11(bolt11_options)
                }
                MintQuoteRequest::Bolt12(bolt12_request) => {
                    if let Some(ref desc) = bolt12_request.description {
                        if desc.len() > MAX_REQUEST_FIELD_LEN {
                            return Err(Error::RequestFieldTooLarge {
                                field: "description".to_string(),
                                actual: desc.len(),
                                max: MAX_REQUEST_FIELD_LEN,
                            });
                        }
                    }

                    let description = bolt12_request.description;

                    let bolt12_options = Bolt12IncomingPaymentOptions {
                        description,
                        amount: amount.map(|a| a.with_unit(unit.clone())),
                        unix_expiry: None,
                    };

                    IncomingPaymentOptions::Bolt12(Box::new(bolt12_options))
                }
                MintQuoteRequest::Custom { method, request } => {
                    if let Some(ref desc) = request.description {
                        if desc.len() > MAX_REQUEST_FIELD_LEN {
                            return Err(Error::RequestFieldTooLarge {
                                field: "description".to_string(),
                                actual: desc.len(),
                                max: MAX_REQUEST_FIELD_LEN,
                            });
                        }
                    }

                    if !request.extra.is_null() {
                        let extra_str = request.extra.to_string();
                        if extra_str.len() > MAX_REQUEST_FIELD_LEN {
                            return Err(Error::RequestFieldTooLarge {
                                field: "extra".to_string(),
                                actual: extra_str.len(),
                                max: MAX_REQUEST_FIELD_LEN,
                            });
                        }
                    }

                    let mint_ttl = self.quote_ttl().await?.mint_ttl;
                    let quote_expiry = unix_time() + mint_ttl;

                    // Convert extra serde_json::Value to JSON string if not null
                    let extra_json = if request.extra.is_null() {
                        None
                    } else {
                        Some(request.extra.to_string())
                    };

                    let custom_options = CustomIncomingPaymentOptions {
                        method: method.to_string(),
                        description: request.description,
                        amount: request.amount.with_unit(unit.clone()),
                        unix_expiry: Some(quote_expiry),
                        extra_json,
                    };

                    IncomingPaymentOptions::Custom(Box::new(custom_options))
                }
            };

            let create_invoice_response = ln
                .create_incoming_payment_request(payment_options)
                .await
                .map_err(|err| {
                    tracing::error!("Could not create invoice: {}", err);
                    Error::InvalidPaymentRequest
                })?;

            let quote = MintQuote::new(
                None,
                create_invoice_response.request.to_string(),
                unit.clone(),
                amount.map(|a| a.with_unit(unit.clone())),
                create_invoice_response.expiry.unwrap_or(0),
                create_invoice_response.request_lookup_id.clone(),
                pubkey,
                Amount::new(0, unit.clone()),
                Amount::new(0, unit.clone()),
                payment_method.clone(),
                unix_time(),
                vec![],
                vec![],
                Some(create_invoice_response.extra_json.unwrap_or_default()),
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

            if payment_method.is_bolt11() {
                let res: MintQuoteBolt11Response<QuoteId> = quote.clone().into();
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt11Response(res));
            } else if payment_method.is_bolt12() {
                let res: MintQuoteBolt12Response<QuoteId> = quote.clone().try_into()?;
                self.pubsub_manager
                    .publish(NotificationPayload::MintQuoteBolt12Response(res));
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
            if wait_payment_response.payment_amount.value() == 0 {
                tracing::warn!(
                    "Received payment response with 0 amount with payment id {}.",
                    wait_payment_response.payment_id.to_string()
                );
                return Err(Error::AmountUndefined);
            }

            let mut tx = self.localstore.begin_transaction().await?;

            let should_notify = if let Ok(Some(mut mint_quote)) = tx
                .get_mint_quote_by_request_lookup_id(&wait_payment_response.payment_identifier)
                .await
            {
                let notify = self
                    .pay_mint_quote(&mut tx, &mut mint_quote, wait_payment_response)
                    .await?;
                if notify {
                    Some((mint_quote.clone(), mint_quote.amount_paid()))
                } else {
                    None
                }
            } else {
                tracing::warn!(
                    "Could not get request for request lookup id {:?}.",
                    wait_payment_response.payment_identifier
                );
                None
            };

            tx.commit().await?;

            // Publish notification AFTER transaction commits
            if let Some((quote, amount_paid)) = should_notify {
                self.pubsub_manager.mint_quote_payment(&quote, amount_paid);
            }

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
    /// Updates the mint quote with payment information and records it in the
    /// database within the given transaction.
    ///
    /// Returns `true` if a payment was recorded and a pubsub notification should
    /// be published **after** the enclosing transaction commits.
    ///
    /// # Arguments
    /// * `tx` - The database transaction to use
    /// * `mint_quote` - The mint quote to mark as paid
    /// * `wait_payment_response` - Payment response containing payment details
    ///
    /// # Returns
    /// * `Ok(true)` if a payment was recorded and notification should be sent
    /// * `Ok(false)` if no new payment was recorded
    /// * `Error` if the update fails
    #[instrument(skip_all)]
    pub async fn pay_mint_quote(
        &self,
        tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
        mint_quote: &mut Acquired<MintQuote>,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<bool, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("pay_mint_quote");

        let result =
            async { Self::handle_mint_quote_payment(tx, mint_quote, wait_payment_response).await }
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
        let result: Result<MintQuoteResponse, Error> = async {
            Ok(self
                .check_mint_quotes(std::slice::from_ref(quote_id))
                .await?
                .first()
                .ok_or(Error::UnknownQuote)?
                .to_owned())
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

    /// Checks the status of multiple mint quotes (NUT-29 batch quote check)
    ///
    /// Validates that all quotes exist and returns their current states.
    /// Returns quotes in the same order as the input.
    ///
    /// # Arguments
    /// * `quote_ids` - The list of quote IDs to check
    ///
    /// # Returns
    /// * `Vec<MintQuoteResponse>` - Current states of all quotes in order
    /// * `Error` if any quote doesn't exist
    #[instrument(skip(self))]
    pub async fn check_mint_quotes(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<MintQuoteResponse>, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("check_mint_quotes");

        let result = async {
            if quote_ids.is_empty() {
                return Err(Error::UnknownQuote);
            }

            let unique_ids: std::collections::HashSet<_> = quote_ids.iter().collect();
            if unique_ids.len() != quote_ids.len() {
                return Err(Error::DuplicateInputs);
            }

            let mut responses = Vec::with_capacity(quote_ids.len());

            for quote_id in quote_ids {
                let mut quote = self
                    .localstore
                    .get_mint_quote(quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;

                self.check_mint_quote_paid(&mut quote).await?;

                responses.push(quote.try_into()?);
            }

            Ok(responses)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("check_mint_quotes");
            METRICS.record_mint_operation("check_mint_quotes", result.is_ok());
            if result.is_err() {
                METRICS.record_error();
            }
        }

        result
    }

    /// Processes a mint request to issue new tokens
    ///
    /// Supports both single (NUT-04) and batch (NUT-29) mint requests.
    /// For batch requests, all quotes must succeed or all fail (atomic).
    ///
    /// This function:
    /// 1. Validates the input structure via `MintInput::validate()`
    /// 2. Validates all quotes (existence, payment, state, amounts, signatures)
    /// 3. Signs the blinded messages
    /// 4. Atomically updates all quotes in a single transaction
    ///
    /// # Arguments
    /// * `input` - Either a single `MintRequest` or a batch `BatchMintRequest`
    ///
    /// # Returns
    /// * `MintResponse` - Response containing all blind signatures
    /// * `Error` if any validation fails or signing fails
    #[instrument(skip_all)]
    pub async fn process_mint_request(&self, input: MintInput) -> Result<MintResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_mint_request");

        let result = async {
            // Phase 1: Validate input structure
            input.validate()?;

            let nut29_settings = if let MintInput::Batch(batch) = &input {
                let mint_info = self.mint_info().await?;
                let settings = mint_info.nuts.nut29;

                if settings.is_empty() {
                    return Err(Error::UnsupportedPaymentMethod);
                }

                if let Some(max_batch_size) = settings.max_batch_size {
                    let max = usize::try_from(max_batch_size).unwrap_or(usize::MAX);
                    if batch.quotes.len() > max {
                        return Err(Error::MaxInputsExceeded {
                            actual: batch.quotes.len(),
                            max,
                        });
                    }
                }

                Some(settings)
            } else {
                None
            };

            let quote_entries = input.quote_entries();
            let quote_ids = input.quote_ids();

            // Verify outputs (keyset, unique blinded secrets, etc.)
            let outputs_amount = self
                .verify_outputs(input.outputs())
                .inspect_err(|_| {
                    tracing::debug!("Could not verify mint outputs");
                })?
                .amount;

            // Fetch all quotes
            let mut quote_map = std::collections::HashMap::new();
            for quote_id in &quote_ids {
                let mut mint_quote = self
                    .localstore
                    .get_mint_quote(quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
                self.check_mint_quote_paid(&mut mint_quote).await?;
                quote_map.insert(quote_id.clone(), mint_quote);
            }

            // Validate all quotes have the same payment method and currency unit
            let Some(first_quote) = quote_map.values().next() else {
                return Err(Error::UnknownQuote);
            };
            let batch_method = first_quote.payment_method.clone();
            let batch_unit = first_quote.unit.clone();
            for (quote_id, quote) in &quote_map {
                if quote.payment_method != batch_method {
                    tracing::error!(
                        "Quote {} has payment method {} but expected {}",
                        quote_id,
                        quote.payment_method,
                        batch_method
                    );
                    return Err(Error::InvalidPaymentMethod);
                }
                if quote.unit != batch_unit {
                    tracing::error!(
                        "Quote {} has unit {} but expected {}",
                        quote_id,
                        quote.unit,
                        batch_unit
                    );
                    return Err(Error::UnitMismatch);
                }
            }

            if let Some(settings) = &nut29_settings {
                if let Some(methods) = &settings.methods {
                    let method = batch_method.to_string();
                    if !methods.iter().any(|configured| configured == &method) {
                        return Err(Error::UnsupportedPaymentMethod);
                    }
                }
            }

            // Phase 2: Per-quote validation
            let mut total_expected_value: u64 = 0;
            let mut expected_amounts: std::collections::HashMap<QuoteId, Amount<CurrencyUnit>> =
                std::collections::HashMap::new();

            for entry in &quote_entries {
                let mint_quote = quote_map.get(&entry.quote_id).ok_or(Error::UnknownQuote)?;

                // Validate quote state
                match mint_quote.state() {
                    MintQuoteState::Unpaid => {
                        return Err(Error::UnpaidQuote);
                    }
                    MintQuoteState::Issued => {
                        if mint_quote.payment_method.is_bolt12()
                            && mint_quote.amount_paid() > mint_quote.amount_issued()
                        {
                            tracing::warn!(
                                "Mint quote {} should have been set to issued upon new payment",
                                entry.quote_id
                            );
                        }
                        return Err(Error::IssuedQuote);
                    }
                    MintQuoteState::Paid => (),
                }

                // Determine the expected amount for this quote.
                //
                // For bolt11 (all-or-nothing): the expected amount is always the
                // quote amount since bolt11 invoices are paid in full.
                //
                // For bolt12 and other methods: the expected amount comes from
                // the batch `quote_amounts` field if present, otherwise it falls
                // back to the full mintable amount (amount_paid - amount_issued).
                let expected_amount = if mint_quote.payment_method.is_bolt11() {
                    mint_quote.amount.clone().ok_or(Error::AmountUndefined)?
                } else if let Some(expected) = entry.expected_amount {
                    Amount::new(expected, mint_quote.unit.clone())
                } else {
                    mint_quote.amount_mintable()
                };

                // Validate the expected amount does not exceed what is actually mintable
                let mintable = mint_quote.amount_mintable();
                if expected_amount > mintable {
                    tracing::error!(
                        "Quote {} expected amount {} exceeds mintable {}",
                        entry.quote_id,
                        expected_amount,
                        mintable
                    );
                    return Err(Error::TransactionUnbalanced(
                        mintable.value(),
                        expected_amount.value(),
                        0,
                    ));
                }

                if expected_amount == Amount::new(0, mint_quote.unit.clone()) {
                    tracing::error!("Quote {} has no mintable amount", entry.quote_id);
                    return Err(Error::UnpaidQuote);
                }

                // Validate bolt12 pubkey requirement
                if mint_quote.payment_method.is_bolt12() && mint_quote.pubkey.is_none() {
                    tracing::warn!(
                        "Bolt12 mint quote {} created without pubkey",
                        entry.quote_id
                    );
                    return Err(Error::SignatureMissingOrInvalid);
                }

                // Verify NUT-20 signature
                if let Some(ref pubkey) = mint_quote.pubkey {
                    match &input {
                        MintInput::Single(request) => request
                            .verify_signature(*pubkey)
                            .map_err(|_| Error::SignatureMissingOrInvalid)?,
                        MintInput::Batch(request) => {
                            let signature = entry
                                .signature
                                .as_ref()
                                .ok_or(Error::SignatureMissingOrInvalid)?;

                            request
                                .verify_quote_signature(&entry.quote_id, signature, pubkey)
                                .map_err(|_| Error::SignatureMissingOrInvalid)?;
                        }
                    }
                } else if entry.signature.is_some() {
                    // Quote is unlocked but signature was provided
                    return Err(Error::SignatureMissingOrInvalid);
                }

                total_expected_value = total_expected_value
                    .checked_add(expected_amount.value())
                    .ok_or(Error::AmountOverflow)?;
                expected_amounts.insert(entry.quote_id.clone(), expected_amount);
            }

            // Phase 3: Amount validation
            ensure_cdk!(outputs_amount.unit() == &batch_unit, Error::UnsupportedUnit);

            if outputs_amount.value() != total_expected_value {
                return Err(Error::TransactionUnbalanced(
                    total_expected_value,
                    outputs_amount.value(),
                    0,
                ));
            }

            // Phase 4: Generate blind signatures (stateless, safe outside transaction)
            let all_blind_signatures = self.blind_sign(input.outputs().to_vec()).await?;
            let blinded_secrets = input
                .outputs()
                .iter()
                .map(|p| p.blinded_secret)
                .collect::<Vec<PublicKey>>();

            // Phase 5: Atomic database transaction
            let mut tx = self.localstore.begin_transaction().await?;

            // For batch minting, outputs are shared across all quotes and should be persisted once.
            if input.is_batch() {
                let batch_operation =
                    Operation::new_batch_mint(outputs_amount.clone().into(), batch_method.clone());
                tx.add_blinded_messages(None, input.outputs(), &batch_operation)
                    .await?;
                tx.add_blind_signatures(&blinded_secrets, &all_blind_signatures, None)
                    .await?;
                let fee_by_keyset = std::collections::HashMap::new();
                tx.add_completed_operation(&batch_operation, &fee_by_keyset)
                    .await?;
            }

            for quote_id in &quote_ids {
                // Get the mutable quote from transaction
                let mut mint_quote = tx
                    .get_mint_quote(quote_id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;

                // Re-validate state within transaction (protects against race conditions)
                match mint_quote.state() {
                    MintQuoteState::Unpaid => {
                        return Err(Error::UnpaidQuote);
                    }
                    MintQuoteState::Issued => {
                        return Err(Error::IssuedQuote);
                    }
                    MintQuoteState::Paid => (),
                }

                let amount_issued = if input.is_batch() {
                    // For batch: each quote is issued for its expected amount
                    // (outputs are shared, not split per-quote)
                    expected_amounts
                        .get(quote_id)
                        .cloned()
                        .ok_or(Error::UnknownQuote)?
                } else {
                    // For single: issued amount = total outputs amount
                    outputs_amount.clone()
                };

                let operation = Operation::new_mint(
                    amount_issued.clone().into(),
                    mint_quote.payment_method.clone(),
                );

                if !input.is_batch() {
                    tx.add_blinded_messages(Some(quote_id), input.outputs(), &operation)
                        .await?;

                    tx.add_blind_signatures(
                        &blinded_secrets,
                        &all_blind_signatures,
                        Some(quote_id.clone()),
                    )
                    .await?;
                }

                mint_quote.add_issuance(amount_issued)?;
                tx.update_mint_quote(&mut mint_quote).await?;

                // Mint operations have no input fees
                // Only persist operation for non-batch mints (batch operations are persisted above)
                if !input.is_batch() {
                    let fee_by_keyset = std::collections::HashMap::new();
                    tx.add_completed_operation(&operation, &fee_by_keyset)
                        .await?;
                }
            }

            tx.commit().await?;

            let localstore = Arc::clone(&self.localstore);
            let pubsub_manager = Arc::clone(&self.pubsub_manager);
            tokio::spawn(async move {
                // Publish notifications after successful commit
                if let Ok(quotes) = localstore.get_mint_quotes_by_ids(&quote_ids).await {
                    for mint_quote in quotes.iter().flatten() {
                        pubsub_manager.mint_quote_issue(mint_quote, mint_quote.amount_issued());
                    }
                }
            });

            Ok(MintResponse {
                signatures: all_blind_signatures,
            })
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
}

#[cfg(test)]
mod batch_mint_tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk_common::amount::SplitTarget;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::PreMintSecrets;
    use cdk_common::{
        Amount, BatchMintRequest, CurrencyUnit, Error, MintQuoteBolt11Request,
        MintQuoteBolt11Response, MintQuoteState, MintRequest, PaymentMethod, QuoteId,
    };
    use cdk_fake_wallet::FakeWallet;
    use tokio::time::sleep;

    use crate::mint::{Mint, MintBuilder, MintMeltLimits};
    use crate::types::{FeeReserve, QuoteTTL};

    async fn create_test_mint() -> Mint {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());

        let mut mint_builder = MintBuilder::new(db.clone());

        let fee_reserve = FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };

        let ln_fake_backend = FakeWallet::new(
            fee_reserve.clone(),
            HashMap::default(),
            HashSet::default(),
            2,
            CurrencyUnit::Sat,
        );

        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(ln_fake_backend),
            )
            .await
            .unwrap();

        let mnemonic = Mnemonic::generate(12).unwrap();

        mint_builder = mint_builder
            .with_name("test mint".to_string())
            .with_description("test mint for unit tests".to_string())
            .with_urls(vec!["https://test-mint".to_string()])
            .with_batch_minting(None, Some(vec!["bolt11".to_string()]));

        let quote_ttl = QuoteTTL::new(10000, 10000);

        let mint = mint_builder
            .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();

        mint.set_quote_ttl(quote_ttl).await.unwrap();

        mint.start().await.unwrap();

        mint
    }

    async fn configure_nut29(
        mint: &Mint,
        max_batch_size: Option<u64>,
        methods: Option<Vec<String>>,
    ) {
        let mut mint_info = mint.mint_info().await.unwrap();
        mint_info.nuts.nut29 = cdk_common::nut29::Settings::new(max_batch_size, methods);
        mint.set_mint_info(mint_info).await.unwrap();
    }

    async fn wait_for_quote_paid(mint: &Mint, quote_id: &QuoteId) {
        loop {
            let check = mint
                .check_mint_quotes(std::slice::from_ref(quote_id))
                .await
                .unwrap();
            if let crate::mint::MintQuoteResponse::Bolt11(quote) = &check[0] {
                if quote.state == MintQuoteState::Paid {
                    break;
                }
            }
            sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    #[tokio::test]
    async fn test_process_batch_mint_basic() {
        let mint = create_test_mint().await;

        // Create two quotes
        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        // Wait for both to be paid
        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        // Get active keyset and create outputs for total amount (64)
        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let response = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await
            .unwrap();

        // Should return 64 sats worth of blind signatures
        let total_sig_amount: u64 = response.signatures.iter().map(|s| s.amount.to_u64()).sum();
        assert_eq!(total_sig_amount, 64);
    }

    #[tokio::test]
    async fn test_process_batch_mint_unpaid_quote() {
        let mint = create_test_mint().await;

        // Create two quotes
        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        // Wait for only one to be paid
        wait_for_quote_paid(&mint, &quote1.quote).await;

        // Get active keyset and create outputs
        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(32), // Only quote1's amount
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::UnpaidQuote | Error::TransactionUnbalanced(_, _, _)
        ));
    }

    #[tokio::test]
    async fn test_process_batch_mint_inflated_outputs() {
        let mint = create_test_mint().await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        // Create outputs for 128 sats (double the total quotes)
        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(128),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::TransactionUnbalanced(_, _, _)
        ));
    }

    #[tokio::test]
    async fn test_process_batch_mint_duplicate_quotes() {
        let mint = create_test_mint().await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(32),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        // Duplicate quote ID
        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote1.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::DuplicateInputs));
    }

    #[tokio::test]
    async fn test_process_batch_mint_mismatched_amounts_length() {
        let mint = create_test_mint().await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        // Only one amount for two quotes
        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: Some(vec![Amount::from(32)]), // Only one amount!
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::TransactionUnbalanced(_, _, _)
        ));
    }

    #[tokio::test]
    async fn test_process_batch_mint_atomicity() {
        let mint = create_test_mint().await;

        // First, mint normally for one quote
        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_single = PreMintSecrets::random(
            keyset_id,
            Amount::from(32),
            &SplitTarget::None,
            &fees.clone().into(),
        )
        .unwrap();

        let single_request = MintRequest {
            quote: quote1.quote.clone(),
            outputs: premint_single.blinded_messages().to_vec(),
            signature: None,
        };

        mint.process_mint_request(crate::mint::MintInput::Single(single_request))
            .await
            .unwrap();

        // Now create a second quote and try to batch mint with the already-issued one
        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote2.quote).await;

        let premint_batch = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.clone().into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_batch.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::IssuedQuote));

        let statuses = mint
            .check_mint_quotes(&[quote1.quote.clone(), quote2.quote.clone()])
            .await
            .unwrap();

        let quote1_status = statuses
            .iter()
            .find_map(|status| match status {
                crate::mint::MintQuoteResponse::Bolt11(quote) if quote.quote == quote1.quote => {
                    Some(quote.state)
                }
                _ => None,
            })
            .expect("quote1 status");
        let quote2_status = statuses
            .iter()
            .find_map(|status| match status {
                crate::mint::MintQuoteResponse::Bolt11(quote) if quote.quote == quote2.quote => {
                    Some(quote.state)
                }
                _ => None,
            })
            .expect("quote2 status");

        assert_eq!(quote1_status, MintQuoteState::Issued);
        assert_eq!(quote2_status, MintQuoteState::Paid);
    }

    #[tokio::test]
    async fn test_process_batch_mint_enforces_max_batch_size() {
        let mint = create_test_mint().await;
        configure_nut29(&mint, Some(1), None).await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::MaxInputsExceeded { actual: 2, max: 1 }
        ));
    }

    #[tokio::test]
    async fn test_process_batch_mint_enforces_allowed_methods() {
        let mint = create_test_mint().await;
        configure_nut29(&mint, None, Some(vec!["bolt12".to_string()])).await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::UnsupportedPaymentMethod
        ));
    }

    #[tokio::test]
    async fn test_process_batch_mint_rejects_when_nut29_not_configured() {
        let mint = create_test_mint().await;
        configure_nut29(&mint, None, None).await;

        let quote1: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        let quote2: MintQuoteBolt11Response<QuoteId> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        wait_for_quote_paid(&mint, &quote1.quote).await;
        wait_for_quote_paid(&mint, &quote2.quote).await;

        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let keys = mint
            .keyset_pubkeys(&keyset_id)
            .unwrap()
            .keysets
            .first()
            .unwrap()
            .keys
            .clone();
        let fees: (u64, Vec<u64>) = (0, keys.iter().map(|a| a.0.to_u64()).collect::<Vec<_>>());

        let premint_secrets = PreMintSecrets::random(
            keyset_id,
            Amount::from(64),
            &SplitTarget::None,
            &fees.into(),
        )
        .unwrap();

        let batch_request = BatchMintRequest {
            quotes: vec![quote1.quote.clone(), quote2.quote.clone()],
            quote_amounts: None,
            outputs: premint_secrets.blinded_messages().to_vec(),
            signatures: None,
        };

        let result = mint
            .process_mint_request(crate::mint::MintInput::Batch(batch_request))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::UnsupportedPaymentMethod
        ));
    }
}
