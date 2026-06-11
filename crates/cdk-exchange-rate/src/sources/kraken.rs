//! Kraken exchange-rate source.

use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use serde::Deserialize;

use crate::{RateOracleError, RateSource};

/// Kraken ticker source.
#[derive(Debug, Clone)]
pub struct KrakenRateSource {
    client: reqwest::Client,
}

impl KrakenRateSource {
    /// Create a source with a default HTTP client.
    pub fn new() -> Self {
        Self::with_client(reqwest::Client::new())
    }

    /// Create a source with an operator-provided HTTP client.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for KrakenRateSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RateSource for KrakenRateSource {
    fn name(&self) -> &str {
        "kraken"
    }

    async fn fetch(
        &self,
        fiat: &CurrencyUnit,
    ) -> Result<(u64, Option<SystemTime>), RateOracleError> {
        let url = format!("https://api.kraken.com/0/public/Ticker?pair=XBT{fiat}");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(source_error)?
            .error_for_status()
            .map_err(source_error)?
            .json::<KrakenTickerResponse>()
            .await
            .map_err(source_error)?;

        let ticker = response
            .result
            .values()
            .next()
            .ok_or_else(|| RateOracleError::SourceError("missing kraken ticker".to_owned()))?;
        let last = ticker
            .c
            .first()
            .ok_or_else(|| RateOracleError::SourceError("missing kraken close".to_owned()))?;

        Ok((btc_fiat_to_sats_per_fiat(last)?, None))
    }
}

#[derive(Debug, Deserialize)]
struct KrakenTickerResponse {
    result: HashMap<String, KrakenTicker>,
}

#[derive(Debug, Deserialize)]
struct KrakenTicker {
    c: Vec<String>,
}

fn btc_fiat_to_sats_per_fiat(value: &str) -> Result<u64, RateOracleError> {
    crate::sources::parse_btc_fiat_to_sats_per_fiat(value)
}

fn source_error(error: reqwest::Error) -> RateOracleError {
    RateOracleError::SourceError(error.to_string())
}
