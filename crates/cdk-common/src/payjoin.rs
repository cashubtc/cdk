//! Payjoin helper functions for CDK integrations.
//!
//! Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`.

use std::num::ParseIntError;

use bitcoin::bech32::primitives::decode::{
    CharError, CheckedHrpstring, CheckedHrpstringError, UncheckedHrpstringError,
};
use bitcoin::bech32::{self, Hrp, NoChecksum};
use thiserror::Error;

use crate::nuts::nut31::{PayjoinV2, PayjoinV2KeyError};

/// Number of satoshis in one bitcoin.
pub const SATS_PER_BTC: u64 = 100_000_000;
/// Extra JSON key used for Payjoin v2 parameters.
pub const ONCHAIN_PAYJOIN_EXTRA_KEY: &str = "payjoin";
/// Internal extra JSON key used to persist destination Payjoin v2 parameters
/// for onchain melt recovery.
pub const ONCHAIN_PAYJOIN_DESTINATION_EXTRA_KEY: &str = "payjoin_destination";

/// Errors for converting BIP21 BTC amount strings to satoshis.
#[derive(Debug, Error)]
pub enum Bip21AmountError {
    /// Whole BTC part could not be parsed.
    #[error("invalid BIP21 amount whole BTC value '{amount}': {source}")]
    InvalidWhole {
        /// Original BIP21 amount string.
        amount: String,
        /// Integer parsing source error.
        source: ParseIntError,
    },
    /// Fractional BTC part could not be parsed.
    #[error("invalid BIP21 amount fractional BTC value '{amount}': {source}")]
    InvalidFractional {
        /// Original BIP21 amount string.
        amount: String,
        /// Integer parsing source error.
        source: ParseIntError,
    },
    /// BIP21 amount contains more precision than satoshis allow.
    #[error("BIP21 amount has more than 8 decimal places: {amount}")]
    TooPrecise {
        /// Original BIP21 amount string.
        amount: String,
    },
    /// BIP21 amount cannot fit in a u64 satoshi value.
    #[error("BIP21 amount is too large to convert to satoshis: {amount}")]
    AmountOverflow {
        /// Original BIP21 amount string.
        amount: String,
    },
}

