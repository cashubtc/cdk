use std::str::FromStr;

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
#[cfg(feature = "swagger")]
use cdk::error::ErrorResponse;
use cdk::nuts::{
    AuthToken, BlindAuthToken, CheckBlindAuthStateRequest, CheckBlindAuthStateResponse,
    KeysResponse, KeysetResponse, MintAuthRequest, MintResponse, SpendBlindAuthRequest,
    SpendBlindAuthResponse,
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "auth")]
use crate::{get_keyset_pubkeys, into_response, MintState};

const CLEAR_AUTH_KEY: &str = "Clear-auth";
const BLIND_AUTH_KEY: &str = "Blind-auth";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthHeader {
    /// Clear Auth token
    Clear(String),
    /// Blind Auth token
    Blind(BlindAuthToken),
    /// No auth
    None,
}

impl From<AuthHeader> for Option<AuthToken> {
    fn from(value: AuthHeader) -> Option<AuthToken> {
        match value {
            AuthHeader::Clear(token) => Some(AuthToken::ClearAuth(token)),
            AuthHeader::Blind(token) => Some(AuthToken::BlindAuth(token)),
            AuthHeader::None => None,
        }
    }
}

impl<S> FromRequestParts<S> for AuthHeader
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Check for Blind-auth header
        if let Some(bat) = parts.headers.get(BLIND_AUTH_KEY) {
            let token = bat
                .to_str()
                .map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        "Invalid Blind-auth header value".to_string(),
                    )
                })?
                .to_string();

            let token = BlindAuthToken::from_str(&token).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid Blind-auth header value".to_string(),
                )
            })?;

            return Ok(AuthHeader::Blind(token));
        }

        // Check for Clear-auth header
        if let Some(cat) = parts.headers.get(CLEAR_AUTH_KEY) {
            let token = cat
                .to_str()
                .map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        "Invalid Clear-auth header value".to_string(),
                    )
                })?
                .to_string();
            return Ok(AuthHeader::Clear(token));
        }

        // No authentication headers found - this is now valid
        Ok(AuthHeader::None)
    }
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1/auth/blind",
    path = "/keysets",
    responses(
        (status = 200, description = "Successful response", body = KeysetResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get all active keyset IDs of the mint
///
/// This endpoint returns a list of keysets that the mint currently supports and will accept tokens from.
#[cfg(feature = "auth")]
pub async fn get_auth_keysets(
    State(state): State<MintState>,
) -> Result<Json<KeysetResponse>, Response> {
    Ok(Json(state.mint.auth_keysets()))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1/auth/blind",
    path = "/keys",
    responses(
        (status = 200, description = "Successful response", body = KeysResponse, content_type = "application/json")
    )
))]
/// Get the public keys of the newest blind auth mint keyset
///
/// This endpoint returns a dictionary of all supported token values of the mint and their associated public key.
pub async fn get_blind_auth_keys(
    State(state): State<MintState>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.auth_pubkeys().map_err(|err| {
        tracing::error!("Could not get keys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

/// Mint tokens by paying a BOLT11 Lightning invoice.
///
/// Requests the minting of tokens belonging to a paid payment request.
///
/// Call this endpoint after `POST /v1/mint/quote`.
#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1/auth",
    path = "/blind/mint",
    request_body(content = MintAuthRequest, description = "Request params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = MintResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_mint_auth(
    auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintAuthRequest>,
) -> Result<Json<MintResponse>, Response> {
    let auth_token = match auth {
        AuthHeader::Clear(cat) => {
            if cat.is_empty() {
                tracing::debug!("Received blind auth mint request without cat");
                return Err(into_response(cdk::Error::ClearAuthRequired));
            }

            AuthToken::ClearAuth(cat)
        }
        _ => {
            tracing::debug!("Received blind auth mint request without cat");
            return Err(into_response(cdk::Error::ClearAuthRequired));
        }
    };

    let res = state
        .mint
        .mint_blind_auth(auth_token, payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process blind auth mint: {}", err);
            into_response(err)
        })?;

    Ok(Json(res))
}

/// Check state of blind auth proofs
///
/// This endpoint allows external apps to check if BATs are valid and unspent
/// without consuming them. Use this to verify a BAT before accepting it.
#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1/auth",
    path = "/blind/checkstate",
    request_body(content = CheckBlindAuthStateRequest, description = "Auth proofs to check", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = CheckBlindAuthStateResponse, content_type = "application/json"),
        (status = 400, description = "Invalid request", body = ErrorResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_blind_auth_checkstate(
    State(state): State<MintState>,
    Json(payload): Json<CheckBlindAuthStateRequest>,
) -> Result<Json<CheckBlindAuthStateResponse>, Response> {
    let response = state
        .mint
        .get_blind_auth_states(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not check blind auth states: {}", err);
            into_response(err)
        })?;

    Ok(Json(response))
}

/// Spend a blind auth proof
///
/// This endpoint allows external apps to mark a BAT as spent after
/// successfully processing a request. The BAT cannot be reused after this.
#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1/auth",
    path = "/blind/spend",
    request_body(content = SpendBlindAuthRequest, description = "Auth proof to spend", content_type = "application/json"),
    responses(
        (status = 200, description = "Successfully spent", body = SpendBlindAuthResponse, content_type = "application/json"),
        (status = 400, description = "Already spent or invalid", body = ErrorResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_blind_auth_spend(
    State(state): State<MintState>,
    Json(payload): Json<SpendBlindAuthRequest>,
) -> Result<Json<SpendBlindAuthResponse>, Response> {
    let response = state.mint.spend_blind_auth(payload).await.map_err(|err| {
        tracing::error!("Could not spend blind auth: {}", err);
        into_response(err)
    })?;

    Ok(Json(response))
}

pub fn create_auth_router(state: MintState) -> Router<MintState> {
    Router::new()
        .nest(
            "/auth/blind",
            Router::new()
                .route("/keys", get(get_blind_auth_keys))
                .route("/keysets", get(get_auth_keysets))
                .route("/keys/{keyset_id}", get(get_keyset_pubkeys))
                .route("/mint", post(post_mint_auth))
                .route("/checkstate", post(post_blind_auth_checkstate))
                .route("/spend", post(post_blind_auth_spend)),
        )
        .with_state(state)
}
