//! Open Id Connect

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::Deserialize;
#[cfg(feature = "wallet")]
use serde::Serialize;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::instrument;

use crate::HttpClient;

fn validate_client_id_claim(
    claim_name: &str,
    claim_value: &serde_json::Value,
    client_id: &str,
) -> Result<(), Error> {
    let Some(token_client_id) = claim_value.as_str() else {
        tracing::warn!("{} claim is not a string", claim_name);
        return Err(Error::InvalidClientId);
    };

    if token_client_id != client_id {
        tracing::warn!(
            "Client ID ({}) mismatch: expected {}, got {}",
            claim_name,
            client_id,
            token_client_id
        );
        return Err(Error::InvalidClientId);
    }

    Ok(())
}

fn validate_client_id_claims(
    claims: &HashMap<String, serde_json::Value>,
    client_id: &str,
) -> Result<(), Error> {
    match claims.get("client_id") {
        Some(token_client_id) => validate_client_id_claim("client_id", token_client_id, client_id),
        None => match claims.get("azp") {
            Some(azp) => validate_client_id_claim("azp", azp, client_id),
            None => {
                tracing::warn!("CAT missing client_id or azp claim for configured client ID");
                Err(Error::InvalidClientId)
            }
        },
    }
}

/// OIDC Error
#[derive(Debug, Error)]
pub enum Error {
    /// From HTTP error
    #[error(transparent)]
    Http(#[from] crate::HttpError),
    /// From JWT error
    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),
    /// Missing kid header
    #[error("Missing kid header")]
    MissingKidHeader,
    /// Missing jwk header
    #[error("Missing jwk")]
    MissingJwkHeader,
    /// Unsupported Algo
    #[error("Unsupported signing algo")]
    UnsupportedSigningAlgo,
    /// Invalid Client ID
    #[error("Invalid Client ID")]
    InvalidClientId,
}

impl From<Error> for crate::error::Error {
    fn from(value: Error) -> Self {
        tracing::debug!("Clear auth verification failed: {}", value);
        crate::error::Error::ClearAuthFailed
    }
}

/// Open Id Config
#[derive(Debug, Clone, Deserialize)]
pub struct OidcConfig {
    /// URI for the JSON Web Key Set
    pub jwks_uri: String,
    /// Token issuer identifier
    pub issuer: String,
    /// Token endpoint URL
    pub token_endpoint: String,
    /// Device authorization endpoint URL
    pub device_authorization_endpoint: String,
}

/// Http Client
#[derive(Debug, Clone)]
pub struct OidcClient {
    client: HttpClient,
    openid_discovery: String,
    client_id: Option<String>,
    oidc_config: Arc<RwLock<Option<OidcConfig>>>,
    jwks_set: Arc<RwLock<Option<JwkSet>>>,
}

/// OAuth2 grant type
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// Refresh token grant
    RefreshToken,
}

/// Request to refresh an access token
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, Serialize)]
pub struct RefreshTokenRequest {
    /// The grant type for this request
    pub grant_type: GrantType,
    /// OAuth2 client identifier
    pub client_id: String,
    /// The refresh token to exchange
    pub refresh_token: String,
}

/// Response from token endpoint
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    /// The access token issued by the authorization server
    pub access_token: String,
    /// Optional refresh token for obtaining new access tokens
    pub refresh_token: Option<String>,
    /// Optional lifetime in seconds of the access token
    pub expires_in: Option<i64>,
    /// The type of token issued (typically "Bearer")
    pub token_type: String,
}

impl OidcClient {
    /// Create new [`OidcClient`]
    pub fn new(openid_discovery: String, client_id: Option<String>) -> Self {
        Self {
            client: HttpClient::new(),
            openid_discovery,
            client_id,
            oidc_config: Arc::new(RwLock::new(None)),
            jwks_set: Arc::new(RwLock::new(None)),
        }
    }

    /// Get client id
    pub fn client_id(&self) -> Option<String> {
        self.client_id.clone()
    }

    /// Get config from oidc server
    #[instrument(skip(self))]
    pub async fn get_oidc_config(&self) -> Result<OidcConfig, Error> {
        tracing::debug!("Getting oidc config");
        let oidc_config: OidcConfig = self.client.fetch(&self.openid_discovery).await?;

        let mut current_config = self.oidc_config.write().await;

        *current_config = Some(oidc_config.clone());

        Ok(oidc_config)
    }

    /// Get jwk set
    #[instrument(skip(self))]
    pub async fn get_jwkset(&self, jwks_uri: &str) -> Result<JwkSet, Error> {
        tracing::debug!("Getting jwks set");
        let jwks_set: JwkSet = self.client.fetch(jwks_uri).await?;

        let mut current_set = self.jwks_set.write().await;

        *current_set = Some(jwks_set.clone());

        Ok(jwks_set)
    }

