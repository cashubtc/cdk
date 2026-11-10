//! Payjoin for onchain payment method

use core::fmt;
use core::str::FromStr;

use bitcoin::bech32::primitives::decode::{
    CharError, CheckedHrpstring, CheckedHrpstringError, UncheckedHrpstringError,
};
use bitcoin::bech32::{self, Hrp, NoChecksum};
use bitcoin::secp256k1;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const OHTTP_KEYS_PREFIX: &str = "OH1";
const OHTTP_KEYS_HRP: &str = "OH";
const OHTTP_KEYS_BYTES: usize = 34;
const RECEIVER_KEY_PREFIX: &str = "RK1";
const RECEIVER_KEY_HRP: &str = "RK";
const RECEIVER_KEY_BYTES: usize = 33;

/// Implement Display/FromStr/Serialize/Deserialize for a prefixless BIP77
/// fragment key newtype. Only the per-type public-key offset validation
/// differs between the key types.
macro_rules! impl_prefixless_key {
    ($ty:ident, $field:literal, $hrp:expr, $prefix:expr, $bytes:expr, $pubkey_offset:expr) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_prefixless_key(f, $field, $hrp, $prefix, &self.0)
            }
        }

        impl FromStr for $ty {
            type Err = PayjoinV2KeyError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let bytes = decode_prefixless_key::<{ $bytes }>(value, $field, $hrp, $prefix)?;
                validate_compressed_pubkey($field, &bytes[$pubkey_offset..])?;
                Ok(Self(bytes))
            }
        }

        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

/// Encoded OHTTP key material needed by the sender.
///
/// The wire representation is the BIP77 `OH` fragment value without the
/// `OH1` prefix. Internally this stores the decoded key identifier and
/// compressed secp256k1 public key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PayjoinOhttpKeys([u8; OHTTP_KEYS_BYTES]);

impl PayjoinOhttpKeys {
    /// Return decoded OHTTP key bytes.
    pub fn as_bytes(&self) -> &[u8; OHTTP_KEYS_BYTES] {
        &self.0
    }
}

// The first byte is the OHTTP key identifier; the pubkey follows it.
impl_prefixless_key!(
    PayjoinOhttpKeys,
    "ohttp_keys",
    OHTTP_KEYS_HRP,
    OHTTP_KEYS_PREFIX,
    OHTTP_KEYS_BYTES,
    1
);

/// Encoded receiver session key.
///
/// The wire representation is the BIP77 `RK` fragment value without the
/// `RK1` prefix. Internally this stores the decoded compressed secp256k1
/// public key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PayjoinReceiverKey([u8; RECEIVER_KEY_BYTES]);

impl PayjoinReceiverKey {
    /// Return decoded receiver public key bytes.
    pub fn as_bytes(&self) -> &[u8; RECEIVER_KEY_BYTES] {
        &self.0
    }
}

impl_prefixless_key!(
    PayjoinReceiverKey,
    "receiver_key",
    RECEIVER_KEY_HRP,
    RECEIVER_KEY_PREFIX,
    RECEIVER_KEY_BYTES,
    0
);

/// Errors for Payjoin v2 key encoding.
#[derive(Debug, Error)]
pub enum PayjoinV2KeyError {
    /// Key string included the BIP77 fragment prefix.
    #[error("{field} must not include the {prefix} prefix")]
    HasPrefix {
        /// Field name.
        field: &'static str,
        /// Disallowed prefix.
        prefix: &'static str,
    },
    /// Key string has an invalid bech32 prefix.
    #[error("{field} has invalid bech32 prefix")]
    InvalidPrefix {
        /// Field name.
        field: &'static str,
    },
    /// Key string has an invalid bech32 character.
    #[error("{field} has invalid bech32 character: {character}")]
    InvalidCharacter {
        /// Field name.
        field: &'static str,
        /// Invalid character.
        character: char,
    },
    /// Key string decodes to the wrong byte length.
    #[error("{field} has invalid decoded length: {actual}, expected {expected}")]
    InvalidLength {
        /// Field name.
        field: &'static str,
        /// Actual decoded byte length.
        actual: usize,
        /// Expected decoded byte length.
        expected: usize,
    },
    /// Key string contains non-zero padding bits.
    #[error("{field} has invalid bech32 padding")]
    InvalidPadding {
        /// Field name.
        field: &'static str,
    },
    /// Key string does not contain a valid compressed secp256k1 public key.
    #[error("{field} does not contain a valid compressed secp256k1 public key")]
    InvalidPublicKey {
        /// Field name.
        field: &'static str,
    },
}

