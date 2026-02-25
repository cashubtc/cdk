//! Errors

use std::array::TryFromSliceError;
use std::fmt;

use cashu::{CurrencyUnit, PaymentMethod};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::nuts::Id;
use crate::util::hex;
#[cfg(feature = "wallet")]
use crate::wallet::WalletKey;
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
    /// Over issue - tried to issue more than paid
    #[error("Cannot issue more than amount paid")]
    OverIssue,
    /// Witness missing or invalid
    #[error("Signature missing or invalid")]
    SignatureMissingOrInvalid,
    /// Amountless Invoice Not supported
    #[error("Amount Less Invoice is not allowed")]
    AmountLessNotAllowed,
    /// Multi-Part Internal Melt Quotes are not supported
    #[error("Multi-Part Internal Melt Quotes are not supported")]
    InternalMultiPartMeltQuote,
    /// Multi-Part Payment not supported for unit and method
    #[error("Multi-Part payment is not supported for unit `{0}` and method `{1}`")]
    MppUnitMethodNotSupported(CurrencyUnit, PaymentMethod),
    /// Clear Auth Required
    #[error("Clear Auth Required")]
    ClearAuthRequired,
    /// Blind Auth Required
    #[error("Blind Auth Required")]
    BlindAuthRequired,
    /// Clear Auth Failed
    #[error("Clear Auth Failed")]
    ClearAuthFailed,
    /// Blind Auth Failed
    #[error("Blind Auth Failed")]
    BlindAuthFailed,
    /// Auth settings undefined
    #[error("Auth settings undefined")]
    AuthSettingsUndefined,
    /// Mint time outside of tolerance
    #[error("Mint time outside of tolerance")]
    MintTimeExceedsTolerance,
    /// Insufficient blind auth tokens
    #[error("Insufficient blind auth tokens, must reauth")]
    InsufficientBlindAuthTokens,
    /// Auth localstore undefined
    #[error("Auth localstore undefined")]
    AuthLocalstoreUndefined,
    /// Wallet cat not set
    #[error("Wallet cat not set")]
    CatNotSet,
    /// Could not get mint info
    #[error("Could not get mint info")]
    CouldNotGetMintInfo,
    /// Multi-Part Payment not supported for unit and method
    #[error("Amountless invoices are not supported for unit `{0}` and method `{1}`")]
    AmountlessInvoiceNotSupported(CurrencyUnit, PaymentMethod),
    /// Duplicate Payment id
    #[error("Payment id seen for mint")]
    DuplicatePaymentId,
    /// Pubkey required
    #[error("Pubkey required")]
    PubkeyRequired,
    /// Invalid payment method
    #[error("Invalid payment method")]
    InvalidPaymentMethod,
    /// Amount undefined
    #[error("Amount undefined")]
    AmountUndefined,
    /// Unsupported payment method
    #[error("Payment method unsupported")]
    UnsupportedPaymentMethod,
    /// Payment method required
    #[error("Payment method required")]
    PaymentMethodRequired,
    /// Could not parse bolt12
    #[error("Could not parse bolt12")]
    Bolt12parse,
    /// Could not parse invoice (bolt11 or bolt12)
    #[error("Could not parse invoice")]
    InvalidInvoice,

    /// BIP353 address parsing error
    #[error("Failed to parse BIP353 address: {0}")]
    Bip353Parse(String),

    /// Operation timeout
    #[error("Operation timeout")]
    Timeout,

    /// BIP353 address resolution error
    #[error("Failed to resolve BIP353 address: {0}")]
    Bip353Resolve(String),
    /// BIP353 no Lightning offer found
    #[error("No Lightning offer found in BIP353 payment instructions")]
    Bip353NoLightningOffer,

    /// Lightning Address parsing error
    #[error("Failed to parse Lightning address: {0}")]
    LightningAddressParse(String),
    /// Lightning Address request error
    #[error("Failed to request invoice from Lightning address service: {0}")]
    LightningAddressRequest(String),

    /// Internal Error - Send error
    #[error("Internal send error: {0}")]
    SendError(String),

    /// Internal Error - Recv error
    #[error("Internal receive error: {0}")]
    RecvError(String),

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
    #[error("Amount must be between `{0}` and `{1}` is `{2}`")]
    AmountOutofLimitRange(Amount, Amount, Amount),
    /// Quote is not paid
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
    #[error("Melting is disabled")]
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
    #[error("Duplicate Inputs")]
    DuplicateInputs,
    /// Duplicate output
    #[error("Duplicate outputs")]
    DuplicateOutputs,
    /// Maximum number of inputs exceeded
    #[error("Maximum inputs exceeded: {actual} provided, max {max}")]
    MaxInputsExceeded {
        /// Actual number of inputs provided
        actual: usize,
        /// Maximum allowed inputs
        max: usize,
    },
    /// Maximum number of outputs exceeded
    #[error("Maximum outputs exceeded: {actual} provided, max {max}")]
    MaxOutputsExceeded {
        /// Actual number of outputs provided
        actual: usize,
        /// Maximum allowed outputs
        max: usize,
    },
    /// Proof content too large (secret or witness exceeds max length)
    #[error("Proof content too large: {actual} bytes, max {max}")]
    ProofContentTooLarge {
        /// Actual size in bytes
        actual: usize,
        /// Maximum allowed size in bytes
        max: usize,
    },
    /// Request field content too large (description or extra exceeds max length)
    #[error("Request field '{field}' too large: {actual} bytes, max {max}")]
    RequestFieldTooLarge {
        /// Name of the field that exceeded the limit
        field: String,
        /// Actual size in bytes
        actual: usize,
        /// Maximum allowed size in bytes
        max: usize,
    },
    /// Multiple units provided
    #[error("Cannot have multiple units")]
    MultipleUnits,
    /// Unit mismatch
    #[error("Input unit must match output")]
    UnitMismatch,
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
    /// Oidc config not set
    #[error("Oidc client not set")]
    OidcNotSet,
    /// Unit String collision
    #[error("Unit string picked collided: `{0}`")]
    UnitStringCollision(CurrencyUnit),
    // Wallet Errors
    /// P2PK spending conditions not met
    #[error("P2PK condition not met `{0}`")]
    P2PKConditionsNotMet(String),
    /// Duplicate signature from same pubkey in P2PK
    #[error("Duplicate signature from same pubkey in P2PK")]
    DuplicateSignatureError,
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
    #[error("Unknown wallet: `{0}`")]
    #[cfg(feature = "wallet")]
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
    /// Dleq Proof not provided for signature
    #[error("Dleq proof not provided for signature")]
    DleqProofNotProvided,
    /// Incorrect Mint
    /// Token does not match wallet mint
    #[error("Token does not match wallet mint")]
    IncorrectMint,
    /// Receive can only be used with tokens from single mint
    #[error("Multiple mint tokens not supported by receive. Please deconstruct the token and use receive with_proof")]
    MultiMintTokenNotSupported,
    /// Preimage not provided
    #[error("Preimage not provided")]
    PreimageNotProvided,

    /// Unknown mint
    #[error("Unknown mint: {mint_url}")]
    UnknownMint {
        /// URL of the unknown mint
        mint_url: String,
    },
    /// Transfer between mints timed out
    #[error("Transfer timeout: failed to transfer {amount} from {source_mint} to {target_mint}")]
    TransferTimeout {
        /// Source mint URL
        source_mint: String,
        /// Target mint URL
        target_mint: String,
        /// Amount that failed to transfer
        amount: Amount,
    },
    /// Insufficient Funds
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// Unexpected proof state
    #[error("Unexpected proof state")]
    UnexpectedProofState,
    /// No active keyset
    #[error("No active keyset")]
    NoActiveKeyset,
    /// Incorrect quote amount
    #[error("Incorrect quote amount")]
    IncorrectQuoteAmount,
    /// Invoice Description not supported
    #[error("Invoice Description not supported")]
    InvoiceDescriptionUnsupported,
    /// Invalid transaction direction
    #[error("Invalid transaction direction")]
    InvalidTransactionDirection,
    /// Invalid transaction id
    #[error("Invalid transaction id")]
    InvalidTransactionId,
    /// Transaction not found
    #[error("Transaction not found")]
    TransactionNotFound,
    /// Invalid operation kind
    #[error("Invalid operation kind")]
    InvalidOperationKind,
    /// Invalid operation state
    #[error("Invalid operation state")]
    InvalidOperationState,
    /// Operation not found
    #[error("Operation not found")]
    OperationNotFound,
    /// KV Store invalid key or namespace
    #[error("Invalid KV store key or namespace: {0}")]
    KVStoreInvalidKey(String),
    /// Concurrent update detected
    #[error("Concurrent update detected")]
    ConcurrentUpdate,
    /// Invalid response from mint
    #[error("Invalid mint response: {0}")]
    InvalidMintResponse(String),
    /// Subscription error
    #[error("Subscription error: {0}")]
    SubscriptionError(String),
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
    /// Parse 9rl Error
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
    /// Http transport error
    #[error("Http transport error {0:?}: {1}")]
    HttpError(Option<u16>, String),
    /// Parse invoice error
    #[cfg(feature = "mint")]
    #[error(transparent)]
    Uuid(#[from] uuid::Error),
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
    /// NUT04 error
    #[error(transparent)]
    NUT04(#[from] crate::nuts::nut04::Error),
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
    #[cfg(feature = "wallet")]
    NUT13(#[from] crate::nuts::nut13::Error),
    /// NUT14 Error
    #[error(transparent)]
    NUT14(#[from] crate::nuts::nut14::Error),
    /// NUT18 Error
    #[error(transparent)]
    NUT18(#[from] crate::nuts::nut18::Error),
    /// NUT20 Error
    #[error(transparent)]
    NUT20(#[from] crate::nuts::nut20::Error),
    /// NUT21 Error
    #[error(transparent)]
    NUT21(#[from] crate::nuts::nut21::Error),
    /// NUT22 Error
    #[error(transparent)]
    NUT22(#[from] crate::nuts::nut22::Error),
    /// NUT23 Error
    #[error(transparent)]
    NUT23(#[from] crate::nuts::nut23::Error),
    /// Quote ID Error
    #[error(transparent)]
    #[cfg(feature = "mint")]
    QuoteId(#[from] crate::quote_id::QuoteIdError),
    /// From slice error
    #[error(transparent)]
    TryFromSliceError(#[from] TryFromSliceError),
    /// Database Error
    #[error(transparent)]
    Database(crate::database::Error),
    /// Payment Error
    #[error(transparent)]
    #[cfg(feature = "mint")]
    Payment(#[from] crate::payment::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_definitive_failure() {
        // Test definitive failures
        assert!(Error::AmountOverflow.is_definitive_failure());
        assert!(Error::TokenAlreadySpent.is_definitive_failure());
        assert!(Error::MintingDisabled.is_definitive_failure());

        // Test HTTP client errors (4xx) - simulated
        assert!(Error::HttpError(Some(400), "Bad Request".to_string()).is_definitive_failure());
        assert!(Error::HttpError(Some(404), "Not Found".to_string()).is_definitive_failure());
        assert!(
            Error::HttpError(Some(429), "Too Many Requests".to_string()).is_definitive_failure()
        );

        // Test ambiguous failures
        assert!(!Error::Timeout.is_definitive_failure());
        assert!(!Error::Internal.is_definitive_failure());
        assert!(!Error::ConcurrentUpdate.is_definitive_failure());

        // Test HTTP server errors (5xx)
        assert!(
            !Error::HttpError(Some(500), "Internal Server Error".to_string())
                .is_definitive_failure()
        );
        assert!(!Error::HttpError(Some(502), "Bad Gateway".to_string()).is_definitive_failure());
        assert!(
            !Error::HttpError(Some(503), "Service Unavailable".to_string()).is_definitive_failure()
        );

        // Test HTTP network errors (no status)
        assert!(!Error::HttpError(None, "Connection refused".to_string()).is_definitive_failure());
    }
}

impl Error {
    /// Check if the error is a definitive failure
    ///
    /// A definitive failure means the mint definitely rejected the request
    /// and did not update its state. In these cases, it is safe to revert
    /// the transaction locally.
    ///
    /// If false, the failure is ambiguous (e.g. timeout, network error, 500)
    /// and the transaction state at the mint is unknown.
    pub fn is_definitive_failure(&self) -> bool {
        match self {
            // Logic/Validation Errors (Safe to revert)
            Self::AmountKey
            | Self::KeysetUnknown(_)
            | Self::UnsupportedUnit
            | Self::InvoiceAmountUndefined
            | Self::SplitValuesGreater
            | Self::AmountOverflow
            | Self::OverIssue
            | Self::SignatureMissingOrInvalid
            | Self::AmountLessNotAllowed
            | Self::InternalMultiPartMeltQuote
            | Self::MppUnitMethodNotSupported(_, _)
            | Self::AmountlessInvoiceNotSupported(_, _)
            | Self::DuplicatePaymentId
            | Self::PubkeyRequired
            | Self::InvalidPaymentMethod
            | Self::UnsupportedPaymentMethod
            | Self::InvalidInvoice
            | Self::MintingDisabled
            | Self::UnknownQuote
            | Self::ExpiredQuote(_, _)
            | Self::AmountOutofLimitRange(_, _, _)
            | Self::UnpaidQuote
            | Self::PendingQuote
            | Self::IssuedQuote
            | Self::PaidQuote
            | Self::MeltingDisabled
            | Self::UnknownKeySet
            | Self::BlindedMessageAlreadySigned
            | Self::InactiveKeyset
            | Self::TransactionUnbalanced(_, _, _)
            | Self::DuplicateInputs
            | Self::DuplicateOutputs
            | Self::MultipleUnits
            | Self::UnitMismatch
            | Self::SigAllUsedInMelt
            | Self::TokenAlreadySpent
            | Self::TokenPending
            | Self::P2PKConditionsNotMet(_)
            | Self::DuplicateSignatureError
            | Self::LocktimeNotProvided
            | Self::InvalidSpendConditions(_)
            | Self::IncorrectWallet(_)
            | Self::MaxFeeExceeded
            | Self::DleqProofNotProvided
            | Self::IncorrectMint
            | Self::MultiMintTokenNotSupported
            | Self::PreimageNotProvided
            | Self::UnknownMint { .. }
            | Self::UnexpectedProofState
            | Self::NoActiveKeyset
            | Self::IncorrectQuoteAmount
            | Self::InvoiceDescriptionUnsupported
            | Self::InvalidTransactionDirection
            | Self::InvalidTransactionId
            | Self::InvalidOperationKind
            | Self::InvalidOperationState
            | Self::OperationNotFound
            | Self::KVStoreInvalidKey(_) => true,

            // HTTP Errors
            Self::HttpError(Some(status), _) => {
                // Client errors (400-499) are definitive failures
                // Server errors (500-599) are ambiguous
                (400..500).contains(status)
            }

            // Ambiguous Errors (Unsafe to revert)
            Self::Timeout
            | Self::Internal
            | Self::UnknownPaymentState
            | Self::CouldNotGetMintInfo
            | Self::UnknownErrorResponse(_)
            | Self::InvalidMintResponse(_)
            | Self::ConcurrentUpdate
            | Self::SendError(_)
            | Self::RecvError(_)
            | Self::TransferTimeout { .. } => false,

            // Network/IO/Parsing Errors (Usually ambiguous as they could happen reading response)
            Self::HttpError(None, _) // No status code means network error
            | Self::SerdeJsonError(_) // Could be malformed success response
            | Self::Database(_)
            | Self::Custom(_) => false,

            // Auth Errors (Generally definitive if rejected)
            Self::ClearAuthRequired
            | Self::BlindAuthRequired
            | Self::ClearAuthFailed
            | Self::BlindAuthFailed
            | Self::InsufficientBlindAuthTokens
            | Self::AuthSettingsUndefined
            | Self::AuthLocalstoreUndefined
            | Self::OidcNotSet => true,

            // External conversions - check specifically
            Self::Invoice(_) => true, // Parsing error
            Self::Bip32(_) => true, // Key derivation error
            Self::ParseInt(_) => true,
            Self::UrlParseError(_) => true,
            Self::Utf8ParseError(_) => true,
            Self::Base64Error(_) => true,
            Self::HexError(_) => true,
            #[cfg(feature = "mint")]
            Self::Uuid(_) => true,
            Self::CashuUrl(_) => true,
            Self::Secret(_) => true,
            Self::AmountError(_) => true,
            Self::DHKE(_) => true, // Crypto errors
            Self::NUT00(_) => true,
            Self::NUT01(_) => true,
            Self::NUT02(_) => true,
            Self::NUT03(_) => true,
            Self::NUT04(_) => true,
            Self::NUT05(_) => true,
            Self::NUT11(_) => true,
            Self::NUT12(_) => true,
            #[cfg(feature = "wallet")]
            Self::NUT13(_) => true,
            Self::NUT14(_) => true,
            Self::NUT18(_) => true,
            Self::NUT20(_) => true,
            Self::NUT21(_) => true,
            Self::NUT22(_) => true,
            Self::NUT23(_) => true,
            #[cfg(feature = "mint")]
            Self::QuoteId(_) => true,
            Self::TryFromSliceError(_) => true,
            #[cfg(feature = "mint")]
            Self::Payment(_) => false, // Payment errors could be ambiguous? assume ambiguous to be safe

            // Catch-all
            _ => false,
        }
    }
}

/// CDK Error Response
///
/// See NUT definition in [00](https://github.com/cashubtc/nuts/blob/main/00.md)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct ErrorResponse {
    /// Error Code
    pub code: ErrorCode,
    /// Human readable description
    #[serde(default)]
    pub detail: String,
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "code: {}, detail: {}", self.code, self.detail)
    }
}

impl ErrorResponse {
    /// Create new [`ErrorResponse`]
    pub fn new(code: ErrorCode, detail: String) -> Self {
        Self { code, detail }
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
                detail: value.to_string(),
            }),
        }
    }
}

/// Maps NUT11 errors to appropriate error codes
/// All NUT11 errors are witness/signature related, so they map to WitnessMissingOrInvalid (20008)
fn map_nut11_error(_nut11_error: &crate::nuts::nut11::Error) -> ErrorCode {
    // All NUT11 errors relate to P2PK/witness validation, which maps to 20008
    ErrorCode::WitnessMissingOrInvalid
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenAlreadySpent => ErrorResponse {
                code: ErrorCode::TokenAlreadySpent,
                detail: err.to_string(),
            },
            Error::UnsupportedUnit => ErrorResponse {
                code: ErrorCode::UnsupportedUnit,
                detail: err.to_string(),
            },
            Error::PaymentFailed => ErrorResponse {
                code: ErrorCode::LightningError,
                detail: err.to_string(),
            },
            Error::RequestAlreadyPaid => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                detail: "Invoice already paid.".to_string(),
            },
            Error::TransactionUnbalanced(inputs_total, outputs_total, fee_expected) => {
                ErrorResponse {
                    code: ErrorCode::TransactionUnbalanced,
                    detail: format!(
                        "Inputs: {inputs_total}, Outputs: {outputs_total}, expected_fee: {fee_expected}. Transaction inputs should equal outputs less fee"
                    ),
                }
            }
            Error::MintingDisabled => ErrorResponse {
                code: ErrorCode::MintingDisabled,
                detail: err.to_string(),
            },
            Error::BlindedMessageAlreadySigned => ErrorResponse {
                code: ErrorCode::BlindedMessageAlreadySigned,
                detail: err.to_string(),
            },
            Error::InsufficientFunds => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                detail: err.to_string(),
            },
            Error::AmountOutofLimitRange(_min, _max, _amount) => ErrorResponse {
                code: ErrorCode::AmountOutofLimitRange,
                detail: err.to_string(),
            },
            Error::ExpiredQuote(_, _) => ErrorResponse {
                code: ErrorCode::QuoteExpired,
                detail: err.to_string(),
            },
            Error::PendingQuote => ErrorResponse {
                code: ErrorCode::QuotePending,
                detail: err.to_string(),
            },
            Error::TokenPending => ErrorResponse {
                code: ErrorCode::TokenPending,
                detail: err.to_string(),
            },
            Error::ClearAuthRequired => ErrorResponse {
                code: ErrorCode::ClearAuthRequired,
                detail: Error::ClearAuthRequired.to_string(),
            },
            Error::ClearAuthFailed => ErrorResponse {
                code: ErrorCode::ClearAuthFailed,
                detail: Error::ClearAuthFailed.to_string(),
            },
            Error::BlindAuthRequired => ErrorResponse {
                code: ErrorCode::BlindAuthRequired,
                detail: Error::BlindAuthRequired.to_string(),
            },
            Error::BlindAuthFailed => ErrorResponse {
                code: ErrorCode::BlindAuthFailed,
                detail: Error::BlindAuthFailed.to_string(),
            },
            Error::NUT20(err) => ErrorResponse {
                code: ErrorCode::WitnessMissingOrInvalid,
                detail: err.to_string(),
            },
            Error::DuplicateInputs => ErrorResponse {
                code: ErrorCode::DuplicateInputs,
                detail: err.to_string(),
            },
            Error::DuplicateOutputs => ErrorResponse {
                code: ErrorCode::DuplicateOutputs,
                detail: err.to_string(),
            },
            Error::MultipleUnits => ErrorResponse {
                code: ErrorCode::MultipleUnits,
                detail: err.to_string(),
            },
            Error::UnitMismatch => ErrorResponse {
                code: ErrorCode::UnitMismatch,
                detail: err.to_string(),
            },
            Error::UnpaidQuote => ErrorResponse {
                code: ErrorCode::QuoteNotPaid,
                detail: Error::UnpaidQuote.to_string(),
            },
            Error::NUT11(err) => {
                let code = map_nut11_error(&err);
                let extra = if matches!(err, crate::nuts::nut11::Error::SignaturesNotProvided) {
                    Some("P2PK signatures are required but not provided".to_string())
                } else {
                    None
                };
                ErrorResponse {
                    code,
                    detail: match extra {
                        Some(extra) => format!("{err}. {extra}"),
                        None => err.to_string(),
                    },
                }
            },
            Error::DuplicateSignatureError => ErrorResponse {
                code: ErrorCode::WitnessMissingOrInvalid,
                detail: err.to_string(),
            },
            Error::IssuedQuote => ErrorResponse {
                code: ErrorCode::TokensAlreadyIssued,
                detail: err.to_string(),
            },
            Error::UnknownKeySet => ErrorResponse {
                code: ErrorCode::KeysetNotFound,
                detail: err.to_string(),
            },
            Error::InactiveKeyset => ErrorResponse {
                code: ErrorCode::KeysetInactive,
                detail: err.to_string(),
            },
            Error::AmountLessNotAllowed => ErrorResponse {
                code: ErrorCode::AmountlessInvoiceNotSupported,
                detail: err.to_string(),
            },
            Error::IncorrectQuoteAmount => ErrorResponse {
                code: ErrorCode::IncorrectQuoteAmount,
                detail: err.to_string(),
            },
            Error::PubkeyRequired => ErrorResponse {
                code: ErrorCode::PubkeyRequired,
                detail: err.to_string(),
            },
            Error::PaidQuote => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                detail: err.to_string(),
            },
            Error::DuplicatePaymentId => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                detail: err.to_string(),
            },
            // Database duplicate error indicates another quote with same invoice is already pending/paid
            Error::Database(crate::database::Error::Duplicate) => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                detail: "Invoice already paid or pending".to_string(),
            },

            // DHKE errors - TokenNotVerified for actual verification failures
            Error::DHKE(crate::dhke::Error::TokenNotVerified) => ErrorResponse {
                code: ErrorCode::TokenNotVerified,
                detail: err.to_string(),
            },
            Error::DHKE(_) => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },

            // Verification errors
            Error::CouldNotVerifyDleq => ErrorResponse {
                code: ErrorCode::TokenNotVerified,
                detail: err.to_string(),
            },
            Error::SignatureMissingOrInvalid => ErrorResponse {
                code: ErrorCode::WitnessMissingOrInvalid,
                detail: err.to_string(),
            },
            Error::SigAllUsedInMelt => ErrorResponse {
                code: ErrorCode::WitnessMissingOrInvalid,
                detail: err.to_string(),
            },

            // Keyset/key errors
            Error::AmountKey => ErrorResponse {
                code: ErrorCode::KeysetNotFound,
                detail: err.to_string(),
            },
            Error::KeysetUnknown(_) => ErrorResponse {
                code: ErrorCode::KeysetNotFound,
                detail: err.to_string(),
            },
            Error::NoActiveKeyset => ErrorResponse {
                code: ErrorCode::KeysetInactive,
                detail: err.to_string(),
            },

            // Quote/payment errors
            Error::UnknownQuote => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },
            Error::MeltingDisabled => ErrorResponse {
                code: ErrorCode::MintingDisabled,
                detail: err.to_string(),
            },
            Error::PaymentPending => ErrorResponse {
                code: ErrorCode::QuotePending,
                detail: err.to_string(),
            },
            Error::UnknownPaymentState => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },

            // Transaction/amount errors
            Error::SplitValuesGreater => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                detail: err.to_string(),
            },
            Error::AmountOverflow => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                detail: err.to_string(),
            },
            Error::OverIssue => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                detail: err.to_string(),
            },

            // Invoice parsing errors - no spec code for invalid format
            Error::InvalidPaymentRequest => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },
            Error::InvoiceAmountUndefined => ErrorResponse {
                code: ErrorCode::AmountlessInvoiceNotSupported,
                detail: err.to_string(),
            },

            // Internal/system errors - use Unknown(99999)
            Error::Internal => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },
            Error::Database(_) => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },
            Error::ConcurrentUpdate => ErrorResponse {
                code: ErrorCode::ConcurrentUpdate,
                detail: err.to_string(),
            },
            Error::MaxInputsExceeded { .. } => ErrorResponse {
                code: ErrorCode::MaxInputsExceeded,
                detail: err.to_string()
            },
            Error::MaxOutputsExceeded { .. } => ErrorResponse {
                code: ErrorCode::MaxOutputsExceeded,
                detail: err.to_string()
            },
            // Fallback for any remaining errors - use Unknown(99999) instead of TokenNotVerified
            _ => ErrorResponse {
                code: ErrorCode::Unknown(50000),
                detail: err.to_string(),
            },
        }
    }
}

