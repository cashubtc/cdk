//! Errors

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::nuts::Id;
use crate::util::hex;
#[cfg(feature = "wallet")]
use crate::wallet::multi_mint_wallet::WalletKey;
use crate::Amount;

/// CDK Error
#[derive(Debug, Error)]
pub enum Error {
    /// Mint does not have a key for amount
    #[error("No Key for Amount")]
    AmountKey,
    /// Keyset is not known
    #[error("Keyset id not known: `{0}`")]
    KeysetUnknown(Id),
    /// Unsupported unit
    #[error("Unit unsupported")]
    UnsupportedUnit,
    /// Payment failed
    #[error("Payment failed")]
    PaymentFailed,
    /// Payment pending
    #[error("Payment pending")]
    PaymentPending,
    /// Invoice already paid
    #[error("Request already paid")]
    RequestAlreadyPaid,
    /// Invalid payment request
    #[error("Invalid payment request")]
    InvalidPaymentRequest,
    /// Bolt11 invoice does not have amount
    #[error("Invoice Amount undefined")]
    InvoiceAmountUndefined,
    /// Split Values must be less then or equal to amount
    #[error("Split Values must be less then or equal to amount")]
    SplitValuesGreater,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,

    // Mint Errors
    /// Minting is disabled
    #[error("Minting is disabled")]
    MintingDisabled,
    /// Quote is not known
    #[error("Unknown quote")]
    UnknownQuote,
    /// Quote is expired
    #[error("Expired quote: Expired: `{0}`, Time: `{1}`")]
    ExpiredQuote(u64, u64),
    /// Amount is outside of allowed range
    #[error("Amount but be between `{0}` and `{1}` is `{2}`")]
    AmountOutofLimitRange(Amount, Amount, Amount),
    /// Quote is not paiud
    #[error("Quote not paid")]
    UnpaidQuote,
    /// Quote is pending
    #[error("Quote pending")]
    PendingQuote,
    /// ecash already issued for quote
    #[error("Quote already issued")]
    IssuedQuote,
    /// Quote has already been paid
    #[error("Quote is already paid")]
    PaidQuote,
    /// Payment state is unknown
    #[error("Payment state is unknown")]
    UnknownPaymentState,
    /// Melting is disabled
    #[error("Minting is disabled")]
    MeltingDisabled,
    /// Unknown Keyset
    #[error("Unknown Keyset")]
    UnknownKeySet,
    /// BlindedMessage is already signed
    #[error("Blinded Message is already signed")]
    BlindedMessageAlreadySigned,
    /// Inactive Keyset
    #[error("Inactive Keyset")]
    InactiveKeyset,
    /// Transaction unbalanced
    #[error("Inputs: `{0}`, Outputs: `{1}`, Expected Fee: `{2}`")]
    TransactionUnbalanced(u64, u64, u64),
    /// Duplicate proofs provided
    #[error("Duplicate proofs")]
    DuplicateProofs,
    /// Multiple units provided
    #[error("Cannot have multiple units")]
    MultipleUnits,
    /// Sig all cannot be used in melt
    #[error("Sig all cannot be used in melt")]
    SigAllUsedInMelt,
    /// Token is already spent
    #[error("Token Already Spent")]
    TokenAlreadySpent,
    /// Token is already pending
    #[error("Token Pending")]
    TokenPending,
    /// Internal Error
    #[error("Internal Error")]
    Internal,

    // Wallet Errors
    /// P2PK spending conditions not met
    #[error("P2PK condition not met `{0}`")]
    P2PKConditionsNotMet(String),
    /// Spending Locktime not provided
    #[error("Spending condition locktime not provided")]
    LocktimeNotProvided,
    /// Invalid Spending Conditions
    #[error("Invalid spending conditions: `{0}`")]
    InvalidSpendConditions(String),
    /// Incorrect Wallet
    #[error("Incorrect wallet: `{0}`")]
    IncorrectWallet(String),
    /// Unknown Wallet
    #[cfg(feature = "wallet")]
    #[error("Unknown wallet: `{0}`")]
    UnknownWallet(WalletKey),
    /// Max Fee Ecxeded
    #[error("Max fee exceeded")]
    MaxFeeExceeded,
    /// Url path segments could not be joined
    #[error("Url path segments could not be joined")]
    UrlPathSegments,
    ///  Unknown error response
    #[error("Unknown error response: `{0}`")]
    UnknownErrorResponse(String),
    /// Invalid DLEQ proof
    #[error("Could not verify DLEQ proof")]
    CouldNotVerifyDleq,
    /// Incorrect Mint
    /// Token does not match wallet mint
    #[error("Token does not match wallet mint")]
    IncorrectMint,
    /// Receive can only be used with tokens from single mint
    #[error("Multiple mint tokens not supported by receive. Please deconstruct the token and use receive with_proof")]
    MultiMintTokenNotSupported,
    /// Unit Not supported
    #[error("Unit not supported for method")]
    UnitUnsupported,
    /// Preimage not provided
    #[error("Preimage not provided")]
    PreimageNotProvided,
    /// Insufficient Funds
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// No active keyset
    #[error("No active keyset")]
    NoActiveKeyset,
    /// Incorrect quote amount
    #[error("Incorrect quote amount")]
    IncorrectQuoteAmount,
    /// Invoice Description not supported
    #[error("Invoice Description not supported")]
    InvoiceDescriptionUnsupported,
    /// Custom Error
    #[error("`{0}`")]
    Custom(String),