/// Errors for BIP77 Payjoin v2 parameter conversion.
#[derive(Debug, Error)]
pub enum PayjoinV2Error {
    /// Endpoint URL failed to parse.
    #[error("invalid Payjoin endpoint URL: {0}")]
    InvalidEndpoint(#[from] url::ParseError),
    /// Endpoint fragment contains both `+` and `-` delimiters.
    #[error("ambiguous Payjoin fragment delimiter")]
    AmbiguousFragmentDelimiter,
    /// Endpoint fragment parameter is missing.
    #[error("Payjoin URI is missing {prefix} fragment parameter")]
    MissingFragmentParam {
        /// Missing fragment parameter prefix.
        prefix: &'static str,
    },
    /// Fragment value is missing the expected prefix.
    #[error("Payjoin fragment value is missing {prefix} prefix")]
    MissingFragmentPrefix {
        /// Missing fragment parameter prefix.
        prefix: &'static str,
    },
    /// Expiry fragment has the wrong HRP.
    #[error("invalid EX1 expiry prefix")]
    InvalidExpiryPrefix,
    /// Expiry fragment has an invalid character.
    #[error("invalid EX1 expiry character: {0}")]
    InvalidExpiryCharacter(char),
    /// Expiry fragment decodes to the wrong number of bytes.
    #[error("invalid EX1 expiry length: {0}")]
    InvalidExpiryLength(usize),
    /// Expiry fragment contains non-zero padding bits.
    #[error("invalid EX1 expiry padding")]
    InvalidExpiryPadding,
    /// Expiry timestamp cannot fit in the BIP77 u32 timestamp encoding.
    #[error("Payjoin expiry exceeds BIP77 u32 range: {0}")]
    ExpiryOutOfRange(u64),
    /// Payjoin key material is invalid.
    #[error("{0}")]
    InvalidKey(#[from] PayjoinV2KeyError),
}

/// Read Cashu Payjoin v2 parameters from an `extra_json` object.
pub fn payjoin_v2_from_extra_json(extra_json: Option<&serde_json::Value>) -> Option<PayjoinV2> {
    extra_json
        .and_then(|extra| extra.get(ONCHAIN_PAYJOIN_EXTRA_KEY))
        .cloned()
        .and_then(|payjoin| serde_json::from_value(payjoin).ok())
}

/// Format a satoshi amount as a BIP21 BTC decimal string.
pub fn format_bip21_amount_from_sats(amount_sat: u64) -> String {
    let btc = amount_sat / SATS_PER_BTC;
    let sats = amount_sat % SATS_PER_BTC;
    if sats == 0 {
        return btc.to_string();
    }

    format!("{btc}.{sats:08}").trim_end_matches('0').to_string()
}

/// Parse a BIP21 BTC decimal amount string into satoshis.
pub fn parse_bip21_amount_to_sats(amount: &str) -> Result<u64, Bip21AmountError> {
    let (whole, fractional) = amount.split_once('.').unwrap_or((amount, ""));
    let whole_sat = whole
        .parse::<u64>()
        .map_err(|source| Bip21AmountError::InvalidWhole {
            amount: amount.to_string(),
            source,
        })?
        .checked_mul(SATS_PER_BTC)
        .ok_or_else(|| Bip21AmountError::AmountOverflow {
            amount: amount.to_string(),
        })?;

    if fractional.len() > 8 {
        return Err(Bip21AmountError::TooPrecise {
            amount: amount.to_string(),
        });
    }

    let mut padded_fractional = fractional.to_string();
    while padded_fractional.len() < 8 {
        padded_fractional.push('0');
    }

    let fractional_sat = if padded_fractional.is_empty() {
        0
    } else {
        padded_fractional
            .parse::<u64>()
            .map_err(|source| Bip21AmountError::InvalidFractional {
                amount: amount.to_string(),
                source,
            })?
    };

    whole_sat
        .checked_add(fractional_sat)
        .ok_or_else(|| Bip21AmountError::AmountOverflow {
            amount: amount.to_string(),
        })
}

/// Parse Cashu Payjoin v2 parameters from a BIP77 mailbox endpoint.
///
/// BIP77 encodes expiry in the `EX1...` fragment. Cashu uses Unix timestamp;
/// BIP77 URI fragments use encoded `EX1`, so this normalizes `EX1...` into
/// [`PayjoinV2::expires_at`].
pub fn payjoin_v2_from_bip77_endpoint(endpoint: &str) -> Result<PayjoinV2, PayjoinV2Error> {
    let mut endpoint_url = url::Url::parse(endpoint)?;
    let ohttp_keys = extract_payjoin_fragment_value(&endpoint_url, "OH1")?;
    let receiver_key = extract_payjoin_fragment_value(&endpoint_url, "RK1")?;
    let expires_at = extract_payjoin_fragment_value(&endpoint_url, "EX1")?;
    let expires_at = decode_bip77_expiry(&expires_at)?;
    endpoint_url.set_fragment(None);

    PayjoinV2::new(
        endpoint_url.to_string(),
        strip_fragment_prefix(&ohttp_keys, "OH1")?,
        strip_fragment_prefix(&receiver_key, "RK1")?,
        expires_at,
    )
    .map_err(Into::into)
}

/// Build a BIP77 mailbox endpoint with `EX1`, `OH1`, and `RK1` fragment parameters.
///
/// Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`. This
/// conversion is for payjoin-library sender/integration calls that require a
/// BIP21/BIP77-style `pj` URI.
pub fn payjoin_v2_to_bip77_endpoint(payjoin: &PayjoinV2) -> Result<String, PayjoinV2Error> {
    let mut endpoint = url::Url::parse(&payjoin.endpoint)?;
    endpoint.set_fragment(Some(&format!(
        "{}-OH1{}-RK1{}",
        encode_bip77_expiry(payjoin.expires_at)?,
        payjoin.ohttp_keys,
        payjoin.receiver_key
    )));
    Ok(endpoint.to_string())
}

/// Returns true when the Payjoin parameters are expired at `now`.
pub fn payjoin_v2_is_expired_at(payjoin: &PayjoinV2, now: u64) -> bool {
    now >= payjoin.expires_at
}

/// Decode a BIP77 `EX1` expiry fragment parameter into a Unix timestamp.
fn decode_bip77_expiry(value: &str) -> Result<u64, PayjoinV2Error> {
    let hrp_string = CheckedHrpstring::new::<NoChecksum>(value).map_err(map_bech32_error)?;
    if hrp_string.hrp() != expiry_hrp()? {
        return Err(PayjoinV2Error::InvalidExpiryPrefix);
    }

    let bytes = hrp_string.byte_iter().collect::<Vec<u8>>();
    if bytes.len() != 4 {
        return Err(PayjoinV2Error::InvalidExpiryLength(bytes.len()));
    }

    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64)
}

/// Encode a Unix timestamp as a BIP77 `EX1` expiry fragment parameter.
fn encode_bip77_expiry(expires_at: u64) -> Result<String, PayjoinV2Error> {
    let expires_at =
        u32::try_from(expires_at).map_err(|_| PayjoinV2Error::ExpiryOutOfRange(expires_at))?;

    bech32::encode_upper::<NoChecksum>(expiry_hrp()?, &expires_at.to_le_bytes())
        .map_err(|_| PayjoinV2Error::InvalidExpiryPrefix)
}

fn expiry_hrp() -> Result<Hrp, PayjoinV2Error> {
    Hrp::parse("EX").map_err(|_| PayjoinV2Error::InvalidExpiryPrefix)
}

fn map_bech32_error(error: CheckedHrpstringError) -> PayjoinV2Error {
    match error {
        CheckedHrpstringError::Parse(UncheckedHrpstringError::Char(CharError::InvalidChar(ch))) => {
            PayjoinV2Error::InvalidExpiryCharacter(ch)
        }
        CheckedHrpstringError::Parse(UncheckedHrpstringError::Char(
            CharError::MissingSeparator | CharError::NothingAfterSeparator | CharError::MixedCase,
        ))
        | CheckedHrpstringError::Parse(UncheckedHrpstringError::Hrp(_))
        | CheckedHrpstringError::Checksum(_) => PayjoinV2Error::InvalidExpiryPrefix,
        _ => PayjoinV2Error::InvalidExpiryPrefix,
    }
}

fn extract_payjoin_fragment_value(
    endpoint_url: &url::Url,
    prefix: &'static str,
) -> Result<String, PayjoinV2Error> {
    let fragment = endpoint_url
        .fragment()
        .ok_or(PayjoinV2Error::MissingFragmentParam { prefix })?;
    if fragment.contains('+') && fragment.contains('-') {
        return Err(PayjoinV2Error::AmbiguousFragmentDelimiter);
    }
    let delimiter = if fragment.contains('+') { '+' } else { '-' };

    fragment
        .split(delimiter)
        .find(|part| part.starts_with(prefix))
        .map(|part| part.to_string())
        .ok_or(PayjoinV2Error::MissingFragmentParam { prefix })
}

fn strip_fragment_prefix<'a>(
    value: &'a str,
    prefix: &'static str,
) -> Result<&'a str, PayjoinV2Error> {
    value
        .strip_prefix(prefix)
        .ok_or(PayjoinV2Error::MissingFragmentPrefix { prefix })
}

