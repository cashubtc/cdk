//! Generic handlers for custom payment methods
//!
//! These handlers work for ANY custom payment method without requiring
//! method-specific validation or request parsing.
//!
//! Special handling for bolt11 and bolt12:
//! When the method parameter is "bolt11" or "bolt12", these handlers use the
//! specific Bolt11/Bolt12 request/response types instead of the generic custom types.

use axum::extract::{Json, Path, State};
use axum::response::{IntoResponse, Response};
use cdk::mint::QuoteId;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltQuoteBolt12Request,
    MeltQuoteCustomRequest, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteBolt12Request, MintQuoteBolt12Response, MintQuoteCustomRequest, MintRequest,
    MintResponse,
};
use serde_json::Value;
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::router_handlers::into_response;
use crate::MintState;

/// Generic handler for custom payment method mint quotes
///
/// This handler works for ANY custom payment method (e.g., paypal, venmo, cashapp, bolt11, bolt12).
/// For bolt11/bolt12, it handles the specific request/response types.
/// For other methods, it passes the request data directly to the payment processor.
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_mint_custom_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Response, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    match method.as_str() {
        "bolt11" => {
            let bolt11_request: MintQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote = state
                .mint
                .get_mint_quote(bolt11_request.into())
                .await
                .map_err(into_response)?;

            let response: MintQuoteBolt11Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            let bolt12_request: MintQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote = state
                .mint
                .get_mint_quote(bolt12_request.into())
                .await
                .map_err(into_response)?;

            let response: MintQuoteBolt12Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            let custom_request: MintQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::mint::MintQuoteRequest::Custom {
                method,
                request: custom_request,
            };

            let response = state
                .mint
                .get_mint_quote(quote_request)
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
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Response, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuote(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    let quote_response = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(into_response)?;

    match method.as_str() {
        "bolt11" => {
            let response: MintQuoteBolt11Response<QuoteId> =
                quote_response.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            let response: MintQuoteBolt12Response<QuoteId> =
                quote_response.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            // Extract and verify it's a Custom payment method
            match quote_response {
                cdk::mint::MintQuoteResponse::Custom {
                    method: quote_method,
                    response,
                } => {
                    if quote_method != method {
                        return Err(into_response(cdk::Error::InvalidPaymentMethod));
                    }
                    Ok(Json(response).into_response())
                }
                _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
            }
        }
    }
}

/// Mint tokens with custom payment method
#[instrument(skip_all, fields(method = ?method, quote_id = ?payload.quote))]
pub async fn post_mint_custom(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Mint(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    // Note: process_mint_request will validate the quote internally
    // including checking if it's paid and matches the expected payment method
    let res = state
        .mint
        .process_mint_request(payload)
        .await
        .map_err(into_response)?;

    Ok(Json(res))
}

/// Request a melt quote for custom payment method
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_melt_custom_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuote(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    let response = match method.as_str() {
        "bolt11" => {
            let bolt11_request: MeltQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            state
                .mint
                .get_melt_quote(bolt11_request.into())
                .await
                .map_err(into_response)?
        }
        "bolt12" => {
            let bolt12_request: MeltQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            state
                .mint
                .get_melt_quote(bolt12_request.into())
                .await
                .map_err(into_response)?
        }
        _ => {
            let custom_request: MeltQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            state
                .mint
                .get_melt_quote(custom_request.into())
                .await
                .map_err(into_response)?
        }
    };

    Ok(Json(response))
}

/// Get custom payment method melt quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_melt_custom_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuote(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    // Note: check_melt_quote returns the response directly
    // The payment method validation is done when the quote was created
    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

/// Melt tokens with custom payment method
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_melt_custom(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<cdk::nuts::MeltRequest<QuoteId>>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Melt(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    // Note: melt() will validate the quote internally
    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res))
}

// ============================================================================
// CACHED HANDLERS FOR NUT-19 SUPPORT
// ============================================================================

/// Cached version of post_mint_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method, quote_id = ?payload.quote))]
pub async fn cache_post_mint_custom(
    #[cfg(feature = "auth")] auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state.clone();
    let json_extracted_payload = payload.deref();

    let cache_key = match mint_state.cache.calculate_key(json_extracted_payload) {
        Some(key) => key,
        None => {
            // Could not calculate key, just return the handler result
            #[cfg(feature = "auth")]
            return post_mint_custom(auth, state, method, payload).await;
            #[cfg(not(feature = "auth"))]
            return post_mint_custom(state, method, payload).await;
        }
    };

    if let Some(cached_response) = mint_state.cache.get::<MintResponse>(&cache_key).await {
        return Ok(Json(cached_response));
    }

    #[cfg(feature = "auth")]
    let result = post_mint_custom(auth, state, method, payload).await?;
    #[cfg(not(feature = "auth"))]
    let result = post_mint_custom(state, method, payload).await?;

    // Cache the response
    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}

/// Cached version of post_melt_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method))]
pub async fn cache_post_melt_custom(
    #[cfg(feature = "auth")] auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<cdk::nuts::MeltRequest<QuoteId>>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state.clone();
    let json_extracted_payload = payload.deref();

    let cache_key = match mint_state.cache.calculate_key(json_extracted_payload) {
        Some(key) => key,
        None => {
            // Could not calculate key, just return the handler result
            #[cfg(feature = "auth")]
            return post_melt_custom(auth, state, method, payload).await;
            #[cfg(not(feature = "auth"))]
            return post_melt_custom(state, method, payload).await;
        }
    };

    if let Some(cached_response) = mint_state
        .cache
        .get::<MeltQuoteBolt11Response<QuoteId>>(&cache_key)
        .await
    {
        return Ok(Json(cached_response));
    }

    #[cfg(feature = "auth")]
    let result = post_melt_custom(auth, state, method, payload).await?;
    #[cfg(not(feature = "auth"))]
    let result = post_melt_custom(state, method, payload).await?;

    // Cache the response
    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}
