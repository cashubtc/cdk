//! Generic handlers for custom payment methods
//!
//! These handlers work for ANY custom payment method without requiring
//! method-specific validation or request parsing.
//!
//! Special handling for bolt11 and bolt12:
//! When the method parameter is "bolt11" or "bolt12", these handlers use the
//! specific Bolt11/Bolt12 request/response types instead of the generic custom types.

use axum::extract::{FromRequestParts, Json, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::mint::QuoteId;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, MeltOnchainRequest, MeltQuoteBolt11Request,
    MeltQuoteBolt12Request, MeltQuoteCustomRequest, MeltQuoteOnchainRequest,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteBolt12Request,
    MintQuoteBolt12Response, MintQuoteCustomRequest, MintQuoteOnchainRequest,
    MintQuoteOnchainResponse, MintRequest, MintResponse, PaymentMethod,
};
use cdk::{MeltQuoteCreateResponse, MeltQuoteResponse};
use serde_json::Value;
use tracing::instrument;

use crate::auth::AuthHeader;
use crate::router_handlers::into_response;
use crate::MintState;

const PREFER_HEADER_KEY: &str = "Prefer";
#[cfg(feature = "federation")]
const FEDERATION_MINT_PAYMENT_WAIT_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);

/// Header extractor for the Prefer header
///
/// This extractor checks for the `Prefer: respond-async` header
/// to determine if the client wants asynchronous processing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferHeader {
    pub respond_async: bool,
}

async fn create_mint_quote_for_route(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
) -> Result<
    (
        cdk::mint::MintQuoteResponse<QuoteId>,
        Option<cdk::cdk_payment::PaymentIdentifier>,
    ),
    cdk::Error,
> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        #[cfg(feature = "federation-bark")]
        if matches!(request, cdk::mint::MintQuoteRequest::Bolt11(_))
            && state.federation.bark_runtime().is_some()
        {
            let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let quote_id = crate::federation::bark_incoming::federated_bark_quote_id(
                federation.federation_id(),
                &request,
            )?;
            state.mint.check_mint_request_acceptable(&request).await?;
            let (response, request_lookup_id) =
                crate::federation::bark_incoming::prepare_federated_bark_mint_quote(
                    state, quote_id, request,
                )
                .await?;
            return Ok((response, Some(request_lookup_id)));
        }
        #[cfg(feature = "federation-bdk")]
        if matches!(request, cdk::mint::MintQuoteRequest::Onchain(_)) {
            let quote_id = QuoteId::new();
            state.mint.check_mint_request_acceptable(&request).await?;
            let runtime = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let application = runtime.cdk_onchain_application()?;
            let address = state
                .allocate_federation_onchain_address(
                    &quote_id.to_string(),
                    application.application_id,
                    application.application_key_epoch,
                    cdk::federation::FederationAddressKeychain::External,
                )
                .await?;
            let (response, request_lookup_id) = state
                .mint
                .prepare_onchain_mint_quote(quote_id, request, address)
                .await?;
            return Ok((response, Some(request_lookup_id)));
        }
        let (response, request_lookup_id) = state.mint.prepare_mint_quote(request).await?;
        return Ok((response, Some(request_lookup_id)));
    }

    Ok((state.mint.get_mint_quote(request).await?, None))
}

async fn check_mint_quotes_for_route(
    state: &MintState,
    quote_ids: &[QuoteId],
) -> Result<Vec<cdk::mint::MintQuoteResponse<QuoteId>>, cdk::Error> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_mint_quotes(state, quote_ids),
        )
        .await?;
        let mut responses = Vec::with_capacity(quote_ids.len());
        for quote_id in quote_ids {
            let quote = state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            responses.push(quote.try_into()?);
        }
        return Ok(responses);
    }

    state.mint.check_mint_quotes(quote_ids).await
}

async fn check_melt_quote_for_route(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_melt_quote(state, quote_id),
        )
        .await?;
        return federation_melt_response_from_store(state, quote_id).await;
    }

    state.mint.check_melt_quote(quote_id).await
}

#[cfg(feature = "federation")]
type RouteFederationMeltQuoteMetadata = cdk::federation::FederationMeltQuoteMetadata;
#[cfg(not(feature = "federation"))]
type RouteFederationMeltQuoteMetadata = ();

async fn create_melt_quote_for_route(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
) -> Result<
    (
        MeltQuoteCreateResponse<QuoteId>,
        Option<RouteFederationMeltQuoteMetadata>,
    ),
    cdk::Error,
> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        let (response, metadata) = state.mint.prepare_melt_quote(request).await?;
        return Ok((response, Some(metadata)));
    }

    Ok((state.mint.get_melt_quote(request).await?, None))
}

impl<S> FromRequestParts<S> for PreferHeader
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> anyhow::Result<Self, Self::Rejection> {
        // Check for Prefer header
        if let Some(prefer_value) = parts.headers.get(PREFER_HEADER_KEY) {
            let value = prefer_value.to_str().map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid Prefer header value".to_string(),
                )
            })?;

            // Check if it contains "respond-async"
            let respond_async = value.to_lowercase().contains("respond-async");

            return Ok(PreferHeader { respond_async });
        }

        // No Prefer header found - default to synchronous processing
        Ok(PreferHeader {
            respond_async: false,
        })
    }
}

/// Serialize a `MeltQuoteResponse` into an HTTP JSON response that carries the
/// per-variant payload shape rather than the enum tag.
fn melt_quote_response_to_json(response: MeltQuoteResponse<QuoteId>) -> Response {
    match response {
        MeltQuoteResponse::Bolt11(r) => Json(r).into_response(),
        MeltQuoteResponse::Bolt12(r) => Json(r).into_response(),
        MeltQuoteResponse::Onchain(r) => Json(r).into_response(),
        MeltQuoteResponse::Custom((_, r)) => Json(r).into_response(),
    }
}

/// Serialize a `MeltQuoteCreateResponse` into an HTTP JSON response that
/// carries the per-variant payload shape rather than the enum tag.
fn melt_quote_create_response_to_json(response: MeltQuoteCreateResponse<QuoteId>) -> Response {
    match response {
        MeltQuoteCreateResponse::Bolt11(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Bolt12(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Onchain(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Custom((_, r)) => Json(r).into_response(),
    }
}

async fn validate_melt_quote_method(
    state: &MintState,
    method: &str,
    quote_id: &QuoteId,
) -> Result<(), cdk::Error> {
    let quote_method = state.mint.get_melt_quote_method(quote_id).await?;
    let expected_method = PaymentMethod::from(method);

    if quote_method != expected_method {
        return Err(cdk::Error::InvalidPaymentMethod);
    }

    Ok(())
}

async fn validate_mint_quote_methods(
    state: &MintState,
    method: &str,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    let expected_method = PaymentMethod::from(method);

    for quote_id in quote_ids {
        let quote_method = state.mint.get_mint_quote_method(quote_id).await?;

        if quote_method != expected_method {
            return Err(cdk::Error::InvalidPaymentMethod);
        }
    }

    Ok(())
}

async fn validate_mint_request_route(
    auth: AuthHeader,
    state: &MintState,
    method: &str,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Mint(method.to_owned())),
        )
        .await?;

    validate_mint_quote_methods(state, method, quote_ids).await
}

async fn process_mint_input(
    state: &MintState,
    input: cdk::mint::MintInput,
) -> Result<MintResponse, cdk::Error> {
    #[cfg(feature = "federation")]
    if let Some(response) = process_federation_mint_input(state, &input).await? {
        return Ok(response);
    }

    state.mint.process_mint_request(input).await
}

#[cfg(feature = "federation")]
async fn process_federation_mint_input(
    state: &MintState,
    input: &cdk::mint::MintInput,
) -> Result<Option<MintResponse>, cdk::Error> {
    if state.mint.federation_config().is_none() {
        return Ok(None);
    }
    let correlation_id = input
        .quote_ids()
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| "quote-less-mint".to_string());
    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        correlation_id,
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Incoming,
        crate::federation::latency::FederationLatencyPhase::CashuMint,
    );
    let result = async {
        state.ensure_federation_ready_for_public_submit().await?;
        ensure_federation_mint_quotes_paid_for_signing(state, input).await?;

        let Some(operation_id) = submit_federation_mint_operation(state, input.clone()).await?
        else {
            return Ok(None);
        };

        Ok(Some(
            state
                .mint
                .persisted_mint_share_response(operation_id, input)
                .await?,
        ))
    }
    .await;
    timer
        .finish(crate::federation::latency::FederationLatencyOutcome::from_success(result.is_ok()));
    result
}

#[cfg(feature = "federation")]
async fn ensure_federation_mint_quotes_paid_for_signing(
    state: &MintState,
    input: &cdk::mint::MintInput,
) -> Result<(), cdk::Error> {
    if state.mint.federation_config().is_none() {
        return Ok(());
    }

    let quote_ids = input.quote_ids();
    if quote_ids.is_empty() {
        return Ok(());
    }

    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        quote_ids
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "quote-less-mint".to_string()),
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Incoming,
        crate::federation::latency::FederationLatencyPhase::MintPaymentWait,
    );
    let result = async {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_mint_quotes(state, &quote_ids),
        )
        .await?;

        let wait_timeout = state
            .federation
            .peer_sync_config
            .unwrap_or_default()
            .operation_submission_timeout;
        tokio::time::timeout(
            wait_timeout,
            wait_for_federation_mint_quotes_paid_locally(state, &quote_ids),
        )
        .await
        .map_err(|_| cdk::Error::PendingQuote)?
    }
    .await;
    let outcome = match &result {
        Ok(()) => crate::federation::latency::FederationLatencyOutcome::Success,
        Err(cdk::Error::PendingQuote) => {
            crate::federation::latency::FederationLatencyOutcome::Timeout
        }
        Err(_) => crate::federation::latency::FederationLatencyOutcome::Error,
    };
    timer.finish(outcome);
    result
}

