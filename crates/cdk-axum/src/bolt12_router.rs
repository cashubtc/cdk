use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
use cdk::mint::QuoteId;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteBolt11Response, MeltQuoteBolt12Request, MeltRequest, MintQuoteBolt12Request,
    MintQuoteBolt12Response, MintRequest, MintResponse,
};
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::{into_response, MintState};

// Manually define cache_post_mint_bolt12 with status code caching
pub async fn cache_post_mint_bolt12(
    #[cfg(feature = "auth")] auth: AuthHeader,
    state: State<MintState>,
    payload: Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let json_extracted_payload = payload.deref();
    let State(mint_state) = state.clone();

    let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
        Some(key) => key,
        None => {
            // Could not calculate key, just return the handler result
            #[cfg(feature = "auth")]
            return post_mint_bolt12(auth, state, payload).await;
            #[cfg(not(feature = "auth"))]
            return post_mint_bolt12(state, payload).await;
        }
    };

    // Try to get cached response
    if let Some((_status_code, response_data)) = mint_state
        .cache
        .get::<(u16, MintResponse)>(&cache_key)
        .await
    {
        return Ok(Json(response_data));
    }

    // No cached response, execute the handler
    #[cfg(feature = "auth")]
    let response = post_mint_bolt12(auth, state, payload).await?;
    #[cfg(not(feature = "auth"))]
    let response = post_mint_bolt12(state, payload).await?;

    // Cache the response with status code (200 for successful mint)
    mint_state
        .cache
        .set(cache_key, &(200u16, response.deref().clone()))
        .await;

    Ok(response)
}

// Cache wrapper for post_melt_bolt12 that implements full NUT-19 compliant caching
// This properly caches successful responses (status_code == 200) as per NUT-19 spec
pub async fn cache_post_melt_bolt12(
    #[cfg(feature = "auth")] auth: AuthHeader,
    headers: HeaderMap,
    state: State<MintState>,
    payload: Json<MeltRequest<QuoteId>>,
) -> Result<Response, Response> {
    use std::ops::Deref;

    use cdk::nuts::MeltQuoteBolt11Response;

    let json_extracted_payload = payload.deref();
    let State(mint_state) = state.clone();

    let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
        Some(key) => key,
        None => {
            // Could not calculate key, just return the handler result
            #[cfg(feature = "auth")]
            return post_melt_bolt12(auth, headers, state, payload).await;
            #[cfg(not(feature = "auth"))]
            return post_melt_bolt12(headers, state, payload).await;
        }
    };

    // Check if we have a cached response tuple (status_code, data)
    if let Some((cached_status, cached_data)) = mint_state
        .cache
        .get::<(u16, MeltQuoteBolt11Response<QuoteId>)>(&cache_key)
        .await
    {
        let status_code = StatusCode::from_u16(cached_status).unwrap_or(StatusCode::OK);
        return Ok((status_code, Json(cached_data)).into_response());
    }

    // Extract headers before moving them into the handler
    let is_async_preferred = headers
        .get("PREFER")
        .and_then(|value| value.to_str().ok())
        .map(|prefer_value| prefer_value.contains("respond-async"))
        .unwrap_or(false);

    // Execute the mint's melt operation directly to get typed response and status
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let (res, receiver) = state.mint.melt(&payload).await.map_err(into_response)?;

    let (final_response, actual_status_code) = if !is_async_preferred {
        if let Some(rx) = receiver {
            let final_res = rx
                .await
                .map_err(|e| {
                    tracing::error!("Task join error: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(())).into_response()
                })?
                .map_err(into_response)?;
            (final_res, 202u16) // ACCEPTED for synchronous processing
        } else {
            (res, 200u16) // OK for async preferred or immediate responses
        }
    } else {
        (res, 200u16) // OK for async preferred or immediate responses
    };

    // Cache successful responses (status 200) as per NUT-19 spec
    // Only cache final successful results to avoid caching intermediate states
    if actual_status_code == 200 {
        mint_state
            .cache
            .set(cache_key, &(actual_status_code, final_response.clone()))
            .await;
    }

    let status_code = StatusCode::from_u16(actual_status_code).unwrap_or(StatusCode::OK);
    Ok((status_code, Json(final_response)).into_response())
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/mint/quote/bolt12",
    responses(
        (status = 200, description = "Successful response", body = MintQuoteBolt12Response<String>, content_type = "application/json")
    )
))]
/// Get mint bolt12 quote
#[instrument(skip_all, fields(amount = ?payload.amount))]
pub async fn post_mint_bolt12_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt12Request>,
) -> Result<Json<MintQuoteBolt12Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .get_mint_quote(payload.into())
        .await
        .map_err(into_response)?;

    Ok(Json(quote.try_into().map_err(into_response)?))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/mint/quote/bolt12/{quote_id}",
    params(
        ("quote_id" = String, description = "The quote ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = MintQuoteBolt12Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get mint bolt12 quote
#[instrument(skip_all, fields(quote_id = ?quote_id))]
pub async fn get_check_mint_bolt12_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(quote_id): Path<QuoteId>,
) -> Result<Json<MintQuoteBolt12Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote.try_into().map_err(into_response)?))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/mint/bolt12",
    request_body(content = MintRequest<String>, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Request a quote for melting tokens
#[instrument(skip_all, fields(quote_id = ?payload.quote))]
pub async fn post_mint_bolt12(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let res = state
        .mint
        .process_mint_request(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process mint: {}", err);
            into_response(err)
        })?;

    Ok(Json(res))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/melt/quote/bolt12",
    request_body(content = MeltQuoteBolt12Request, description = "Quote params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_melt_bolt12_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    _headers: HeaderMap,
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt12Request>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .get_melt_quote(payload.into())
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/melt/bolt12",
    request_body(content = MeltRequest<String>, description = "Melt params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Melt tokens for a Bitcoin payment that the mint will make for the user in exchange
///
/// Requests tokens to be destroyed and sent out via Lightning.
pub async fn post_melt_bolt12(
    #[cfg(feature = "auth")] auth: AuthHeader,
    headers: HeaderMap,
    State(state): State<MintState>,
    Json(payload): Json<MeltRequest<QuoteId>>,
) -> Result<Response, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt12),
            )
            .await
            .map_err(into_response)?;
    }

    let is_async_preferred = headers
        .get("PREFER")
        .and_then(|value| value.to_str().ok())
        .map(|prefer_value| prefer_value.contains("respond-async"))
        .unwrap_or(false);

    let (res, receiver) = state.mint.melt(&payload).await.map_err(into_response)?;

    if !is_async_preferred {
        if let Some(rx) = receiver {
            let res = rx
                .await
                .map_err(|e| {
                    tracing::error!("Task join error: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(())).into_response()
                })?
                .map_err(into_response)?;

            // Return 202 Accepted for synchronous processing
            Ok((StatusCode::ACCEPTED, Json(res)).into_response())
        } else {
            // Return 200 OK for async preferred or no receiver
            Ok((StatusCode::OK, Json(res)).into_response())
        }
    } else {
        // Return 200 OK for async preferred or no receiver
        Ok((StatusCode::OK, Json(res)).into_response())
    }
}
