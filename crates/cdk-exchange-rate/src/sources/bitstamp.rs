//! Bitstamp exchange-rate source.

use std::time::SystemTime;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use serde::Deserialize;

use crate::{RateOracleError, RateSource};

/// Bitstamp ticker source.
#[derive(Debug, Clone)]
pub struct BitstampRateSource {
    client: reqwest::Client,
}

impl BitstampRateSource {
    /// Create a source with a default HTTP client.
    pub fn new() -> Self {
        Self::with_client(reqwest::Client::new())
    }

    /// Create a source with an operator-provided HTTP client.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for BitstampRateSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RateSource for BitstampRateSource {
    fn name(&self) -> &str {
        "bitstamp"
    }

    async fn fetch(
        &self,
        fiat: &CurrencyUnit,
    ) -> Result<(f64, Option<SystemTime>), RateOracleError> {
        let fiat_lower = fiat.to_string().to_lowercase();
        let url = format!("https://www.bitstamp.net/api/v2/ticker/btc{fiat_lower}/");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(source_error)?
            .error_for_status()
            .map_err(source_error)?
            .json::<BitstampTickerResponse>()
            .await
            .map_err(source_error)?;

        Ok((btc_fiat_to_sats_per_fiat(&response.last)?, None))
    }
}

#[derive(Debug, Deserialize)]
struct BitstampTickerResponse {
    last: String,
}

fn btc_fiat_to_sats_per_fiat(value: &str) -> Result<f64, RateOracleError> {
    let btc_fiat = value
        .parse::<f64>()
        .map_err(|error| RateOracleError::SourceError(error.to_string()))?;
    if !btc_fiat.is_finite() || btc_fiat <= 0.0 {
        return Err(RateOracleError::SourceError(format!(
            "invalid BTC/fiat rate: {btc_fiat}"
        )));
    }
    Ok(100_000_000.0 / btc_fiat)
}

fn source_error(error: reqwest::Error) -> RateOracleError {
    RateOracleError::SourceError(error.to_string())
}
