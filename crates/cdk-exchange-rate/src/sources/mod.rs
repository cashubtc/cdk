//! Optional exchange-rate source implementations.

#[cfg(feature = "sources-http")]
pub mod bitstamp;
#[cfg(feature = "sources-http")]
pub mod coinbase;
#[cfg(feature = "sources-http")]
pub mod kraken;

#[cfg(feature = "sources-http")]
pub use bitstamp::BitstampRateSource;
#[cfg(feature = "sources-http")]
pub use coinbase::CoinbaseRateSource;
#[cfg(feature = "sources-http")]
pub use kraken::KrakenRateSource;

#[cfg(feature = "sources-http")]
pub(crate) fn parse_btc_fiat_to_sats_per_fiat(value: &str) -> Result<u64, crate::RateOracleError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return Err(crate::RateOracleError::SourceError(format!(
            "invalid BTC/fiat rate: {value}"
        )));
    }

    let mut parts = trimmed.split('.');
    let whole = parts.next().unwrap_or_default();
    let fractional = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return Err(crate::RateOracleError::SourceError(format!(
            "invalid BTC/fiat rate: {value}"
        )));
    }

    let mut digits = String::with_capacity(whole.len() + fractional.len());
    digits.push_str(whole);
    digits.push_str(fractional);
    let digits = digits.trim_start_matches('0');
    if digits.is_empty()
        || !whole.chars().all(|ch| ch.is_ascii_digit())
        || !fractional.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(crate::RateOracleError::SourceError(format!(
            "invalid BTC/fiat rate: {value}"
        )));
    }

    let numerator = digits
        .parse::<u128>()
        .map_err(|error| crate::RateOracleError::SourceError(error.to_string()))?;
    let scale =
        10_u128
            .checked_pow(fractional.len().try_into().map_err(
                |error: std::num::TryFromIntError| {
                    crate::RateOracleError::SourceError(error.to_string())
                },
            )?)
            .ok_or_else(|| {
                crate::RateOracleError::SourceError("BTC/fiat scale overflow".to_string())
            })?;
    let numerator_sats = 100_000_000_u128.checked_mul(scale).ok_or_else(|| {
        crate::RateOracleError::SourceError("BTC/fiat numerator overflow".to_string())
    })?;
    let sats = div_ceil(numerator_sats, numerator);
    u64::try_from(sats).map_err(|error| crate::RateOracleError::SourceError(error.to_string()))
}

#[cfg(feature = "sources-http")]
fn div_ceil(numerator: u128, denominator: u128) -> u128 {
    numerator / denominator + u128::from(numerator % denominator != 0)
}