#[cfg(feature = "mint")]
impl From<crate::database::Error> for Error {
    fn from(db_error: crate::database::Error) -> Self {
        match db_error {
            crate::database::Error::InvalidStateTransition(state) => match state {
                crate::state::Error::Pending => Self::TokenPending,
                crate::state::Error::AlreadySpent => Self::TokenAlreadySpent,
                crate::state::Error::AlreadyPaid => Self::RequestAlreadyPaid,
                state => Self::Database(crate::database::Error::InvalidStateTransition(state)),
            },
            crate::database::Error::ConcurrentUpdate => Self::ConcurrentUpdate,
            db_error => Self::Database(db_error),
        }
    }
}

#[cfg(not(feature = "mint"))]
impl From<crate::database::Error> for Error {
    fn from(db_error: crate::database::Error) -> Self {
        match db_error {
            crate::database::Error::ConcurrentUpdate => Self::ConcurrentUpdate,
            db_error => Self::Database(db_error),
        }
    }
}

impl From<ErrorResponse> for Error {
    fn from(err: ErrorResponse) -> Error {
        match err.code {
            // 10xxx - Proof/Token verification errors
            ErrorCode::TokenNotVerified => Self::DHKE(crate::dhke::Error::TokenNotVerified),
            // 11xxx - Input/Output errors
            ErrorCode::TokenAlreadySpent => Self::TokenAlreadySpent,
            ErrorCode::TokenPending => Self::TokenPending,
            ErrorCode::BlindedMessageAlreadySigned => Self::BlindedMessageAlreadySigned,
            ErrorCode::OutputsPending => Self::TokenPending, // Map to closest equivalent
            ErrorCode::TransactionUnbalanced => Self::TransactionUnbalanced(0, 0, 0),
            ErrorCode::AmountOutofLimitRange => {
                Self::AmountOutofLimitRange(Amount::default(), Amount::default(), Amount::default())
            }
            ErrorCode::DuplicateInputs => Self::DuplicateInputs,
            ErrorCode::DuplicateOutputs => Self::DuplicateOutputs,
            ErrorCode::MultipleUnits => Self::MultipleUnits,
            ErrorCode::UnitMismatch => Self::UnitMismatch,
            ErrorCode::AmountlessInvoiceNotSupported => Self::AmountLessNotAllowed,
            ErrorCode::IncorrectQuoteAmount => Self::IncorrectQuoteAmount,
            ErrorCode::UnsupportedUnit => Self::UnsupportedUnit,
            // 12xxx - Keyset errors
            ErrorCode::KeysetNotFound => Self::UnknownKeySet,
            ErrorCode::KeysetInactive => Self::InactiveKeyset,
            // 20xxx - Quote/Payment errors
            ErrorCode::QuoteNotPaid => Self::UnpaidQuote,
            ErrorCode::TokensAlreadyIssued => Self::IssuedQuote,
            ErrorCode::MintingDisabled => Self::MintingDisabled,
            ErrorCode::LightningError => Self::PaymentFailed,
            ErrorCode::QuotePending => Self::PendingQuote,
            ErrorCode::InvoiceAlreadyPaid => Self::RequestAlreadyPaid,
            ErrorCode::QuoteExpired => Self::ExpiredQuote(0, 0),
            ErrorCode::WitnessMissingOrInvalid => Self::SignatureMissingOrInvalid,
            ErrorCode::PubkeyRequired => Self::PubkeyRequired,
            // 30xxx - Clear auth errors
            ErrorCode::ClearAuthRequired => Self::ClearAuthRequired,
            ErrorCode::ClearAuthFailed => Self::ClearAuthFailed,
            // 31xxx - Blind auth errors
            ErrorCode::BlindAuthRequired => Self::BlindAuthRequired,
            ErrorCode::BlindAuthFailed => Self::BlindAuthFailed,
            ErrorCode::BatMintMaxExceeded => Self::InsufficientBlindAuthTokens,
            ErrorCode::BatRateLimitExceeded => Self::InsufficientBlindAuthTokens,
            _ => Self::UnknownErrorResponse(err.to_string()),
        }
    }
}