#[cfg(test)]
mod tests {
    use super::{
        decode_bip77_expiry, encode_bip77_expiry, format_bip21_amount_from_sats,
        parse_bip21_amount_to_sats, payjoin_v2_from_bip77_endpoint, payjoin_v2_from_extra_json,
        payjoin_v2_is_expired_at, payjoin_v2_to_bip77_endpoint, Bip21AmountError,
        ONCHAIN_PAYJOIN_EXTRA_KEY,
    };
    use crate::nuts::nut31::PayjoinV2;

    const OHTTP_KEYS: &str = "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ";
    const RECEIVER_KEY: &str = "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG";

    #[test]
    fn bip77_expiry_roundtrips() {
        assert_eq!(encode_bip77_expiry(1_720_547_781).unwrap(), "EX1C4UC6ES");
        assert_eq!(decode_bip77_expiry("EX1C4UC6ES").unwrap(), 1_720_547_781);
    }

    #[test]
    fn rejects_malformed_bip77_expiry() {
        assert!(matches!(
            decode_bip77_expiry("EY1C4UC6ES"),
            Err(super::PayjoinV2Error::InvalidExpiryPrefix)
        ));
        assert!(matches!(
            decode_bip77_expiry("EX1*"),
            Err(super::PayjoinV2Error::InvalidExpiryCharacter('*'))
        ));
        assert!(matches!(
            decode_bip77_expiry("EX1Q"),
            Err(super::PayjoinV2Error::InvalidExpiryLength(0))
        ));
        assert!(matches!(
            encode_bip77_expiry(u64::from(u32::MAX) + 1),
            Err(super::PayjoinV2Error::ExpiryOutOfRange(value)) if value == u64::from(u32::MAX) + 1
        ));
    }

