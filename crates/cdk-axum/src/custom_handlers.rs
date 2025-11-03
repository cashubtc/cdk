//! Generic handlers for custom payment methods
//!
//! These handlers work for ANY custom payment method without requiring
//! method-specific validation or request parsing.

use axum::extract::{Json, Path, State};
use axum::response::Response;
use cdk::mint::QuoteId;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteBolt11Response, MeltQuoteCustomRequest, MintQuoteCustomRequest,
    MintQuoteCustomResponse, MintRequest, MintResponse,
};
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::router_handlers::into_response;
use crate::MintState;

/// Generic handler for custom payment method mint quotes
///
/// This handler works for ANY custom payment method (e.g., paypal, venmo, cashapp).
/// It passes the request data directly to the payment processor without validation.
#[instrument(skip_all, fields(method = ?method, amount = ?payload.amount))]
pub async fn post_mint_custom_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<MintQuoteCustomRequest>,
) -> Result<Json<MintQuoteCustomResponse<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Custom(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    let quote_request = cdk::mint::MintQuoteRequest::Custom {
        method,
        request: payload,
    };

    let response = state
        .mint
        .get_mint_quote(quote_request)
        .await
        .map_err(into_response)?;

    match response {
        cdk::mint::MintQuoteResponse::Custom { response, .. } => Ok(Json(response)),
        _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
    }
}

/// Get custom payment method mint quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_mint_custom_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Json<MintQuoteCustomResponse<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::Custom(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    let quote_response = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(into_response)?;

    // Extract and verify it's a Custom payment method
    match quote_response {
        cdk::mint::MintQuoteResponse::Custom {
            method: quote_method,
            response,
        } => {
            if quote_method != method {
                return Err(into_response(cdk::Error::InvalidPaymentMethod));
            }
            Ok(Json(response))
        }
        _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
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
                &ProtectedEndpoint::new(Method::Post, RoutePath::Custom(method.clone())),
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
    Json(payload): Json<MeltQuoteCustomRequest>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Custom(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .get_melt_quote(payload.into())
        .await
        .map_err(into_response)?;

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
                &ProtectedEndpoint::new(Method::Get, RoutePath::Custom(method.clone())),
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
                &ProtectedEndpoint::new(Method::Post, RoutePath::Custom(method.clone())),
            )
            .await
            .map_err(into_response)?;
    }

    // Note: melt() will validate the quote internally
    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res))
}
