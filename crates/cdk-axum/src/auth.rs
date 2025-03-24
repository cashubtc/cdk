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
    AuthToken, BlindAuthToken, KeysResponse, KeysetResponse, MintAuthRequest, MintBolt11Response,
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
    let keysets = state.mint.auth_keysets().await.map_err(|err| {
        tracing::error!("Could not get keysets: {}", err);
        into_response(err)
    })?;

    Ok(Json(keysets))
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
    let pubkeys = state.mint.auth_pubkeys().await.map_err(|err| {
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
        (status = 200, description = "Successful response", body = MintBolt11Response, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
pub async fn post_mint_auth(
    auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<MintAuthRequest>,
) -> Result<Json<MintBolt11Response>, Response> {
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

pub fn create_auth_router(state: MintState) -> Router<MintState> {
    Router::new()
        .nest(
            "/auth/blind",
            Router::new()
                .route("/keys", get(get_blind_auth_keys))
                .route("/keysets", get(get_auth_keysets))
                .route("/keys/{keyset_id}", get(get_keyset_pubkeys))
                .route("/mint", post(post_mint_auth)),
        )
        .with_state(state)
}
