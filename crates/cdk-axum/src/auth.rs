use std::str::FromStr;

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, RequestExt, Router};
use cdk::nuts::{
    AuthToken, BlindAuthToken, KeysResponse, KeysetResponse, MintAuthRequest, MintResponse,
};
use serde::{Deserialize, Serialize};

use crate::{get_keyset_pubkeys, into_response, MintState};

const CLEAR_AUTH_KEY: &str = "Clear-auth";
const BLIND_AUTH_KEY: &str = "Blind-auth";

#[derive(Clone)]
struct AuthorizedBody(axum::body::Bytes);

/// Preserve the exact incoming HTTP bytes for version-02 blind authentication.
/// Respect the same configured body limit as the JSON extractors.
pub async fn bind_blind_auth_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !request.headers().contains_key(BLIND_AUTH_KEY) {
        return next.run(request).await;
    }
    let (mut parts, body) = request.with_limited_body().into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return axum::response::IntoResponse::into_response((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Invalid request body",
            ))
        }
    };
    parts.extensions.insert(AuthorizedBody(bytes.clone()));
    next.run(axum::extract::Request::from_parts(
        parts,
        axum::body::Body::from(bytes),
    ))
    .await
}

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

            let mut token = BlindAuthToken::from_str(&token).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid Blind-auth header value".to_string(),
                )
            })?;

            if token.auth_proof.keyset_id.get_version() == cdk::nuts::KeySetVersion::Version02 {
                let body = parts.extensions.get::<AuthorizedBody>().ok_or((
                    StatusCode::BAD_REQUEST,
                    "Missing request authentication context".to_owned(),
                ))?;
                let uri = parts
                    .extensions
                    .get::<axum::extract::OriginalUri>()
                    .map(|uri| &uri.0)
                    .unwrap_or(&parts.uri);
                let target = uri
                    .path_and_query()
                    .map(|value| value.as_str())
                    .unwrap_or("/");
                token
                    .set_request_context(parts.method.as_str(), target, &body.0)
                    .map_err(|_| {
                        (
                            StatusCode::BAD_REQUEST,
                            "Invalid request authentication context".to_owned(),
                        )
                    })?;
            }
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
    use axum::body::Body;
    use axum::http::Request;
    use cdk::nuts::nut10::nutroot::SpendInfo;
    use tower::ServiceExt;

    use super::*;

    async fn authorize(header: AuthHeader, body: axum::body::Bytes) -> StatusCode {
        assert_eq!(&body[..], b"{ }");
        match header {
            AuthHeader::Blind(token) if token.verify_request().is_ok() => StatusCode::OK,
            _ => StatusCode::UNAUTHORIZED,
        }
    }

    #[tokio::test]
    async fn nutroot_auth_uses_original_uri_and_exact_body_before_json() {
        let key = cdk::nuts::SecretKey::from_slice(&[9; 32]).unwrap();
        let id = cdk::nuts::Id::from_bytes(&[vec![2], vec![1; 32]].concat()).unwrap();
        let c = cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"signature").into();
        let mut proof = cdk::nuts::Proof::new(
            1.into(),
            id,
            key.public_key().to_string().parse().unwrap(),
            c,
        );
        proof.spend_info = Some(SpendInfo {
            bearer_key: Some(key),
            ..Default::default()
        });
        let mut token = BlindAuthToken::from_proof(proof).unwrap();
        token.sign_request("POST", "/v1/swap?x=1", b"{ }").unwrap();
        let router = Router::new()
            .nest("/v1", Router::new().route("/swap", post(authorize)))
            .layer(axum::middleware::from_fn(bind_blind_auth_request));
        for (query, expected) in [(1, StatusCode::OK), (2, StatusCode::UNAUTHORIZED)] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/v1/swap?x={query}"))
                        .header(BLIND_AUTH_KEY, token.to_string())
                        .body(Body::from("{ }"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        let limited = router.layer(axum::extract::DefaultBodyLimit::max(2));
        let response = limited
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/swap?x=1")
                    .header(BLIND_AUTH_KEY, token.to_string())
                    .body(Body::from("{ }"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
