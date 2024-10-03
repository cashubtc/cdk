use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::error::ErrorResponse;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeysResponse, KeysetResponse, MeltBolt11Request,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request, MintBolt11Response,
    MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
use cdk::util::unix_time;

use crate::MintState;

pub async fn get_keys(State(state): State<MintState>) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.pubkeys().await.map_err(|err| {
        tracing::error!("Could not get keys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

pub async fn get_keyset_pubkeys(
    State(state): State<MintState>,
    Path(keyset_id): Path<Id>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.keyset_pubkeys(&keyset_id).await.map_err(|err| {
        tracing::error!("Could not get keyset pubkeys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

pub async fn get_keysets(State(state): State<MintState>) -> Result<Json<KeysetResponse>, Response> {
    let mint = state.mint.keysets().await.map_err(|err| {
        tracing::error!("Could not get keyset: {}", err);
        into_response(err)
    })?;

    Ok(Json(mint))
}

pub async fn get_mint_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt11Request>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .get_mint_bolt11_quote(payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

pub async fn get_check_mint_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not check mint quote {}: {}", quote_id, err);
            into_response(err)
        })?;

    Ok(Json(quote))
}

pub async fn post_mint_bolt11(
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

pub async fn get_melt_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt11Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .get_melt_bolt11_quote(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

pub async fn get_check_melt_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not check melt quote: {}", err);
            into_response(err)
        })?;

    Ok(Json(quote))
}

pub async fn post_melt_bolt11(
    State(state): State<MintState>,
    Json(payload): Json<MeltBolt11Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let res = state
        .mint
        .melt_bolt11(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(res))
}

pub async fn post_check(
    State(state): State<MintState>,
    Json(payload): Json<CheckStateRequest>,
) -> Result<Json<CheckStateResponse>, Response> {
    let state = state.mint.check_state(&payload).await.map_err(|err| {
        tracing::error!("Could not check state of proofs");
        into_response(err)
    })?;

    Ok(Json(state))
}

pub async fn get_mint_info(State(state): State<MintState>) -> Result<Json<MintInfo>, Response> {
    Ok(Json(state.mint.mint_info().clone().time(unix_time())))
}

pub async fn post_swap(
    State(state): State<MintState>,
    Json(payload): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, Response> {
    let swap_response = state
        .mint
        .process_swap_request(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process swap request: {}", err);
            into_response(err)
        })?;
    Ok(Json(swap_response))
}

pub async fn post_restore(
    State(state): State<MintState>,
    Json(payload): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    let restore_response = state.mint.restore(payload).await.map_err(|err| {
        tracing::error!("Could not process restore: {}", err);
        into_response(err)
    })?;

    Ok(Json(restore_response))
}

pub fn into_response<T>(error: T) -> Response
where
    T: Into<ErrorResponse>,
{
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json::<ErrorResponse>(error.into()),
    )
        .into_response()
}
