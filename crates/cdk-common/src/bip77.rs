//! BIP21 and BIP77 helpers for Payjoin integrations.

use cashu::bip77::{
    decode_bech32_fragment, encode_bech32_fragment, PayjoinKeyError, PayjoinReceiver,
};
use thiserror::Error;

use crate::bitcoin_uri::{
    format_bip21_amount_from_sats, normalize_bip21_address, parse_bip21_amount_to_sats,
    uppercase_bip21_address, Bip21AmountError,
};
/// Parsed BIP21 payment URI with optional Payjoin v2 parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bip21PayjoinUri {
    /// Normalized onchain address.
    pub address: String,
    /// Optional BIP21 amount in satoshis.
    pub amount_sat: Option<u64>,
    /// Optional Payjoin v2 receiver parameters.
    pub payjoin: Option<PayjoinReceiver>,
    /// Optional BIP21 `pjos` output-substitution preference.
    pub pjos: Option<bool>,
}

/// Errors while building or parsing a BIP21 Payjoin URI.
#[derive(Debug, Error)]
pub enum Bip21PayjoinUriError {
    /// URI parsing failed.
    #[error("invalid BIP21 URI: {0}")]
    InvalidUri(#[from] url::ParseError),
    /// URI scheme is not `bitcoin`.
    #[error("expected bitcoin URI scheme, found '{0}'")]
    InvalidScheme(String),
    /// URI has no onchain address.
    #[error("bitcoin URI is missing an onchain address")]
    MissingAddress,
    /// BIP21 amount is invalid.
    #[error(transparent)]
    InvalidAmount(#[from] Bip21AmountError),
    /// BIP21 `pjos` is not `0` or `1`.
    #[error("invalid pjos value '{0}', expected 0 or 1")]
    InvalidPjos(String),
    /// BIP21 URI contains a required parameter that this parser does not support.
    #[error("unsupported required BIP21 parameter '{parameter}'")]
    UnsupportedRequiredParameter {
        /// Required query parameter name.
        parameter: String,
    },
    /// Payjoin v2 endpoint conversion failed.
    #[error(transparent)]
    InvalidPayjoin(#[from] PayjoinReceiverError),
}

/// Errors for BIP77 Payjoin v2 parameter conversion.
#[derive(Debug, Error)]
pub enum PayjoinReceiverError {
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
    InvalidKey(#[from] PayjoinKeyError),
}

/// Build a BIP21 URI containing Payjoin v2 receiver parameters.
pub fn build_bip21_payjoin_uri(
    address: &str,
    amount_sat: Option<u64>,
    payjoin: &PayjoinReceiver,
    pjos: Option<bool>,
) -> Result<String, Bip21PayjoinUriError> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(amount_sat) = amount_sat {
        serializer.append_pair("amount", &format_bip21_amount_from_sats(amount_sat));
    }
    if let Some(pjos) = pjos {
        serializer.append_pair("pjos", if pjos { "1" } else { "0" });
    }
    serializer.append_pair("pj", &build_receiver_endpoint(payjoin)?);

    Ok(format!(
        "BITCOIN:{}?{}",
        uppercase_bip21_address(address),
        serializer.finish()
    ))
}

/// Parse a BIP21 URI and its optional Payjoin v2 receiver parameters.
pub fn parse_bip21_payjoin_uri(value: &str) -> Result<Bip21PayjoinUri, Bip21PayjoinUriError> {
    let uri = url::Url::parse(value)?;
    if !uri.scheme().eq_ignore_ascii_case("bitcoin") {
        return Err(Bip21PayjoinUriError::InvalidScheme(
            uri.scheme().to_string(),
        ));
    }

    let address = normalize_bip21_address(uri.path());
    if address.is_empty() {
        return Err(Bip21PayjoinUriError::MissingAddress);
    }

    let mut amount_sat = None;
    let mut endpoint = None;
    let mut pjos = None;
    for (key, value) in uri.query_pairs() {
        match key.as_ref() {
            "amount" => amount_sat = Some(parse_bip21_amount_to_sats(&value)?),
            "pj" => endpoint = Some(value.into_owned()),
            "pjos" => {
                pjos = Some(match value.as_ref() {
                    "0" => false,
                    "1" => true,
                    invalid => {
                        return Err(Bip21PayjoinUriError::InvalidPjos(invalid.to_string()));
                    }
                });
            }
            _ if key.starts_with("req-") => {
                return Err(Bip21PayjoinUriError::UnsupportedRequiredParameter {
                    parameter: key.into_owned(),
                });
            }
            _ => {}
        }
    }

    let payjoin = endpoint
        .as_deref()
        .map(parse_receiver_endpoint)
        .transpose()?;

    Ok(Bip21PayjoinUri {
        address,
        amount_sat,
        payjoin,
        pjos,
    })
}

/// Parse receiver parameters from a BIP77 mailbox endpoint.
///
/// This decodes the `EX1` expiry fragment into
/// [`PayjoinReceiver::expires_at`].
pub fn parse_receiver_endpoint(endpoint: &str) -> Result<PayjoinReceiver, PayjoinReceiverError> {
    let mut endpoint_url = url::Url::parse(endpoint)?;
    let ohttp_keys = extract_payjoin_fragment_value(&endpoint_url, "OH1")?;
    let receiver_key = extract_payjoin_fragment_value(&endpoint_url, "RK1")?;
    let expires_at = extract_payjoin_fragment_value(&endpoint_url, "EX1")?;
    let expires_at = decode_bip77_expiry(&expires_at)?;
    endpoint_url.set_fragment(None);

    PayjoinReceiver::new(
        endpoint_url.to_string(),
        strip_fragment_prefix(&ohttp_keys, "OH1")?,
        strip_fragment_prefix(&receiver_key, "RK1")?,
        expires_at,
    )
    .map_err(Into::into)
}

/// Build a BIP77 mailbox endpoint with `EX1`, `OH1`, and `RK1` fragment parameters.
///
/// This encodes the receiver expiry timestamp as an `EX1` fragment.
pub fn build_receiver_endpoint(payjoin: &PayjoinReceiver) -> Result<String, PayjoinReceiverError> {
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
pub fn receiver_is_expired_at(payjoin: &PayjoinReceiver, now: u64) -> bool {
    payjoin_expires_at_is_expired(payjoin.expires_at, now)
}

/// Returns true when a Payjoin expiry timestamp has been reached.
pub fn payjoin_expires_at_is_expired(expires_at: u64, now: u64) -> bool {
    now >= expires_at
}

/// Decode a BIP77 `EX1` expiry fragment parameter into a Unix timestamp.
fn decode_bip77_expiry(value: &str) -> Result<u64, PayjoinReceiverError> {
    let bytes = decode_bech32_fragment::<4>(value, "expiry", "EX").map_err(expiry_key_error)?;
    Ok(u32::from_le_bytes(bytes) as u64)
}

/// Encode a Unix timestamp as a BIP77 `EX1` expiry fragment parameter.
fn encode_bip77_expiry(expires_at: u64) -> Result<String, PayjoinReceiverError> {
    let expires_at = u32::try_from(expires_at)
        .map_err(|_| PayjoinReceiverError::ExpiryOutOfRange(expires_at))?;

    encode_bech32_fragment("expiry", "EX", &expires_at.to_le_bytes()).map_err(expiry_key_error)
}

fn expiry_key_error(error: PayjoinKeyError) -> PayjoinReceiverError {
    match error {
        PayjoinKeyError::InvalidCharacter { character, .. } => {
            PayjoinReceiverError::InvalidExpiryCharacter(character)
        }
        PayjoinKeyError::InvalidLength { actual, .. } => {
            PayjoinReceiverError::InvalidExpiryLength(actual)
        }
        PayjoinKeyError::InvalidPadding { .. } => PayjoinReceiverError::InvalidExpiryPadding,
        _ => PayjoinReceiverError::InvalidExpiryPrefix,
    }
}

fn extract_payjoin_fragment_value(
    endpoint_url: &url::Url,
    prefix: &'static str,
) -> Result<String, PayjoinReceiverError> {
    let fragment = endpoint_url
        .fragment()
        .ok_or(PayjoinReceiverError::MissingFragmentParam { prefix })?;
    if fragment.contains('+') && fragment.contains('-') {
        return Err(PayjoinReceiverError::AmbiguousFragmentDelimiter);
    }
    let delimiter = if fragment.contains('+') { '+' } else { '-' };

    fragment
        .split(delimiter)
        .find(|part| part.starts_with(prefix))
        .map(|part| part.to_string())
        .ok_or(PayjoinReceiverError::MissingFragmentParam { prefix })
}

fn strip_fragment_prefix<'a>(
    value: &'a str,
    prefix: &'static str,
) -> Result<&'a str, PayjoinReceiverError> {
    value
        .strip_prefix(prefix)
        .ok_or(PayjoinReceiverError::MissingFragmentPrefix { prefix })
}

#[cfg(test)]
mod tests {
    use super::{
        build_bip21_payjoin_uri, build_receiver_endpoint, decode_bip77_expiry, encode_bip77_expiry,
        parse_bip21_payjoin_uri, parse_receiver_endpoint, receiver_is_expired_at,
        Bip21PayjoinUriError,
    };
    use cashu::bip77::PayjoinReceiver;

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
            Err(super::PayjoinReceiverError::InvalidExpiryPrefix)
        ));
        assert!(matches!(
            decode_bip77_expiry("EX1*"),
            Err(super::PayjoinReceiverError::InvalidExpiryCharacter('*'))
        ));
        assert!(matches!(
            decode_bip77_expiry("EX1Q"),
            Err(super::PayjoinReceiverError::InvalidExpiryLength(0))
        ));
        assert!(matches!(
            decode_bip77_expiry("EX1C4UC6ET"),
            Err(super::PayjoinReceiverError::InvalidExpiryPadding)
        ));
        assert!(matches!(
            encode_bip77_expiry(u64::from(u32::MAX) + 1),
            Err(super::PayjoinReceiverError::ExpiryOutOfRange(value)) if value == u64::from(u32::MAX) + 1
        ));
    }

    #[test]
    fn parses_bip77_receiver_endpoint() {
        let payjoin = parse_receiver_endpoint(
            "HTTPS://PAYJO.IN/E73HSW759WNES#EX12XHZ26S-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
        )
        .unwrap();

        assert_eq!(payjoin.endpoint, "https://payjo.in/E73HSW759WNES");
        assert_eq!(payjoin.ohttp_keys.to_string(), OHTTP_KEYS);
        assert_eq!(payjoin.receiver_key.to_string(), RECEIVER_KEY);
        assert_eq!(payjoin.expires_at, 1_780_854_353);
    }

    #[test]
    fn builds_bip77_receiver_endpoint() {
        let payjoin = PayjoinReceiver::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            1_720_547_781,
        )
        .expect("valid Payjoin keys");

        assert_eq!(
            build_receiver_endpoint(&payjoin).unwrap(),
            "https://payjoin.example/pj#EX1C4UC6ES-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
    }

    #[test]
    fn detects_expired_payjoin() {
        let payjoin = PayjoinReceiver::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            10,
        )
        .expect("valid Payjoin keys");

        assert!(!receiver_is_expired_at(&payjoin, 9));
        assert!(receiver_is_expired_at(&payjoin, 10));
    }

    #[test]
    fn bip21_payjoin_uri_roundtrips() {
        let payjoin = PayjoinReceiver::new(
            "https://payjoin.example/pj".to_string(),
            OHTTP_KEYS,
            RECEIVER_KEY,
            1_720_547_781,
        )
        .expect("valid Payjoin keys");
        let uri = build_bip21_payjoin_uri(
            "bcrt1q6d3a2w975yny0asuvd9a67ner4nks58ff0q8g4",
            Some(50_000),
            &payjoin,
            Some(false),
        )
        .expect("build Payjoin URI");

        assert!(uri.starts_with("BITCOIN:BCRT1Q6D3A2W975YNY0ASUVD9A67NER4NKS58FF0Q8G4?"));
        assert_eq!(
            parse_bip21_payjoin_uri(&uri).expect("parse Payjoin URI"),
            super::Bip21PayjoinUri {
                address: "bcrt1q6d3a2w975yny0asuvd9a67ner4nks58ff0q8g4".to_string(),
                amount_sat: Some(50_000),
                payjoin: Some(payjoin),
                pjos: Some(false),
            }
        );
    }

    #[test]
    fn bip21_uri_without_payjoin_is_supported() {
        let parsed = parse_bip21_payjoin_uri("bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=1")
            .expect("parse BIP21 URI");

        assert_eq!(parsed.address, "12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX");
        assert_eq!(parsed.amount_sat, Some(100_000_000));
        assert_eq!(parsed.payjoin, None);
        assert_eq!(parsed.pjos, None);
    }

    #[test]
    fn bip21_uri_rejects_invalid_pjos() {
        let err = parse_bip21_payjoin_uri("bitcoin:bc1qexample?pjos=2")
            .expect_err("invalid pjos must fail");

        assert!(matches!(err, Bip21PayjoinUriError::InvalidPjos(value) if value == "2"));
    }

    #[test]
    fn bip21_uri_rejects_unsupported_required_parameters() {
        for parameter in ["req-pj", "req-unknown", "req%2Dpj"] {
            let uri = format!(
                "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=1&{parameter}=https%3A%2F%2Fexample.com"
            );
            assert!(matches!(
                parse_bip21_payjoin_uri(&uri),
                Err(Bip21PayjoinUriError::UnsupportedRequiredParameter { parameter })
                    if parameter == "req-pj" || parameter == "req-unknown"
            ));
        }
    }

    #[test]
    fn bip21_uri_ignores_unknown_optional_parameters() {
        let parsed = parse_bip21_payjoin_uri(
            "bitcoin:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?amount=1&unknown=value",
        )
        .expect("unknown optional parameters are allowed");

        assert_eq!(parsed.amount_sat, Some(100_000_000));
        assert_eq!(parsed.payjoin, None);
    }
}