    #[test]
    fn parses_bip77_endpoint_into_cashu_fields() {
        let payjoin = payjoin_v2_from_bip77_endpoint(
            "HTTPS://PAYJO.IN/E73HSW759WNES#EX12XHZ26S-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
        )
        .unwrap();

        assert_eq!(payjoin.endpoint, "https://payjo.in/E73HSW759WNES");
        assert_eq!(payjoin.ohttp_keys.to_string(), OHTTP_KEYS);
        assert_eq!(payjoin.receiver_key.to_string(), RECEIVER_KEY);
        assert_eq!(payjoin.expires_at, 1_780_854_353);
    }

    #[test]
    fn builds_bip77_endpoint_from_cashu_fields() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            1_720_547_781,
        )
        .expect("valid Payjoin keys");

        assert_eq!(
            payjoin_v2_to_bip77_endpoint(&payjoin).unwrap(),
            "https://payjoin.example/pj#EX1C4UC6ES-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
    }

    #[test]
    fn reads_payjoin_from_extra_json() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            1_720_547_781,
        )
        .expect("valid Payjoin keys");
        let extra = serde_json::json!({ ONCHAIN_PAYJOIN_EXTRA_KEY: payjoin.clone() });

        assert_eq!(payjoin_v2_from_extra_json(Some(&extra)).unwrap(), payjoin);
        assert_eq!(payjoin_v2_from_extra_json(None), None);
    }

    #[test]
    fn formats_bip21_amount_from_sats() {
        assert_eq!(format_bip21_amount_from_sats(0), "0");
        assert_eq!(format_bip21_amount_from_sats(1), "0.00000001");
        assert_eq!(format_bip21_amount_from_sats(100_000_000), "1");
        assert_eq!(format_bip21_amount_from_sats(123_456_780), "1.2345678");
    }

    #[test]
    fn parses_bip21_amount_to_sats() {
        assert_eq!(parse_bip21_amount_to_sats("0").unwrap(), 0);
        assert_eq!(parse_bip21_amount_to_sats("0.00000001").unwrap(), 1);
        assert_eq!(parse_bip21_amount_to_sats("1").unwrap(), 100_000_000);
        assert_eq!(
            parse_bip21_amount_to_sats("1.2345678").unwrap(),
            123_456_780
        );
        assert!(matches!(
            parse_bip21_amount_to_sats("1.000000001"),
            Err(Bip21AmountError::TooPrecise { .. })
        ));
    }

    #[test]
    fn detects_expired_payjoin() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            10,
        )
        .expect("valid Payjoin keys");

        assert!(!payjoin_v2_is_expired_at(&payjoin, 9));
        assert!(payjoin_v2_is_expired_at(&payjoin, 10));
    }
}