/// Possible Error Codes
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum ErrorCode {
    // 10xxx - Proof/Token verification errors
    /// Proof verification failed (10001)
    TokenNotVerified,

    // 11xxx - Input/Output errors
    /// Proofs already spent (11001)
    TokenAlreadySpent,
    /// Proofs are pending (11002)
    TokenPending,
    /// Outputs already signed (11003)
    BlindedMessageAlreadySigned,
    /// Outputs are pending (11004)
    OutputsPending,
    /// Transaction is not balanced (11005)
    TransactionUnbalanced,
    /// Amount outside of limit range (11006)
    AmountOutofLimitRange,
    /// Duplicate inputs provided (11007)
    DuplicateInputs,
    /// Duplicate outputs provided (11008)
    DuplicateOutputs,
    /// Inputs/Outputs of multiple units (11009)
    MultipleUnits,
    /// Inputs and outputs not of same unit (11010)
    UnitMismatch,
    /// Amountless invoice is not supported (11011)
    AmountlessInvoiceNotSupported,
    /// Amount in request does not equal invoice (11012)
    IncorrectQuoteAmount,
    /// Unit in request is not supported (11013)
    UnsupportedUnit,
    /// The max number of inputs is exceeded
    MaxInputsExceeded,
    /// The max number of outputs is exceeded
    MaxOutputsExceeded,
    // 12xxx - Keyset errors
    /// Keyset is not known (12001)
    KeysetNotFound,
    /// Keyset is inactive, cannot sign messages (12002)
    KeysetInactive,

    // 20xxx - Quote/Payment errors
    /// Quote request is not paid (20001)
    QuoteNotPaid,
    /// Quote has already been issued (20002)
    TokensAlreadyIssued,
    /// Minting is disabled (20003)
    MintingDisabled,
    /// Lightning payment failed (20004)
    LightningError,
    /// Quote is pending (20005)
    QuotePending,
    /// Invoice already paid (20006)
    InvoiceAlreadyPaid,
    /// Quote is expired (20007)
    QuoteExpired,
    /// Signature for mint request invalid (20008)
    WitnessMissingOrInvalid,
    /// Pubkey required for mint quote (20009)
    PubkeyRequired,

    // 30xxx - Clear auth errors
    /// Endpoint requires clear auth (30001)
    ClearAuthRequired,
    /// Clear authentication failed (30002)
    ClearAuthFailed,

    // 31xxx - Blind auth errors
    /// Endpoint requires blind auth (31001)
    BlindAuthRequired,
    /// Blind authentication failed (31002)
    BlindAuthFailed,
    /// Maximum BAT mint amount exceeded (31003)
    BatMintMaxExceeded,
    /// BAT mint rate limit exceeded (31004)
    BatRateLimitExceeded,

    /// Concurrent update detected
    ConcurrentUpdate,

    /// Unknown error code
    Unknown(u16),
}

