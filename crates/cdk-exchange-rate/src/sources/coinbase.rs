//! Coinbase exchange-rate source.

use std::time::SystemTime;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use serde::Deserialize;

use crate::{RateOracleError, RateSource};

/// Coinbase spot-price source.
#[derive(Debug, Clone)]
pub struct CoinbaseRateSource {
    client: reqwest::Client,
}

impl CoinbaseRateSource {
    /// Create a source with a default HTTP client.
    pub fn new() -> Self {
        Self::with_client(reqwest::Client::new())
    }

    /// Create a source with an operator-provided HTTP client.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for CoinbaseRateSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RateSource for CoinbaseRateSource {
    fn name(&self) -> &str {
        "coinbase"
    }

    async fn fetch(
        &self,
        fiat: &CurrencyUnit,
    ) -> Result<(f64, Option<SystemTime>), RateOracleError> {
        let url = format!("https://api.coinbase.com/v2/prices/BTC-{fiat}/spot");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(source_error)?
            .error_for_status()
            .map_err(source_error)?
            .json::<CoinbaseSpotResponse>()
            .await
            .map_err(source_error)?;

        Ok((btc_fiat_to_sats_per_fiat(&response.data.amount)?, None))
    }
}

#[derive(Debug, Deserialize)]
struct CoinbaseSpotResponse {
    data: CoinbaseSpotData,
}

#[derive(Debug, Deserialize)]
struct CoinbaseSpotData {
    amount: String,
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