    /// Verify cat token
    #[instrument(skip_all)]
    pub async fn verify_cat(&self, cat_jwt: &str) -> Result<(), Error> {
        tracing::debug!("Verifying cat");
        let header = decode_header(cat_jwt)?;

        let kid = header.kid.ok_or(Error::MissingKidHeader)?;

        let oidc_config = {
            let locked = self.oidc_config.read().await;
            match locked.deref() {
                Some(config) => config.clone(),
                None => {
                    drop(locked);
                    self.get_oidc_config().await?
                }
            }
        };

        let jwks = {
            let locked = self.jwks_set.read().await;
            match locked.deref() {
                Some(set) => set.clone(),
                None => {
                    drop(locked);
                    self.get_jwkset(&oidc_config.jwks_uri).await?
                }
            }
        };

        let jwk = match jwks.find(&kid) {
            Some(jwk) => jwk.clone(),
            None => {
                let refreshed_jwks = self.get_jwkset(&oidc_config.jwks_uri).await?;
                refreshed_jwks
                    .find(&kid)
                    .ok_or(Error::MissingKidHeader)?
                    .clone()
            }
        };

        let decoding_key = match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)?,
            AlgorithmParameters::EllipticCurve(ecdsa) => {
                DecodingKey::from_ec_components(&ecdsa.x, &ecdsa.y)?
            }
            _ => return Err(Error::UnsupportedSigningAlgo),
        };

        let validation = {
            let mut validation = Validation::new(header.alg);
            validation.validate_exp = true;
            validation.validate_aud = false;
            validation.set_issuer(&[oidc_config.issuer]);
            validation
        };

        match decode::<HashMap<String, serde_json::Value>>(cat_jwt, &decoding_key, &validation) {
            Ok(claims) => {
                tracing::debug!("Successfully verified cat");
                if let Some(client_id) = &self.client_id {
                    validate_client_id_claims(&claims.claims, client_id)?;
                }
            }
            Err(err) => {
                tracing::debug!("Could not verify cat: {}", err);
                return Err(err.into());
            }
        }

        Ok(())
    }

    /// Get new access token using refresh token
    #[cfg(feature = "wallet")]
    pub async fn refresh_access_token(
        &self,
        client_id: String,
        refresh_token: String,
    ) -> Result<TokenResponse, Error> {
        let token_url = self.get_oidc_config().await?.token_endpoint;

        let request = RefreshTokenRequest {
            grant_type: GrantType::RefreshToken,
            client_id,
            refresh_token,
        };

        let response: TokenResponse = self.client.post_form(&token_url, &request).await?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn claims(value: serde_json::Value) -> HashMap<String, serde_json::Value> {
        serde_json::from_value(value).expect("claims should be an object")
    }

    #[test]
    fn validate_client_id_claims_accepts_client_id() {
        let claims = claims(json!({
            "client_id": "expected-client",
            "azp": "other-client",
        }));

        assert!(validate_client_id_claims(&claims, "expected-client").is_ok());
    }

    #[test]
    fn validate_client_id_claims_accepts_azp_fallback() {
        let claims = claims(json!({
            "azp": "expected-client",
        }));

        assert!(validate_client_id_claims(&claims, "expected-client").is_ok());
    }

    #[test]
    fn validate_client_id_claims_rejects_missing_claims() {
        let claims = claims(json!({
            "sub": "user",
        }));

        assert!(matches!(
            validate_client_id_claims(&claims, "expected-client"),
            Err(Error::InvalidClientId)
        ));
    }

    #[test]
    fn validate_client_id_claims_rejects_non_string_client_id() {
        let claims = claims(json!({
            "client_id": null,
            "azp": "expected-client",
        }));

        assert!(matches!(
            validate_client_id_claims(&claims, "expected-client"),
            Err(Error::InvalidClientId)
        ));
    }

    #[test]
    fn validate_client_id_claims_rejects_non_string_azp() {
        let claims = claims(json!({
            "azp": 42,
        }));

        assert!(matches!(
            validate_client_id_claims(&claims, "expected-client"),
            Err(Error::InvalidClientId)
        ));
    }

    #[test]
    fn validate_client_id_claims_rejects_mismatch() {
        let claims = claims(json!({
            "client_id": "other-client",
        }));

        assert!(matches!(
            validate_client_id_claims(&claims, "expected-client"),
            Err(Error::InvalidClientId)
        ));
    }
}
