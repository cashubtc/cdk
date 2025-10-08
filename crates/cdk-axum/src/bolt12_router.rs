use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
use cdk::mint::QuoteId;
use cdk::nuts::nut17::Kind;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteBolt11Response, MeltQuoteBolt12Request, MeltQuoteState, MeltRequest,
    MintQuoteBolt12Request, MintQuoteBolt12Response, MintRequest, MintResponse,
    NotificationPayload,
};
use cdk::subscription::{Params, SubId};
use paste::paste;
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::{into_response, post_cache_wrapper, MintState};

post_cache_wrapper!(post_mint_bolt12, MintRequest<QuoteId>, MintResponse);

/// Cache wrapper for post_melt_bolt12
pub async fn cache_post_melt_bolt12(
    #[cfg(feature = "auth")] auth: AuthHeader,
    headers: HeaderMap,
    state: State<MintState>,
    payload: Json<MeltRequest<QuoteId>>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    use std::ops::Deref;
    let json_extracted_payload = payload.deref();
    let State(mint_state) = state.clone();
    let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
        Some(key) => key,
        None => {
            #[cfg(feature = "auth")]
            return post_melt_bolt12(auth, headers, state, payload).await;
            #[cfg(not(feature = "auth"))]
            return post_melt_bolt12(headers, state, payload).await;
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
    let response = post_melt_bolt12(auth, headers, state, payload).await?;
    #[cfg(not(feature = "auth"))]
    let response = post_melt_bolt12(headers, state, payload).await?;
    mint_state.cache.set(cache_key, &response.deref()).await;
    Ok(response)
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
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
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
        .map(|prefer_value| prefer_value.to_lowercase().contains("respond-async"))
        .unwrap_or(false);

    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    if !is_async_preferred && res.state == MeltQuoteState::Pending {
        let quote_id = payload.quote().to_string();
        let subscription_params = Params {
            kind: Kind::Bolt11MeltQuote,
            filters: vec![quote_id.clone()],
            id: Arc::new(SubId::from(quote_id)),
        };

        match state.mint.pubsub_manager().subscribe(subscription_params) {
            Ok(mut receiver) => {
                while let Some(event) = receiver.recv().await {
                    if let NotificationPayload::MeltQuoteBolt11Response(updated_res) =
                        event.into_inner()
                    {
                        if updated_res.state != MeltQuoteState::Pending {
                            return Ok(Json(updated_res));
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!("Failed to subscribe to melt quote: {}", err);
            }
        }
    }

    Ok(Json(res))
}
