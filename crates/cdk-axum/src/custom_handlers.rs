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

/// Header extractor for the Prefer header
///
/// This extractor checks for the `Prefer: respond-async` header
/// to determine if the client wants asynchronous processing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferHeader {
    pub respond_async: bool,
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
    let Some((operation_id, config)) =
        submit_federation_mint_operation(state, input.clone()).await?
    else {
        return Ok(None);
    };

    Ok(Some(config.mint_share_response(operation_id, input)?))
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_operation(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let envelope = config.mint_quote_operation(
        response.quote().clone(),
        request,
        mint_quote_response_to_string(response),
    );
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_payment_observation(
    state: &MintState,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let (amount_paid, amount_issued) = mint_quote_response_amounts(response);
    let envelope = config.mint_quote_payment_observation(
        response.quote().clone(),
        response.method(),
        response
            .state()
            .unwrap_or(cdk::nuts::MintQuoteState::Unpaid),
        amount_paid,
        amount_issued,
        None,
    );
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_operation(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
    response: &MeltQuoteCreateResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let quote = response.quote().cloned().ok_or(cdk::Error::UnknownQuote)?;
    let envelope = config.melt_quote_operation(quote, request);
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_payment_observation(
    state: &MintState,
    response: &MeltQuoteResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let envelope = config.melt_quote_payment_observation(
        response.quote().clone(),
        response.method(),
        response.state(),
        response.amount(),
        response.fee_reserve(),
        response.payment_proof().map(str::to_string),
        None,
    );
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_operation(
    state: &MintState,
    input: cdk::mint::MintInput,
) -> Result<
    Option<(
        cdk::federation::FederationOperationId,
        cdk::mint::FederationRuntimeConfig,
    )>,
    cdk::Error,
> {
    let Some(config) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let envelope = config.mint_operation(input)?;
    let response = state.submit_federation_envelope(envelope).await?;
    Ok(Some((response.operation_id, config)))
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

#[cfg(feature = "federation")]
fn mint_quote_response_amounts(
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> (cdk::Amount, cdk::Amount) {
    match response {
        cdk::mint::MintQuoteResponse::Bolt11(response) => {
            bolt11_mint_quote_amounts(response.state, response.amount)
        }
        cdk::mint::MintQuoteResponse::Bolt12(response) => {
            (response.amount_paid, response.amount_issued)
        }
        cdk::mint::MintQuoteResponse::Onchain(response) => {
            (response.amount_paid, response.amount_issued)
        }
        cdk::mint::MintQuoteResponse::Custom { response, .. } => {
            (response.amount_paid, response.amount_issued)
        }
    }
}

#[cfg(feature = "federation")]
fn bolt11_mint_quote_amounts(
    state: cdk::nuts::MintQuoteState,
    amount: Option<cdk::Amount>,
) -> (cdk::Amount, cdk::Amount) {
    let amount = amount.unwrap_or(cdk::Amount::ZERO);
    match state {
        cdk::nuts::MintQuoteState::Unpaid => (cdk::Amount::ZERO, cdk::Amount::ZERO),
        cdk::nuts::MintQuoteState::Paid => (amount, cdk::Amount::ZERO),
        cdk::nuts::MintQuoteState::Issued => (amount, amount),
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

    match method.as_str() {
        "bolt11" => {
            let bolt11_request: MintQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = bolt11_request.into();
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
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
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
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
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
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

            let response = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &response)
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

    let quote_response = state
        .mint
        .check_mint_quotes(&[quote_id])
        .await
        .map_err(into_response)?
        .first()
        .cloned()
        .ok_or(cdk::Error::UnknownQuote)
        .map_err(into_response)?;
    #[cfg(feature = "federation")]
    submit_federation_mint_quote_payment_observation(&state, &quote_response)
        .await
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

    let responses = state
        .mint
        .check_mint_quotes(&payload.quotes)
        .await
        .map_err(into_response)?;
    #[cfg(feature = "federation")]
    for response in &responses {
        submit_federation_mint_quote_payment_observation(&state, response)
            .await
            .map_err(into_response)?;
    }

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

    let (quote_request, response) = match method.as_str() {
        "bolt11" => {
            let bolt11_request: MeltQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt11(bolt11_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
        "bolt12" => {
            let bolt12_request: MeltQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt12(bolt12_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
        "onchain" => {
            let onchain_request: MeltQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Onchain(onchain_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
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
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
    };

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_operation(&state, quote_request, &response)
        .await
        .map_err(into_response)?;
    #[cfg(not(feature = "federation"))]
    let _ = quote_request;

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

    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_payment_observation(&state, &quote)
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
    if let Some(response) = process_federation_melt_request(state, payload).await? {
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
    state.submit_federation_envelope(envelope).await?;

    Ok(Some(
        federation_melt_response_from_store(state, payload.quote()).await?,
    ))
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
    use std::collections::{HashMap, HashSet};
    #[cfg(feature = "federation")]
    use std::pin::Pin;
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
    use cdk::federation::FederationConsensusEngine;
    use cdk::mint::{MintBuilder, MintMeltLimits, MintQuoteResponse};
    use cdk::nuts::nut00::KnownMethod;
    #[cfg(feature = "federation")]
    use cdk::nuts::nut01::{verify_blind_signature_share, BlsBlindSignatureShare};
    #[cfg(feature = "federation")]
    use cdk::nuts::MeltQuoteState;
    #[cfg(feature = "federation")]
    use cdk::nuts::SecretKey;
    use cdk::nuts::{BlindedMessage, CurrencyUnit, MintQuoteState, PaymentMethod};
    use cdk::types::{FeeReserve, QuoteTTL};
    use cdk::Amount;
    use cdk_fake_wallet::FakeWallet;
    #[cfg(feature = "federation")]
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
    #[cfg(feature = "federation")]
    use futures::Stream;

    use super::*;
    use crate::cache::HttpCache;

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
            federation_consensus: None,
            #[cfg(feature = "federation")]
            federation_sequences: None,
            #[cfg(feature = "federation")]
            federation_sync: None,
            #[cfg(feature = "federation")]
            federation_notifications: None,
            #[cfg(feature = "federation")]
            federation_finalizer: None,
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
    fn test_federation_runtime_config() -> cdk::mint::FederationRuntimeConfig {
        let member = cdk::federation::TrustedDealerMember::new(
            cdk::federation::MemberId::new(1).expect("member id"),
            "https://mint1.example.com"
                .parse::<cdk::mint_url::MintUrl>()
                .expect("public mint url"),
            "https://fed1.example.com"
                .parse::<cdk::mint_url::MintUrl>()
                .expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        );
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
        let bundle = cdk::federation::generate_trusted_dealer_configs(
            cdk::federation::ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member],
            vec![keyset],
        )
        .expect("trusted dealer bundle");
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
        let runtime_config = test_federation_runtime_config();
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
            federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(
                cdk::federation::FederationConsensusRuntime::in_memory(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                ),
            ))),
            federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                cdk::federation::FederationRestSequenceTracker::new(),
            ))),
            federation_sync: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationSyncState::default(),
            ))),
            federation_notifications: Some(Arc::new(
                crate::FederationOperationNotifications::default(),
            )),
            federation_finalizer: None,
        }
    }

    #[cfg(feature = "federation")]
    async fn federation_operation_kinds(
        state: &MintState,
    ) -> Vec<cdk::federation::FederationOperationKind> {
        let consensus = state
            .federation_consensus
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
            federation_consensus: None,
            #[cfg(feature = "federation")]
            federation_sequences: None,
            #[cfg(feature = "federation")]
            federation_sync: None,
            #[cfg(feature = "federation")]
            federation_notifications: None,
            #[cfg(feature = "federation")]
            federation_finalizer: None,
        }
    }

    async fn create_custom_quote(state: &MintState, method: &str, amount: u64) -> QuoteId {
        let quote = state
            .mint
            .get_mint_quote(cdk::mint::MintQuoteRequest::Custom {
                method: PaymentMethod::Custom(method.to_string()),
                request: MintQuoteCustomRequest {
                    amount: Amount::from(amount),
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

    fn outputs_for_amount(state: &MintState, amount: u64) -> Vec<BlindedMessage> {
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

        let blinded_secret =
            cdk::nuts::PublicKey::from(cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(
                format!("axum federated mint output {amount}").as_bytes(),
            ));

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
    async fn create_federated_melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
        create_federated_melt_quote_with_description(state, amount_msat, "".to_string()).await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        let invoice = create_fake_invoice(amount_msat, description);
        let response = state
            .mint
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote");

        match response {
            MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        }
    }

    async fn response_error_detail(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
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
    async fn post_mint_custom_quote_records_federation_quote_operation() {
        let state = create_federated_test_state().await;
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
            vec![cdk::federation::FederationOperationKind::MintQuote]
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
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;

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
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(result.is_ok(), "melt quote check should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuotePayment]
        );
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
        let quote_id = create_paid_bolt11_quote(&state).await;
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
            vec![cdk::federation::FederationOperationKind::Mint]
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
    async fn cache_post_batch_mint_returns_federation_signature_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
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
        .await
        .expect("federation batch mint should return share response");
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
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
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
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 2, "no-change")],
            None,
        );

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
            vec![cdk::federation::FederationOperationKind::Melt]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_returns_federated_change_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let blinded_secret = change_output
            .blinded_secret
            .as_bls_g1()
            .expect("change output should be BLS");
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 4, "with-change")],
            Some(vec![change_output.clone()]),
        );

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
            vec![cdk::federation::FederationOperationKind::Melt]
        );
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
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![proof_for_amount(&state, 4, "pending-with-change")],
            Some(vec![change_output.clone()]),
        );
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
    async fn pending_federated_melt_quote_check_later_finalizes_with_change() {
        let backend = Arc::new(DelayedStatusFakeWallet::new(2, Some(MeltQuoteState::Paid)));
        let state = create_federated_test_state_with_backend(backend).await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let input = proof_for_amount(&state, 4, "pending-quote-check-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request =
            cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], Some(vec![change_output]));

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

        let settled = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("later quote check should reconcile pending melt");
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
    async fn pending_federated_melt_verified_paid_finalizes_with_change() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 4, "pending-later-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![input],
            Some(vec![change_output.clone()]),
        );

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
        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_some());

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);

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
            vec![cdk::federation::FederationOperationKind::Melt]
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
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "pending-check-error");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request =
            cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], Some(vec![change_output]));

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("ambiguous fake-wallet melt with failed check should stay unresolved");

        assert!(matches!(error, cdk::Error::Internal));
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
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
        assert_eq!(saga.state.state(), "payment_attempted");
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
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "failed-melt");
        let input_y = input.y().expect("input y");
        let request = cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], None);

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("federated fake-wallet melt should fail and compensate");

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
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