    // External Error conversions
    /// Parse invoice error
    #[error(transparent)]
    Invoice(#[from] lightning_invoice::ParseOrSemanticError),
    /// Bip32 error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    #[cfg(feature = "wallet")]
    /// From hex error
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),

    // Crate error conversions
    /// Cashu Url Error
    #[error(transparent)]
    CashuUrl(#[from] crate::mint_url::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// Amount Error
    #[error(transparent)]
    AmountError(#[from] crate::amount::Error),
    /// DHKE Error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// Nut01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// NUT02 error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// NUT03 error
    #[error(transparent)]
    NUT03(#[from] crate::nuts::nut03::Error),
    /// NUT05 error
    #[error(transparent)]
    NUT05(#[from] crate::nuts::nut05::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// NUT12 Error
    #[error(transparent)]
    NUT12(#[from] crate::nuts::nut12::Error),
    /// NUT13 Error
    #[error(transparent)]
    NUT13(#[from] crate::nuts::nut13::Error),
    /// NUT14 Error
    #[error(transparent)]
    NUT14(#[from] crate::nuts::nut14::Error),
    /// NUT18 Error
    #[error(transparent)]
    NUT18(#[from] crate::nuts::nut18::Error),
    /// Database Error
    #[cfg(any(feature = "wallet", feature = "mint"))]
    #[error(transparent)]
    Database(#[from] crate::cdk_database::Error),
    /// Lightning Error
    #[cfg(feature = "mint")]
    #[error(transparent)]
    Lightning(#[from] crate::cdk_lightning::Error),
}

/// CDK Error Response
///
/// See NUT definition in [00](https://github.com/cashubtc/nuts/blob/main/00.md)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct ErrorResponse {
    /// Error Code
    pub code: ErrorCode,
    /// Human readable Text
    pub error: Option<String>,
    /// Longer human readable description
    pub detail: Option<String>,
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "code: {}, error: {}, detail: {}",
            self.code,
            self.error.clone().unwrap_or_default(),
            self.detail.clone().unwrap_or_default()
        )
    }
}

impl ErrorResponse {
    /// Create new [`ErrorResponse`]
    pub fn new(code: ErrorCode, error: Option<String>, detail: Option<String>) -> Self {
        Self {
            code,
            error,
            detail,
        }
    }

    /// Error response from json
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(json)?;

        Self::from_value(value)
    }

    /// Error response from json Value
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        match serde_json::from_value::<ErrorResponse>(value.clone()) {
            Ok(res) => Ok(res),
            Err(_) => Ok(Self {
                code: ErrorCode::Unknown(999),
                error: Some(value.to_string()),
                detail: None,
            }),
        }
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenAlreadySpent => ErrorResponse {
                code: ErrorCode::TokenAlreadySpent,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::UnsupportedUnit => ErrorResponse {
                code: ErrorCode::UnitUnsupported,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::PaymentFailed => ErrorResponse {
                code: ErrorCode::LightningError,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::RequestAlreadyPaid => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                error: Some("Invoice already paid.".to_string()),
                detail: None,
            },
            Error::TransactionUnbalanced(inputs_total, outputs_total, fee_expected) => {
                ErrorResponse {
                    code: ErrorCode::TransactionUnbalanced,
                    error: Some(format!(
                        "Inputs: {}, Outputs: {}, expected_fee: {}",
                        inputs_total, outputs_total, fee_expected,
                    )),
                    detail: Some("Transaction inputs should equal outputs less fee".to_string()),
                }
            }
            Error::MintingDisabled => ErrorResponse {
                code: ErrorCode::MintingDisabled,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::BlindedMessageAlreadySigned => ErrorResponse {
                code: ErrorCode::BlindedMessageAlreadySigned,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::InsufficientFunds => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::AmountOutofLimitRange(_min, _max, _amount) => ErrorResponse {
                code: ErrorCode::AmountOutofLimitRange,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::ExpiredQuote(_, _) => ErrorResponse {
                code: ErrorCode::QuoteExpired,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::PendingQuote => ErrorResponse {
                code: ErrorCode::QuotePending,
                error: Some(err.to_string()),
                detail: None,
            },
            Error::TokenPending => ErrorResponse {
                code: ErrorCode::TokenPending,
                error: Some(err.to_string()),
                detail: None,
            },
            _ => ErrorResponse {
                code: ErrorCode::Unknown(9999),
                error: Some(err.to_string()),
                detail: None,
            },
        }
    }
}

impl From<ErrorResponse> for Error {
    fn from(err: ErrorResponse) -> Error {
        match err.code {
            ErrorCode::TokenAlreadySpent => Self::TokenAlreadySpent,
            ErrorCode::QuoteNotPaid => Self::UnpaidQuote,
            ErrorCode::QuotePending => Self::PendingQuote,
            ErrorCode::QuoteExpired => Self::ExpiredQuote(0, 0),
            ErrorCode::KeysetNotFound => Self::UnknownKeySet,
            ErrorCode::KeysetInactive => Self::InactiveKeyset,
            ErrorCode::BlindedMessageAlreadySigned => Self::BlindedMessageAlreadySigned,
            ErrorCode::UnitUnsupported => Self::UnitUnsupported,
            ErrorCode::TransactionUnbalanced => Self::TransactionUnbalanced(0, 0, 0),
            ErrorCode::MintingDisabled => Self::MintingDisabled,
            ErrorCode::InvoiceAlreadyPaid => Self::RequestAlreadyPaid,
            ErrorCode::TokenNotVerified => Self::DHKE(crate::dhke::Error::TokenNotVerified),
            ErrorCode::LightningError => Self::PaymentFailed,
            ErrorCode::AmountOutofLimitRange => {
                Self::AmountOutofLimitRange(Amount::default(), Amount::default(), Amount::default())
            }
            ErrorCode::TokenPending => Self::TokenPending,
            _ => Self::UnknownErrorResponse(err.to_string()),
        }
    }
}

/// Possible Error Codes
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum ErrorCode {
    /// Token is already spent
    TokenAlreadySpent,
    /// Token Pending
    TokenPending,
    /// Quote is not paid
    QuoteNotPaid,
    /// Quote is not expired
    QuoteExpired,
    /// Quote Pending
    QuotePending,
    /// Keyset is not found
    KeysetNotFound,
    /// Keyset inactive
    KeysetInactive,
    /// Blinded Message Already signed
    BlindedMessageAlreadySigned,
    /// Unsupported unit
    UnitUnsupported,
    /// Token already issed for quote
    TokensAlreadyIssued,
    /// Minting Disabled
    MintingDisabled,
    /// Invoice Already Paid
    InvoiceAlreadyPaid,
    /// Token Not Verified
    TokenNotVerified,
    /// Lightning Error
    LightningError,
    /// Unbalanced Error
    TransactionUnbalanced,
    /// Amount outside of allowed range
    AmountOutofLimitRange,
    /// Unknown error code
    Unknown(u16),
}

impl ErrorCode {
    /// Error code from u16
    pub fn from_code(code: u16) -> Self {
        match code {
            10002 => Self::BlindedMessageAlreadySigned,
            10003 => Self::TokenNotVerified,
            11001 => Self::TokenAlreadySpent,
            11002 => Self::TransactionUnbalanced,
            11005 => Self::UnitUnsupported,
            11006 => Self::AmountOutofLimitRange,
            11007 => Self::TokenPending,
            12001 => Self::KeysetNotFound,
            12002 => Self::KeysetInactive,
            20000 => Self::LightningError,
            20001 => Self::QuoteNotPaid,
            20002 => Self::TokensAlreadyIssued,
            20003 => Self::MintingDisabled,
            20005 => Self::QuotePending,
            20006 => Self::InvoiceAlreadyPaid,
            20007 => Self::QuoteExpired,
            _ => Self::Unknown(code),
        }
    }

    /// Error code to u16
    pub fn to_code(&self) -> u16 {
        match self {
            Self::BlindedMessageAlreadySigned => 10002,
            Self::TokenNotVerified => 10003,
            Self::TokenAlreadySpent => 11001,
            Self::TransactionUnbalanced => 11002,
            Self::UnitUnsupported => 11005,
            Self::AmountOutofLimitRange => 11006,
            Self::TokenPending => 11007,
            Self::KeysetNotFound => 12001,
            Self::KeysetInactive => 12002,
            Self::LightningError => 20000,
            Self::QuoteNotPaid => 20001,
            Self::TokensAlreadyIssued => 20002,
            Self::MintingDisabled => 20003,
            Self::QuotePending => 20005,
            Self::InvoiceAlreadyPaid => 20006,
            Self::QuoteExpired => 20007,
            Self::Unknown(code) => *code,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.to_code())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = u16::deserialize(deserializer)?;

        Ok(ErrorCode::from_code(code))
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_code())
    }
}
