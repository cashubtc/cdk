use anyhow::Result;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::error::{ErrorCode, ErrorResponse};
use cdk::mint::QuoteId;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeysResponse, KeysetResponse,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintRequest, MintResponse, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
use cdk::util::unix_time;
use paste::paste;
use tracing::instrument;

#[cfg(feature = "auth")]
use crate::auth::AuthHeader;
use crate::ws::main_websocket;
use crate::MintState;

/// Macro to add cache to endpoint
#[macro_export]
macro_rules! post_cache_wrapper {
    ($handler:ident, $request_type:ty, $response_type:ty) => {
        paste! {
            /// Cache wrapper function for $handler:
            /// Wrap $handler into a function that caches responses using the request as key
            pub async fn [<cache_ $handler>](
                #[cfg(feature = "auth")] auth: AuthHeader,
                state: State<MintState>,
                payload: Json<$request_type>
            ) -> Result<Json<$response_type>, Response> {
                use std::ops::Deref;
                let json_extracted_payload = payload.deref();
                let State(mint_state) = state.clone();
                let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
                    Some(key) => key,
                    None => {
                        // Could not calculate key, just return the handler result
                        #[cfg(feature = "auth")]
                        return $handler(auth, state, payload).await;
                        #[cfg(not(feature = "auth"))]
                        return $handler( state, payload).await;
                    }
                };
                if let Some(cached_response) = mint_state.cache.get::<$response_type>(&cache_key).await {
                    return Ok(Json(cached_response));
                }
                #[cfg(feature = "auth")]
                let response = $handler(auth, state, payload).await?;
                #[cfg(not(feature = "auth"))]
                let response = $handler(state, payload).await?;
                mint_state.cache.set(cache_key, &response.deref()).await;
                Ok(response)
            }
        }
    };
}

post_cache_wrapper!(post_swap, SwapRequest, SwapResponse);
post_cache_wrapper!(post_mint_bolt11, MintRequest<QuoteId>, MintResponse);
post_cache_wrapper!(
    post_melt_bolt11,
    MeltRequest<QuoteId>,
    MeltQuoteBolt11Response<QuoteId>
);

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keys",
    responses(
        (status = 200, description = "Successful response", body = KeysResponse, content_type = "application/json")
    )
))]
/// Get the public keys of the newest mint keyset
///
/// This endpoint returns a dictionary of all supported token values of the mint and their associated public key.
#[instrument(skip_all)]
pub(crate) async fn get_keys(
    State(state): State<MintState>,
) -> Result<Json<KeysResponse>, Response> {
    Ok(Json(state.mint.pubkeys()))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keys/{keyset_id}",
    params(
        ("keyset_id" = String, description = "The keyset ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = KeysResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get the public keys of a specific keyset
///
/// Get the public keys of the mint from a specific keyset ID.
#[instrument(skip_all, fields(keyset_id = ?keyset_id))]
pub(crate) async fn get_keyset_pubkeys(
    State(state): State<MintState>,
    Path(keyset_id): Path<Id>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.keyset_pubkeys(&keyset_id).map_err(|err| {
        tracing::error!("Could not get keyset pubkeys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keysets",
    responses(
        (status = 200, description = "Successful response", body = KeysetResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get all active keyset IDs of the mint
///
/// This endpoint returns a list of keysets that the mint currently supports and will accept tokens from.
#[instrument(skip_all)]
pub(crate) async fn get_keysets(
    State(state): State<MintState>,
) -> Result<Json<KeysetResponse>, Response> {
    Ok(Json(state.mint.keysets()))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/mint/quote/bolt11",
    request_body(content = MintQuoteBolt11Request, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Request a quote for minting of new tokens
///
/// Request minting of new tokens. The mint responds with a Lightning invoice. This endpoint can be used for a Lightning invoice UX flow.
#[instrument(skip_all, fields(amount = ?payload.amount))]
pub(crate) async fn post_mint_bolt11_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt11Request>,
) -> Result<Json<MintQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteBolt11),
        )
        .await
        .map_err(into_response)?;

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
    path = "/mint/quote/bolt11/{quote_id}",
    params(
        ("quote_id" = String, description = "The quote ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = MintQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get mint quote by ID
///
/// Get mint quote state.
#[instrument(skip_all, fields(quote_id = ?quote_id))]
pub(crate) async fn get_check_mint_bolt11_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(quote_id): Path<QuoteId>,
) -> Result<Json<MintQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11),
            )
            .await
            .map_err(into_response)?;
    }

    let quote = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not check mint quote {}: {}", quote_id, err);
            into_response(err)
        })?;

    Ok(Json(quote.try_into().map_err(into_response)?))
}

#[instrument(skip_all)]
pub(crate) async fn ws_handler(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::Ws),
            )
            .await
            .map_err(into_response)?;
    }

    Ok(ws.on_upgrade(|ws| main_websocket(ws, state)))
}

