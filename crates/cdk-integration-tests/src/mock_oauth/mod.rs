use axum::response::{IntoResponse, Response, Result};
use axum::routing::get;
use axum::{Json, Router};
use cdk::oidc_client::OidcConfig;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use serde_json::{json, Value};

async fn crate_mock_oauth() -> Router {
    let router = Router::new()
        .route("/config", get(handler_get_config))
        .route("/token", get(handler_get_token))
        .route("/jwks", get(handler_get_jwkset));
    router
}

async fn handler_get_config() -> Result<Json<OidcConfig>> {
    Ok(Json(OidcConfig {
        jwks_uri: "/jwks".to_string(),
        issuer: "127.0.0.1".to_string(),
        token_endpoint: "/token".to_string(),
    }))
}

async fn handler_get_jwkset() -> Result<Json<JwkSet>> {
    let jwk:Jwk = serde_json::from_value(json!({
        "kty": "RSA",
        "n": "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ",
        "e": "AQAB",
        "kid": "rsa01",
        "alg": "RS256",
        "use": "sig"
    })).unwrap();

    Ok(Json(JwkSet { keys: vec![jwk] }))
}

async fn handler_get_token() -> Result<Json<Value>> {
    Ok(Json(json!({"access_token": ""})))
}
