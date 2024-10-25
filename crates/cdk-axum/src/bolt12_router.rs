use anyhow::Result;
use axum::extract::{Json, State};
use axum::response::Response;
use cdk::nuts::nut19::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use cdk::nuts::{
    MeltBolt12Request, MeltQuoteBolt11Response, MeltQuoteBolt12Request, MintBolt11Request,
    MintBolt11Response,
};

use crate::{into_response, MintState};

/// Get mint bolt12 quote
pub async fn get_mint_bolt12_quote(
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt12Request>,
) -> Result<Json<MintQuoteBolt12Response>, Response> {
    let quote = state
        .mint
        .get_mint_bolt12_quote(payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

/// Request a quote for melting tokens
pub async fn post_mint_bolt12(
    State(state): State<MintState>,
    Json(payload): Json<MintBolt11Request>,
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

pub async fn get_melt_bolt12_quote(
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt12Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .get_melt_bolt12_quote(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

pub async fn post_melt_bolt12(
    State(state): State<MintState>,
    Json(payload): Json<MeltBolt12Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res))
}
