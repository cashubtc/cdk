//! Open Id Connect

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::instrument;

/// OIDC Error
#[derive(Debug, Error)]
pub enum Error {
    /// From Reqwest error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// From Reqwest error
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
    /// Access token not returned
    #[error("Error getting access token")]
    AccessTokenMissing,
}

impl From<Error> for cdk_common::error::Error {
    fn from(value: Error) -> Self {
        cdk_common::error::Error::Custom(value.to_string())
    }
}

/// Open Id Config
#[derive(Debug, Clone, Deserialize)]
pub struct OidcConfig {
    pub jwks_uri: String,
    pub issuer: String,
    pub token_endpoint: String,
}

/// Http Client
#[derive(Debug, Clone)]
pub struct OidcClient {
    client: Client,
    openid_discovery: String,
    oidc_config: Arc<RwLock<Option<OidcConfig>>>,
    jwks_set: Arc<RwLock<Option<JwkSet>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccessTokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub username: String,
    pub password: String,
}

impl OidcClient {
    /// Create new [`OidcClient`]
    pub fn new(openid_discovery: String) -> Self {
        Self {
            client: Client::new(),
            openid_discovery,
            oidc_config: Arc::new(RwLock::new(None)),
            jwks_set: Arc::new(RwLock::new(None)),
        }
    }

    /// Get config from oidc server
    #[instrument(skip(self))]
    pub async fn get_oidc_config(&self) -> Result<OidcConfig, Error> {
        tracing::debug!("Getting oidc config");
        let oidc_config = self
            .client
            .get(&self.openid_discovery)
            .send()
            .await?
            .json::<OidcConfig>()
            .await?;

        let mut current_config = self.oidc_config.write().await;

        *current_config = Some(oidc_config.clone());

        Ok(oidc_config)
    }

    /// Get jwk set
    #[instrument(skip(self))]
    pub async fn get_jwkset(&self, jwks_uri: &str) -> Result<JwkSet, Error> {
        tracing::debug!("Getting jwks set");
        let jwks_set = self
            .client
            .get(jwks_uri)
            .send()
            .await?
            .json::<JwkSet>()
            .await?;

        let mut current_set = self.jwks_set.write().await;

        *current_set = Some(jwks_set.clone());

        Ok(jwks_set)
    }

    /// Verify cat token
    #[instrument(skip(self, cat_jwt))]
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
            // REVIEW: Mint doesnt verify aud but i think wallet does?
            validation.validate_aud = false;
            //     validation.set_issuer(&[oidc_config.issuer]);
            validation
        };

        if let Err(err) =
            decode::<HashMap<String, serde_json::Value>>(cat_jwt, &decoding_key, &validation)
        {
            tracing::debug!("Could not verify cat: {}", err);
            return Err(err.into());
        }

        Ok(())
    }

    /// Get Access token (CAT)
    #[cfg(feature = "wallet")]
    pub async fn get_access_token_with_user_password(
        &self,
        username: String,
        password: String,
    ) -> Result<String, Error> {
        let token_url = self.get_oidc_config().await?.token_endpoint;

        let request = AccessTokenRequest {
            grant_type: "password".to_string(),
            client_id: "cashu-client".to_string(),
            username,
            password,
        };

        let response: Value = self
            .client
            .post(token_url)
            .form(&request)
            .send()
            .await?
            .json()
            .await?;

        let token = response
            .get("access_token")
            .ok_or(Error::AccessTokenMissing)?;

        Ok(token.to_string())
    }
}