/// Mint tokens by paying a BOLT11 Lightning invoice.
///
/// Requests the minting of tokens belonging to a paid payment request.
///
/// Call this endpoint after `POST /v1/mint/quote`.
#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/mint/bolt11",
    request_body(content = MintRequest<String>, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
#[instrument(skip_all, fields(quote_id = ?payload.quote))]
pub(crate) async fn post_mint_bolt11(
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
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt11),
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
    path = "/melt/quote/bolt11",
    request_body(content = MeltQuoteBolt11Request, description = "Quote params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
#[instrument(skip_all)]
/// Request a quote for melting tokens
pub(crate) async fn post_melt_bolt11_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt11Request>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt11),
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
    get,
    context_path = "/v1",
    path = "/melt/quote/bolt11/{quote_id}",
    params(
        ("quote_id" = String, description = "The quote ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get melt quote by ID
///
/// Get melt quote state.
#[instrument(skip_all, fields(quote_id = ?quote_id))]
pub(crate) async fn get_check_melt_bolt11_quote(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(quote_id): Path<QuoteId>,
) -> Result<Json<MeltQuoteBolt11Response<QuoteId>>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteBolt11),
            )
            .await
            .map_err(into_response)?;
    }

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

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/melt/bolt11",
    request_body(content = MeltRequest<String>, description = "Melt params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MeltQuoteBolt11Response<String>, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Melt tokens for a Bitcoin payment that the mint will make for the user in exchange
///
/// Requests tokens to be destroyed and sent out via Lightning.
#[instrument(skip_all)]
pub(crate) async fn post_melt_bolt11(
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
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11),
            )
            .await
            .map_err(into_response)?;
    }

    let res = state.mint.melt(&payload).await.map_err(into_response)?;

    Ok(Json(res))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/checkstate",
    request_body(content = CheckStateRequest, description = "State params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = CheckStateResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Check whether a proof is spent already or is pending in a transaction
///
/// Check whether a secret has been spent already or not.
#[instrument(skip_all, fields(y_count = ?payload.ys.len()))]
pub(crate) async fn post_check(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<CheckStateRequest>,
) -> Result<Json<CheckStateResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate),
            )
            .await
            .map_err(into_response)?;
    }

    let state = state.mint.check_state(&payload).await.map_err(|err| {
        tracing::error!("Could not check state of proofs");
        into_response(err)
    })?;

    Ok(Json(state))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/info",
    responses(
        (status = 200, description = "Successful response", body = MintInfo)
    )
))]
/// Mint information, operator contact information, and other info
#[instrument(skip_all)]
pub(crate) async fn get_mint_info(
    State(state): State<MintState>,
) -> Result<Json<MintInfo>, Response> {
    Ok(Json(
        state
            .mint
            .mint_info()
            .await
            .map_err(|err| {
                tracing::error!("Could not get mint info: {}", err);
                into_response(err)
            })?
            .clone()
            .time(unix_time()),
    ))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/swap",
    request_body(content = SwapRequest, description = "Swap params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = SwapResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Swap inputs for outputs of the same value
///
/// Requests a set of Proofs to be swapped for another set of BlindSignatures.
///
/// This endpoint can be used by Alice to swap a set of proofs before making a payment to Carol. It can then used by Carol to redeem the tokens for new proofs.
#[instrument(skip_all, fields(inputs_count = ?payload.inputs().len()))]
pub(crate) async fn post_swap(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            )
            .await
            .map_err(into_response)?;
    }

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

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/restore",
    request_body(content = RestoreRequest, description = "Restore params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = RestoreResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Restores blind signature for a set of outputs.
#[instrument(skip_all, fields(outputs_count = ?payload.outputs.len()))]
pub(crate) async fn post_restore(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Restore),
            )
            .await
            .map_err(into_response)?;
    }

    let restore_response = state.mint.restore(payload).await.map_err(|err| {
        tracing::error!("Could not process restore: {}", err);
        into_response(err)
    })?;

    Ok(Json(restore_response))
}

#[instrument(skip_all)]
pub(crate) fn into_response<T>(error: T) -> Response
where
    T: Into<ErrorResponse>,
{
    let err_response: ErrorResponse = error.into();
    let status_code = match err_response.code {
        // Client errors (400 Bad Request)
        ErrorCode::TokenAlreadySpent
        | ErrorCode::TokenPending
        | ErrorCode::QuoteNotPaid
        | ErrorCode::QuoteExpired
        | ErrorCode::QuotePending
        | ErrorCode::KeysetNotFound
        | ErrorCode::KeysetInactive
        | ErrorCode::BlindedMessageAlreadySigned
        | ErrorCode::UnsupportedUnit
        | ErrorCode::TokensAlreadyIssued
        | ErrorCode::MintingDisabled
        | ErrorCode::InvoiceAlreadyPaid
        | ErrorCode::TokenNotVerified
        | ErrorCode::TransactionUnbalanced
        | ErrorCode::AmountOutofLimitRange
        | ErrorCode::WitnessMissingOrInvalid
        | ErrorCode::DuplicateSignature
        | ErrorCode::DuplicateInputs
        | ErrorCode::DuplicateOutputs
        | ErrorCode::MultipleUnits
        | ErrorCode::UnitMismatch
        | ErrorCode::ClearAuthRequired
        | ErrorCode::BlindAuthRequired => StatusCode::BAD_REQUEST,

        // Auth failures (401 Unauthorized)
        ErrorCode::ClearAuthFailed | ErrorCode::BlindAuthFailed => StatusCode::UNAUTHORIZED,

        // Lightning/payment errors and unknown errors (500 Internal Server Error)
        ErrorCode::LightningError | ErrorCode::Unknown(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (status_code, Json(err_response)).into_response()
}