#[cfg(feature = "federation")]
async fn wait_for_federation_mint_quotes_paid_locally(
    state: &MintState,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    loop {
        let mut all_paid = true;
        for quote_id in quote_ids {
            let quote = state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            if !matches!(
                quote.state(),
                cdk::nuts::MintQuoteState::Paid | cdk::nuts::MintQuoteState::Issued
            ) {
                all_paid = false;
                break;
            }
        }

        if all_paid {
            return Ok(());
        }

        tokio::time::sleep(FEDERATION_MINT_PAYMENT_WAIT_POLL_INTERVAL).await;
    }
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_operation(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
    request_lookup_id: Option<cdk::cdk_payment::PaymentIdentifier>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let request_lookup_id = request_lookup_id.ok_or(cdk::Error::Internal)?;
    let envelope = config.mint_quote_operation_with_lookup_id(
        response.quote().clone(),
        request_lookup_id,
        request,
        mint_quote_response_to_string(response),
    );
    state.submit_federation_envelope(envelope).await?;
    match Box::pin(
        state
            .federation
            .payment_observer
            .observe_mint_quotes(state, std::slice::from_ref(response.quote())),
    )
    .await
    {
        Ok(submitted) if submitted > 0 => {
            tracing::debug!(
                quote = %response.quote(),
                "Submitted immediate federation mint payment observation"
            );
        }
        Ok(_) => {}
        Err(err) => {
            tracing::debug!(
                quote = %response.quote(),
                "Immediate federation mint payment observation failed: {}",
                err
            );
        }
    }
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_operation(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
    response: &MeltQuoteCreateResponse<QuoteId>,
    metadata: Option<RouteFederationMeltQuoteMetadata>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let quote = response.quote().cloned().ok_or(cdk::Error::UnknownQuote)?;
    let metadata = metadata.ok_or(cdk::Error::Internal)?;
    let envelope = config.melt_quote_operation(quote, request, response.clone(), metadata);
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_operation(
    state: &MintState,
    input: cdk::mint::MintInput,
) -> Result<Option<cdk::federation::FederationOperationId>, cdk::Error> {
    let Some(config) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let envelope = config.mint_operation(input)?;
    let response = state.submit_federation_envelope(envelope).await?;
    Ok(Some(response.operation_id))
}

#[cfg(feature = "federation")]
fn mint_quote_response_to_string(
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> cdk::mint::MintQuoteResponse<String> {
    match response {
        cdk::mint::MintQuoteResponse::Bolt11(response) => {
            cdk::mint::MintQuoteResponse::Bolt11(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Bolt12(response) => {
            cdk::mint::MintQuoteResponse::Bolt12(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Onchain(response) => {
            cdk::mint::MintQuoteResponse::Onchain(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Custom { method, response } => {
            cdk::mint::MintQuoteResponse::Custom {
                method: method.clone(),
                response: response.to_string_id(),
            }
        }
    }
}

async fn validate_melt_request_route(
    auth: AuthHeader,
    state: &MintState,
    method: &str,
    quote_id: &QuoteId,
) -> Result<(), cdk::Error> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Melt(method.to_owned())),
        )
        .await?;

    validate_melt_quote_method(state, method, quote_id).await
}

/// Generic handler for custom payment method mint quotes
///
/// This handler works for ANY custom payment method (e.g., paypal, venmo, cashapp, bolt11, bolt12).
/// For bolt11/bolt12, it handles the specific request/response types.
/// For other methods, it passes the request data directly to the payment processor.
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_mint_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    state
        .ensure_federation_ready_for_public_submit()
        .await
        .map_err(into_response)?;

    match method.as_str() {
        "bolt11" => {
            let bolt11_request: MintQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = bolt11_request.into();
            let (quote, _request_lookup_id) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
            )
            .await
            .map_err(into_response)?;

            let response: MintQuoteBolt11Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            if payload.get("pubkey").is_none_or(|v| v.is_null()) {
                return Err(into_response(cdk::Error::PubkeyRequired));
            }
            let bolt12_request: MintQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = bolt12_request.into();
            let (quote, _request_lookup_id) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
            )
            .await
            .map_err(into_response)?;

            let response: MintQuoteBolt12Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "onchain" => {
            let onchain_request: MintQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain request: {}", e);
                    if e.to_string().contains("missing field `pubkey`") {
                        into_response(cdk::Error::PubkeyRequired)
                    } else {
                        into_response(cdk::Error::InvalidPaymentMethod)
                    }
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = onchain_request.into();
            let (quote, _request_lookup_id) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
            )
            .await
            .map_err(into_response)?;

            let response: MintQuoteOnchainResponse<QuoteId> =
                MintQuoteOnchainResponse::try_from(quote).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            let custom_request: MintQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::mint::MintQuoteRequest::Custom {
                method: cdk::nuts::PaymentMethod::from(method.clone()),
                request: custom_request,
            };

            let (response, _request_lookup_id) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &response,
                _request_lookup_id,
            )
            .await
            .map_err(into_response)?;

            match response {
                cdk::mint::MintQuoteResponse::Custom { response, .. } => {
                    Ok(Json(response).into_response())
                }
                _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
            }
        }
    }
}

/// Get custom payment method mint quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_mint_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    let quote_response = check_mint_quotes_for_route(&state, &[quote_id])
        .await
        .map_err(into_response)?
        .first()
        .cloned()
        .ok_or(cdk::Error::UnknownQuote)
        .map_err(into_response)?;
    match method.as_str() {
        "bolt11" => {
            let response: MintQuoteBolt11Response<QuoteId> =
                MintQuoteBolt11Response::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            let response: MintQuoteBolt12Response<QuoteId> =
                MintQuoteBolt12Response::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "onchain" => {
            let response: MintQuoteOnchainResponse<QuoteId> =
                MintQuoteOnchainResponse::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            // Extract and verify it's a Custom payment method
            match quote_response {
                cdk::mint::MintQuoteResponse::Custom {
                    method: quote_method,
                    response,
                    ..
                } => {
                    if quote_method.to_string() != method {
                        return Err(into_response(cdk::Error::InvalidPaymentMethod));
                    }
                    Ok(Json(response).into_response())
                }
                _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
            }
        }
    }
}

/// Batch check mint quote status (NUT-29)
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_batch_check_mint_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<BatchCheckMintQuoteRequest<QuoteId>>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    validate_mint_quote_methods(&state, &method, &payload.quotes)
        .await
        .map_err(into_response)?;

    let responses = check_mint_quotes_for_route(&state, &payload.quotes)
        .await
        .map_err(into_response)?;

    match method.as_str() {
        "bolt11" => {
            let responses: Vec<MintQuoteBolt11Response<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteBolt11Response::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        "bolt12" => {
            let responses: Vec<MintQuoteBolt12Response<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteBolt12Response::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        "onchain" => {
            let responses: Vec<MintQuoteOnchainResponse<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteOnchainResponse::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        _ => {
            let responses: Vec<cdk::nuts::MintQuoteCustomResponse<QuoteId>> = responses
                .into_iter()
                .map(|r| match r {
                    cdk::mint::MintQuoteResponse::Custom { response, .. } => Ok(response),
                    _ => Err(cdk::Error::InvalidPaymentMethod),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
    }
}

/// Request a melt quote for custom payment method
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_melt_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    state
        .ensure_federation_ready_for_public_submit()
        .await
        .map_err(into_response)?;

    let (quote_request, response, federation_metadata) = match method.as_str() {
        "bolt11" => {
            let bolt11_request: MeltQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt11(bolt11_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
        "bolt12" => {
            let bolt12_request: MeltQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt12(bolt12_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
        "onchain" => {
            let onchain_request: MeltQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Onchain(onchain_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
        _ => {
            let custom_request: MeltQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let request_method = PaymentMethod::from(custom_request.method.as_str());
            let route_method = PaymentMethod::from(method.as_str());

            if request_method != route_method {
                return Err(into_response(cdk::Error::InvalidPaymentMethod));
            }

            let quote_request = cdk::MeltQuoteRequest::Custom(custom_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
    };

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_operation(&state, quote_request, &response, federation_metadata)
        .await
        .map_err(into_response)?;
    #[cfg(not(feature = "federation"))]
    let _ = (quote_request, federation_metadata);

    Ok(melt_quote_create_response_to_json(response))
}

/// Get custom payment method melt quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_melt_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    validate_melt_quote_method(&state, &method, &quote_id)
        .await
        .map_err(into_response)?;

    let quote = Box::pin(check_melt_quote_for_route(&state, &quote_id))
        .await
        .map_err(into_response)?;

    Ok(melt_quote_response_to_json(quote))
}

async fn process_melt_request(
    prefer: PreferHeader,
    state: &MintState,
    method: &str,
    payload: &cdk::nuts::MeltRequest<QuoteId>,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    #[cfg(feature = "federation")]
    if let Some(response) = Box::pin(process_federation_melt_request(state, payload)).await? {
        return Ok(response);
    }

    // Check for async preference in either the Prefer header or the request body
    // For onchain we always want to do the async flow
    let respond_async = prefer.respond_async || payload.is_prefer_async() || method == "onchain";

    let pending = state.mint.melt(payload).await?;

    let res = if respond_async {
        // Asynchronous processing - return immediately after setup
        pending.into_pending_response()
    } else {
        // Synchronous processing - wait for completion
        pending.await?
    };

    Ok(res)
}

#[cfg(feature = "federation")]
async fn process_federation_melt_request(
    state: &MintState,
    payload: &cdk::nuts::MeltRequest<QuoteId>,
) -> Result<Option<MeltQuoteResponse<QuoteId>>, cdk::Error> {
    let Some(config) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let envelope = config.melt_operation(payload.clone())?;
    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        payload.quote().to_string(),
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Outgoing,
        crate::federation::latency::FederationLatencyPhase::CashuMelt,
    );
    let result = async {
        state.submit_federation_envelope(envelope).await?;
        if let Err(err) = Box::pin(
            state
                .federation
                .payment_observer
                .observe_melt_quote(state, payload.quote()),
        )
        .await
        {
            tracing::debug!(
                quote = %payload.quote(),
                "Immediate federation melt payment observation deferred: {}",
                err
            );
        }
        if federation_melt_quote_is_failed_terminal(state, payload.quote()).await? {
            return Err(cdk::Error::PaymentFailed);
        }

        Ok(Some(
            federation_melt_response_from_store(state, payload.quote()).await?,
        ))
    }
    .await;
    timer
        .finish(crate::federation::latency::FederationLatencyOutcome::from_success(result.is_ok()));
    result
}

#[cfg(feature = "federation")]
async fn federation_melt_response_from_store(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    let quote = state
        .mint
        .localstore()
        .get_melt_quote(quote_id)
        .await?
        .ok_or(cdk::Error::UnknownQuote)?;
    let change = state
        .mint
        .localstore()
        .get_blind_signatures_for_quote(quote_id)
        .await?;
    let change_opt = if change.is_empty() {
        None
    } else {
        Some(change)
    };

    Ok(quote.into_response(change_opt))
}

#[cfg(feature = "federation")]
async fn federation_melt_quote_is_failed_terminal(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<bool, cdk::Error> {
    let Some(quote) = state.mint.localstore().get_melt_quote(quote_id).await? else {
        return Ok(false);
    };
    if quote.state != cdk::nuts::MeltQuoteState::Unpaid {
        return Ok(false);
    }
    let saga = state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(quote_id)
        .await?;

    Ok(saga.is_none())
}

// ============================================================================
// CACHED HANDLERS FOR NUT-19 SUPPORT
// ============================================================================

/// Cached version of post_mint_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method, quote_id = ?payload.quote))]
pub async fn cache_post_mint_custom(
    auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state;
    let method = method.0;
    let payload = payload.0;

    validate_mint_request_route(
        auth,
        &mint_state,
        &method,
        std::slice::from_ref(&payload.quote),
    )
    .await
    .map_err(into_response)?;

    let cache_key = match mint_state
        .cache
        .calculate_key(&("mint", method.as_str(), &payload))
    {
        Some(key) => key,
        None => {
            let result = process_mint_input(&mint_state, cdk::mint::MintInput::Single(payload))
                .await
                .map_err(into_response)?;

            return Ok(Json(result));
        }
    };

    if let Some(cached_response) = mint_state.cache.get::<MintResponse>(&cache_key).await {
        return Ok(Json(cached_response));
    }

    let result = Json(
        process_mint_input(&mint_state, cdk::mint::MintInput::Single(payload))
            .await
            .map_err(into_response)?,
    );

    // Cache the response
    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}

/// Cached version of post_melt_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method))]
pub async fn cache_post_melt_custom(
    auth: AuthHeader,
    prefer: PreferHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<Value>,
) -> Result<Response, Response> {
    use std::ops::Deref;

    let State(mint_state) = state.clone();
    let method = method.0;
    tracing::debug!(method = %method, "melt request received");
    let parsed_payload = parse_melt_payload(&method, payload.deref().clone())?;

    validate_melt_request_route(auth, &mint_state, &method, parsed_payload.quote())
        .await
        .map_err(into_response)?;

    let cache_key =
        match mint_state
            .cache
            .calculate_key(&("melt", method.as_str(), &parsed_payload))
        {
            Some(key) => key,
            None => {
                let result = process_melt_request(prefer, &mint_state, &method, &parsed_payload)
                    .await
                    .map_err(into_response)?;

                return Ok(melt_quote_response_to_json(result));
            }
        };

    if let Some(cached_response) = mint_state
        .cache
        .get::<MeltQuoteResponse<QuoteId>>(&cache_key)
        .await
    {
        return Ok(melt_quote_response_to_json(cached_response));
    }

    let result = process_melt_request(prefer, &mint_state, &method, &parsed_payload)
        .await
        .map_err(into_response)?;

    mint_state.cache.set(cache_key, &result).await;

    Ok(melt_quote_response_to_json(result))
}

// The `Err` variant carries an axum `Response` (a pre-built 400 with the full
// body), which is large but matches the style used throughout this module.
// Refactoring to a boxed/smaller error would be a cross-cutting change outside
// the scope of this bug fix.
#[allow(clippy::result_large_err)]
fn parse_melt_payload(
    method: &str,
    payload: Value,
) -> Result<cdk::nuts::MeltRequest<QuoteId>, Response> {
    if method == "onchain" {
        let request: MeltOnchainRequest<QuoteId> =
            serde_json::from_value(payload).map_err(|e| {
                tracing::warn!(
                    method = %method,
                    "failed to parse onchain melt request body: {e}",
                );
                into_response(cdk::Error::InvalidPaymentRequest)
            })?;
        Ok(request.into())
    } else {
        serde_json::from_value(payload).map_err(|e| {
            tracing::warn!(
                method = %method,
                "failed to parse melt request body: {e}",
            );
            into_response(cdk::Error::InvalidPaymentRequest)
        })
    }
}

/// Cached version of post_batch_mint for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method))]
pub async fn cache_post_batch_mint(
    auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<BatchMintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state;
    let method = method.0;
    let payload = payload.0;

    validate_mint_request_route(auth, &mint_state, &method, &payload.quotes)
        .await
        .map_err(into_response)?;

    let cache_key = match mint_state
        .cache
        .calculate_key(&("mint_batch", method.as_str(), &payload))
    {
        Some(key) => key,
        None => {
            let result = process_mint_input(&mint_state, cdk::mint::MintInput::Batch(payload))
                .await
                .map_err(into_response)?;

            return Ok(Json(result));
        }
    };

    if let Some(cached_response) = mint_state.cache.get::<MintResponse>(&cache_key).await {
        return Ok(Json(cached_response));
    }

    let result = Json(
        process_mint_input(&mint_state, cdk::mint::MintInput::Batch(payload))
            .await
            .map_err(into_response)?,
    );

    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "federation")]
    use std::collections::BTreeMap;
    use std::collections::{HashMap, HashSet};
    #[cfg(feature = "federation")]
    use std::pin::Pin;
    #[cfg(feature = "federation")]
    use std::str::FromStr;
    #[cfg(feature = "federation")]
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[cfg(feature = "federation")]
    use async_trait::async_trait;
    use axum::http::{HeaderValue, Request, StatusCode};
    use bip39::Mnemonic;
    #[cfg(feature = "federation")]
    use cdk::cdk_payment::{
        self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
        MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
        SettingsResponse, WaitPaymentResponse,
    };
    #[cfg(feature = "federation")]
    use cdk::federation::{
        generate_trusted_dealer_configs, FederationAlephBftUnitPayload, FederationConfig,
        FederationConsensusItem, FederationConsensusRuntime, FederationError,
        FederationJournalEntry, FederationKeysetRotationOperation,
        FederationMeltQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
        FederationOperationId, FederationPaymentObservationPolicy, InMemoryFederationJournal,
        MemberConfigPrivate, MemberId, ThresholdParams, TrustedDealerConfigBundle,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    #[cfg(feature = "federation")]
    use cdk::mint::Mint;
    use cdk::mint::{MintBuilder, MintMeltLimits, MintQuoteResponse};
    use cdk::nuts::nut00::KnownMethod;
    #[cfg(feature = "federation")]
    use cdk::nuts::nut01::{
        verify_blind_signature_share, BlsBlindSignatureShare, BlsG1PublicKey, BlsSecretKey,
    };
    #[cfg(feature = "federation")]
    use cdk::nuts::MeltQuoteState;
    #[cfg(feature = "federation")]
    use cdk::nuts::SecretKey;
    use cdk::nuts::{
        BatchMintRequest, BlindedMessage, CurrencyUnit, MintQuoteState, MintRequest, PaymentMethod,
    };
    #[cfg(feature = "federation")]
    use cdk::nuts::{
        BlindSignature, CheckStateRequest, MeltRequest, Proof, RestoreRequest, SwapRequest,
    };
    #[cfg(feature = "federation")]
    use cdk::secret::Secret;
    use cdk::types::{FeeReserve, QuoteTTL};
    #[cfg(feature = "federation")]
    use cdk::wallet::{AuthWallet, FederatedMintConnector, MintConnector};
    use cdk::Amount;
    use cdk_fake_wallet::FakeWallet;
    #[cfg(feature = "federation")]
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
    #[cfg(feature = "federation")]
    use futures::Stream;

    use super::*;
    use crate::cache::HttpCache;

    #[cfg(feature = "federation")]
    struct TempMintDbFile {
        path: String,
    }

    #[cfg(feature = "federation")]
    impl TempMintDbFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
                .to_string_lossy()
                .to_string();

            Self { path }
        }

        fn path(&self) -> &str {
            &self.path
        }
    }

    #[cfg(feature = "federation")]
    impl Drop for TempMintDbFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}-shm", self.path));
            let _ = std::fs::remove_file(format!("{}-wal", self.path));
        }
    }

    fn create_test_request(prefer_header: Option<&str>) -> Request<()> {
        let mut req = Request::builder()
            .method("POST")
            .uri("/test")
            .body(())
            .unwrap();

        if let Some(header_value) = prefer_header {
            req.headers_mut().insert(
                PREFER_HEADER_KEY,
                HeaderValue::from_str(header_value).unwrap(),
            );
        }

        req
    }

    fn create_test_request_with_bytes(bytes: &[u8]) -> Request<()> {
        let mut req = Request::builder()
            .method("POST")
            .uri("/test")
            .body(())
            .unwrap();

        req.headers_mut()
            .insert(PREFER_HEADER_KEY, HeaderValue::from_bytes(bytes).unwrap());

        req
    }

    async fn create_test_state() -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]));
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .unwrap();

        let mnemonic = Mnemonic::generate(12).unwrap();
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(HttpCache::default()),
            #[cfg(feature = "federation")]
            federation: crate::federation::FederationNodeRuntime::disabled(),
        }
    }

    #[cfg(feature = "federation")]
    struct DelayedStatusFakeWallet {
        inner: FakeWallet,
        status_checks: AtomicUsize,
        settle_after_checks: usize,
        final_status: Option<MeltQuoteState>,
    }

    #[cfg(feature = "federation")]
    impl DelayedStatusFakeWallet {
        fn new(settle_after_checks: usize, final_status: Option<MeltQuoteState>) -> Self {
            let fee_reserve = FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            };

            Self {
                inner: FakeWallet::new(
                    fee_reserve,
                    HashMap::default(),
                    HashSet::default(),
                    0,
                    CurrencyUnit::Sat,
                ),
                status_checks: AtomicUsize::new(0),
                settle_after_checks,
                final_status,
            }
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintPayment for DelayedStatusFakeWallet {
        type Err = cdk_payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            self.inner.get_settings().await
        }

        async fn create_incoming_payment_request(
            &self,
            options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            self.inner.create_incoming_payment_request(options).await
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            let mut response = self.inner.make_payment(unit, options).await?;
            response.status = MeltQuoteState::Pending;
            response.payment_proof = None;
            Ok(response)
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            Ok(Box::pin(futures::stream::pending()))
        }

        fn is_payment_event_stream_active(&self) -> bool {
            false
        }

        fn cancel_payment_event_stream(&self) {}

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            self.inner
                .check_incoming_payment_status(payment_identifier)
                .await
        }

        async fn check_outgoing_payment(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            let mut response = self
                .inner
                .check_outgoing_payment(payment_identifier)
                .await?;
            let attempts = self.status_checks.fetch_add(1, Ordering::SeqCst) + 1;
            if attempts < self.settle_after_checks {
                response.status = MeltQuoteState::Pending;
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
                return Ok(response);
            }

            let Some(final_status) = self.final_status else {
                response.status = MeltQuoteState::Pending;
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
                return Ok(response);
            };

            response.status = final_status;
            if final_status != MeltQuoteState::Paid {
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
            }
            Ok(response)
        }
    }

    #[cfg(feature = "federation")]
    struct CountingFakeWallet {
        inner: FakeWallet,
        make_payment_calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "federation")]
    impl CountingFakeWallet {
        fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let fee_reserve = FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            };
            let wallet = Self {
                inner: FakeWallet::new(
                    fee_reserve,
                    HashMap::default(),
                    HashSet::default(),
                    0,
                    CurrencyUnit::Sat,
                ),
                make_payment_calls: calls.clone(),
            };

            (Arc::new(wallet), calls)
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintPayment for CountingFakeWallet {
        type Err = cdk_payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            self.inner.get_settings().await
        }

        async fn create_incoming_payment_request(
            &self,
            options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            self.inner.create_incoming_payment_request(options).await
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.make_payment_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.make_payment(unit, options).await
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            self.inner.wait_payment_event().await
        }

        fn is_payment_event_stream_active(&self) -> bool {
            self.inner.is_payment_event_stream_active()
        }

        fn cancel_payment_event_stream(&self) {
            self.inner.cancel_payment_event_stream();
        }

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            self.inner
                .check_incoming_payment_status(payment_identifier)
                .await
        }

        async fn check_outgoing_payment(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.check_outgoing_payment(payment_identifier).await
        }
    }

    #[cfg(feature = "federation")]
    struct InstantIncomingPaymentWallet {
        inner: FakeWallet,
        incoming_payments:
            Arc<tokio::sync::Mutex<HashMap<PaymentIdentifier, Vec<WaitPaymentResponse>>>>,
    }

    #[cfg(feature = "federation")]
    impl InstantIncomingPaymentWallet {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: FakeWallet::new(
                    FeeReserve {
                        min_fee_reserve: 1.into(),
                        percent_fee_reserve: 0.0,
                    },
                    HashMap::default(),
                    HashSet::default(),
                    0,
                    CurrencyUnit::Sat,
                ),
                incoming_payments: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            })
        }

        fn amount_for_incoming_payment(options: &IncomingPaymentOptions) -> Amount<CurrencyUnit> {
            match options {
                IncomingPaymentOptions::Bolt11(options) => options.amount.clone(),
                IncomingPaymentOptions::Bolt12(options) => options
                    .amount
                    .clone()
                    .unwrap_or_else(|| Amount::new(0, CurrencyUnit::Sat)),
                IncomingPaymentOptions::Custom(options) => options
                    .amount
                    .clone()
                    .unwrap_or_else(|| Amount::new(0, CurrencyUnit::Sat)),
                IncomingPaymentOptions::Onchain(_) => Amount::new(1, CurrencyUnit::Sat),
            }
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintPayment for InstantIncomingPaymentWallet {
        type Err = cdk_payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            self.inner.get_settings().await
        }

        async fn create_incoming_payment_request(
            &self,
            options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            let amount = Self::amount_for_incoming_payment(&options);
            let response = self.inner.create_incoming_payment_request(options).await?;
            let payment = WaitPaymentResponse {
                payment_identifier: response.request_lookup_id.clone(),
                payment_amount: amount,
                payment_id: response.request_lookup_id.to_string(),
            };
            self.incoming_payments
                .lock()
                .await
                .entry(response.request_lookup_id.clone())
                .or_default()
                .push(payment);

            Ok(response)
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.make_payment(unit, options).await
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            self.inner.wait_payment_event().await
        }

        fn is_payment_event_stream_active(&self) -> bool {
            self.inner.is_payment_event_stream_active()
        }

        fn cancel_payment_event_stream(&self) {
            self.inner.cancel_payment_event_stream();
        }

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            if let Some(payments) = self
                .incoming_payments
                .lock()
                .await
                .get(payment_identifier)
                .cloned()
            {
                return Ok(payments);
            }

            self.inner
                .check_incoming_payment_status(payment_identifier)
                .await
        }

        async fn check_outgoing_payment(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.check_outgoing_payment(payment_identifier).await
        }
    }

    #[cfg(feature = "federation")]
    fn test_federation_runtime_config() -> cdk::mint::FederationRuntimeConfig {
        let members = (1..=2)
            .map(|id| {
                cdk::federation::TrustedDealerMember::new(
                    cdk::federation::MemberId::new(id).expect("member id"),
                    format!("https://mint{id}.example.com")
                        .parse::<cdk::mint_url::MintUrl>()
                        .expect("public mint url"),
                    format!("https://fed{id}.example.com")
                        .parse::<cdk::mint_url::MintUrl>()
                        .expect("federation api url"),
                    SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
                )
            })
            .collect();
        let keyset = cdk::federation::TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            std::collections::BTreeMap::from([
                (
                    Amount::from(1),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[2; 32])],
                ),
                (
                    Amount::from(2),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[3; 32])],
                ),
                (
                    Amount::from(4),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[4; 32])],
                ),
            ]),
        );
        let mut bundle = cdk::federation::generate_trusted_dealer_configs(
            cdk::federation::ThresholdParams::with_bft_consensus(2, 1).expect("params"),
            members,
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        bundle.public_config.payment_observation_policy =
            cdk::federation::FederationPaymentObservationPolicy::development_single_observation_for_testing();
        bundle
            .public_config
            .validate()
            .expect("development payment observation policy");
        let private_config = bundle
            .private_configs
            .get(&cdk::federation::MemberId::new(1).expect("member id"))
            .expect("private config")
            .clone();

        cdk::mint::FederationRuntimeConfig::new(bundle.public_config, private_config)
            .expect("runtime config")
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state() -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        create_federated_test_state_with_backend(Arc::new(fake)).await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
    ) -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::generate(12).unwrap();
        create_federated_test_state_with_backend_db_and_seed(
            payment_processor,
            db,
            &mnemonic.to_seed_normalized(""),
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend_db_and_seed(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
        db: Arc<cdk_sqlite::MintSqliteDatabase>,
        seed: &[u8],
    ) -> MintState {
        create_federated_test_state_with_backend_db_seed_and_finality(
            payment_processor,
            db,
            seed,
            true,
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn create_aleph_bft_federated_test_state() -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::generate(12).unwrap();
        create_federated_test_state_with_backend_db_seed_and_finality(
            Arc::new(fake),
            db,
            &mnemonic.to_seed_normalized(""),
            false,
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn finalize_next_local_aleph_bft_unit(
        state: &MintState,
        expected_operation_id: FederationOperationId,
    ) {
        let operation_id = apply_next_local_aleph_bft_unit(state).await;
        assert_eq!(operation_id, expected_operation_id);
    }

    #[cfg(feature = "federation")]
    async fn apply_next_local_aleph_bft_unit(state: &MintState) -> FederationOperationId {
        let unit = {
            let mut consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            consensus
                .next_aleph_bft_unit_payload()
                .expect("next AlephBFT unit")
                .expect("queued AlephBFT unit")
        };
        let finalized = state
            .apply_federation_aleph_bft_finalized_unit(unit)
            .await
            .expect("local AlephBFT finality");
        assert_eq!(finalized.len(), 1);
        finalized[0].operation_id
    }

    #[cfg(feature = "federation")]
    async fn finalize_local_test_operation(
        state: &MintState,
        envelope: FederationOperationEnvelope,
    ) {
        let operation_id = envelope.operation_id().expect("test operation id");
        let unit = {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            let status = consensus.status();
            let unit_index = consensus
                .finalized_items()
                .iter()
                .filter(|item| {
                    item.unit_submitting_member == status.local_member
                        && item.session == status.session
                })
                .map(|item| item.unit_index)
                .max()
                .map_or(0, |unit_index| unit_index.saturating_add(1));
            FederationAlephBftUnitPayload::new(
                status.federation_id,
                status.local_member,
                status.session,
                unit_index,
                vec![FederationConsensusItem::operation(envelope)],
            )
            .expect("local finalized test unit")
        };
        let finalized = state
            .apply_federation_aleph_bft_finalized_unit(unit)
            .await
            .expect("local finalized test operation");
        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].operation_id, operation_id);
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend_db_seed_and_finality(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
        db: Arc<cdk_sqlite::MintSqliteDatabase>,
        seed: &[u8],
        local_finality_test: bool,
    ) -> MintState {
        let runtime_config = test_federation_runtime_config();
        let runtime_config = cdk::mint::FederationRuntimeConfig::new(
            runtime_config.public_config().clone(),
            runtime_config.private_config().clone(),
        )
        .expect("runtime config");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config.clone());
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                payment_processor,
            )
            .await
            .unwrap();

        let mint = builder.build_with_seed(db, seed).await.unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        let consensus = FederationConsensusRuntime::authenticated_aleph_bft(
            runtime_config.public_config(),
            runtime_config.member_id(),
            0,
        )
        .expect("AlephBFT consensus runtime");
        let mint = Arc::new(mint);
        MintState {
            mint: Arc::clone(&mint),
            cache: Arc::new(HttpCache::default()),
            federation: crate::federation::FederationNodeRuntime {
                transport: crate::default_federation_transport(),
                mint: Some(mint),
                consensus: Some(Arc::new(tokio::sync::Mutex::new(consensus))),
                peer_sync_config: Some(crate::FederationPeerSyncConfig::default()),
                sequences: Some(Arc::new(tokio::sync::Mutex::new(
                    cdk::federation::FederationRestSequenceTracker::new(),
                ))),
                notifications: Some(Arc::new(crate::FederationOperationNotifications::default())),
                private_rest_admission: Some(Arc::new(tokio::sync::Semaphore::new(
                    crate::FederationPeerSyncConfig::default()
                        .private_rest_inbound_concurrency_limit,
                ))),
                invalid_operation_alerts: Some(Arc::new(tokio::sync::Mutex::new(
                    crate::FederationInvalidOperationAlertTracker::default(),
                ))),
                payment_observer: crate::federation::FederationPaymentObserver::default(),
                #[cfg(feature = "federation-bdk")]
                transaction_broadcaster:
                    crate::federation::FederationTransactionBroadcaster::default(),
                #[cfg(feature = "federation-bdk")]
                wallet_liveness: crate::federation::FederationWalletLivenessWorker::default(),
                components: crate::federation::runtime_components::FederationRuntimeComponents::new(
                    Some(Arc::new(tokio::sync::Mutex::new(if local_finality_test {
                        crate::FederationSyncState::default()
                    } else {
                        crate::FederationSyncState::for_aleph_bft()
                    }))),
                    None,
                    None,
                    #[cfg(feature = "federation-bdk")]
                    Vec::new(),
                    None,
                    #[cfg(feature = "federation-bark")]
                    None,
                ),
                aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                local_finality_test,
                manual_finality_test: false,
            },
        }
    }

    #[cfg(feature = "federation")]
    async fn restart_federated_test_state_from_db(
        db_file: &TempMintDbFile,
        seed: &[u8],
    ) -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, seed).await;
        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
        state.federation.consensus = consensus;
        state.federation.sequences = sequences;
        state
    }

    #[cfg(feature = "federation")]
    async fn restart_federated_test_state_from_db_with_backend(
        db_file: &TempMintDbFile,
        seed: &[u8],
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
    ) -> MintState {
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut state =
            create_federated_test_state_with_backend_db_and_seed(payment_processor, db, seed).await;
        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
        state.federation.consensus = consensus;
        state.federation.sequences = sequences;
        state
    }

    #[cfg(feature = "federation")]
    fn limit_federation_response_cache_to_one(state: &mut MintState) {
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            response_cache_limit: 1,
            ..crate::FederationPeerSyncConfig::default()
        });
    }

    #[cfg(feature = "federation")]
    async fn federation_operation_kinds(
        state: &MintState,
    ) -> Vec<cdk::federation::FederationOperationKind> {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        consensus
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.envelope.operation.kind())
            .collect()
    }

    #[cfg(feature = "federation")]
    async fn first_federation_operation_id(
        state: &MintState,
        kind: cdk::federation::FederationOperationKind,
    ) -> cdk::federation::FederationOperationId {
        state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("federation journal lookup")
            .into_iter()
            .find(|entry| entry.envelope.operation.kind() == kind)
            .expect("federation operation entry")
            .operation_id
    }

    #[cfg(feature = "federation")]
    fn test_aleph_bft_peer(
        federation_id: cdk::federation::FederationId,
        member_id: MemberId,
    ) -> FederationConsensusRuntime {
        FederationConsensusRuntime::new(federation_id, member_id, 0)
    }

    #[cfg(feature = "federation")]
    fn finalize_test_operation(
        peer: &mut FederationConsensusRuntime,
        envelope: FederationOperationEnvelope,
    ) -> Result<cdk::federation::FederationSubmitOperationResponse, FederationError> {
        let status = peer.status();
        let unit_index = peer
            .finalized_items()
            .last()
            .map_or(0, |item| item.unit_index.saturating_add(1));
        let unit = FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            unit_index,
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("peer finalized unit");
        let mut responses = peer.finalize_unit(unit)?;
        Ok(responses
            .pop()
            .expect("one finalized response for one test operation"))
    }

    #[cfg(feature = "federation")]
    async fn apply_peer_catch_up(state: &MintState, peer_consensus: &FederationConsensusRuntime) {
        let local_consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus");
        let request = local_consensus.lock().await.catch_up_request();
        let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
        let catch_up_response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
            peer_consensus.status().federation_id,
            peer_consensus.journal(),
            cdk::federation::FederationMaterializedStateDigest::genesis(),
            &request,
            50,
            100,
        )
        .with_aleph_bft_finalized_items(
            peer_consensus
                .finalized_items()
                .get(finalized_from..)
                .unwrap_or_default()
                .to_vec(),
            true,
        );

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply peer catch-up response");
    }

    async fn create_test_state_with_custom_methods(methods: &[&str]) -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone()).with_batch_minting(
            Some(10),
            Some(methods.iter().map(|method| method.to_string()).collect()),
        );

        let custom_methods = methods
            .iter()
            .map(|method| (method.to_string(), "{}".to_string()))
            .collect::<HashMap<_, _>>();

        for method in methods {
            let fake = FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 0.0,
                },
                HashMap::default(),
                HashSet::default(),
                0,
                CurrencyUnit::Sat,
            )
            .with_custom_payment_methods(custom_methods.clone());

            builder
                .add_payment_processor(
                    CurrencyUnit::Sat,
                    PaymentMethod::Custom(method.to_string()),
                    MintMeltLimits::new(1, 10_000),
                    Arc::new(fake),
                )
                .await
                .unwrap();
        }

        let mnemonic = Mnemonic::generate(12).unwrap();
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(HttpCache::default()),
            #[cfg(feature = "federation")]
            federation: crate::federation::FederationNodeRuntime::disabled(),
        }
    }

    async fn create_custom_quote(state: &MintState, method: &str, amount: u64) -> QuoteId {
        let quote = state
            .mint
            .get_mint_quote(cdk::mint::MintQuoteRequest::Custom {
                method: PaymentMethod::Custom(method.to_string()),
                request: MintQuoteCustomRequest {
                    amount: Some(Amount::from(amount)),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                    extra: Value::Null,
                },
            })
            .await
            .unwrap();

        quote.quote().clone()
    }

    async fn create_paid_bolt11_quote(state: &MintState) -> QuoteId {
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
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

        for _ in 0..100 {
            #[cfg(feature = "federation")]
            if state.mint.federation_config().is_some() {
                if Box::pin(state.mint.probe_federation_mint_quote_payment(&quote.quote))
                    .await
                    .expect("mint payment probe")
                    .is_some()
                {
                    return quote.quote;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            let check = state
                .mint
                .check_mint_quotes(std::slice::from_ref(&quote.quote))
                .await
                .unwrap();
            if let MintQuoteResponse::Bolt11(q) = &check[0] {
                if q.state == MintQuoteState::Paid {
                    return quote.quote;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("bolt11 quote was not paid by the fake wallet");
    }

    #[cfg(feature = "federation")]
    async fn create_federation_paid_bolt11_quote(state: &MintState) -> QuoteId {
        let request = cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        });
        let (response, request_lookup_id) = create_mint_quote_for_route(state, request.clone())
            .await
            .expect("prepare federation mint quote");
        let quote_id = response.quote().clone();
        let config = state.mint.federation_config().expect("federation config");
        let envelope = config.mint_quote_operation_with_lookup_id(
            quote_id.clone(),
            request_lookup_id.expect("federation mint quote lookup id"),
            request,
            mint_quote_response_to_string(&response),
        );
        finalize_local_test_operation(state, envelope).await;
        for _ in 0..100 {
            if Box::pin(state.mint.probe_federation_mint_quote_payment(&quote_id))
                .await
                .expect("mint payment probe")
                .is_some()
            {
                return quote_id;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("federation bolt11 quote was not paid by the fake wallet");
    }

    #[cfg(feature = "federation")]
    async fn create_federation_observed_paid_bolt11_quote(state: &MintState) -> QuoteId {
        let quote_id = create_federation_paid_bolt11_quote(state).await;
        let observation = Box::pin(state.mint.probe_federation_mint_quote_payment(&quote_id))
            .await
            .expect("mint payment probe")
            .expect("paid mint payment observation");
        let config = state.mint.federation_config().expect("federation config");
        let envelope = config.mint_quote_payment_observation_with_metadata(
            observation.quote,
            observation.method,
            observation.state,
            observation.amount_paid,
            observation.amount_issued,
            observation.payment_proof,
            observation.payment_id,
        );
        finalize_local_test_operation(state, envelope).await;
        quote_id
    }

    fn outputs_for_amount(state: &MintState, amount: u64) -> Vec<BlindedMessage> {
        outputs_for_amount_with_label(
            state,
            amount,
            &format!("axum federated mint output {amount}"),
        )
    }

    fn outputs_for_amount_with_label(
        state: &MintState,
        amount: u64,
        label: &str,
    ) -> Vec<BlindedMessage> {
        #[cfg(feature = "federation")]
        let keyset_id = match state.mint.federation_config() {
            Some(config) => config.public_config().keysets[0].id,
            None => *state
                .mint
                .get_active_keysets()
                .get(&CurrencyUnit::Sat)
                .expect("active sat keyset"),
        };
        #[cfg(not(feature = "federation"))]
        let keyset_id = *state
            .mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .expect("active sat keyset");

        let blinded_secret = cdk::nuts::PublicKey::from(
            cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(label.as_bytes()),
        );

        vec![BlindedMessage::new(
            Amount::from(amount),
            keyset_id,
            blinded_secret,
        )]
    }

    #[cfg(feature = "federation")]
    fn non_v3_output_for_state(state: &MintState, amount: Amount, label: &[u8]) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let non_v3_keyset_id = cdk::nuts::Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        let blinded_secret =
            cdk::nuts::PublicKey::from(cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(label));

        BlindedMessage::new(amount, non_v3_keyset_id, blinded_secret)
    }

    #[cfg(feature = "federation")]
    fn non_v3_change_output_for_state(state: &MintState) -> BlindedMessage {
        non_v3_output_for_state(
            state,
            Amount::ZERO,
            b"axum federated melt non-v3 change output",
        )
    }

    #[cfg(feature = "federation")]
    fn aggregate_secret_for_amount(amount: u64) -> cdk::nuts::nut01::BlsSecretKey {
        let byte = match amount {
            1 => 2,
            2 => 3,
            4 => 4,
            _ => panic!("missing aggregate secret fixture for amount {amount}"),
        };

        cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    #[cfg(feature = "federation")]
    fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let secret = cdk::secret::Secret::new(format!("federated-axum-proof-{label}"));
        let signature = cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        cdk::nuts::Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            cdk::nuts::PublicKey::from(signature),
        )
    }

    #[cfg(feature = "federation")]
    fn threshold_member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    #[cfg(feature = "federation")]
    fn threshold_identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    #[cfg(feature = "federation")]
    fn threshold_member(
        member_id: MemberId,
        public_mint_url: cdk::mint_url::MintUrl,
        federation_api_url: cdk::mint_url::MintUrl,
    ) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            public_mint_url,
            federation_api_url,
            threshold_identity_secret(member_id),
        )
    }

    #[cfg(feature = "federation")]
    fn threshold_bundle(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
    ) -> TrustedDealerConfigBundle {
        let members = member_urls
            .iter()
            .map(|(member_id, url)| threshold_member(*member_id, url.clone(), url.clone()))
            .collect::<Vec<_>>();

        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[12; 32]),
                        ],
                    ),
                    (
                        Amount::from(2),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                            BlsSecretKey::from_reduced_bytes(&[13; 32]),
                        ],
                    ),
                    (
                        Amount::from(4),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            BlsSecretKey::from_reduced_bytes(&[14; 32]),
                        ],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer threshold bundle")
    }

    #[cfg(feature = "federation")]
    fn threshold_all_signers_bundle(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
    ) -> TrustedDealerConfigBundle {
        let members = member_urls
            .iter()
            .map(|(member_id, url)| threshold_member(*member_id, url.clone(), url.clone()))
            .collect::<Vec<_>>();

        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 3).expect("threshold params"),
            members,
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[12; 32]),
                            BlsSecretKey::from_reduced_bytes(&[22; 32]),
                        ],
                    ),
                    (
                        Amount::from(2),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                            BlsSecretKey::from_reduced_bytes(&[13; 32]),
                            BlsSecretKey::from_reduced_bytes(&[23; 32]),
                        ],
                    ),
                    (
                        Amount::from(4),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            BlsSecretKey::from_reduced_bytes(&[14; 32]),
                            BlsSecretKey::from_reduced_bytes(&[24; 32]),
                        ],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer all-signers threshold bundle")
    }

    #[cfg(feature = "federation")]
    fn threshold_all_signers_bundle_with_payment_policy(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
        policy: FederationPaymentObservationPolicy,
    ) -> TrustedDealerConfigBundle {
        let mut bundle = threshold_all_signers_bundle(member_urls);
        bundle.public_config.payment_observation_policy = policy;
        bundle
            .public_config
            .validate()
            .expect("trusted dealer all-signers threshold policy");
        bundle
    }

    #[cfg(feature = "federation")]
    fn threshold_rotation_bundle(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
    ) -> TrustedDealerConfigBundle {
        let members = member_urls
            .iter()
            .map(|(member_id, url)| threshold_member(*member_id, url.clone(), url.clone()))
            .collect::<Vec<_>>();

        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            vec![
                TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[2; 32]),
                                BlsSecretKey::from_reduced_bytes(&[12; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[3; 32]),
                                BlsSecretKey::from_reduced_bytes(&[13; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[4; 32]),
                                BlsSecretKey::from_reduced_bytes(&[14; 32]),
                            ],
                        ),
                    ]),
                ),
                TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    false,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[5; 32]),
                                BlsSecretKey::from_reduced_bytes(&[15; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                                BlsSecretKey::from_reduced_bytes(&[16; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[7; 32]),
                                BlsSecretKey::from_reduced_bytes(&[17; 32]),
                            ],
                        ),
                    ]),
                ),
            ],
        )
        .expect("trusted dealer threshold rotation bundle")
    }

    #[cfg(feature = "federation")]
    fn active_keyset(bundle: &TrustedDealerConfigBundle) -> cdk::federation::FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| keyset.active)
            .expect("active keyset")
            .clone()
    }

    #[cfg(feature = "federation")]
    fn inactive_keyset(bundle: &TrustedDealerConfigBundle) -> cdk::federation::FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| !keyset.active)
            .expect("inactive keyset")
            .clone()
    }

    #[cfg(feature = "federation")]
    fn activated_rotation_keyset(
        bundle: &TrustedDealerConfigBundle,
    ) -> cdk::federation::FederatedKeyset {
        let mut keyset = inactive_keyset(bundle);
        keyset.active = true;
        keyset
    }

    #[cfg(feature = "federation")]
    fn rotated_public_config(bundle: &TrustedDealerConfigBundle) -> FederationConfig {
        bundle
            .public_config
            .apply_keyset_rotation(&FederationKeysetRotationOperation {
                keyset: activated_rotation_keyset(bundle),
            })
            .expect("rotated public config")
    }

    #[cfg(feature = "federation")]
    async fn build_threshold_member_mint(
        public_config: FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Arc<Mint> {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        build_threshold_member_mint_with_backend(public_config, private_config, Arc::new(fake))
            .await
    }

    #[cfg(feature = "federation")]
    async fn build_threshold_member_mint_with_backend(
        public_config: FederationConfig,
        private_config: MemberConfigPrivate,
        payment_processor: cdk::cdk_payment::DynMintPayment,
    ) -> Arc<Mint> {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let runtime_config = cdk::mint::FederationRuntimeConfig::new(public_config, private_config)
            .expect("runtime config");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config);
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                payment_processor,
            )
            .await
            .expect("payment processor");

        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .expect("quote ttl");
        mint.start().await.expect("mint start");
        Arc::new(mint)
    }

    #[cfg(feature = "federation")]
    async fn local_member_listeners() -> (
        Vec<tokio::net::TcpListener>,
        Vec<(MemberId, cdk::mint_url::MintUrl)>,
    ) {
        let mut listeners = Vec::new();
        let mut member_urls = Vec::new();
        for id in 1..=3 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local member listener");
            let url = cdk::mint_url::MintUrl::from_str(&format!(
                "http://{}/",
                listener.local_addr().expect("local address")
            ))
            .expect("member URL");
            member_urls.push((threshold_member_id(id), url));
            listeners.push(listener);
        }
        (listeners, member_urls)
    }

    #[cfg(feature = "federation")]
    fn static_member_urls() -> Vec<(MemberId, cdk::mint_url::MintUrl)> {
        (1..=3)
            .map(|id| {
                (
                    threshold_member_id(id),
                    cdk::mint_url::MintUrl::from_str(&format!("https://member{id}.example.com/"))
                        .expect("static member URL"),
                )
            })
            .collect()
    }

    #[cfg(feature = "federation")]
    async fn spawn_member_routers(
        mints: &[Arc<Mint>],
        listeners: Vec<tokio::net::TcpListener>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();
        for (mint, listener) in mints.iter().cloned().zip(listeners) {
            let router = crate::create_mint_router_without_federation_peer_sync(
                mint,
                vec![KnownMethod::Bolt11.to_string()],
            )
            .await
            .expect("mint router without peer sync");
            handles.push(tokio::spawn(async move {
                axum::serve(listener, router).await.expect("member router");
            }));
        }
        handles
    }

    #[cfg(feature = "federation")]
    async fn threshold_member_state(mint: Arc<Mint>) -> MintState {
        let (federation_consensus, federation_sequences) =
            crate::create_federation_runtime_state(&mint)
                .await
                .expect("federation runtime state");
        let is_federated = federation_consensus.is_some();
        MintState {
            mint: Arc::clone(&mint),
            cache: Arc::new(HttpCache::default()),
            federation: crate::federation::FederationNodeRuntime {
                transport: crate::default_federation_transport(),
                mint: is_federated.then(|| Arc::clone(&mint)),
                consensus: federation_consensus,
                peer_sync_config: is_federated.then(crate::FederationPeerSyncConfig::default),
                sequences: federation_sequences,
                notifications: is_federated
                    .then(|| Arc::new(crate::FederationOperationNotifications::default())),
                private_rest_admission: is_federated.then(|| {
                    Arc::new(tokio::sync::Semaphore::new(
                        crate::FederationPeerSyncConfig::default()
                            .private_rest_inbound_concurrency_limit,
                    ))
                }),
                invalid_operation_alerts: is_federated.then(|| {
                    Arc::new(tokio::sync::Mutex::new(
                        crate::FederationInvalidOperationAlertTracker::default(),
                    ))
                }),
                payment_observer: crate::federation::FederationPaymentObserver::default(),
                #[cfg(feature = "federation-bdk")]
                transaction_broadcaster:
                    crate::federation::FederationTransactionBroadcaster::default(),
                #[cfg(feature = "federation-bdk")]
                wallet_liveness: crate::federation::FederationWalletLivenessWorker::default(),
                components: crate::federation::runtime_components::FederationRuntimeComponents::new(
                    is_federated.then(|| {
                        Arc::new(tokio::sync::Mutex::new(
                            crate::FederationSyncState::default(),
                        ))
                    }),
                    None,
                    None,
                    #[cfg(feature = "federation-bdk")]
                    Vec::new(),
                    None,
                    #[cfg(feature = "federation-bark")]
                    None,
                ),
                aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
                aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                local_finality_test: is_federated,
                manual_finality_test: false,
            },
        }
    }

    #[cfg(feature = "federation")]
    struct DirectRouteMeltCoordinator {
        states: Vec<MintState>,
        completed_melts: tokio::sync::Mutex<HashSet<FederationOperationId>>,
    }

    #[cfg(feature = "federation")]
    impl DirectRouteMeltCoordinator {
        fn new(states: &[MintState]) -> Arc<Self> {
            Arc::new(Self {
                states: states.to_vec(),
                completed_melts: tokio::sync::Mutex::new(HashSet::new()),
            })
        }

        async fn settle_paid_once(&self, request: &MeltRequest<QuoteId>) -> Result<(), cdk::Error> {
            let primary_state = self.states.first().ok_or_else(|| {
                cdk::Error::InvalidMintResponse(
                    "direct-route melt coordinator has no member states".to_string(),
                )
            })?;
            let primary_config = primary_state
                .mint
                .effective_federation_config()
                .await?
                .ok_or_else(|| {
                    cdk::Error::InvalidMintResponse(
                        "direct-route member is not in federation mode".to_string(),
                    )
                })?;
            let melt_operation_id = primary_config
                .melt_operation(request.clone())?
                .operation_id()?;

            let mut completed_melts = self.completed_melts.lock().await;
            if completed_melts.contains(&melt_operation_id) {
                return Ok(());
            }

            for state in &self.states {
                let config = state
                    .mint
                    .effective_federation_config()
                    .await?
                    .ok_or_else(|| {
                        cdk::Error::InvalidMintResponse(
                            "direct-route member is not in federation mode".to_string(),
                        )
                    })?;
                let envelope = config.melt_operation(request.clone())?;
                state
                    .submit_federation_envelope_to_consensus(envelope)
                    .await?;
            }

            let mut observed_payment = None;
            for state in &self.states {
                if let Some(observation) = Box::pin(
                    state
                        .mint
                        .probe_federation_melt_quote_payment(request.quote()),
                )
                .await?
                {
                    observed_payment = Some(observation);
                    break;
                }
            }
            let observed_payment = observed_payment.ok_or(cdk::Error::PendingQuote)?;

            for observer_state in &self.states {
                let config = observer_state
                    .mint
                    .effective_federation_config()
                    .await?
                    .ok_or_else(|| {
                        cdk::Error::InvalidMintResponse(
                            "direct-route member is not in federation mode".to_string(),
                        )
                    })?;
                let envelope = config.operation_envelope(FederationOperation::MeltQuotePayment(
                    FederationMeltQuotePaymentObservation {
                        quote: observed_payment.quote.clone(),
                        observer: config.member_id(),
                        method: observed_payment.method.clone(),
                        state: observed_payment.state,
                        amount: observed_payment.amount,
                        fee_reserve: observed_payment.fee_reserve,
                        total_spent: observed_payment.total_spent,
                        payment_id: observed_payment.payment_id.clone(),
                        payment_proof: observed_payment.payment_proof.clone(),
                    },
                ));

                for state in &self.states {
                    state
                        .submit_federation_envelope_to_consensus(envelope.clone())
                        .await?;
                }
            }

            completed_melts.insert(melt_operation_id);
            Ok(())
        }
    }

    #[cfg(feature = "federation")]
    #[derive(Clone)]
    struct DirectRouteMemberConnector {
        state: MintState,
        melt_coordinator: Option<Arc<DirectRouteMeltCoordinator>>,
    }

    #[cfg(feature = "federation")]
    impl std::fmt::Debug for DirectRouteMemberConnector {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("DirectRouteMemberConnector")
        }
    }

    #[cfg(feature = "federation")]
    impl DirectRouteMemberConnector {
        fn new(state: MintState) -> Self {
            Self {
                state,
                melt_coordinator: None,
            }
        }

        fn with_melt_coordinator(
            state: MintState,
            melt_coordinator: Arc<DirectRouteMeltCoordinator>,
        ) -> Self {
            Self {
                state,
                melt_coordinator: Some(melt_coordinator),
            }
        }

        fn unsupported(operation: &str) -> cdk::Error {
            cdk::Error::InvalidMintResponse(format!(
                "unsupported direct-route member operation: {operation}"
            ))
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintConnector for DirectRouteMemberConnector {
        async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, cdk::Error> {
            Err(Self::unsupported("resolve_dns_txt"))
        }

        async fn fetch_lnurl_pay_request(
            &self,
            _url: &str,
        ) -> Result<cdk::wallet::LnurlPayResponse, cdk::Error> {
            Err(Self::unsupported("fetch_lnurl_pay_request"))
        }

        async fn fetch_lnurl_invoice(
            &self,
            _url: &str,
        ) -> Result<cdk::wallet::LnurlPayInvoiceResponse, cdk::Error> {
            Err(Self::unsupported("fetch_lnurl_invoice"))
        }

        async fn get_mint_keys(&self) -> Result<Vec<cdk::nuts::KeySet>, cdk::Error> {
            let response = self
                .state
                .mint
                .effective_federation_pubkeys()
                .await?
                .unwrap_or_else(|| self.state.mint.pubkeys());

            Ok(response.keysets)
        }

        async fn get_mint_keyset(
            &self,
            _keyset_id: cdk::nuts::Id,
        ) -> Result<cdk::nuts::KeySet, cdk::Error> {
            Err(Self::unsupported("get_mint_keyset"))
        }

        async fn get_mint_keysets(&self) -> Result<cdk::nuts::KeysetResponse, cdk::Error> {
            self.state
                .mint
                .effective_federation_keysets()
                .await
                .map(|keysets| keysets.unwrap_or_else(|| self.state.mint.keysets()))
        }

        async fn post_mint_quote(
            &self,
            _request: cdk::MintQuoteRequest,
        ) -> Result<cdk::MintQuoteResponse<String>, cdk::Error> {
            Err(Self::unsupported("post_mint_quote"))
        }

        async fn post_mint(
            &self,
            _method: &PaymentMethod,
            _request: MintRequest<String>,
        ) -> Result<cdk::nuts::MintResponse, cdk::Error> {
            Err(Self::unsupported("post_mint"))
        }

        async fn post_batch_check_mint_quote_status(
            &self,
            _method: &PaymentMethod,
            _request: BatchCheckMintQuoteRequest<String>,
        ) -> Result<Vec<cdk::MintQuoteResponse<String>>, cdk::Error> {
            Err(Self::unsupported("post_batch_check_mint_quote_status"))
        }

        async fn post_batch_mint(
            &self,
            method: &PaymentMethod,
            request: BatchMintRequest<String>,
        ) -> Result<cdk::nuts::MintResponse, cdk::Error> {
            if method != &PaymentMethod::Known(KnownMethod::Bolt11) {
                return Err(cdk::Error::InvalidPaymentMethod);
            }

            let BatchMintRequest {
                quotes,
                quote_amounts,
                outputs,
                signatures,
            } = request;
            let quotes = quotes
                .into_iter()
                .map(|quote| QuoteId::from_str(&quote).map_err(cdk::Error::from))
                .collect::<Result<Vec<_>, cdk::Error>>()?;
            let request = BatchMintRequest {
                quotes,
                quote_amounts,
                outputs,
                signatures,
            };
            let input = cdk::mint::MintInput::Batch(request);
            let config = self
                .state
                .mint
                .effective_federation_config()
                .await?
                .ok_or_else(|| {
                    cdk::Error::InvalidMintResponse(
                        "direct-route member is not in federation mode".to_string(),
                    )
                })?;
            let envelope = config.mint_operation(input.clone())?;
            let response = self
                .state
                .submit_federation_envelope_to_consensus(envelope)
                .await?;

            self.state
                .mint
                .persisted_mint_share_response(response.operation_id, &input)
                .await
        }

        async fn post_melt_quote(
            &self,
            _request: cdk::MeltQuoteRequest,
        ) -> Result<cdk::MeltQuoteCreateResponse<String>, cdk::Error> {
            Err(Self::unsupported("post_melt_quote"))
        }

        async fn get_mint_quote_status(
            &self,
            _method: PaymentMethod,
            _quote_id: &str,
        ) -> Result<cdk::MintQuoteResponse<String>, cdk::Error> {
            Err(Self::unsupported("get_mint_quote_status"))
        }

        async fn get_melt_quote_status(
            &self,
            _method: PaymentMethod,
            _quote_id: &str,
        ) -> Result<cdk::MeltQuoteResponse<String>, cdk::Error> {
            Err(Self::unsupported("get_melt_quote_status"))
        }

        async fn post_melt(
            &self,
            method: &PaymentMethod,
            request: MeltRequest<String>,
        ) -> Result<cdk::MeltQuoteResponse<String>, cdk::Error> {
            if method != &PaymentMethod::Known(KnownMethod::Bolt11) {
                return Err(cdk::Error::InvalidPaymentMethod);
            }

            let request =
                cdk::nuts::MeltRequest::<QuoteId>::try_from(request).map_err(cdk::Error::from)?;
            if let Some(coordinator) = &self.melt_coordinator {
                Box::pin(coordinator.settle_paid_once(&request)).await?;
            } else {
                let config = self
                    .state
                    .mint
                    .effective_federation_config()
                    .await?
                    .ok_or_else(|| {
                        cdk::Error::InvalidMintResponse(
                            "direct-route member is not in federation mode".to_string(),
                        )
                    })?;
                let envelope = config.melt_operation(request.clone())?;
                self.state
                    .submit_federation_envelope_to_consensus(envelope)
                    .await?;
            }

            federation_melt_response_from_store(&self.state, request.quote())
                .await
                .map(cdk::MeltQuoteResponse::to_string_id)
        }

        async fn post_swap(
            &self,
            request: SwapRequest,
        ) -> Result<cdk::nuts::SwapResponse, cdk::Error> {
            let config = self
                .state
                .mint
                .effective_federation_config()
                .await?
                .ok_or_else(|| {
                    cdk::Error::InvalidMintResponse(
                        "direct-route member is not in federation mode".to_string(),
                    )
                })?;
            let envelope = config.swap_operation(request.clone())?;
            let response = self
                .state
                .submit_federation_envelope_to_consensus(envelope)
                .await?;

            self.state
                .mint
                .persisted_swap_share_response(response.operation_id, &request)
                .await
        }

        async fn get_mint_info(&self) -> Result<cdk::nuts::MintInfo, cdk::Error> {
            self.state.mint.mint_info().await
        }

        async fn post_check_state(
            &self,
            _request: CheckStateRequest,
        ) -> Result<cdk::nuts::CheckStateResponse, cdk::Error> {
            Err(Self::unsupported("post_check_state"))
        }

        async fn post_restore(
            &self,
            _request: RestoreRequest,
        ) -> Result<cdk::nuts::RestoreResponse, cdk::Error> {
            Err(Self::unsupported("post_restore"))
        }

        async fn get_auth_wallet(&self) -> Option<AuthWallet> {
            None
        }

        async fn set_auth_wallet(&self, _wallet: Option<AuthWallet>) {}
    }

    #[cfg(feature = "federation")]
    fn direct_route_member_connectors(
        states: &[MintState],
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        states
            .iter()
            .map(|state| {
                let member_id = state
                    .mint
                    .federation_config()
                    .expect("federation config")
                    .member_id();
                (
                    member_id,
                    Arc::new(DirectRouteMemberConnector::new(state.clone()))
                        as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    #[cfg(feature = "federation")]
    fn direct_route_member_connectors_with_melt_coordinator(
        states: &[MintState],
        coordinator: Arc<DirectRouteMeltCoordinator>,
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        states
            .iter()
            .map(|state| {
                let member_id = state
                    .mint
                    .federation_config()
                    .expect("federation config")
                    .member_id();
                (
                    member_id,
                    Arc::new(DirectRouteMemberConnector::with_melt_coordinator(
                        state.clone(),
                        coordinator.clone(),
                    )) as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    #[cfg(feature = "federation")]
    fn appended_entry(
        journal: &mut InMemoryFederationJournal,
        envelope: FederationOperationEnvelope,
    ) -> FederationJournalEntry {
        journal.append(envelope).expect("append").entry().clone()
    }

    #[cfg(feature = "federation")]
    async fn persist_and_apply_entry(mint: &Mint, entry: &FederationJournalEntry) {
        if mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(entry.operation_id)
            .await
            .expect("load existing journal entry")
            .is_some()
        {
            return;
        }
        let runtime_config = mint.federation_config().expect("federation config");
        let retained_items = mint
            .localstore()
            .get_federation_aleph_bft_finalized_items()
            .await
            .expect("load finalized AlephBFT history");
        let mut adapter = FederationConsensusRuntime::recover_finalized_items_for_test(
            runtime_config.public_config(),
            runtime_config.member_id(),
            0,
            retained_items.clone(),
        )
        .expect("recover finalized AlephBFT history");
        let unit = FederationAlephBftUnitPayload::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            0,
            retained_items.len() as u64,
            vec![FederationConsensusItem::operation(entry.envelope.clone())],
        )
        .expect("test AlephBFT finalized unit");
        adapter
            .finalize_unit(unit)
            .expect("finalize test AlephBFT unit");
        let finalized_entry = adapter
            .journal()
            .entries()
            .last()
            .expect("finalized journal entry");
        let new_items = &adapter.finalized_items()[retained_items.len()..];
        let mut tx = mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        tx.add_federation_aleph_bft_finalized_items(new_items)
            .await
            .expect("persist finalized AlephBFT item");
        tx.add_federation_journal_entry(finalized_entry)
            .await
            .expect("persist journal entry");
        tx.commit().await.expect("commit journal entry");

        mint.apply_federation_journal_entries(std::slice::from_ref(finalized_entry))
            .await
            .expect("apply journal entry");
    }

    #[cfg(feature = "federation")]
    async fn add_paid_bolt11_quote_to_mint(mint: &Mint, quote_id: &QuoteId, amount: u64) {
        let amount = Amount::from(amount);
        let created_time = 1_700_000_000;
        let config = mint.federation_config().expect("federation config");
        let request = cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount,
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        });
        let response = cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(amount),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: created_time,
            state: MintQuoteState::Unpaid,
            expiry: Some(created_time + 10_000),
            pubkey: None,
        });
        let quote_envelope = config.mint_quote_operation_with_lookup_id(
            quote_id.clone(),
            cdk::cdk_payment::PaymentIdentifier::QuoteId(quote_id.clone()),
            request,
            response,
        );
        let mut journal = InMemoryFederationJournal::new();
        let quote_entry = appended_entry(&mut journal, quote_envelope);
        persist_and_apply_entry(mint, &quote_entry).await;

        let payment_envelope = config.mint_quote_payment_observation_with_metadata(
            quote_id,
            PaymentMethod::BOLT11,
            MintQuoteState::Paid,
            amount,
            Amount::ZERO,
            None,
            Some(cdk::cdk_payment::PaymentIdentifier::CustomId(format!(
                "payment-{quote_id}"
            ))),
        );
        let payment_entry = appended_entry(&mut journal, payment_envelope);
        persist_and_apply_entry(mint, &payment_entry).await;
    }

    #[cfg(feature = "federation")]
    async fn add_observed_paid_bolt11_quote_to_members(
        mints: &[Arc<Mint>],
        quote_id: &QuoteId,
        amount: u64,
    ) {
        for mint in mints {
            add_paid_bolt11_quote_to_mint(mint, quote_id, amount).await;
        }

        let payment_id =
            cdk::cdk_payment::PaymentIdentifier::CustomId(format!("payment-{quote_id}"));
        for observer in mints {
            let envelope = observer
                .federation_config()
                .expect("federation config")
                .mint_quote_payment_observation_with_metadata(
                    quote_id,
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    MintQuoteState::Paid,
                    Amount::from(amount),
                    Amount::ZERO,
                    None,
                    Some(payment_id.clone()),
                );
            let entry = appended_entry(&mut InMemoryFederationJournal::new(), envelope);
            for mint in mints {
                persist_and_apply_entry(mint, &entry).await;
            }
        }
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote_for_members(
        mints: &[Arc<Mint>],
        request: cdk::MeltQuoteRequest,
    ) -> QuoteId {
        let primary = mints.first().expect("at least one federation member");
        let (response, metadata) = primary
            .prepare_melt_quote(request.clone())
            .await
            .expect("prepare federated melt quote");
        let quote_id = response.quote().expect("melt quote id").clone();
        let config = primary.federation_config().expect("federation config");
        let envelope = config.melt_quote_operation(quote_id.clone(), request, response, metadata);
        let entry = appended_entry(&mut InMemoryFederationJournal::new(), envelope);

        for mint in mints {
            persist_and_apply_entry(mint, &entry).await;
        }

        quote_id
    }

    #[cfg(feature = "federation")]
    fn blinded_output(
        keyset: &cdk::federation::FederatedKeyset,
        amount: u64,
        label: &str,
    ) -> BlindedMessage {
        BlindedMessage::new(
            Amount::from(amount),
            keyset.id,
            cdk::nuts::PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
        )
    }

    #[cfg(feature = "federation")]
    fn blinded_change_output(
        keyset: &cdk::federation::FederatedKeyset,
        amount_label: &str,
    ) -> BlindedMessage {
        let mut output = blinded_output(keyset, 2, amount_label);
        output.amount = Amount::ZERO;
        output
    }

    #[cfg(feature = "federation")]
    fn proof_for_keyset(
        keyset: &cdk::federation::FederatedKeyset,
        amount: u64,
        label: &str,
    ) -> Proof {
        let secret = Secret::new(format!("federated-axum-http-proof-{label}"));
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            cdk::nuts::PublicKey::from(signature),
        )
    }

    #[cfg(feature = "federation")]
    fn assert_aggregate_signatures_verify(
        public_config: &FederationConfig,
        outputs: &[BlindedMessage],
        signatures: &[BlindSignature],
    ) {
        assert_eq!(signatures.len(), outputs.len());
        for (signature, output) in signatures.iter().zip(outputs) {
            let keyset = public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == signature.keyset_id)
                .expect("signature keyset");
            let aggregate_key = keyset
                .keys()
                .amount_key(signature.amount)
                .expect("amount key");
            cdk::dhke::verify_bls_blind_signature(
                aggregate_key,
                signature.c,
                output.blinded_secret,
            )
            .expect("aggregate signature verifies");
        }
    }

    #[cfg(feature = "federation")]
    fn threshold_http_member_connectors(
        public_config: &FederationConfig,
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        public_config
            .members
            .iter()
            .map(|(member_id, member)| {
                (
                    *member_id,
                    Arc::new(cdk::HttpClient::new(member.public_mint_url.clone(), None))
                        as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    #[cfg(feature = "federation")]
    async fn wait_for_federated_http_members(connector: &FederatedMintConnector) {
        for _ in 0..100 {
            if connector.get_mint_keys().await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        connector
            .get_mint_keys()
            .await
            .expect("federated HTTP members ready");
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
        create_federated_melt_quote_with_description(state, amount_msat, "".to_string()).await
    }

    #[cfg(feature = "federation")]
    async fn reserve_federated_melt_for_observer(state: &MintState, label: &str) -> QuoteId {
        let request = create_federated_melt_request(
            state,
            1_000,
            "".to_string(),
            vec![proof_for_amount(state, 2, label)],
            None,
        )
        .await;
        let quote_id = request.quote().clone();
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federation config");
        state
            .submit_federation_envelope(config.melt_operation(request).expect("melt operation"))
            .await
            .expect("accepted melt reserve");
        quote_id
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        finalize_federated_melt_quote_with_description(state, amount_msat, description).await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_request(
        state: &MintState,
        amount_msat: u64,
        description: String,
        inputs: Vec<Proof>,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> cdk::nuts::MeltRequest<QuoteId> {
        let quote_id =
            finalize_federated_melt_quote_with_description(state, amount_msat, description).await;
        let request = cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, outputs);
        request
    }

    #[cfg(feature = "federation")]
    async fn finalize_federated_melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        let invoice = create_fake_invoice(amount_msat, description);
        let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let (response, metadata) = create_melt_quote_for_route(state, request.clone())
            .await
            .expect("prepare federated melt quote");
        let quote_id = response.quote().expect("melt quote id").clone();
        let config = state.mint.federation_config().expect("federation config");
        let metadata = metadata.expect("federation melt quote metadata");
        let envelope =
            config.melt_quote_operation(quote_id.clone(), request, response.clone(), metadata);
        finalize_local_test_operation(state, envelope).await;

        let response_quote = match response {
            MeltQuoteCreateResponse::Bolt11(response) => {
                assert_eq!(response.quote, quote_id);
                response.quote
            }
            _ => panic!("expected bolt11 melt quote"),
        };
        assert_eq!(response_quote, quote_id);
        quote_id
    }

    #[cfg(feature = "federation")]
    async fn set_federated_melt_quote_state(
        state: &MintState,
        quote_id: &QuoteId,
        target_state: MeltQuoteState,
        payment_proof: Option<&str>,
    ) {
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        let mut quote = tx
            .get_melt_quote(quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        if target_state == MeltQuoteState::Paid {
            tx.update_melt_quote_state(&mut quote, MeltQuoteState::Pending, None)
                .await
                .expect("mark melt quote pending before paid");
        }
        tx.update_melt_quote_state(&mut quote, target_state, payment_proof.map(str::to_string))
            .await
            .expect("update melt quote state");
        tx.commit().await.expect("commit melt quote state");
    }

    async fn response_error_detail(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[cfg(feature = "federation")]
    async fn bolt11_melt_quote_response_from_response(
        response: Response,
    ) -> cdk::nuts::MeltQuoteBolt11Response<QuoteId> {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("melt quote response body");
        serde_json::from_slice(&bytes).expect("bolt11 melt quote response")
    }

    #[cfg(feature = "federation")]
    async fn assert_aleph_bft_http_response_not_ready(response: Response) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_error_detail(response).await;
        assert!(
            body.contains("awaiting quorum peer validation"),
            "AlephBFT public route should fail closed before queueing: {body}"
        );
    }

    #[cfg(feature = "federation")]
    async fn assert_http_response_not_ready_contains(response: Response, expected: &str) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_error_detail(response).await;
        assert!(
            body.contains(expected),
            "public route should fail closed with `{expected}` in the response: {body}"
        );
    }

    #[cfg(feature = "federation")]
    fn assert_aleph_bft_error_not_ready(error: &cdk::Error) {
        match error {
            cdk::Error::Federation(
                cdk::federation::FederationError::NodeAwaitingQuorum { .. }
                | cdk::federation::FederationError::NodeHalted { .. },
            ) => {}
            _ => panic!("AlephBFT public submit should fail closed, got: {error}"),
        }
    }

    #[cfg(feature = "federation")]
    fn assert_catching_up_error(error: &cdk::Error) {
        assert!(matches!(
            error,
            cdk::Error::Federation(cdk::federation::FederationError::NodeCatchingUp { .. })
        ));
    }

    #[cfg(feature = "federation")]
    async fn assert_no_aleph_bft_public_submit_work(state: &MintState, accepted_before: usize) {
        assert_eq!(
            federation_operation_kinds(state).await.len(),
            accepted_before,
            "halted AlephBFT public route must not append a journal entry"
        );
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        assert_eq!(
            consensus.queued_len(),
            0,
            "halted AlephBFT public route must not queue an item"
        );
        assert_eq!(
            consensus.peer_status().operation_count,
            accepted_before as u64,
            "halted AlephBFT public route must not accept an operation"
        );
        drop(consensus);
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "halted AlephBFT public route must not register a response waiter"
        );
    }

    #[cfg(feature = "federation")]
    async fn assert_no_federation_public_submit_work(state: &MintState, accepted_before: usize) {
        assert_eq!(
            federation_operation_kinds(state).await.len(),
            accepted_before,
            "not-ready federation public route must not append a journal entry"
        );
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        assert_eq!(
            consensus.queued_len(),
            0,
            "not-ready federation public route must not queue an item"
        );
        assert_eq!(
            consensus.peer_status().operation_count,
            accepted_before as u64,
            "not-ready federation public route must not accept an operation"
        );
        drop(consensus);
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "not-ready federation public route must not register a response waiter"
        );
    }

    #[tokio::test]
    async fn test_prefer_header_respond_async() {
        let req = create_test_request(Some("respond-async"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_respond_async_with_other_values() {
        let req = create_test_request(Some("respond-async; wait=10"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_case_insensitive() {
        let req = create_test_request(Some("RESPOND-ASYNC"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_no_respond_async() {
        let req = create_test_request(Some("wait=10"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_missing() {
        let req = create_test_request(None);
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_invalid_value() {
        let req = create_test_request_with_bytes(&[0xFF, 0xFE]);
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err());
        let (status, message) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(message, "Invalid Prefer header value");
    }

    #[tokio::test]
    async fn test_prefer_header_empty_value() {
        let req = create_test_request(Some(""));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_custom_quote_records_quote_and_immediate_payment_observation() {
        let state =
            create_federated_test_state_with_backend(InstantIncomingPaymentWallet::new()).await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .unwrap();

        let result = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await;

        assert!(result.is_ok(), "mint quote creation should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quotes()
                .await
                .expect("mint quotes lookup")
                .len(),
            1
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_quote_creation_accepts_standard_cashu_request() {
        let state = create_federated_test_state().await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        let response = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect("standard federation quote request");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            state
                .mint
                .localstore()
                .get_mint_quotes()
                .await
                .expect("mint quotes lookup")
                .len()
                == 1
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuote]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn repeated_standard_federation_quote_requests_create_distinct_quotes() {
        let state = create_federated_test_state().await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        for _ in 0..2 {
            post_mint_custom_quote(
                AuthHeader::None,
                State(state.clone()),
                Path("bolt11".to_string()),
                Json(payload.clone()),
            )
            .await
            .expect("standard federation quote request");
        }

        let quotes = state
            .mint
            .localstore()
            .get_mint_quotes()
            .await
            .expect("mint quotes lookup");
        assert_eq!(quotes.len(), 2);
        assert_ne!(quotes[0].id, quotes[1].id);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuote,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_aleph_bft_mint_quote_is_not_persisted_before_consensus() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            ..crate::FederationPeerSyncConfig::default()
        });
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        let error = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect_err("mint quote should remain pending without AlephBFT finalization");

        let body = response_error_detail(error).await;
        assert!(body.contains("pending AlephBFT finalization"), "{body}");
        assert!(state
            .mint
            .localstore()
            .get_mint_quotes()
            .await
            .expect("pending mint quotes lookup")
            .is_empty());
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        assert_eq!(consensus.queued_len(), 1);
        assert!(consensus.journal().entries().is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_custom_quote_aleph_bft_public_route_fails_closed_before_quote_creation() {
        let state = create_aleph_bft_federated_test_state().await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        let error = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect_err("AlephBFT public mint quote should fail closed before local quote creation");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state, 0).await;
        assert!(
            state
                .mint
                .localstore()
                .get_mint_quotes()
                .await
                .expect("mint quotes lookup")
                .is_empty(),
            "halted AlephBFT public mint quote route must not persist a local quote"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_custom_quote_skips_immediate_observation_when_unpaid() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        let result = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await;

        assert!(result.is_ok(), "unpaid mint quote creation should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuote],
            "unpaid mint quote creation should only record the quote operation"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_melt_custom_quote_records_federation_quote_operation() {
        let state = create_federated_test_state().await;
        let payload = serde_json::to_value(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        })
        .expect("melt quote payload");

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await;

        assert!(result.is_ok(), "melt quote creation should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuote]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quotes()
                .await
                .expect("melt quotes lookup")
                .len(),
            1
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_melt_custom_quote_aleph_bft_public_route_fails_closed_before_quote_creation() {
        let state = create_aleph_bft_federated_test_state().await;
        let payload = serde_json::to_value(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "aleph-bft-halted-melt-quote".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        })
        .expect("melt quote payload");

        let error = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect_err("AlephBFT public melt quote should fail closed before local quote creation");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state, 0).await;
        assert!(
            state
                .mint
                .localstore()
                .get_melt_quotes()
                .await
                .expect("melt quotes lookup")
                .is_empty(),
            "halted AlephBFT public melt quote route must not persist a local quote"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_paid_bolt11_quote(&state).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(result.is_ok(), "mint quote check should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_skips_duplicate_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_paid_bolt11_quote(&state).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id.clone()],
            }),
        )
        .await;

        assert!(result.is_ok(), "initial mint quote check should succeed");

        let duplicate = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            duplicate.is_ok(),
            "duplicate mint quote check should use the recorded observation"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_skips_unpaid_federation_payment_observation() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote.quote],
            }),
        )
        .await;

        assert!(result.is_ok(), "unpaid mint quote check should succeed");
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "unpaid mint quote checks should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_submits_paid_quote_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_paid_bolt11_quote(&state).await;

        let mut submitted = 0;
        for _ in 0..10 {
            submitted = state
                .federation
                .payment_observer
                .run_round(&state)
                .await
                .expect("observer round");
            if submitted == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(submitted, 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
        let checked = state
            .mint
            .check_mint_quotes(std::slice::from_ref(&quote_id))
            .await
            .expect("stored quote status");
        assert_eq!(checked[0].state(), Some(cdk::nuts::MintQuoteState::Paid));

        let duplicate = state
            .federation
            .payment_observer
            .run_round(&state)
            .await
            .expect("duplicate observer round");
        assert_eq!(duplicate, 0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn payment_notification_wakes_observer_without_direct_quote_mutation() {
        let state = create_federated_test_state().await;
        let mut payment_events = state.mint.subscribe_federation_payment_events();
        let quote_id = create_federation_paid_bolt11_quote(&state).await;

        let event = tokio::time::timeout(Duration::from_secs(1), payment_events.recv())
            .await
            .expect("payment notification timeout")
            .expect("payment notification");
        let stored_before_consensus = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(stored_before_consensus.state(), MintQuoteState::Unpaid);

        let submitted = state
            .federation
            .payment_observer
            .observe_event(&state, event)
            .await
            .expect("event observation");

        assert_eq!(submitted, 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
        let stored_after_consensus = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(stored_after_consensus.state(), MintQuoteState::Paid);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_skips_consensus_finalized_quote() {
        let state = create_federated_test_state().await;
        let _quote_id = create_federation_observed_paid_bolt11_quote(&state).await;

        let duplicate = state
            .federation
            .payment_observer
            .run_round(&state)
            .await
            .expect("observer round");

        assert_eq!(duplicate, 0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_skips_unpaid_quotes() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        let _quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");

        let submitted = state
            .federation
            .payment_observer
            .run_round(&state)
            .await
            .expect("observer round");

        assert_eq!(submitted, 0);
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "unpaid observer rounds should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_records_observation_while_sync_not_ready() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_paid_bolt11_quote(&state).await;
        state.mark_federation_awaiting_quorum(1).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "mint quote check should still record an internal payment observation"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = reserve_federated_melt_for_observer(&state, "terminal observation").await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(result.is_ok(), "melt quote check should succeed");
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(kinds.len() <= 3);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_records_observation_while_sync_not_ready() {
        let state = create_federated_test_state().await;
        let quote_id =
            reserve_federated_melt_for_observer(&state, "sync-not-ready observation").await;
        state.mark_federation_awaiting_quorum(1).await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        if let Err(response) = result {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("error response body");
            panic!(
                "melt quote check should still record an internal payment observation: {} {}",
                status,
                String::from_utf8_lossy(&body)
            );
        }
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_skips_duplicate_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = reserve_federated_melt_for_observer(&state, "duplicate observation").await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await;

        assert!(result.is_ok(), "initial melt quote check should succeed");

        let duplicate = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(
            duplicate.is_ok(),
            "duplicate melt quote check should use the recorded observation"
        );
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_skips_non_terminal_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        set_federated_melt_quote_state(&state, &quote_id, MeltQuoteState::Pending, None).await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(result.is_ok(), "pending melt quote check should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuote],
            "pending melt quote checks should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_skips_unattempted_unpaid_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await;

        assert!(
            result.is_ok(),
            "fresh unpaid melt quote check should succeed"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuote],
            "fresh unpaid melt quote checks must not submit terminal payment observations"
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("melt payment observations")
            .is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_mint_advertises_aggregate_keysets() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let active = state.mint.get_active_keysets();
        let pubkeys = state.mint.pubkeys();
        let keysets = state.mint.keysets();
        let specific = state
            .mint
            .keyset_pubkeys(&keyset.id)
            .expect("federation keyset pubkeys");

        assert_eq!(active.get(&CurrencyUnit::Sat), Some(&keyset.id));
        assert_eq!(pubkeys.keysets.len(), 1);
        assert_eq!(pubkeys.keysets[0].id, keyset.id);
        assert_eq!(pubkeys.keysets[0].keys, keyset.keys());
        assert_eq!(
            pubkeys.keysets[0].id.get_version(),
            cdk::nuts::KeySetVersion::Version02
        );
        assert_eq!(keysets.keysets.len(), 1);
        assert_eq!(keysets.keysets[0].id, keyset.id);
        assert_eq!(specific.keysets[0], pubkeys.keysets[0]);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn submit_batch_mint_records_federation_mint_operation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        submit_federation_mint_operation(&state, cdk::mint::MintInput::Batch(batch_request))
            .await
            .expect("federation mint operation should be accepted");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn submit_batch_mint_rejects_non_v3_outputs_before_consensus() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: vec![non_v3_output_for_state(
                &state,
                Amount::from(2),
                b"axum federated mint non-v3 output",
            )],
            signatures: None,
        };

        let error =
            submit_federation_mint_operation(&state, cdk::mint::MintInput::Batch(batch_request))
                .await
                .expect_err("non-v3 mint output should be rejected");

        assert!(matches!(
            error,
            cdk::Error::Federation(
                cdk::federation::FederationError::InvalidFederatedKeysetVersion { .. }
            )
        ));
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated mint outputs must fail before consensus"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_federation_mint_input_waits_for_paid_quote_before_queueing() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let mut state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            ..crate::FederationPeerSyncConfig::default()
        });
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");
        let input = cdk::mint::MintInput::Single(MintRequest {
            quote: quote.quote.clone(),
            outputs: outputs_for_amount(&state, 2),
            signature: None,
        });

        let error = process_federation_mint_input(&state, &input)
            .await
            .expect_err("unpaid federated mint should remain pending");

        assert!(
            matches!(error, cdk::Error::PendingQuote),
            "unpaid federated mint must be retryable: {error}"
        );
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "unpaid federated mint must not enter consensus before payment observation"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote.quote)
                .await
                .expect("mint quote lookup")
                .expect("mint quote")
                .state(),
            MintQuoteState::Unpaid
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn cache_post_batch_mint_returns_federation_signature_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount(&state, 2);
        let blinded_secret = outputs[0]
            .blinded_secret
            .as_bls_g1()
            .expect("blinded output should be BLS");
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs.clone(),
            signatures: None,
        };

        let response = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let detail = response_error_detail(error).await;
                let entries = state
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("journal lookup after rejected mint");
                let outcomes = entries
                    .iter()
                    .map(|entry| {
                        (
                            entry.envelope.operation.kind(),
                            entry.status(),
                            entry.rejection(),
                        )
                    })
                    .collect::<Vec<_>>();
                panic!(
                    "federation batch mint should return share response: {detail}; outcomes={outcomes:?}"
                );
            }
        };
        let signature = response
            .signatures
            .first()
            .expect("one federation signature share");
        let signature_share = signature.c.as_bls_g1().expect("share should be BLS G1");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share =
            BlsBlindSignatureShare::new(config.member_id().to_bls_signer_id(), signature_share);

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, outputs[0].keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        let operation_id =
            first_federation_operation_id(&state, cdk::federation::FederationOperationKind::Mint)
                .await;
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("persisted mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].member_id, config.member_id());
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(share_records[0].keyset_id, signature.keyset_id);
        assert_eq!(share_records[0].amount, signature.amount);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            signature.c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn cache_post_batch_mint_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        let error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request),
        )
        .await
        .expect_err("AlephBFT public mint should fail closed before real finality exists");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state, 2).await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn batch_mint_pending_aleph_bft_retry_returns_persisted_response_without_issuing_twice() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "aleph-public-mint-timeout-retry");
        let request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs.clone(),
            signatures: None,
        };
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .mint_operation(cdk::mint::MintInput::Batch(request.clone()))
            .expect("mint operation")
            .operation_id()
            .expect("mint operation id");

        let error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request.clone()),
        )
        .await
        .expect_err("first AlephBFT batch mint should time out before finality");
        let body = response_error_detail(error).await;
        assert!(
            body.contains("pending AlephBFT finalization"),
            "pending mint timeout should be retryable: {body}"
        );
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "timed-out mint waiters must be released"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
            assert_eq!(consensus.peer_status().operation_count, 2);
        }
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending mint share lookup")
                .is_empty(),
            "pending AlephBFT mint must not issue signature shares"
        );

        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let retry_state = state.clone();
        let retry_request = request.clone();
        let retry = tokio::spawn(async move {
            cache_post_batch_mint(
                AuthHeader::None,
                State(retry_state),
                Path("bolt11".to_string()),
                Json(retry_request),
            )
            .await
        });

        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "retrying an already queued mint should wait on the pending operation"
        );
        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let response = retry
            .await
            .expect("retry task should complete")
            .expect("retry batch mint should return accepted response");
        assert_eq!(response.signatures.len(), 1);
        assert_eq!(response.signatures[0].amount, Amount::from(2));
        assert_eq!(response.signatures[0].keyset_id, outputs[0].keyset_id);

        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            response.signatures[0].c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 3);
        }
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "accepted retry should release its waiter"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);

        let duplicate = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request),
        )
        .await
        .expect("accepted duplicate batch mint should return persisted response");
        assert_eq!(duplicate.0, response.0);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("duplicate mint share lookup"),
            share_records
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_pending_aleph_bft_retry_returns_persisted_response_without_issuing_twice() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "aleph-public-single-mint-retry");
        let request = MintRequest {
            quote: quote_id.clone(),
            outputs: outputs.clone(),
            signature: None,
        };
        let mint_input = cdk::mint::MintInput::Single(request.clone());
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .mint_operation(mint_input)
            .expect("mint operation")
            .operation_id()
            .expect("mint operation id");

        let error = cache_post_mint_custom(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request.clone()),
        )
        .await
        .expect_err("first AlephBFT single mint should time out before finality");
        let body = response_error_detail(error).await;
        assert!(
            body.contains("pending AlephBFT finalization"),
            "pending single mint timeout should be retryable: {body}"
        );
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "timed-out single mint waiters must be released"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
            assert_eq!(consensus.peer_status().operation_count, 2);
        }
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote_id)
                .await
                .expect("mint quote lookup")
                .expect("mint quote")
                .state(),
            MintQuoteState::Paid,
            "pending AlephBFT single mint must not mark the quote issued"
        );
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending single mint share lookup")
                .is_empty(),
            "pending AlephBFT single mint must not issue signature shares"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[outputs[0].blinded_secret])
                .await
                .expect("pending single mint blind signature lookup"),
            vec![None],
            "pending AlephBFT single mint must not persist blind signatures"
        );

        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let retry_state = state.clone();
        let retry_request = request.clone();
        let retry = tokio::spawn(async move {
            cache_post_mint_custom(
                AuthHeader::None,
                State(retry_state),
                Path("bolt11".to_string()),
                Json(retry_request),
            )
            .await
        });

        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "retrying an already queued single mint should wait on the pending operation"
        );
        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let response = retry
            .await
            .expect("retry task should complete")
            .expect("retry single mint should return accepted response");
        assert_eq!(response.signatures.len(), 1);
        assert_eq!(response.signatures[0].amount, Amount::from(2));
        assert_eq!(response.signatures[0].keyset_id, outputs[0].keyset_id);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote_id)
                .await
                .expect("accepted mint quote lookup")
                .expect("accepted mint quote")
                .state(),
            MintQuoteState::Issued
        );

        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted single mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            response.signatures[0].c
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[outputs[0].blinded_secret])
                .await
                .expect("accepted single mint blind signature lookup"),
            vec![Some(response.signatures[0].clone())],
            "accepted AlephBFT single mint must persist the returned signature"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 3);
        }
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "accepted retry should release its waiter"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);

        let duplicate = cache_post_mint_custom(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request),
        )
        .await
        .expect("accepted duplicate single mint should return persisted response");
        assert_eq!(duplicate.0, response.0);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("duplicate single mint share lookup"),
            share_records
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_batch_mint_aleph_bft_pending_requests_join_one_queue_item() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 4,
            ..crate::FederationPeerSyncConfig::default()
        });
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "aleph-public-mint-pending-join");
        let request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs.clone(),
            signatures: None,
        };
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .mint_operation(cdk::mint::MintInput::Batch(request.clone()))
            .expect("mint operation")
            .operation_id()
            .expect("mint operation id");

        let first_state = state.clone();
        let first_request = request.clone();
        let first = tokio::spawn(async move {
            cache_post_batch_mint(
                AuthHeader::None,
                State(first_state),
                Path("bolt11".to_string()),
                Json(first_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "first mint request should wait for AlephBFT finality"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
        }

        let second_state = state.clone();
        let second_request = request.clone();
        let second = tokio::spawn(async move {
            cache_post_batch_mint(
                AuthHeader::None,
                State(second_state),
                Path("bolt11".to_string()),
                Json(second_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(2)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            2,
            "duplicate pending mint request should join the existing waiter"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
            assert_eq!(consensus.peer_status().operation_count, 2);
        }
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending duplicate mint share lookup")
                .is_empty(),
            "duplicate pending mint requests must not issue before finality"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        assert_eq!(stats.duplicate_pending_waiter_joins, 1);

        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let first_response = first
            .await
            .expect("first mint task should complete")
            .expect("first pending mint should return accepted response")
            .0;
        let second_response = second
            .await
            .expect("second mint task should complete")
            .expect("second pending mint should return accepted response")
            .0;
        assert_eq!(second_response, first_response);

        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted duplicate mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            first_response.signatures[0].c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 3);
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 3);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_mint_aleph_bft_pending_requests_join_one_queue_item() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 4,
            ..crate::FederationPeerSyncConfig::default()
        });
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "aleph-single-mint-pending-join");
        let request = MintRequest {
            quote: quote_id.clone(),
            outputs: outputs.clone(),
            signature: None,
        };
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .mint_operation(cdk::mint::MintInput::Single(request.clone()))
            .expect("mint operation")
            .operation_id()
            .expect("mint operation id");

        let first_state = state.clone();
        let first_request = request.clone();
        let first = tokio::spawn(async move {
            cache_post_mint_custom(
                AuthHeader::None,
                State(first_state),
                Path("bolt11".to_string()),
                Json(first_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "first single mint request should wait for AlephBFT finality"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
            assert_eq!(consensus.peer_status().operation_count, 2);
        }

        let second_state = state.clone();
        let second_request = request.clone();
        let second = tokio::spawn(async move {
            cache_post_mint_custom(
                AuthHeader::None,
                State(second_state),
                Path("bolt11".to_string()),
                Json(second_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(2)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            2,
            "duplicate pending single mint request should join the existing waiter"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(consensus.journal().entries().len(), 2);
            assert_eq!(consensus.peer_status().operation_count, 2);
        }
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending duplicate single mint share lookup")
                .is_empty(),
            "duplicate pending single mint requests must not issue before finality"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote_id)
                .await
                .expect("pending duplicate single mint quote lookup")
                .expect("pending duplicate single mint quote")
                .state(),
            MintQuoteState::Paid,
            "duplicate pending single mint requests must not mark the quote issued"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        assert_eq!(stats.duplicate_pending_waiter_joins, 1);

        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let first_response = first
            .await
            .expect("first single mint task should complete")
            .expect("first pending single mint should return accepted response")
            .0;
        let second_response = second
            .await
            .expect("second single mint task should complete")
            .expect("second pending single mint should return accepted response")
            .0;
        assert_eq!(second_response, first_response);

        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted duplicate single mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            first_response.signatures[0].c
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_mint_quote(&quote_id)
                .await
                .expect("accepted duplicate single mint quote lookup")
                .expect("accepted duplicate single mint quote")
                .state(),
            MintQuoteState::Issued
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 3);
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 3);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_batch_mint_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount(&state, 2);
        let input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs,
            signatures: None,
        });

        let first = process_federation_mint_input(&state, &input)
            .await
            .expect("first federation mint submit")
            .expect("first federation mint response");
        let duplicate = process_federation_mint_input(&state, &input)
            .await
            .expect("duplicate federation mint submit")
            .expect("duplicate federation mint response");

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_cache_eviction_returns_persisted_shares() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let first_quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let first_outputs = outputs_for_amount_with_label(&state, 2, "mint-cache-eviction-first");
        let first_input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![first_quote_id],
            quote_amounts: None,
            outputs: first_outputs,
            signatures: None,
        });

        let first = process_federation_mint_input(&state, &first_input)
            .await
            .expect("first federation mint submit")
            .expect("first federation mint response");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let second_quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let second_outputs = outputs_for_amount_with_label(&state, 2, "mint-cache-eviction-second");
        let second_input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![second_quote_id],
            quote_amounts: None,
            outputs: second_outputs,
            signatures: None,
        });
        let second = process_federation_mint_input(&state, &second_input)
            .await
            .expect("second federation mint submit")
            .expect("second federation mint response");
        assert_ne!(second, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = process_federation_mint_input(&state, &first_input)
            .await
            .expect("duplicate federation mint submit")
            .expect("duplicate federation mint response");

        assert_eq!(duplicate, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_sqlite_restart_returns_persisted_shares() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-mint-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "mint-sqlite-restart");
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let first = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request.clone()),
        )
        .await
        .expect("first federation batch mint should return share response")
        .0;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );

        let duplicate = cache_post_batch_mint(
            AuthHeader::None,
            State(restarted_state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await
        .expect("duplicate federation batch mint after restart should return persisted shares")
        .0;

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_catch_up_returns_persisted_shares() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let quote_id = QuoteId::new();
        let quote_request = cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(2),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        });
        let quote_response = cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(Amount::from(2)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        });
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs: outputs_for_amount_with_label(&state, 2, "mint-catch-up-output"),
            signatures: None,
        };
        let mint_input = cdk::mint::MintInput::Batch(batch_request.clone());
        let mint_envelope = config
            .mint_operation(mint_input.clone())
            .expect("mint operation envelope");
        let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
        let mut peer_consensus = test_aleph_bft_peer(config.federation_id(), config.member_id());
        finalize_test_operation(
            &mut peer_consensus,
            config.mint_quote_operation(quote_id.clone(), quote_request, quote_response),
        )
        .expect("peer accepted mint quote");
        finalize_test_operation(
            &mut peer_consensus,
            config.mint_quote_payment_observation(
                quote_id,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintQuoteState::Paid,
                Amount::from(2),
                Amount::ZERO,
                Some("mint-catch-up-payment-proof".to_string()),
            ),
        )
        .expect("peer accepted mint quote payment");
        finalize_test_operation(&mut peer_consensus, mint_envelope)
            .expect("peer accepted mint operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = state
            .mint
            .persisted_mint_share_response(mint_operation_id, &mint_input)
            .await
            .expect("persisted mint response after catch-up");

        let duplicate = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await
        .expect("duplicate federation batch mint after catch-up should return persisted shares")
        .0;

        assert_eq!(duplicate, expected);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn concurrent_identical_value_routes_are_idempotent() {
        let (wallet, payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(wallet).await;
        let request_count = 8;

        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let mint_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount_with_label(&state, 2, "concurrent-mint-output"),
            signatures: None,
        };
        let mint_results = futures::future::join_all((0..request_count).map(|_| {
            let state = state.clone();
            let request = mint_request.clone();
            async move {
                cache_post_batch_mint(
                    AuthHeader::None,
                    State(state),
                    Path("bolt11".to_string()),
                    Json(request),
                )
                .await
            }
        }))
        .await;
        let mut mint_responses = Vec::new();
        for result in mint_results {
            match result {
                Ok(response) => mint_responses.push(response.0),
                Err(error) => {
                    panic!(
                        "concurrent identical mint request failed: {}",
                        response_error_detail(error).await
                    );
                }
            }
        }
        for response in mint_responses.iter().skip(1) {
            assert_eq!(response, &mint_responses[0]);
        }
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 3);

        let swap_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "concurrent-swap-input")],
            outputs_for_amount_with_label(&state, 2, "concurrent-swap-output"),
        );
        let swap_results = futures::future::join_all((0..request_count).map(|_| {
            let state = state.clone();
            let request = swap_request.clone();
            async move {
                crate::router_handlers::post_swap(AuthHeader::None, State(state), Json(request))
                    .await
            }
        }))
        .await;
        let mut swap_responses = Vec::new();
        for result in swap_results {
            match result {
                Ok(response) => swap_responses.push(response.0),
                Err(error) => {
                    panic!(
                        "concurrent identical swap request failed: {}",
                        response_error_detail(error).await
                    );
                }
            }
        }
        for response in swap_responses.iter().skip(1) {
            assert_eq!(response, &swap_responses[0]);
        }
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Swap,
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 4);

        let mut change_output = outputs_for_amount_with_label(&state, 2, "concurrent-melt-change")
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let melt_request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "concurrent-melt-input")],
            Some(vec![change_output]),
        )
        .await;
        let melt_results = futures::future::join_all((0..request_count).map(|_| {
            let state = state.clone();
            let request = melt_request.clone();
            async move {
                process_melt_request(
                    PreferHeader {
                        respond_async: false,
                    },
                    &state,
                    "bolt11",
                    &request,
                )
                .await
            }
        }))
        .await;
        let mut melt_responses = Vec::new();
        for result in melt_results {
            match result {
                Ok(response) => melt_responses.push(response),
                Err(error) => panic!("concurrent identical melt request failed: {error}"),
            }
        }
        for response in melt_responses.iter().skip(1) {
            assert_eq!(response, &melt_responses[0]);
        }
        if melt_responses[0].state() == MeltQuoteState::Pending {
            assert_eq!(payment_calls.load(Ordering::SeqCst), 0);
            return;
        }
        assert_eq!(melt_responses[0].state(), MeltQuoteState::Paid);
        assert!(melt_responses[0].change().is_some());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Swap,
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 7);
        assert_eq!(
            payment_calls.load(Ordering::SeqCst),
            1,
            "concurrent identical melt requests must make one outgoing payment attempt"
        );
        let stats = state.federation_operation_notification_stats();
        assert!(
            stats.duplicate_submissions >= (request_count - 1) as u64,
            "concurrent duplicate requests should be recorded as duplicate submissions"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn batch_mint_rejects_same_quote_with_different_outputs_without_signing() {
        let state = create_federated_test_state().await;
        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let first_outputs = outputs_for_amount_with_label(&state, 2, "same-quote-first-output");
        let second_outputs = outputs_for_amount_with_label(&state, 2, "same-quote-second-output");

        let _first_response = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchMintRequest {
                quotes: vec![quote_id.clone()],
                quote_amounts: None,
                outputs: first_outputs,
                signatures: None,
            }),
        )
        .await
        .expect("first mint request should succeed");

        let error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchMintRequest {
                quotes: vec![quote_id],
                quote_amounts: None,
                outputs: second_outputs.clone(),
                signatures: None,
            }),
        )
        .await
        .expect_err("same quote with different outputs should be rejected");
        let body = response_error_detail(error).await.to_lowercase();
        assert!(
            body.contains("conflict") || body.contains("issued") || body.contains("not paid"),
            "rejection should mention conflicting or already-issued quote state: {body}"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0].status(),
            cdk::federation::FederationOperationStatus::Applied
        );
        assert_eq!(
            entries[3].status(),
            cdk::federation::FederationOperationStatus::Rejected
        );
        assert_eq!(state.federation_response_cache_entry_count(), 4);

        let unsigned_output = second_outputs
            .first()
            .expect("second output")
            .blinded_secret;
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[unsigned_output])
                .await
                .expect("blind signature lookup"),
            vec![None],
            "conflicting mint output must not receive a member signature share"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_returns_federation_signature_shares() {
        let state = create_federated_test_state().await;
        let outputs = outputs_for_amount(&state, 2);
        let inputs = vec![proof_for_amount(&state, 2, "swap-input")];
        let input_y = inputs[0].y().expect("input y");
        let blinded_secret = outputs[0]
            .blinded_secret
            .as_bls_g1()
            .expect("blinded output should be BLS");

        let response = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::SwapRequest::new(inputs, outputs.clone())),
        )
        .await
        .expect("federation swap should return share response");
        let signature = response
            .signatures
            .first()
            .expect("one federation signature share");
        let signature_share = signature.c.as_bls_g1().expect("share should be BLS G1");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share =
            BlsBlindSignatureShare::new(config.member_id().to_bls_signer_id(), signature_share);

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, outputs[0].keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        let operation_id =
            first_federation_operation_id(&state, cdk::federation::FederationOperationKind::Swap)
                .await;
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("persisted swap share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].member_id, config.member_id());
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(share_records[0].keyset_id, signature.keyset_id);
        assert_eq!(share_records[0].amount, signature.amount);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            signature.c
        );
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let stored_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[outputs[0].blinded_secret])
            .await
            .expect("signature lookup");
        assert_eq!(stored_signatures, vec![Some(signature.clone())]);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_reports_typed_federation_catch_up_error() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let peer = config
            .public_config()
            .members
            .keys()
            .copied()
            .find(|member| *member != config.member_id())
            .expect("peer member");
        state.mark_federation_catching_up(peer).await;
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "catch-up-swap-input")],
            outputs_for_amount_with_label(&state, 2, "catch-up-swap-output"),
        );

        let response =
            crate::router_handlers::post_swap(AuthHeader::None, State(state), Json(request))
                .await
                .expect_err("catching-up member must fail closed with a retryable Cashu error");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("catch-up error body");
        let error: cdk_common::error::ErrorResponse =
            serde_json::from_slice(&bytes).expect("Cashu error response");
        assert_eq!(error.code, cdk_common::error::ErrorCode::FederationCatchUp);
        assert!(error.detail.contains("catching up"));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "aleph-bft-halted-swap-input")],
            outputs_for_amount_with_label(&state, 2, "aleph-bft-halted-swap-output"),
        );

        let error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect_err("AlephBFT public swap should fail closed before real finality exists");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state, 0).await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_pending_aleph_bft_retry_returns_persisted_response_after_finality() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let input = proof_for_amount(&state, 2, "aleph-public-swap-pending-input");
        let input_y = input.y().expect("swap input y");
        let outputs = outputs_for_amount_with_label(&state, 2, "aleph-public-swap-pending-output");
        let request = cdk::nuts::SwapRequest::new(vec![input], outputs.clone());
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .swap_operation(request.clone())
            .expect("swap operation")
            .operation_id()
            .expect("swap operation id");

        let error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request.clone()),
        )
        .await
        .expect_err("first AlephBFT swap should time out before finality");
        let body = response_error_detail(error).await;
        assert!(
            body.contains("pending AlephBFT finalization"),
            "pending swap timeout should be retryable: {body}"
        );
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "timed-out swap waiters must be released"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert!(
                consensus.journal().entries().is_empty(),
                "pending AlephBFT swap must not be journaled before finality"
            );
            assert_eq!(consensus.peer_status().operation_count, 0);
        }
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("pending swap proof lookup");
        assert!(
            proof_states.iter().all(|state| !matches!(
                state,
                Some(cdk::nuts::State::Pending) | Some(cdk::nuts::State::Spent)
            )),
            "pending AlephBFT swap must not reserve or spend inputs before finality"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[outputs[0].blinded_secret])
                .await
                .expect("pending swap signature lookup"),
            vec![None],
            "pending AlephBFT swap must not issue signature shares before finality"
        );

        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let retry_state = state.clone();
        let retry_request = request.clone();
        let retry = tokio::spawn(async move {
            crate::router_handlers::post_swap(
                AuthHeader::None,
                State(retry_state),
                Json(retry_request),
            )
            .await
        });

        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "retrying an already queued swap should wait on the pending operation"
        );
        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let response = retry
            .await
            .expect("retry task should complete")
            .expect("retry swap should return accepted response")
            .0;
        assert_eq!(response.signatures.len(), 1);
        assert_eq!(response.signatures[0].amount, Amount::from(2));
        assert_eq!(response.signatures[0].keyset_id, outputs[0].keyset_id);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("accepted swap proof lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted swap share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            response.signatures[0].c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_aleph_bft_pending_requests_join_one_queue_item() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 4,
            ..crate::FederationPeerSyncConfig::default()
        });
        let input = proof_for_amount(&state, 2, "aleph-public-swap-pending-join-input");
        let input_y = input.y().expect("swap input y");
        let outputs =
            outputs_for_amount_with_label(&state, 2, "aleph-public-swap-pending-join-output");
        let request = cdk::nuts::SwapRequest::new(vec![input], outputs.clone());
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .swap_operation(request.clone())
            .expect("swap operation")
            .operation_id()
            .expect("swap operation id");

        let first_state = state.clone();
        let first_request = request.clone();
        let first = tokio::spawn(async move {
            crate::router_handlers::post_swap(
                AuthHeader::None,
                State(first_state),
                Json(first_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "first swap request should wait for AlephBFT finality"
        );

        let second_state = state.clone();
        let second_request = request.clone();
        let second = tokio::spawn(async move {
            crate::router_handlers::post_swap(
                AuthHeader::None,
                State(second_state),
                Json(second_request),
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(2)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            2,
            "duplicate pending swap request should join the existing waiter"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert!(
                consensus.journal().entries().is_empty(),
                "pending duplicate swap must not append before finality"
            );
            assert_eq!(consensus.peer_status().operation_count, 0);
        }
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("pending duplicate swap proof lookup");
        assert!(
            proof_states.iter().all(|state| !matches!(
                state,
                Some(cdk::nuts::State::Pending) | Some(cdk::nuts::State::Spent)
            )),
            "duplicate pending swap requests must not reserve or spend inputs before finality"
        );
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending duplicate swap share lookup")
                .is_empty(),
            "duplicate pending swap requests must not issue shares before finality"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[outputs[0].blinded_secret])
                .await
                .expect("pending duplicate swap signature lookup"),
            vec![None],
            "duplicate pending swap requests must not persist signatures before finality"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        assert_eq!(stats.duplicate_pending_waiter_joins, 1);

        finalize_next_local_aleph_bft_unit(&state, operation_id).await;

        let first_response = first
            .await
            .expect("first swap task should complete")
            .expect("first pending swap should return accepted response")
            .0;
        let second_response = second
            .await
            .expect("second swap task should complete")
            .expect("second pending swap should return accepted response")
            .0;
        assert_eq!(second_response, first_response);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("accepted duplicate swap proof lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted duplicate swap share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            first_response.signatures[0].c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 1);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let outputs = outputs_for_amount(&state, 2);
        let inputs = vec![proof_for_amount(&state, 2, "duplicate-swap-input")];
        let request = cdk::nuts::SwapRequest::new(inputs, outputs);

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request.clone()),
        )
        .await
        .expect("first federation swap should return share response");
        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap should return persisted share response");

        assert_eq!(duplicate.0, first.0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_cache_eviction_returns_persisted_signature_shares() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let first_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-cache-eviction-first")],
            outputs_for_amount_with_label(&state, 2, "swap-cache-eviction-first"),
        );

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(first_request.clone()),
        )
        .await
        .expect("first federation swap should return share response");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let second_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-cache-eviction-second")],
            outputs_for_amount_with_label(&state, 2, "swap-cache-eviction-second"),
        );
        let second = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(second_request),
        )
        .await
        .expect("second federation swap should return share response");
        assert_ne!(second.0, first.0);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(first_request),
        )
        .await
        .expect("duplicate federation swap should return persisted share response");

        assert_eq!(duplicate.0, first.0);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::Swap,
                cdk::federation::FederationOperationKind::Swap,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_sqlite_restart_returns_persisted_signature_shares() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-swap-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-sqlite-restart-input")],
            outputs_for_amount_with_label(&state, 2, "swap-sqlite-restart-output"),
        );

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request.clone()),
        )
        .await
        .expect("first federation swap should return share response")
        .0;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(restarted_state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap after restart should return persisted shares")
        .0;

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_catch_up_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-catch-up-input")],
            outputs_for_amount_with_label(&state, 2, "swap-catch-up-output"),
        );
        let swap_envelope = config
            .swap_operation(request.clone())
            .expect("swap operation envelope");
        let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");
        let mut peer_consensus = test_aleph_bft_peer(config.federation_id(), config.member_id());
        finalize_test_operation(&mut peer_consensus, swap_envelope)
            .expect("peer accepted swap operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = state
            .mint
            .persisted_swap_share_response(swap_operation_id, &request)
            .await
            .expect("persisted swap response after catch-up");

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap after catch-up should return persisted shares")
        .0;

        assert_eq!(duplicate, expected);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn concurrent_conflicting_swaps_spend_one_input_once_without_signing_losers() {
        let state = create_federated_test_state().await;
        let request_count = 6;
        let proof = proof_for_amount(&state, 2, "concurrent-conflicting-swap-input");
        let input_y = proof.y().expect("input y");
        let mut requests = Vec::new();
        let mut output_secrets = Vec::new();

        for index in 0..request_count {
            let outputs = outputs_for_amount_with_label(
                &state,
                2,
                &format!("concurrent-conflicting-swap-output-{index}"),
            );
            output_secrets.push(outputs[0].blinded_secret);
            requests.push(cdk::nuts::SwapRequest::new(vec![proof.clone()], outputs));
        }

        let results =
            futures::future::join_all(requests.into_iter().enumerate().map(|(index, request)| {
                let state = state.clone();
                async move {
                    (
                        index,
                        crate::router_handlers::post_swap(
                            AuthHeader::None,
                            State(state),
                            Json(request),
                        )
                        .await,
                    )
                }
            }))
            .await;
        let mut successes = Vec::new();
        let mut errors = Vec::new();
        for (index, result) in results {
            match result {
                Ok(response) => successes.push((index, response.0)),
                Err(error) => errors.push((index, response_error_detail(error).await)),
            }
        }

        assert_eq!(
            successes.len(),
            1,
            "exactly one conflicting swap should spend the shared input"
        );
        assert_eq!(
            errors.len(),
            request_count - 1,
            "all other conflicting swaps should be rejected"
        );
        for (_, body) in &errors {
            let body = body.to_lowercase();
            assert!(
                body.contains("spent") || body.contains("conflict"),
                "conflicting swap rejection should mention spent input or conflict: {body}"
            );
        }

        let winner = &successes[0];
        assert_eq!(winner.1.signatures.len(), 1);
        let winner_signature = state
            .mint
            .localstore()
            .get_blind_signatures(&[output_secrets[winner.0]])
            .await
            .expect("winner signature lookup");
        assert_eq!(winner_signature, vec![Some(winner.1.signatures[0].clone())]);

        let loser_outputs = output_secrets
            .iter()
            .enumerate()
            .filter_map(|(index, output)| (index != winner.0).then_some(*output))
            .collect::<Vec<_>>();
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&loser_outputs)
                .await
                .expect("loser signature lookup"),
            vec![None; loser_outputs.len()],
            "conflicting swap outputs must not receive member signature shares"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("proof state lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        let journal_entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(journal_entries.len(), request_count);
        assert!(journal_entries
            .iter()
            .all(|entry| entry.envelope.operation.kind()
                == cdk::federation::FederationOperationKind::Swap));
        assert_eq!(
            journal_entries
                .iter()
                .filter(
                    |entry| entry.status() == cdk::federation::FederationOperationStatus::Applied
                )
                .count(),
            1,
            "exactly one conflicting swap should apply"
        );
        assert_eq!(
            journal_entries
                .iter()
                .filter(|entry| matches!(
                    entry.status(),
                    cdk::federation::FederationOperationStatus::Rejected
                        | cdk::federation::FederationOperationStatus::Failed
                ))
                .count(),
            request_count - 1,
            "losing conflicting swaps should be terminal rejected or failed outcomes"
        );
        assert_eq!(state.federation_response_cache_entry_count(), request_count);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_rejects_non_v3_outputs_before_consensus() {
        let state = create_federated_test_state().await;
        let outputs = vec![non_v3_output_for_state(
            &state,
            Amount::from(2),
            b"axum federated swap non-v3 output",
        )];

        let error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::SwapRequest::new(Vec::new(), outputs)),
        )
        .await
        .expect_err("non-v3 swap output should be rejected");

        let body = response_error_detail(error).await;
        assert!(
            body.contains("Cashu keyset v3"),
            "error response should mention the v3 federation keyset requirement: {body}"
        );
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated swap outputs must fail before consensus"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_restore_accepts_v3_restore_placeholders() {
        let state = create_federated_test_state().await;
        let mut restore_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("restore output");
        restore_output.amount = Amount::ZERO;

        let response = crate::router_handlers::post_restore(
            AuthHeader::None,
            State(state),
            Json(cdk::nuts::RestoreRequest {
                outputs: vec![restore_output],
            }),
        )
        .await
        .expect("restore response")
        .0;

        assert!(response.signatures.is_empty());
        assert!(response.outputs.is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_restore_rejects_non_v3_outputs() {
        let state = create_federated_test_state().await;

        let error = crate::router_handlers::post_restore(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::RestoreRequest {
                outputs: vec![non_v3_change_output_for_state(&state)],
            }),
        )
        .await
        .expect_err("non-v3 restore output should be rejected");

        let body = response_error_detail(error).await;
        assert!(
            body.contains("Cashu keyset v3"),
            "error response should mention the v3 federation keyset requirement: {body}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_records_federation_melt_operation() {
        let state = create_federated_test_state().await;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 2, "no-change")],
            None,
        )
        .await;

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should settle");

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_none());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![proof_for_amount(&state, 2, "aleph-bft-halted-melt")],
            None,
        );

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("AlephBFT public melt should fail closed before real finality exists");

        assert_aleph_bft_error_not_ready(&error);
        assert_no_aleph_bft_public_submit_work(&state, 1).await;
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_pending_aleph_bft_retry_returns_persisted_response_after_finality() {
        let mut state = create_aleph_bft_federated_test_state().await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(10),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let input = proof_for_amount(&state, 4, "aleph-public-melt-pending-input");
        let input_y = input.y().expect("melt input y");
        let mut change_output =
            outputs_for_amount_with_label(&state, 2, "aleph-public-melt-pending-change")
                .into_iter()
                .next()
                .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1_000,
            "".to_string(),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .melt_operation(request.clone())
            .expect("melt operation")
            .operation_id()
            .expect("melt operation id");

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("first AlephBFT melt should time out before finality");
        assert!(matches!(
            error,
            cdk::Error::Federation(
                cdk::federation::FederationError::OperationPendingFinalization {
                    operation_id: found_operation_id,
                    timeout_ms: 10,
                }
            ) if found_operation_id == operation_id
        ));
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "timed-out melt waiters must be released"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(
                consensus.journal().entries().len(),
                1,
                "only the setup quote may be journaled before melt finality"
            );
            assert_eq!(consensus.peer_status().operation_count, 1);
        }
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("pending melt proof lookup");
        assert!(
            proof_states.iter().all(|state| !matches!(
                state,
                Some(cdk::nuts::State::Pending) | Some(cdk::nuts::State::Spent)
            )),
            "pending AlephBFT melt must not reserve or spend inputs before finality"
        );
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("pending melt quote lookup")
            .expect("pending melt quote");
        assert_eq!(
            quote.state,
            MeltQuoteState::Unpaid,
            "pending AlephBFT melt must not mutate quote state before finality"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[change_output.blinded_secret])
                .await
                .expect("pending melt change signature lookup"),
            vec![None],
            "pending AlephBFT melt must not sign change before finality"
        );

        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 2,
            ..crate::FederationPeerSyncConfig::default()
        });
        let retry_state = state.clone();
        let retry_request = request.clone();
        let retry = tokio::spawn(async move {
            process_melt_request(
                PreferHeader {
                    respond_async: false,
                },
                &retry_state,
                "bolt11",
                &retry_request,
            )
            .await
        });

        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "retrying an already queued melt should wait on the pending operation"
        );
        finalize_next_local_aleph_bft_unit(&state, operation_id).await;
        for _ in 0..100 {
            let queued = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await
                .queued_len();
            if queued == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let payment_operation_id = if state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await
            .queued_len()
            == 1
        {
            Some(apply_next_local_aleph_bft_unit(&state).await)
        } else {
            None
        };
        assert!(payment_operation_id.is_none_or(|id| id != operation_id));

        let response = retry
            .await
            .expect("retry task should complete")
            .expect("retry melt should return accepted response");
        if response.state() == MeltQuoteState::Pending {
            assert!(payment_operation_id.is_none());
            return;
        }
        assert_eq!(response.state(), MeltQuoteState::Paid);
        let change = response.change().expect("paid melt change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(2));
        assert_eq!(change[0].keyset_id, change_output.keyset_id);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("accepted melt proof lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        let stored_quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("accepted melt quote lookup")
            .expect("accepted melt quote");
        assert_eq!(stored_quote.state, MeltQuoteState::Paid);
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted melt share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(
            share_records[0].blinded_secret,
            change_output.blinded_secret
        );
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            change[0].c
        );
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_process_melt_aleph_bft_pending_requests_join_one_queue_item() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::generate(12).unwrap();
        let mut state = create_federated_test_state_with_backend_db_seed_and_finality(
            backend,
            db,
            &mnemonic.to_seed_normalized(""),
            false,
        )
        .await;
        state.mark_federation_healthy().await;
        state.federation.manual_finality_test = true;
        state.federation.peer_sync_config = Some(crate::FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(5),
            response_waiter_limit: 4,
            ..crate::FederationPeerSyncConfig::default()
        });
        let input = proof_for_amount(&state, 4, "aleph-public-melt-pending-join-input");
        let input_y = input.y().expect("melt input y");
        let mut change_output =
            outputs_for_amount_with_label(&state, 2, "aleph-public-melt-pending-join-change")
                .into_iter()
                .next()
                .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1_000,
            "".to_string(),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();
        let config = state
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federated mint config");
        let operation_id = config
            .melt_operation(request.clone())
            .expect("melt operation")
            .operation_id()
            .expect("melt operation id");

        let first_state = state.clone();
        let first_request = request.clone();
        let first = tokio::spawn(async move {
            process_melt_request(
                PreferHeader {
                    respond_async: false,
                },
                &first_state,
                "bolt11",
                &first_request,
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(1)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            1,
            "first melt request should wait for AlephBFT finality"
        );

        let second_state = state.clone();
        let second_request = request.clone();
        let second = tokio::spawn(async move {
            process_melt_request(
                PreferHeader {
                    respond_async: false,
                },
                &second_state,
                "bolt11",
                &second_request,
            )
            .await
        });
        state
            .wait_for_federation_pending_response_waiter_count(2)
            .await;
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            2,
            "duplicate pending melt request should join the existing waiter"
        );
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 1);
            assert_eq!(
                consensus.journal().entries().len(),
                1,
                "pending duplicate melt must not append beyond the setup quote"
            );
            assert_eq!(consensus.peer_status().operation_count, 1);
        }
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("pending duplicate melt proof lookup");
        assert!(
            proof_states.iter().all(|state| !matches!(
                state,
                Some(cdk::nuts::State::Pending) | Some(cdk::nuts::State::Spent)
            )),
            "duplicate pending melt requests must not reserve or spend inputs before finality"
        );
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("pending duplicate melt quote lookup")
            .expect("pending duplicate melt quote");
        assert_eq!(
            quote.state,
            MeltQuoteState::Unpaid,
            "duplicate pending melt requests must not mutate quote state before finality"
        );
        assert!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("pending duplicate melt share lookup")
                .is_empty(),
            "duplicate pending melt requests must not sign change before finality"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_blind_signatures(&[change_output.blinded_secret])
                .await
                .expect("pending duplicate melt change signature lookup"),
            vec![None],
            "duplicate pending melt requests must not persist change signatures before finality"
        );
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            0,
            "duplicate pending melt requests must not attempt payment before finality"
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        assert_eq!(stats.duplicate_pending_waiter_joins, 1);

        finalize_next_local_aleph_bft_unit(&state, operation_id).await;
        for _ in 0..100 {
            let queued = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await
                .queued_len();
            if queued == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let payment_operation_id = if state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await
            .queued_len()
            == 1
        {
            Some(apply_next_local_aleph_bft_unit(&state).await)
        } else {
            None
        };
        assert!(payment_operation_id.is_none_or(|id| id != operation_id));

        let first_response = first
            .await
            .expect("first melt task should complete")
            .expect("first pending melt should return accepted response");
        let second_response = second
            .await
            .expect("second melt task should complete")
            .expect("second pending melt should return accepted response");
        assert_eq!(second_response, first_response);
        if first_response.state() == MeltQuoteState::Pending {
            assert!(payment_operation_id.is_none());
            return;
        }
        assert_eq!(first_response.state(), MeltQuoteState::Paid);
        let change = first_response.change().expect("paid melt change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(2));
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "joined duplicate melt waiters must result in one outgoing payment attempt"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("accepted duplicate melt proof lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        let stored_quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("accepted duplicate melt quote lookup")
            .expect("accepted duplicate melt quote");
        assert_eq!(stored_quote.state, MeltQuoteState::Paid);
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("accepted duplicate melt share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].operation_id, operation_id);
        assert_eq!(
            share_records[0].blinded_secret,
            change_output.blinded_secret
        );
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            change[0].c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(state.federation_response_cache_entry_count(), 3);
        assert_eq!(state.federation_pending_response_waiter_count(), 0);
        {
            let consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("federation consensus")
                .lock()
                .await;
            assert_eq!(consensus.queued_len(), 0);
            assert_eq!(consensus.peer_status().operation_count, 3);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn value_routes_fail_closed_while_catching_up_and_resume_when_healthy() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let peer = config.member_id();
        let not_ready = "catching up";

        let quote_id = create_federation_observed_paid_bolt11_quote(&state).await;
        let mint_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount_with_label(&state, 2, "catching-up-mint-output"),
            signatures: None,
        };
        let swap_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "catching-up-swap-input")],
            outputs_for_amount_with_label(&state, 2, "catching-up-swap-output"),
        );
        let melt_request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 2, "catching-up-melt-input")],
            None,
        )
        .await;
        let melt_quote_id = melt_request.quote().clone();

        state.mark_federation_catching_up(peer).await;

        let mint_error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request.clone()),
        )
        .await
        .expect_err("mint should fail while catching up");
        assert_http_response_not_ready_contains(mint_error, not_ready).await;

        let swap_error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(swap_request.clone()),
        )
        .await
        .expect_err("swap should fail while catching up");
        assert_http_response_not_ready_contains(swap_error, not_ready).await;

        let melt_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &melt_request,
        )
        .await
        .expect_err("melt should fail while catching up");
        assert_catching_up_error(&melt_error);
        assert_no_federation_public_submit_work(&state, 3).await;

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);

        state.mark_federation_healthy().await;

        let mint_response = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request),
        )
        .await
        .expect("mint should resume after catch-up")
        .0;
        assert_eq!(mint_response.signatures.len(), 1);

        let swap_response = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(swap_request),
        )
        .await
        .expect("swap should resume after catch-up")
        .0;
        assert_eq!(swap_response.signatures.len(), 1);

        let melt_response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &melt_request,
        )
        .await
        .expect("melt should resume after catch-up");
        assert!(matches!(
            melt_response.state(),
            MeltQuoteState::Pending | MeltQuoteState::Paid
        ));

        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Swap,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert!(kinds.contains(&cdk::federation::FederationOperationKind::MeltQuote));
        assert!(kinds.contains(&cdk::federation::FederationOperationKind::Melt));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_returns_federated_change_shares() {
        let state = create_federated_test_state().await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let blinded_secret = change_output
            .blinded_secret
            .as_bls_g1()
            .expect("change output should be BLS");
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "with-change")],
            Some(vec![change_output.clone()]),
        )
        .await;

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should settle with change");
        if response.state() == MeltQuoteState::Pending {
            assert!(response.change().is_none());
            return;
        }
        let change = response.change().expect("federated change share");
        let signature = change.first().expect("one change share");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share = BlsBlindSignatureShare::new(
            config.member_id().to_bls_signer_id(),
            signature.c.as_bls_g1().expect("share should be BLS G1"),
        );

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert_eq!(change.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, change_output.keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn successful_federation_melt_quote_check_records_terminal_payment_observation() {
        let state = create_federated_test_state().await;
        let input = proof_for_amount(&state, 4, "happy-melt-observation");
        let input_y = input.y().expect("input y");
        let mut change_output =
            outputs_for_amount_with_label(&state, 2, "happy-melt-observation-change")
                .into_iter()
                .next()
                .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should settle with change");

        if response.state() == MeltQuoteState::Pending {
            assert_eq!(
                state
                    .mint
                    .localstore()
                    .get_proofs_states(&[input_y])
                    .await
                    .expect("pending proof state lookup"),
                vec![Some(cdk::nuts::State::Pending)]
            );
            return;
        }
        assert_eq!(response.state(), MeltQuoteState::Paid);
        let response_change = response.change().expect("paid melt change");
        assert_eq!(response_change.len(), 1);
        assert_eq!(response_change[0].amount, Amount::from(2));
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("proof state lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup")
                .is_none(),
            "paid happy-path melt should clear saga state"
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("change signature lookup")[0]
            .is_some());

        let check_response = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await;
        assert!(
            check_response.is_ok(),
            "terminal melt quote check should submit a payment observation"
        );

        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let observations = state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("melt payment observations");
        assert_eq!(observations.len(), 1);
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        assert!(matches!(
            &observations[0].observation,
            cdk::federation::FederationPaymentObservation::MeltQuote(observation)
                if observation.quote == quote_id.to_string()
                    && observation.observer == local_member
                    && observation.state == MeltQuoteState::Paid
                    && observation.amount == Amount::from(1)
                    && observation.fee_reserve == Amount::from(1)
                    && observation.payment_proof.is_some()
        ));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_returns_persisted_response() {
        let state = create_federated_test_state().await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "duplicate-melt-with-change")],
            Some(vec![change_output]),
        )
        .await;

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("duplicate federated fake-wallet melt should return persisted response");

        assert_eq!(duplicate, first);
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("federation journal entries");
        assert!(
            entries.iter().all(|entry| {
                entry.status() == cdk::federation::FederationOperationStatus::Applied
            }),
            "all quote, melt, and observation operations should apply: {:?}",
            entries
                .iter()
                .map(|entry| (
                    entry.envelope.operation.kind(),
                    entry.status(),
                    entry.rejection().cloned(),
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(duplicate.state(), MeltQuoteState::Paid);
        assert!(duplicate.change().is_some());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_cache_eviction_returns_persisted_response() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let mut first_change_output =
            outputs_for_amount_with_label(&state, 2, "melt-cache-eviction-first")
                .into_iter()
                .next()
                .expect("first change output");
        first_change_output.amount = Amount::ZERO;
        let first_request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "melt-cache-eviction-first")],
            Some(vec![first_change_output]),
        )
        .await;

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &first_request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let mut second_change_output =
            outputs_for_amount_with_label(&state, 2, "melt-cache-eviction-second")
                .into_iter()
                .next()
                .expect("second change output");
        second_change_output.amount = Amount::ZERO;
        let second_request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "melt-cache-eviction-second")],
            Some(vec![second_change_output]),
        )
        .await;
        let second = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &second_request,
        )
        .await
        .expect("second federated fake-wallet melt should settle");
        assert_ne!(second, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &first_request,
        )
        .await
        .expect("duplicate federated fake-wallet melt should return persisted response");

        assert_eq!(duplicate, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_sqlite_restart_returns_persisted_response() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let mut change_output = outputs_for_amount_with_label(&state, 2, "melt-sqlite-restart")
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "melt-sqlite-restart")],
            Some(vec![change_output]),
        )
        .await;

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        let kinds = federation_operation_kinds(&restarted_state).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &restarted_state,
            "bolt11",
            &request,
        )
        .await
        .expect(
            "duplicate federated fake-wallet melt after restart should return persisted response",
        );

        assert_eq!(duplicate, first);
        match duplicate.state() {
            MeltQuoteState::Paid => assert!(duplicate.change().is_some()),
            MeltQuoteState::Pending => assert!(duplicate.change().is_none()),
            state => panic!("unexpected persisted melt state: {state}"),
        }
        let kinds = federation_operation_kinds(&restarted_state).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_catch_up_returns_persisted_response() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let mut change_output = outputs_for_amount_with_label(&state, 2, "melt-catch-up")
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![proof_for_amount(&state, 4, "melt-catch-up")],
            Some(vec![change_output]),
        )
        .await;
        let melt_envelope = config
            .melt_operation(request.clone())
            .expect("melt operation envelope");
        let mut peer_consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await
            .clone();
        finalize_test_operation(&mut peer_consensus, melt_envelope)
            .expect("peer accepted melt operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = federation_melt_response_from_store(&state, request.quote())
            .await
            .expect("persisted melt response after catch-up");
        assert_eq!(expected.state(), MeltQuoteState::Pending);

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect(
            "duplicate federated fake-wallet melt after catch-up should return persisted response",
        );

        assert!(matches!(
            duplicate.state(),
            MeltQuoteState::Pending | MeltQuoteState::Paid
        ));
        assert_eq!(
            duplicate,
            federation_melt_response_from_store(&state, request.quote())
                .await
                .expect("persisted melt response after observer finalization")
        );
        let kinds = federation_operation_kinds(&state).await;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(kinds[1], cdk::federation::FederationOperationKind::Melt);
        assert!(matches!(kinds.len(), 2 | 3));
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn concurrent_federated_wallet_mint_fanout_aggregates_once_per_member() {
        let member_urls = static_member_urls();
        let bundle = threshold_all_signers_bundle(&member_urls);
        let mint_quote_id = QuoteId::new();
        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            mints.push(mint);
        }
        add_observed_paid_bolt11_quote_to_members(&mints, &mint_quote_id, 3).await;
        let states =
            futures::future::join_all(mints.into_iter().map(|mint| threshold_member_state(mint)))
                .await;

        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            direct_route_member_connectors(&states),
        )
        .expect("federated direct-route connector");
        let connector = Arc::new(connector);

        let keyset = &bundle.public_config.keysets[0];
        let mint_outputs = vec![
            blinded_output(keyset, 1, "concurrent-direct-mint-output-1"),
            blinded_output(keyset, 2, "concurrent-direct-mint-output-2"),
        ];
        let mint_request = BatchMintRequest {
            quotes: vec![mint_quote_id.to_string()],
            quote_amounts: None,
            outputs: mint_outputs.clone(),
            signatures: None,
        };
        let request_count = 4;
        let mint_results = futures::future::join_all((0..request_count).map(|_| {
            let connector = connector.clone();
            let request = mint_request.clone();
            async move {
                connector
                    .post_batch_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
                    .await
            }
        }))
        .await;

        let mut mint_responses = Vec::new();
        for result in mint_results {
            let response = result.expect("concurrent threshold direct-route batch mint");
            assert_aggregate_signatures_verify(
                &bundle.public_config,
                &mint_outputs,
                &response.signatures,
            );
            mint_responses.push(response);
        }

        for response in mint_responses.iter().skip(1) {
            assert_eq!(response, &mint_responses[0]);
        }

        for state in &states {
            assert_eq!(
                federation_operation_kinds(state).await,
                vec![
                    cdk::federation::FederationOperationKind::MintQuote,
                    cdk::federation::FederationOperationKind::MintQuotePayment,
                    cdk::federation::FederationOperationKind::MintQuotePayment,
                    cdk::federation::FederationOperationKind::MintQuotePayment,
                    cdk::federation::FederationOperationKind::Mint,
                ]
            );
            assert_eq!(state.federation_response_cache_entry_count(), 1);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn concurrent_federated_wallet_swap_fanout_aggregates_once_per_member() {
        let member_urls = static_member_urls();
        let bundle = threshold_all_signers_bundle(&member_urls);
        let mut states = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            states.push(threshold_member_state(mint).await);
        }

        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            direct_route_member_connectors(&states),
        )
        .expect("federated direct-route connector");
        let connector = Arc::new(connector);

        let keyset = &bundle.public_config.keysets[0];
        let swap_input = proof_for_keyset(keyset, 2, "concurrent-direct-swap-input");
        let input_y = swap_input.y().expect("swap proof y");
        let swap_outputs = vec![blinded_output(keyset, 2, "concurrent-direct-swap-output")];
        let swap_request = SwapRequest::new(vec![swap_input], swap_outputs.clone());
        let request_count = 4;
        let swap_results = futures::future::join_all((0..request_count).map(|_| {
            let connector = connector.clone();
            let request = swap_request.clone();
            async move { connector.post_swap(request).await }
        }))
        .await;

        let mut swap_responses = Vec::new();
        for result in swap_results {
            let response = result.expect("concurrent threshold direct-route swap");
            assert_aggregate_signatures_verify(
                &bundle.public_config,
                &swap_outputs,
                &response.signatures,
            );
            swap_responses.push(response);
        }

        for response in swap_responses.iter().skip(1) {
            assert_eq!(response, &swap_responses[0]);
        }

        for state in &states {
            assert_eq!(
                federation_operation_kinds(state).await,
                vec![cdk::federation::FederationOperationKind::Swap]
            );
            assert_eq!(state.federation_response_cache_entry_count(), 1);
            let proof_states = state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("proof state lookup");
            assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn concurrent_federated_wallet_melt_fanout_aggregates_once_per_member() {
        let member_urls = static_member_urls();
        let bundle = threshold_all_signers_bundle_with_payment_policy(
            &member_urls,
            FederationPaymentObservationPolicy::member_quorum(3),
        );
        let (payment_processor, make_payment_calls) = CountingFakeWallet::new();
        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint = build_threshold_member_mint_with_backend(
                bundle.public_config.clone(),
                private_config,
                payment_processor.clone(),
            )
            .await;
            mints.push(mint);
        }

        let melt_invoice = create_fake_invoice(1000, "concurrent direct melt".to_string());
        let melt_quote_id = create_federated_melt_quote_for_members(
            &mints,
            cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        )
        .await;

        let mut states = Vec::new();
        for mint in mints {
            states.push(threshold_member_state(mint).await);
        }
        let melt_coordinator = DirectRouteMeltCoordinator::new(&states);
        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            direct_route_member_connectors_with_melt_coordinator(&states, melt_coordinator),
        )
        .expect("federated direct-route connector");
        let connector = Arc::new(connector);

        let keyset = &bundle.public_config.keysets[0];
        let melt_input = proof_for_keyset(keyset, 4, "concurrent-direct-melt-input");
        let input_y = melt_input.y().expect("melt proof y");
        let melt_change_outputs = vec![blinded_change_output(
            keyset,
            "concurrent-direct-melt-change",
        )];
        let melt_request = MeltRequest::new(
            melt_quote_id.to_string(),
            vec![melt_input],
            Some(melt_change_outputs.clone()),
        );
        let request_count = 4;
        let melt_results = futures::future::join_all((0..request_count).map(|_| {
            let connector = connector.clone();
            let request = melt_request.clone();
            async move {
                connector
                    .post_melt(&PaymentMethod::Known(KnownMethod::Bolt11), request)
                    .await
            }
        }))
        .await;

        let mut melt_responses = Vec::new();
        for result in melt_results {
            let response = result.expect("concurrent threshold direct-route melt");
            assert_eq!(response.state(), MeltQuoteState::Paid);
            let change = response.change().expect("melt change");
            assert_aggregate_signatures_verify(&bundle.public_config, &melt_change_outputs, change);
            melt_responses.push(response);
        }

        for response in melt_responses.iter().skip(1) {
            assert_eq!(response, &melt_responses[0]);
        }
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "only the operation author should attempt the outgoing fake-wallet payment"
        );

        for state in &states {
            let operation_kinds = federation_operation_kinds(state).await;
            assert_eq!(
                operation_kinds,
                vec![
                    cdk::federation::FederationOperationKind::MeltQuote,
                    cdk::federation::FederationOperationKind::Melt,
                    cdk::federation::FederationOperationKind::MeltQuotePayment,
                    cdk::federation::FederationOperationKind::MeltQuotePayment,
                    cdk::federation::FederationOperationKind::MeltQuotePayment,
                ]
            );
            let proof_states = state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("proof state lookup");
            assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_wallet_connector_aggregates_real_axum_member_routes() {
        let (listeners, member_urls) = local_member_listeners().await;
        let bundle = threshold_bundle(&member_urls);
        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            mints.push(mint);
        }

        let mint_quote_id = QuoteId::new();
        add_observed_paid_bolt11_quote_to_members(&mints, &mint_quote_id, 3).await;

        let melt_invoice = create_fake_invoice(1000, "axum threshold http melt".to_string());
        let melt_quote_id = create_federated_melt_quote_for_members(
            &mints,
            cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        )
        .await;

        let handles = spawn_member_routers(&mints, listeners).await;

        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            threshold_http_member_connectors(&bundle.public_config),
        )
        .expect("federated HTTP connector");
        wait_for_federated_http_members(&connector).await;

        let keyset = &bundle.public_config.keysets[0];
        let mint_outputs = vec![
            blinded_output(keyset, 1, "axum-http-mint-output-1"),
            blinded_output(keyset, 2, "axum-http-mint-output-2"),
        ];
        let mint_response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec![mint_quote_id.to_string()],
                    quote_amounts: None,
                    outputs: mint_outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("threshold HTTP batch mint");
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &mint_outputs,
            &mint_response.signatures,
        );

        let mint_status = connector
            .get_mint_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &mint_quote_id.to_string(),
            )
            .await
            .expect("mint quote status");
        assert_eq!(mint_status.state(), Some(MintQuoteState::Issued));

        let swap_input = proof_for_keyset(keyset, 2, "swap-input");
        let swap_y = swap_input.y().expect("swap proof y");
        let swap_outputs = vec![blinded_output(keyset, 2, "axum-http-swap-output")];
        let swap_response = connector
            .post_swap(SwapRequest::new(vec![swap_input], swap_outputs.clone()))
            .await
            .expect("threshold HTTP swap");
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &swap_outputs,
            &swap_response.signatures,
        );

        let spent_after_swap = connector
            .post_check_state(CheckStateRequest { ys: vec![swap_y] })
            .await
            .expect("swap check-state");
        assert_eq!(spent_after_swap.states[0].state, cdk::nuts::State::Spent);

        let melt_input = proof_for_keyset(keyset, 4, "melt-input");
        let melt_y = melt_input.y().expect("melt proof y");
        let melt_change_outputs = vec![blinded_change_output(keyset, "axum-http-melt-change")];
        let melt_result = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![melt_input],
                    Some(melt_change_outputs.clone()),
                ),
            )
            .await
            .expect("threshold HTTP melt");
        assert_eq!(melt_result.state(), MeltQuoteState::Paid);
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &melt_change_outputs,
            melt_result.change().expect("immediate melt change"),
        );

        let melt_response = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &melt_quote_id.to_string(),
            )
            .await
            .expect("threshold HTTP settled melt status");
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        let change = melt_response.change().expect("melt change");
        assert_aggregate_signatures_verify(&bundle.public_config, &melt_change_outputs, change);

        let melt_status = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &melt_quote_id.to_string(),
            )
            .await
            .expect("melt quote status");
        assert_eq!(melt_status.state(), MeltQuoteState::Paid);
        let spent_after_melt = connector
            .post_check_state(CheckStateRequest { ys: vec![melt_y] })
            .await
            .expect("melt check-state");
        assert_eq!(spent_after_melt.states[0].state, cdk::nuts::State::Spent);

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: mint_outputs.clone(),
            })
            .await
            .expect("threshold HTTP restore");
        assert_eq!(restore_response.outputs, mint_outputs);
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &restore_response.outputs,
            &restore_response.signatures,
        );

        for handle in handles {
            handle.abort();
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_wallet_connector_aggregates_rotated_real_axum_member_routes() {
        let (listeners, member_urls) = local_member_listeners().await;
        let bundle = threshold_rotation_bundle(&member_urls);
        let rotated_public_config = rotated_public_config(&bundle);
        let old_keyset = active_keyset(&bundle);
        let new_keyset = activated_rotation_keyset(&bundle);
        let new_mint_quote_id = QuoteId::new();
        let old_restore_quote_id = QuoteId::new();
        let old_restore_output =
            blinded_output(&old_keyset, 2, "axum-http-rotated-old-restore-output");

        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            mints.push(mint);
        }
        add_observed_paid_bolt11_quote_to_members(&mints, &new_mint_quote_id, 3).await;
        add_observed_paid_bolt11_quote_to_members(&mints, &old_restore_quote_id, 2).await;

        let config = mints
            .first()
            .expect("at least one federation member")
            .federation_config()
            .expect("federation config");
        let mut journal = InMemoryFederationJournal::new();
        let old_mint_entry = appended_entry(
            &mut journal,
            config
                .mint_operation(cdk::mint::MintInput::Single(MintRequest {
                    quote: old_restore_quote_id.clone(),
                    outputs: vec![old_restore_output.clone()],
                    signature: None,
                }))
                .expect("old-keyset mint operation"),
        );
        for mint in &mints {
            persist_and_apply_entry(&mint, &old_mint_entry).await;
        }
        let rotation_entry = appended_entry(
            &mut journal,
            config.operation_envelope(FederationOperation::KeysetRotation(
                FederationKeysetRotationOperation {
                    keyset: new_keyset.clone(),
                },
            )),
        );
        for mint in &mints {
            persist_and_apply_entry(&mint, &rotation_entry).await;
        }

        let melt_invoice = create_fake_invoice(1000, "axum rotated http melt".to_string());
        let melt_quote_id = create_federated_melt_quote_for_members(
            &mints,
            cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        )
        .await;

        let handles = spawn_member_routers(&mints, listeners).await;
        let connector = FederatedMintConnector::new(
            rotated_public_config.clone(),
            threshold_http_member_connectors(&rotated_public_config),
        )
        .expect("rotated federated HTTP connector");
        wait_for_federated_http_members(&connector).await;

        let active_keysets = connector.get_mint_keys().await.expect("active keys");
        assert_eq!(active_keysets.len(), 1);
        assert_eq!(active_keysets[0].id, new_keyset.id);

        let keysets = connector.get_mint_keysets().await.expect("keysets");
        let old_keyset_info = keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset info");
        let new_keyset_info = keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == new_keyset.id)
            .expect("new keyset info");
        assert_eq!(old_keyset_info.active, false);
        assert_eq!(new_keyset_info.active, true);

        let mint_outputs = vec![
            blinded_output(&new_keyset, 1, "axum-http-rotated-mint-output-1"),
            blinded_output(&new_keyset, 2, "axum-http-rotated-mint-output-2"),
        ];
        let mint_response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec![new_mint_quote_id.to_string()],
                    quote_amounts: None,
                    outputs: mint_outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("rotated threshold HTTP batch mint");
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &mint_outputs,
            &mint_response.signatures,
        );

        let swap_input = proof_for_keyset(&old_keyset, 2, "rotated-swap-input");
        let swap_y = swap_input.y().expect("swap proof y");
        let swap_outputs = vec![blinded_output(
            &new_keyset,
            2,
            "axum-http-rotated-swap-output",
        )];
        let swap_response = connector
            .post_swap(SwapRequest::new(vec![swap_input], swap_outputs.clone()))
            .await
            .expect("rotated threshold HTTP swap");
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &swap_outputs,
            &swap_response.signatures,
        );
        let spent_after_swap = connector
            .post_check_state(CheckStateRequest { ys: vec![swap_y] })
            .await
            .expect("rotated swap check-state");
        assert_eq!(spent_after_swap.states[0].state, cdk::nuts::State::Spent);

        let melt_input = proof_for_keyset(&old_keyset, 4, "rotated-melt-input");
        let melt_y = melt_input.y().expect("melt proof y");
        let melt_change_outputs = vec![blinded_change_output(
            &new_keyset,
            "axum-http-rotated-melt-change",
        )];
        let melt_result = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![melt_input],
                    Some(melt_change_outputs.clone()),
                ),
            )
            .await
            .expect("rotated threshold HTTP melt");
        assert_eq!(melt_result.state(), MeltQuoteState::Paid);
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &melt_change_outputs,
            melt_result.change().expect("immediate rotated melt change"),
        );

        let melt_response = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &melt_quote_id.to_string(),
            )
            .await
            .expect("rotated threshold HTTP settled melt status");
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        let change = melt_response.change().expect("rotated melt change");
        assert_aggregate_signatures_verify(&rotated_public_config, &melt_change_outputs, change);

        let spent_after_melt = connector
            .post_check_state(CheckStateRequest { ys: vec![melt_y] })
            .await
            .expect("rotated melt check-state");
        assert_eq!(spent_after_melt.states[0].state, cdk::nuts::State::Spent);

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: mint_outputs.clone(),
            })
            .await
            .expect("rotated threshold HTTP restore");
        assert_eq!(restore_response.outputs, mint_outputs);
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &restore_response.outputs,
            &restore_response.signatures,
        );

        let old_restore_response = connector
            .post_restore(RestoreRequest {
                outputs: vec![old_restore_output.clone()],
            })
            .await
            .expect("rotated old inactive restore");
        assert_eq!(old_restore_response.outputs, vec![old_restore_output]);
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &old_restore_response.outputs,
            &old_restore_response.signatures,
        );

        for handle in handles {
            handle.abort();
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_pending_melt_request_persists_federation_operation_id() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![proof_for_amount(&state, 4, "pending-with-change")],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();
        let expected_operation_id = state
            .mint
            .federation_config()
            .expect("federation config")
            .melt_operation(request.clone())
            .expect("melt operation")
            .operation_id()
            .expect("operation id")
            .to_string();

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should remain pending");
        let saga = state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga query")
            .expect("pending saga");

        assert_eq!(response.state(), MeltQuoteState::Pending);
        assert!(response.change().is_none());
        assert_eq!(
            saga.federation_operation_id.as_deref(),
            Some(expected_operation_id.as_str())
        );

        let still_pending = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending recovery check");

        assert_eq!(still_pending.state(), MeltQuoteState::Pending);
        assert!(still_pending.change().is_none());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_pending_federation_melt_returns_pending_without_second_payment() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "duplicate-pending-melt");
        let input_y = input.y().expect("input y");
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            None,
        )
        .await;

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("initial pending federated melt should return pending");

        assert_eq!(first.state(), MeltQuoteState::Pending);
        let initial_payment_calls = make_payment_calls.load(Ordering::SeqCst);
        assert!(initial_payment_calls <= 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("pending proof state lookup"),
            vec![Some(cdk::nuts::State::Pending)]
        );

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("duplicate pending federated melt should return persisted pending state");

        assert_eq!(duplicate.state(), MeltQuoteState::Pending);
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            initial_payment_calls,
            "duplicate pending melt must not attempt a second outgoing payment"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("duplicate pending proof state lookup"),
            vec![Some(cdk::nuts::State::Pending)]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_pending_federation_melt_after_sqlite_restart_avoids_second_payment() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-pending-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state = create_federated_test_state_with_backend_db_and_seed(backend, db, &seed).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "pending-melt-sqlite-restart");
        let input_y = input.y().expect("input y");
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            None,
        )
        .await;
        let quote_id = request.quote().clone();

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("first pending federated fake-wallet melt should return pending");

        assert_eq!(first.state(), MeltQuoteState::Pending);
        assert!(make_payment_calls.load(Ordering::SeqCst) <= 1);
        drop(state);

        let (restarted_backend, restarted_make_payment_calls) = CountingFakeWallet::new();
        let restarted_state =
            restart_federated_test_state_from_db_with_backend(&db_file, &seed, restarted_backend)
                .await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &restarted_state,
            "bolt11",
            &request,
        )
        .await
        .expect("duplicate pending melt after restart should return persisted pending state");

        assert_eq!(duplicate.state(), MeltQuoteState::Pending);
        assert_eq!(
            restarted_make_payment_calls.load(Ordering::SeqCst),
            0,
            "duplicate pending melt after restart must not attempt payment"
        );
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
        assert_eq!(
            restarted_state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("restarted pending proof state lookup"),
            vec![Some(cdk::nuts::State::Pending)]
        );
        let saga = restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("restarted saga lookup")
            .expect("pending saga after restart");
        assert!(matches!(
            saga.state.state(),
            "setup_complete" | "payment_attempted"
        ));
        assert!(saga.federation_operation_id.is_some());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_quote_check_later_finalizes_with_change() {
        let backend = Arc::new(DelayedStatusFakeWallet::new(2, Some(MeltQuoteState::Paid)));
        let state = create_federated_test_state_with_backend(backend).await;
        let input = proof_for_amount(&state, 4, "pending-quote-check-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![input],
            Some(vec![change_output]),
        )
        .await;
        let quote_id = request.quote().clone();

        let pending = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should initially remain pending");
        assert_eq!(pending.state(), MeltQuoteState::Pending);
        assert!(pending.change().is_none());

        let settled = check_melt_quote_for_route(&state, &quote_id)
            .await
            .expect("later observer poll should reconcile pending melt");
        assert_eq!(settled.state(), MeltQuoteState::Paid);
        let change = settled.change().expect("settled melt change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(2));

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_public_quote_check_observes_only_terminal_settlement() {
        let backend = Arc::new(DelayedStatusFakeWallet::new(3, Some(MeltQuoteState::Paid)));
        let state = create_federated_test_state_with_backend(backend).await;
        let input = proof_for_amount(&state, 4, "pending-public-route-paid");
        let input_y = input.y().expect("input y");
        let mut change_output =
            outputs_for_amount_with_label(&state, 2, "pending-public-route-change")
                .into_iter()
                .next()
                .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            "".to_string(),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();

        let pending = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should initially remain pending");
        assert_eq!(pending.state(), MeltQuoteState::Pending);
        assert!(pending.change().is_none());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("pending proof state lookup"),
            vec![Some(cdk::nuts::State::Pending)]
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("pending change signature lookup")[0]
            .is_none());

        let still_pending = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await
        .expect("pending public quote check should succeed");
        let still_pending = bolt11_melt_quote_response_from_response(still_pending).await;
        assert_eq!(still_pending.state, MeltQuoteState::Pending);
        assert!(still_pending.change.is_none());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("pending melt payment observations")
            .is_empty());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("still-pending proof state lookup"),
            vec![Some(cdk::nuts::State::Pending)]
        );

        let settled = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await
        .expect("terminal public quote check should succeed");
        let settled = bolt11_melt_quote_response_from_response(settled).await;
        assert_eq!(settled.state, MeltQuoteState::Paid);
        let change = settled.change.expect("settled public change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(2));

        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("settled proof state lookup"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("settled saga lookup")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("settled change signature lookup")[0]
            .is_some());

        let observations = state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("settled melt payment observations");
        assert_eq!(observations.len(), 1);
        assert!(matches!(
            &observations[0].observation,
            cdk::federation::FederationPaymentObservation::MeltQuote(observation)
                if observation.state == MeltQuoteState::Paid
                    && observation.quote == quote_id.to_string()
                    && observation.payment_proof.is_some()
        ));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_verified_paid_finalizes_with_change() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 4, "pending-later-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("pending fake-wallet melt verified as paid should finalize");
        assert!(matches!(
            response.state(),
            MeltQuoteState::Pending | MeltQuoteState::Paid
        ));

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, response.state());

        if response.state() == MeltQuoteState::Pending {
            assert_eq!(
                state
                    .mint
                    .localstore()
                    .get_proofs_states(&[input_y])
                    .await
                    .expect("pending proof state lookup"),
                vec![Some(cdk::nuts::State::Pending)]
            );
            assert!(state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("pending saga lookup")
                .is_some());
            return;
        }

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let change = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("change signature lookup");
        let change_signature = change[0].clone().expect("change share persisted");
        assert_eq!(change_signature.amount, Amount::from(2));
        assert_eq!(change_signature.keyset_id, change_output.keyset_id);
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_check_error_keeps_recovery_state() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: true,
        };
        let input = proof_for_amount(&state, 2, "pending-check-error");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            Some(vec![change_output]),
        )
        .await;
        let quote_id = request.quote().clone();

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("ambiguous fake-wallet melt with failed check should stay unresolved");

        assert_eq!(response.state(), MeltQuoteState::Pending);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Pending);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Pending)]);

        let saga = state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .expect("unresolved saga");
        assert!(matches!(
            saga.state.state(),
            "setup_complete" | "payment_attempted"
        ));
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures_for_quote(&quote_id)
            .await
            .expect("change signature lookup")
            .is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_failed_melt_request_releases_inputs_without_change_shares() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "failed-melt");
        let input_y = input.y().expect("input y");
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            None,
        )
        .await;
        let quote_id = request.quote().clone();

        let result = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(response) if response.state() == MeltQuoteState::Pending => {
                assert_eq!(make_payment_calls.load(Ordering::SeqCst), 0);
                return;
            }
            Ok(response) => panic!(
                "failed melt returned unexpected state: {}",
                response.state()
            ),
        };

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "failed federated melt should release its reserved input proof: {proof_states:?}"
        );

        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures_for_quote(&quote_id)
            .await
            .expect("change signature lookup")
            .is_empty());

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed federated fake-wallet melt should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "duplicate failed melt must not attempt a second outgoing payment"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("duplicate quote lookup")
            .expect("duplicate quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("duplicate proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "duplicate failed federated melt should leave its input released: {proof_states:?}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn failed_federation_melt_quote_check_records_terminal_failed_observation() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "failed-melt-observation");
        let input_y = input.y().expect("input y");
        let mut change_output =
            outputs_for_amount_with_label(&state, 2, "failed-melt-observation-change")
                .into_iter()
                .next()
                .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            Some(vec![change_output.clone()]),
        )
        .await;
        let quote_id = request.quote().clone();

        let result = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(response) if response.state() == MeltQuoteState::Pending => {
                assert_eq!(make_payment_calls.load(Ordering::SeqCst), 0);
                return;
            }
            Ok(response) => panic!(
                "failed melt returned unexpected state: {}",
                response.state()
            ),
        };

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[input_y])
                .await
                .expect("proof state lookup"),
            vec![None]
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("change signature lookup")[0]
            .is_none());

        let check_response = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await;
        assert!(
            check_response.is_ok(),
            "terminal failed melt quote check should submit a payment observation"
        );

        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let observations = state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("melt payment observations");
        assert_eq!(observations.len(), 1);
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let cdk::federation::FederationPaymentObservation::MeltQuote(observation) =
            &observations[0].observation
        else {
            panic!("expected a melt payment observation");
        };
        assert_eq!(observation.quote, quote_id.to_string());
        assert_eq!(observation.observer, local_member);
        assert_eq!(observation.state, MeltQuoteState::Failed);
        assert_eq!(observation.amount, Amount::from(1));
        assert_eq!(observation.fee_reserve, Amount::from(1));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-failed-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state = create_federated_test_state_with_backend_db_and_seed(backend, db, &seed).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "failed-melt-sqlite-restart");
        let input_y = input.y().expect("input y");
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            None,
        )
        .await;
        let quote_id = request.quote().clone();

        let result = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(response) if response.state() == MeltQuoteState::Pending => {
                assert_eq!(make_payment_calls.load(Ordering::SeqCst), 0);
                return;
            }
            Ok(response) => panic!(
                "failed melt returned unexpected state: {}",
                response.state()
            ),
        };

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);
        drop(state);

        let (restarted_backend, restarted_make_payment_calls) = CountingFakeWallet::new();
        let restarted_state =
            restart_federated_test_state_from_db_with_backend(&db_file, &seed, restarted_backend)
                .await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &restarted_state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed melt after restart should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            restarted_make_payment_calls.load(Ordering::SeqCst),
            0,
            "duplicate failed melt after restart must not attempt payment"
        );
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        let quote = restarted_state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("duplicate quote lookup")
            .expect("duplicate quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        let proof_states = restarted_state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("duplicate proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "duplicate failed melt after restart should leave its input released: {proof_states:?}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let config = state.mint.federation_config().expect("federation config");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let input = proof_for_amount(&state, 2, "failed-melt-catch-up");
        let input_y = input.y().expect("input y");
        let request = create_federated_melt_request(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
            vec![input],
            None,
        )
        .await;
        let quote_id = request.quote().clone();
        let melt_envelope = config
            .melt_operation(request.clone())
            .expect("melt operation envelope");
        let mut peer_consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await
            .clone();
        finalize_test_operation(&mut peer_consensus, melt_envelope)
            .expect("peer accepted failed melt operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 0);

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("catch-up quote lookup")
            .expect("catch-up quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Pending);
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("catch-up proof state lookup");
        assert!(
            matches!(proof_states.as_slice(), [Some(cdk::nuts::State::Pending)]),
            "melt catch-up should reserve its input before observation: {proof_states:?}"
        );

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed melt after catch-up should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "the observer should make the first payment attempt after catch-up"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MeltQuote,
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::MeltQuotePayment,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_rejects_non_v3_change_before_consensus() {
        let state = create_federated_test_state().await;
        let request = cdk::nuts::MeltRequest::new(
            QuoteId::new(),
            Vec::new(),
            Some(vec![non_v3_change_output_for_state(&state)]),
        );

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("non-v3 aggregate keysets must be rejected");

        assert!(matches!(
            error,
            cdk::Error::Federation(
                cdk::federation::FederationError::InvalidFederatedKeysetVersion { .. }
            )
        ));
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated change outputs must fail before consensus"
        );
    }

    #[tokio::test]
    async fn cache_post_mint_custom_rejects_cached_url_method_quote_method_mismatch() {
        let state = create_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let mint_request = MintRequest {
            quote: quote_id,
            outputs: outputs_for_amount(&state, 2),
            signature: None,
        };

        let result = cache_post_mint_custom(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request.clone()),
        )
        .await;
        if let Err(response) = result {
            panic!(
                "bolt11 mint should populate the cache: {}",
                response_error_detail(response).await
            );
        }

        let result = cache_post_mint_custom(
            AuthHeader::None,
            State(state),
            Path("bolt12".to_string()),
            Json(mint_request),
        )
        .await;

        assert!(
            result.is_err(),
            "cache_post_mint_custom must reject cross-method cached mint"
        );
    }

    #[tokio::test]
    async fn cache_post_batch_mint_rejects_cached_url_method_quote_method_mismatch() {
        let state = create_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        let result = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request.clone()),
        )
        .await;
        if let Err(response) = result {
            panic!(
                "bolt11 batch mint should populate the cache: {}",
                response_error_detail(response).await
            );
        }

        let result = cache_post_batch_mint(
            AuthHeader::None,
            State(state),
            Path("bolt12".to_string()),
            Json(batch_request),
        )
        .await;

        assert!(
            result.is_err(),
            "cache_post_batch_mint must reject cross-method cached mint"
        );
    }

    #[tokio::test]
    async fn post_batch_check_mint_quote_rejects_url_method_quote_method_mismatch() {
        let state = create_test_state_with_custom_methods(&["paypal", "venmo"]).await;
        let quote_id = create_custom_quote(&state, "venmo", 2).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state),
            Path("paypal".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            result.is_err(),
            "post_batch_check_mint_quote must reject cross-method quote checks"
        );
    }

    #[tokio::test]
    async fn post_melt_custom_quote_rejects_url_method_request_body_mismatch() {
        let state = create_test_state_with_custom_methods(&["paypal", "venmo"]).await;

        let payload = serde_json::to_value(MeltQuoteCustomRequest {
            method: "venmo".to_string(),
            request: "test-payment-request".to_string(),
            unit: CurrencyUnit::Sat,
            amount: None,
            extra: Value::Null,
        })
        .unwrap();

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("paypal".to_string()),
            Json(payload),
        )
        .await;

        assert!(
            result.is_err(),
            "post_melt_custom_quote must reject cross-method quote creation"
        );

        let venmo_quote_created = state
            .mint
            .melt_quotes()
            .await
            .unwrap()
            .into_iter()
            .any(|quote| quote.payment_method == PaymentMethod::Custom("venmo".to_string()));

        assert!(
            !venmo_quote_created,
            "request to the paypal melt-quote endpoint must not create a venmo-method quote"
        );
    }

    #[tokio::test]
    async fn post_melt_custom_quote_accepts_normalized_method_match() {
        let state = create_test_state_with_custom_methods(&["paypal"]).await;

        let payload = serde_json::to_value(MeltQuoteCustomRequest {
            method: "PayPal".to_string(),
            request: "test-payment-request".to_string(),
            unit: CurrencyUnit::Sat,
            amount: None,
            extra: Value::Null,
        })
        .unwrap();

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("paypal".to_string()),
            Json(payload),
        )
        .await;

        assert!(
            result.is_ok(),
            "post_melt_custom_quote must accept path/body methods that normalize to the same value"
        );

        let paypal_quote_created = state
            .mint
            .melt_quotes()
            .await
            .unwrap()
            .into_iter()
            .any(|quote| quote.payment_method == PaymentMethod::Custom("paypal".to_string()));

        assert!(
            paypal_quote_created,
            "mixed-case body method should create a quote under the normalized payment method"
        );
    }
}
