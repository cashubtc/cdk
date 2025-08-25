use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::response::Response;
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    MeltQuoteOnchainRequest, MeltQuoteOnchainResponse, MeltRequest, MintQuoteOnchainRequest,
    MintQuoteOnchainResponse, MintRequest, MintResponse,
};
use paste::paste;
use tracing::instrument;
use uuid::Uuid;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::{into_response, post_cache_wrapper, MintState};

post_cache_wrapper!(post_mint_onchain, MintRequest<Uuid>, MintResponse);
post_cache_wrapper!(
    post_melt_onchain,
    MeltRequest<Uuid>,
    MeltQuoteOnchainResponse<Uuid>
);

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/mint/quote/onchain",
    request_body(content = MintQuoteOnchainRequest, description = "Quote params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintQuoteOnchainResponse<String>, content_type = "application/json")
    )
))]
/// Create mint onchain quote
#[instrument(skip_all)]
pub async fn post_mint_onchain_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteOnchainRequest>,
) -> Result<Json<MintQuoteOnchainResponse<Uuid>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteOnchain),
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
    path = "/mint/quote/onchain/{quote_id}",
    params(
        ("quote_id" = String, description = "The quote ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = MintQuoteOnchainResponse<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get mint onchain quote
#[instrument(skip_all, fields(quote_id = ?quote_id))]
pub async fn get_check_mint_onchain_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(quote_id): Path<Uuid>,
) -> Result<Json<MintQuoteOnchainResponse<Uuid>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteOnchain),
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
    path = "/mint/onchain",
    request_body(content = MintRequest<String>, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Request a quote for melting tokens
#[instrument(skip_all, fields(quote_id = ?payload.quote))]
pub async fn post_mint_onchain(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintRequest<Uuid>>,
) -> Result<Json<MintResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintOnchain),
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
    path = "/melt/quote/onchain",
    request_body(content = MeltQuoteOnchainRequest, description = "Quote params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteOnchainResponse<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_melt_onchain_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteOnchainRequest>,
) -> Result<Json<MeltQuoteOnchainResponse<Uuid>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteOnchain),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .get_melt_quote(payload.into())
        .await
        .map_err(into_response)?;

    Ok(Json(quote.try_into().map_err(into_response)?))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/melt/onchain",
    request_body(content = MeltRequest<String>, description = "Melt params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteOnchainResponse<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Melt tokens for a Bitcoin payment that the mint will make for the user in exchange
///
/// Requests tokens to be destroyed and sent out via Lightning.
pub async fn post_melt_onchain(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MeltRequest<Uuid>>,
) -> Result<Json<MeltQuoteOnchainResponse<Uuid>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltOnchain),
            )
            .await
            .map_err(into_response)?;
    }

    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res.try_into().map_err(into_response)?))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/melt/quote/onchain/{quote_id}",
    params(
        ("quote_id" = String, description = "The quote ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteOnchainResponse<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get melt onchain quote
#[instrument(skip_all, fields(quote_id = ?quote_id))]
pub async fn get_check_melt_onchain_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(quote_id): Path<Uuid>,
) -> Result<Json<MeltQuoteOnchainResponse<Uuid>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteOnchain),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote.try_into().map_err(into_response)?))
}