impl ErrorCode {
    /// Error code from u16
    pub fn from_code(code: u16) -> Self {
        match code {
            // 10xxx - Proof/Token verification errors
            10001 => Self::TokenNotVerified,
            // 11xxx - Input/Output errors
            11001 => Self::TokenAlreadySpent,
            11002 => Self::TokenPending,
            11003 => Self::BlindedMessageAlreadySigned,
            11004 => Self::OutputsPending,
            11005 => Self::TransactionUnbalanced,
            11006 => Self::AmountOutofLimitRange,
            11007 => Self::DuplicateInputs,
            11008 => Self::DuplicateOutputs,
            11009 => Self::MultipleUnits,
            11010 => Self::UnitMismatch,
            11011 => Self::AmountlessInvoiceNotSupported,
            11012 => Self::IncorrectQuoteAmount,
            11013 => Self::UnsupportedUnit,
            11014 => Self::MaxInputsExceeded,
            11015 => Self::MaxOutputsExceeded,
            // 12xxx - Keyset errors
            12001 => Self::KeysetNotFound,
            12002 => Self::KeysetInactive,
            // 20xxx - Quote/Payment errors
            20001 => Self::QuoteNotPaid,
            20002 => Self::TokensAlreadyIssued,
            20003 => Self::MintingDisabled,
            20004 => Self::LightningError,
            20005 => Self::QuotePending,
            20006 => Self::InvoiceAlreadyPaid,
            20007 => Self::QuoteExpired,
            20008 => Self::WitnessMissingOrInvalid,
            20009 => Self::PubkeyRequired,
            // 30xxx - Clear auth errors
            30001 => Self::ClearAuthRequired,
            30002 => Self::ClearAuthFailed,
            // 31xxx - Blind auth errors
            31001 => Self::BlindAuthRequired,
            31002 => Self::BlindAuthFailed,
            31003 => Self::BatMintMaxExceeded,
            31004 => Self::BatRateLimitExceeded,
            _ => Self::Unknown(code),
        }
    }

