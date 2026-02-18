use anyhow::Result;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::error::ErrorResponse;
#[cfg(feature = "auth")]
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeysResponse, KeysetResponse, MintInfo,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
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

/// Macro to add cache to endpoint with prefer header support (for async operations)
#[macro_export]
macro_rules! post_cache_wrapper_with_prefer {
    ($handler:ident, $request_type:ty, $response_type:ty) => {
        paste! {
            /// Cache wrapper function for $handler with PreferHeader support:
            /// Wrap $handler into a function that caches responses using the request as key
            pub async fn [<cache_ $handler>](
                #[cfg(feature = "auth")] auth: AuthHeader,
                prefer: PreferHeader,
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
                        return $handler(auth, prefer, state, payload).await;
                        #[cfg(not(feature = "auth"))]
                        return $handler(prefer, state, payload).await;
                    }
                };
                if let Some(cached_response) = mint_state.cache.get::<$response_type>(&cache_key).await {
                    return Ok(Json(cached_response));
                }
                #[cfg(feature = "auth")]
                let response = $handler(auth, prefer, state, payload).await?;
                #[cfg(not(feature = "auth"))]
                let response = $handler(prefer, state, payload).await?;
                mint_state.cache.set(cache_key, &response.deref()).await;
                Ok(response)
            }
        }
    };
}

post_cache_wrapper!(post_swap, SwapRequest, SwapResponse);

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
    // Per NUT-00 spec: "In case of an error, mints respond with the HTTP status code 400"
    (StatusCode::BAD_REQUEST, Json(err_response)).into_response()
}

// --- NUT-28 Conditional Token Endpoints ---

/// GET /v1/conditions - List all registered conditions
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn get_conditions(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
) -> Result<Json<cdk::nuts::nut28::GetConditionsResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::Conditions),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state.mint.get_conditions().await.map_err(|err| {
        tracing::error!("Could not get conditions: {}", err);
        into_response(err)
    })?;
    Ok(Json(response))
}

/// POST /v1/conditions - Register a new condition
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn post_conditions(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<cdk::nuts::nut28::RegisterConditionRequest>,
) -> Result<Json<cdk::nuts::nut28::RegisterConditionResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::Conditions),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .register_condition(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not register condition: {}", err);
            into_response(err)
        })?;
    Ok(Json(response))
}

/// GET /v1/conditions/{condition_id} - Get a specific condition
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn get_condition(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(condition_id): Path<String>,
) -> Result<Json<cdk::nuts::nut28::ConditionInfo>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::Condition),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .get_condition(&condition_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not get condition: {}", err);
            into_response(err)
        })?;
    Ok(Json(response))
}

/// GET /v1/conditional_keysets - List all conditional keysets
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn get_conditional_keysets(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
) -> Result<Json<cdk::nuts::nut28::ConditionalKeysetsResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Get, RoutePath::ConditionalKeysets),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .get_conditional_keysets()
        .await
        .map_err(|err| {
            tracing::error!("Could not get conditional keysets: {}", err);
            into_response(err)
        })?;
    Ok(Json(response))
}

/// POST /v1/conditions/{condition_id}/partitions - Register a partition for a condition
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn post_register_partition(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Path(condition_id): Path<String>,
    Json(payload): Json<cdk::nuts::nut28::RegisterPartitionRequest>,
) -> Result<Json<cdk::nuts::nut28::RegisterPartitionResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::ConditionPartitions),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .register_partition(&condition_id, payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not register partition: {}", err);
            into_response(err)
        })?;
    Ok(Json(response))
}

/// POST /v1/redeem_outcome - Redeem conditional tokens
#[cfg(feature = "conditional-tokens")]
#[instrument(skip_all)]
pub(crate) async fn post_redeem_outcome(
    #[cfg(feature = "auth")] auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<cdk::nuts::nut28::RedeemOutcomeRequest>,
) -> Result<Json<cdk::nuts::nut28::RedeemOutcomeResponse>, Response> {
    #[cfg(feature = "auth")]
    {
        state
            .mint
            .verify_auth(
                auth.into(),
                &ProtectedEndpoint::new(Method::Post, RoutePath::RedeemOutcome),
            )
            .await
            .map_err(into_response)?;
    }

    let response = state
        .mint
        .process_redeem_outcome(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process redeem outcome: {}", err);
            into_response(err)
        })?;
    Ok(Json(response))
}
