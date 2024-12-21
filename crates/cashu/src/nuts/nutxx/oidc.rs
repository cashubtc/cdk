//! Open Id Connect

use std::collections::HashMap;

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
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
}

/// Open Id Config
#[derive(Debug, Deserialize)]
pub struct OidcConfig {
    pub jwks_uri: String,
    pub issuer: String,
}

/// Http Client
#[derive(Debug, Clone)]
pub struct OidcClient {
    inner: Client,
    openid_discovery: String,
}

impl OidcClient {
    /// Create new [`OidcClient`]
    pub fn new(openid_discovery: String) -> Self {
        Self {
            inner: Client::new(),
            openid_discovery,
        }
    }

    /// Get config from oidc server
    #[instrument(skip(self))]
    pub async fn get_oidc_config(&self) -> Result<OidcConfig, Error> {
        tracing::debug!("Getting oidc config");
        Ok(self
            .inner
            .get(&self.openid_discovery)
            .send()
            .await?
            .json::<OidcConfig>()
            .await?)
    }

    /// Get jwk set
    #[instrument(skip(self))]
    pub async fn get_jwkset(&self, jwks_uri: &str) -> Result<JwkSet, Error> {
        tracing::debug!("Getting jwks set");
        Ok(self
            .inner
            .get(jwks_uri)
            .send()
            .await?
            .json::<JwkSet>()
            .await?)
    }

    /// Verify cat token
    #[instrument(skip(self))]
    pub async fn verify_cat(&self, cat_jwt: &str) -> Result<(), Error> {
        tracing::debug!("Verifying cat");
        let header = decode_header(cat_jwt)?;

        let kid = header.kid.ok_or(Error::MissingKidHeader)?;
        let oidc_config = self.get_oidc_config().await?;
        let jwks = self.get_jwkset(&oidc_config.jwks_uri).await?;

        let jwk = jwks.find(&kid).ok_or(Error::MissingJwkHeader)?;

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
}
