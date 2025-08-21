use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::response::Response;
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
use cdk::mint::QuoteId;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteBolt11Response, MeltQuoteBolt12Request, MeltRequest, MintQuoteBolt12Request,
    MintQuoteBolt12Response, MintRequest, MintResponse,
};
use paste::paste;
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::{into_response, post_cache_wrapper, MintState};

post_cache_wrapper!(post_mint_bolt12, MintRequest<QuoteId>, MintResponse);
post_cache_wrapper!(
    post_melt_bolt12,
    MeltRequest<QuoteId>,
    MeltQuoteBolt11Response<QuoteId>
);

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

    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res))
}