/// Payjoin v2 parameters for an onchain payment.
///
/// Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayjoinV2 {
    /// BIP77 mailbox endpoint URL without receiver fragment parameters.
    ///
    /// When assembled into a `pj` URI parameter, the endpoint value must be
    /// encoded according to BIP77.
    pub endpoint: String,
    /// Encoded OHTTP key material needed by the sender, without the `OH1` prefix.
    pub ohttp_keys: PayjoinOhttpKeys,
    /// Encoded receiver session key, without the `RK1` prefix.
    pub receiver_key: PayjoinReceiverKey,
    /// Unix timestamp until the Payjoin parameters are valid.
    pub expires_at: u64,
}

impl PayjoinV2 {
    /// Construct Payjoin v2 parameters from encoded key strings.
    pub fn new<O, R>(
        endpoint: String,
        ohttp_keys: O,
        receiver_key: R,
        expires_at: u64,
    ) -> Result<Self, PayjoinV2KeyError>
    where
        O: AsRef<str>,
        R: AsRef<str>,
    {
        Ok(Self {
            endpoint,
            ohttp_keys: ohttp_keys.as_ref().parse()?,
            receiver_key: receiver_key.as_ref().parse()?,
            expires_at,
        })
    }
}

/// Decode an uppercase bech32 (`NoChecksum`) BIP77 fragment value like
/// `OH1…`/`RK1…`/`EX1…`, checking the HRP and expecting exactly `N` data bytes.
pub fn decode_bech32_fragment<const N: usize>(
    encoded: &str,
    field: &'static str,
    hrp: &'static str,
) -> Result<[u8; N], PayjoinV2KeyError> {
    let hrp_string = CheckedHrpstring::new::<NoChecksum>(encoded)
        .map_err(|error| map_bech32_error(field, error))?;
    let expected_hrp = Hrp::parse(hrp).map_err(|_| PayjoinV2KeyError::InvalidPrefix { field })?;
    if hrp_string.hrp() != expected_hrp {
        return Err(PayjoinV2KeyError::InvalidPrefix { field });
    }

    let bytes = hrp_string.byte_iter().collect::<Vec<u8>>();
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| PayjoinV2KeyError::InvalidLength {
            field,
            actual: bytes.len(),
            expected: N,
        })
}

/// Encode bytes as an uppercase bech32 (`NoChecksum`) BIP77 fragment value
/// like `EX1…`, including the HRP prefix.
pub fn encode_bech32_fragment(
    field: &'static str,
    hrp: &'static str,
    bytes: &[u8],
) -> Result<String, PayjoinV2KeyError> {
    let hrp = Hrp::parse(hrp).map_err(|_| PayjoinV2KeyError::InvalidPrefix { field })?;
    bech32::encode_upper::<NoChecksum>(hrp, bytes)
        .map_err(|_| PayjoinV2KeyError::InvalidPrefix { field })
}

fn decode_prefixless_key<const N: usize>(
    value: &str,
    field: &'static str,
    hrp: &'static str,
    prefix: &'static str,
) -> Result<[u8; N], PayjoinV2KeyError> {
    if value.starts_with(prefix) {
        return Err(PayjoinV2KeyError::HasPrefix { field, prefix });
    }

    decode_bech32_fragment::<N>(&format!("{prefix}{value}"), field, hrp)
}

fn write_prefixless_key(
    f: &mut fmt::Formatter<'_>,
    field: &'static str,
    hrp: &'static str,
    prefix: &'static str,
    bytes: &[u8],
) -> fmt::Result {
    let encoded = encode_bech32_fragment(field, hrp, bytes).map_err(|_| fmt::Error)?;
    let value = encoded.strip_prefix(prefix).ok_or(fmt::Error)?;
    f.write_str(value)
}

fn validate_compressed_pubkey(field: &'static str, bytes: &[u8]) -> Result<(), PayjoinV2KeyError> {
    secp256k1::PublicKey::from_slice(bytes)
        .map(|_| ())
        .map_err(|_| PayjoinV2KeyError::InvalidPublicKey { field })
}

fn map_bech32_error(field: &'static str, error: CheckedHrpstringError) -> PayjoinV2KeyError {
    match error {
        CheckedHrpstringError::Parse(UncheckedHrpstringError::Char(CharError::InvalidChar(
            character,
        ))) => PayjoinV2KeyError::InvalidCharacter { field, character },
        CheckedHrpstringError::Parse(UncheckedHrpstringError::Char(
            CharError::MissingSeparator | CharError::NothingAfterSeparator | CharError::MixedCase,
        ))
        | CheckedHrpstringError::Parse(UncheckedHrpstringError::Hrp(_))
        | CheckedHrpstringError::Checksum(_) => PayjoinV2KeyError::InvalidPrefix { field },
        _ => PayjoinV2KeyError::InvalidPadding { field },
    }
}
