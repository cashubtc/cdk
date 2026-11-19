use std::str::FromStr;

use axum::body::Body;
use axum::extract::Request;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use cdk::nuts::nut22::AuthorizedRequest;
use cdk::nuts::{
    AuthToken, BlindAuthToken, KeysResponse, KeysetResponse, MintAuthRequest, MintResponse,
};
use serde::{Deserialize, Serialize};

use crate::{get_keyset_pubkeys, into_response, MintState};

const CLEAR_AUTH_KEY: &str = "Clear-auth";
const BLIND_AUTH_KEY: &str = "Blind-auth";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthHeader {
    /// Clear Auth token
    Clear(String),
    /// Blind Auth token
    Blind(Box<BlindAuthToken>),
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

            let mut token = BlindAuthToken::from_str(&token).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid Blind-auth header value".to_string(),
                )
            })?;
            // A v3 BAT signs the request as it arrived (NUT-22), recorded by
            // `blind_auth_request_context`.
            token.request = parts.extensions.get::<AuthorizedRequest>().cloned();

            return Ok(AuthHeader::Blind(Box::new(token)));
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

/// Largest request body hashed for a blind-auth request
const MAX_AUTHORIZED_BODY_LEN: usize = 2 * 1024 * 1024;

/// Record the request a blind-auth token arrives with (NUT-22)
///
/// A v3 BAT signs the method, the target (the path relative to the mint URL
/// plus the query, as sent) and `SHA256` of the body bytes as sent. This
/// middleware runs on the mint router, where paths are relative to the mint
/// URL, buffers the body (bounded), hashes it and hands the request on with
/// the same bytes.
pub async fn blind_auth_request_context(req: Request, next: Next) -> Response {
    use axum::response::IntoResponse;

    if !req.headers().contains_key(BLIND_AUTH_KEY) {
        return next.run(req).await;
    }

    // NUT-22 hashes the body "after any content-coding", but leaves open
    // whether a coded body may be used at all; a proxy that decodes or
    // re-encodes it would break the witness. Refuse coded bodies.
    if req
        .headers()
        .get(axum::http::header::CONTENT_ENCODING)
        .is_some_and(|coding| coding.as_bytes() != b"identity")
    {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Blind-auth requests must not use a content-coding".to_string(),
        )
            .into_response();
    }

    let (mut parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_AUTHORIZED_BODY_LEN).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body too large for blind auth".to_string(),
            )
                .into_response()
        }
    };
    let target = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| parts.uri.path());
    let context = AuthorizedRequest::new(parts.method.as_str(), target, &bytes);
    parts.extensions.insert(context);
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// Get all active keyset IDs of the mint
///
/// This endpoint returns a list of keysets that the mint currently supports and will accept tokens from.
pub async fn get_auth_keysets(
    State(state): State<MintState>,
) -> Result<Json<KeysetResponse>, Response> {
    Ok(Json(state.mint.auth_keysets()))
}

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

#[cfg(test)]
mod tests {
    use axum::http::Request as HttpRequest;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;

    fn v3_token() -> BlindAuthToken {
        let key = cdk::nuts::SecretKey::generate();
        BlindAuthToken::new(cdk::nuts::AuthProof {
            keyset_id: cdk::nuts::Id::from_bytes(&[[2u8].as_slice(), &[1; 32]].concat())
                .expect("v3 id"),
            secret: cdk::secret::Secret::from_point(&key.public_key()),
            c: cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"c").into(),
            dleq: None,
            witness: None,
        })
    }

    /// The request a BAT arrives with is recorded as sent: method, path and
    /// query untouched, body hashed byte for byte
    #[tokio::test]
    async fn blind_auth_context_records_the_request_as_sent() {
        let router = Router::new()
            .route(
                "/v1/swap",
                post(|auth: AuthHeader, body: String| async move {
                    let AuthHeader::Blind(token) = auth else {
                        return "no token".to_string();
                    };
                    let request = token.request.expect("request context");
                    format!(
                        "{}|{}|{}|{}",
                        request.method,
                        request.target,
                        cdk::util::hex::encode(request.body_hash),
                        body
                    )
                }),
            )
            .layer(axum::middleware::from_fn(blind_auth_request_context));

        let body = r#"{ "inputs" : [], "outputs":[] }"#;
        let response = router
            .clone()
            .oneshot(
                HttpRequest::post("/v1/swap?b=2&a=1&q=a%20b")
                    .header(BLIND_AUTH_KEY, v3_token().to_string())
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let expected = AuthorizedRequest::new("POST", "/v1/swap?b=2&a=1&q=a%20b", body.as_bytes());
        assert_eq!(
            String::from_utf8(bytes.to_vec()).expect("utf8"),
            format!(
                "POST|/v1/swap?b=2&a=1&q=a%20b|{}|{}",
                cdk::util::hex::encode(expected.body_hash),
                body
            )
        );

        // Without a Blind-auth header the request passes untouched.
        let response = router
            .oneshot(
                HttpRequest::post("/v1/swap")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(bytes.as_ref(), b"no token");
    }

    #[tokio::test]
    async fn blind_auth_request_with_content_coding_is_refused() {
        let router = Router::new()
            .route("/v1/swap", post(|| async { "ok" }))
            .layer(axum::middleware::from_fn(blind_auth_request_context));
        let response = router
            .oneshot(
                HttpRequest::post("/v1/swap")
                    .header(BLIND_AUTH_KEY, v3_token().to_string())
                    .header("Content-Encoding", "gzip")
                    .body(Body::from(vec![0x1f, 0x8b]))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
}
