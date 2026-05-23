//! Payjoin helper functions for CDK integrations.
//!
//! Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`.

use bitcoin::{Amount, Denomination};
use thiserror::Error;

use crate::nuts::nut31::{
    decode_bech32_fragment, encode_bech32_fragment, PayjoinV2, PayjoinV2KeyError,
};

/// Extra JSON key used for Payjoin v2 parameters.
pub const ONCHAIN_PAYJOIN_EXTRA_KEY: &str = "payjoin";
/// Internal extra JSON key used to persist destination Payjoin v2 parameters
/// for onchain melt recovery.
pub const ONCHAIN_PAYJOIN_DESTINATION_EXTRA_KEY: &str = "payjoin_destination";

/// Error for converting BIP21 BTC amount strings to satoshis.
#[derive(Debug, Error)]
#[error("invalid BIP21 amount '{amount}': {source}")]
pub struct Bip21AmountError {
    /// Original BIP21 amount string.
    amount: String,
    /// Underlying amount parsing error.
    source: bitcoin::amount::ParseAmountError,
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
    Amount::from_sat(amount_sat).to_string_in(Denomination::Bitcoin)
}

/// Parse a BIP21 BTC decimal amount string into satoshis.
pub fn parse_bip21_amount_to_sats(amount: &str) -> Result<u64, Bip21AmountError> {
    Amount::from_str_in(amount, Denomination::Bitcoin)
        .map(Amount::to_sat)
        .map_err(|source| Bip21AmountError {
            amount: amount.to_string(),
            source,
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
    let bytes = decode_bech32_fragment::<4>(value, "expiry", "EX").map_err(expiry_key_error)?;
    Ok(u32::from_le_bytes(bytes) as u64)
}

/// Encode a Unix timestamp as a BIP77 `EX1` expiry fragment parameter.
fn encode_bip77_expiry(expires_at: u64) -> Result<String, PayjoinV2Error> {
    let expires_at =
        u32::try_from(expires_at).map_err(|_| PayjoinV2Error::ExpiryOutOfRange(expires_at))?;

    encode_bech32_fragment("expiry", "EX", &expires_at.to_le_bytes()).map_err(expiry_key_error)
}

fn expiry_key_error(error: PayjoinV2KeyError) -> PayjoinV2Error {
    match error {
        PayjoinV2KeyError::InvalidCharacter { character, .. } => {
            PayjoinV2Error::InvalidExpiryCharacter(character)
        }
        PayjoinV2KeyError::InvalidLength { actual, .. } => {
            PayjoinV2Error::InvalidExpiryLength(actual)
        }
        PayjoinV2KeyError::InvalidPadding { .. } => PayjoinV2Error::InvalidExpiryPadding,
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
        payjoin_v2_is_expired_at, payjoin_v2_to_bip77_endpoint, ONCHAIN_PAYJOIN_EXTRA_KEY,
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
        assert!(
            parse_bip21_amount_to_sats("1.000000001").is_err(),
            "sub-satoshi precision must be rejected"
        );
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
