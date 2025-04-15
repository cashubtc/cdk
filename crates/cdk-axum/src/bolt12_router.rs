use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::response::Response;
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
use cdk::nuts::{
    MeltBolt12Request, MeltQuoteBolt11Response, MeltQuoteBolt12Request, MintBolt11Request,
    MintBolt11Response, MintQuoteBolt12Request, MintQuoteBolt12Response,
};
use tracing::instrument;
use uuid::Uuid;

use crate::{into_response, MintState};

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
pub async fn get_mint_bolt12_quote(
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt12Request>,
) -> Result<Json<MintQuoteBolt12Response<Uuid>>, Response> {
    let quote = state
        .mint
        .get_mint_bolt12_quote(payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
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
    State(state): State<MintState>,
    Path(quote_id): Path<Uuid>,
) -> Result<Json<MintQuoteBolt12Response<Uuid>>, Response> {
    let quote = state
        .mint
        .check_mint_bolt12_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/mint/bolt12",
    request_body(content = MintBolt11Request<String>, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintBolt11Response, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Request a quote for melting tokens
#[instrument(skip_all, fields(quote_id = ?payload.quote))]
pub async fn post_mint_bolt12(
    State(state): State<MintState>,
    Json(payload): Json<MintBolt11Request<Uuid>>,
) -> Result<Json<MintBolt11Response>, Response> {
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
pub async fn get_melt_bolt12_quote(
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt12Request>,
) -> Result<Json<MeltQuoteBolt11Response<Uuid>>, Response> {
    let quote = state
        .mint
        .get_melt_bolt12_quote(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/melt/bolt12",
    request_body(content = MeltBolt12Request<String>, description = "Melt params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Melt tokens for a Bitcoin payment that the mint will make for the user in exchange
///
/// Requests tokens to be destroyed and sent out via Lightning.
pub async fn post_melt_bolt12(
    State(state): State<MintState>,
    Json(payload): Json<MeltBolt12Request<Uuid>>,
) -> Result<Json<MeltQuoteBolt11Response<Uuid>>, Response> {
    let res = state
        .mint
        .melt_bolt11(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(res))
}