    /// Error code to u16
    pub fn to_code(&self) -> u16 {
        match self {
            // 10xxx - Proof/Token verification errors
            Self::TokenNotVerified => 10001,
            // 11xxx - Input/Output errors
            Self::TokenAlreadySpent => 11001,
            Self::TokenPending => 11002,
            Self::BlindedMessageAlreadySigned => 11003,
            Self::OutputsPending => 11004,
            Self::TransactionUnbalanced => 11005,
            Self::AmountOutofLimitRange => 11006,
            Self::DuplicateInputs => 11007,
            Self::DuplicateOutputs => 11008,
            Self::MultipleUnits => 11009,
            Self::UnitMismatch => 11010,
            Self::AmountlessInvoiceNotSupported => 11011,
            Self::IncorrectQuoteAmount => 11012,
            Self::UnsupportedUnit => 11013,
            Self::MaxInputsExceeded => 11014,
            Self::MaxOutputsExceeded => 11015,
            // 12xxx - Keyset errors
            Self::KeysetNotFound => 12001,
            Self::KeysetInactive => 12002,
            // 20xxx - Quote/Payment errors
            Self::QuoteNotPaid => 20001,
            Self::TokensAlreadyIssued => 20002,
            Self::MintingDisabled => 20003,
            Self::LightningError => 20004,
            Self::QuotePending => 20005,
            Self::InvoiceAlreadyPaid => 20006,
            Self::QuoteExpired => 20007,
            Self::WitnessMissingOrInvalid => 20008,
            Self::PubkeyRequired => 20009,
            // 30xxx - Clear auth errors
            Self::ClearAuthRequired => 30001,
            Self::ClearAuthFailed => 30002,
            // 31xxx - Blind auth errors
            Self::BlindAuthRequired => 31001,
            Self::BlindAuthFailed => 31002,
            Self::BatMintMaxExceeded => 31003,
            Self::BatRateLimitExceeded => 31004,
            Self::ConcurrentUpdate => 50000,
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
