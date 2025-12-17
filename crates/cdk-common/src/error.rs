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
    #[error("Duplicate Inputs")]
    DuplicateInputs,
    /// Duplicate output
    #[error("Duplicate outputs")]
    DuplicateOutputs,
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

    // MultiMint Wallet Errors
    /// Currency unit mismatch in MultiMintWallet
    #[error("Currency unit mismatch: wallet uses {expected}, but {found} provided")]
    MultiMintCurrencyUnitMismatch {
        /// Expected currency unit
        expected: CurrencyUnit,
        /// Found currency unit
        found: CurrencyUnit,
    },
    /// Unknown mint in MultiMintWallet
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
    /// KV Store invalid key or namespace
    #[error("Invalid KV store key or namespace: {0}")]
    KVStoreInvalidKey(String),

    // Batch Mint Errors (NUT-XX)
    /// Batch request is empty (no quotes provided)
    #[error("Batch request is empty")]
    BatchEmpty,
    /// All quotes in batch must use same payment method
    #[error("All quotes in batch must use same payment method")]
    BatchPaymentMethodMismatch,
    /// Quote payment method doesn't match endpoint
    #[error("Batch quote payment method does not match endpoint")]
    BatchPaymentMethodEndpointMismatch,
    /// Signature provided for unlocked quote (no pubkey)
    #[error("Signature provided for unlocked quote")]
    BatchUnexpectedSignature,
    /// Batch size exceeds limit (max 100 quotes)
    #[error("Batch size exceeds limit (max 100 quotes)")]
    BatchSizeExceeded,
    /// Signature array size mismatch with quote count
    #[error("Signature array size does not match quote count")]
    BatchSignatureCountMismatch,
    /// Bolt12 batch minting requires spending conditions
    #[error("Bolt12 batch minting requires spending conditions")]
    BatchBolt12RequiresSpendingConditions,
    /// Bolt12 batch minting requires a secret key for each quote
    #[error("Bolt12 batch minting requires secret keys for all quotes")]
    BatchBolt12MissingSecretKey,

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
    #[cfg(feature = "auth")]
    NUT21(#[from] crate::nuts::nut21::Error),
    /// NUT22 Error
    #[error(transparent)]
    #[cfg(feature = "auth")]
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
    /// Optional quote identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(quote) = &self.quote {
            write!(
                f,
                "code: {}, quote: {}, detail: {}",
                self.code, quote, self.detail
            )
        } else {
            write!(f, "code: {}, detail: {}", self.code, self.detail)
        }
    }
}

impl ErrorResponse {
    /// Create new [`ErrorResponse`]
    pub fn new(code: ErrorCode, detail: String) -> Self {
        Self {
            code,
            detail,
            quote: None,
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
                detail: value.to_string(),
                quote: None,
            }),
        }
    }
}

/// Maps NUT11 errors to appropriate error codes
fn map_nut11_error(nut11_error: &crate::nuts::nut11::Error) -> ErrorCode {
    match nut11_error {
        crate::nuts::nut11::Error::SignaturesNotProvided => ErrorCode::WitnessMissingOrInvalid,
        crate::nuts::nut11::Error::InvalidSignature => ErrorCode::WitnessMissingOrInvalid,
        crate::nuts::nut11::Error::DuplicateSignature => ErrorCode::DuplicateSignature,
        _ => ErrorCode::Unknown(9999), // Parsing/validation errors
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenAlreadySpent => ErrorResponse {
                code: ErrorCode::TokenAlreadySpent,
                detail: err.to_string(),
                quote: None,
            },
            Error::UnsupportedUnit => ErrorResponse {
                code: ErrorCode::UnsupportedUnit,
                detail: err.to_string(),
                quote: None,
            },
            Error::PaymentFailed => ErrorResponse {
                code: ErrorCode::LightningError,
                detail: err.to_string(),
                quote: None,
            },
            Error::RequestAlreadyPaid => ErrorResponse {
                code: ErrorCode::InvoiceAlreadyPaid,
                detail: "Invoice already paid.".to_string(),
                quote: None,
            },
            Error::TransactionUnbalanced(inputs_total, outputs_total, fee_expected) => {
                ErrorResponse {
                    code: ErrorCode::TransactionUnbalanced,
                    detail: format!(
                        "Inputs: {inputs_total}, Outputs: {outputs_total}, expected_fee: {fee_expected}. Transaction inputs should equal outputs less fee"
                    ),
                    quote: None,
                }
            }
            Error::MintingDisabled => ErrorResponse {
                code: ErrorCode::MintingDisabled,
                detail: err.to_string(),
                quote: None,
            },
            Error::BlindedMessageAlreadySigned => ErrorResponse {
                code: ErrorCode::BlindedMessageAlreadySigned,
                detail: err.to_string(),
                quote: None,
            },
            Error::InsufficientFunds => ErrorResponse {
                code: ErrorCode::TransactionUnbalanced,
                detail: err.to_string(),
                quote: None,
            },
            Error::AmountOutofLimitRange(_min, _max, _amount) => ErrorResponse {
                code: ErrorCode::AmountOutofLimitRange,
                detail: err.to_string(),
                quote: None,
            },
            Error::ExpiredQuote(_, _) => ErrorResponse {
                code: ErrorCode::QuoteExpired,
                detail: err.to_string(),
                quote: None,
            },
            Error::PendingQuote => ErrorResponse {
                code: ErrorCode::QuotePending,
                detail: err.to_string(),
                quote: None,
            },
            Error::TokenPending => ErrorResponse {
                code: ErrorCode::TokenPending,
                detail: err.to_string(),
                quote: None,
            },
            Error::ClearAuthRequired => ErrorResponse {
                code: ErrorCode::ClearAuthRequired,
                detail: Error::ClearAuthRequired.to_string(),
                quote: None,
            },
            Error::ClearAuthFailed => ErrorResponse {
                code: ErrorCode::ClearAuthFailed,
                detail: Error::ClearAuthFailed.to_string(),
                quote: None,
            },
            Error::BlindAuthRequired => ErrorResponse {
                code: ErrorCode::BlindAuthRequired,
                detail: Error::BlindAuthRequired.to_string(),
                quote: None,
            },
            Error::BlindAuthFailed => ErrorResponse {
                code: ErrorCode::BlindAuthFailed,
                detail: Error::BlindAuthFailed.to_string(),
                quote: None,
            },
            Error::NUT20(err) => ErrorResponse {
                code: ErrorCode::WitnessMissingOrInvalid,
                detail: err.to_string(),
                quote: None,
            },
            Error::DuplicateInputs => ErrorResponse {
                code: ErrorCode::DuplicateInputs,
                detail: err.to_string(),
                quote: None,
            },
            Error::DuplicateOutputs => ErrorResponse {
                code: ErrorCode::DuplicateOutputs,
                detail: err.to_string(),
                quote: None,
            },
            Error::MultipleUnits => ErrorResponse {
                code: ErrorCode::MultipleUnits,
                detail: err.to_string(),
                quote: None,
            },
            Error::UnitMismatch => ErrorResponse {
                code: ErrorCode::UnitMismatch,
                detail: err.to_string(),
                quote: None,
            },
            Error::UnpaidQuote => ErrorResponse {
                code: ErrorCode::QuoteNotPaid,
                detail: Error::UnpaidQuote.to_string(),
                quote: None,
            },
            Error::UnknownQuote => ErrorResponse {
                code: ErrorCode::UnknownQuote,
                detail: err.to_string(),
                quote: None,
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
                    quote: None,
                }
            },
            Error::DuplicateSignatureError => ErrorResponse {
                code: ErrorCode::DuplicateSignature,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchEmpty => ErrorResponse {
                code: ErrorCode::BatchEmpty,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchPaymentMethodMismatch => ErrorResponse {
                code: ErrorCode::PaymentMethodMismatch,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchPaymentMethodEndpointMismatch => ErrorResponse {
                code: ErrorCode::EndpointMethodMismatch,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchUnexpectedSignature => ErrorResponse {
                code: ErrorCode::SignatureUnexpected,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchSizeExceeded => ErrorResponse {
                code: ErrorCode::BatchSizeExceeded,
                detail: err.to_string(),
                quote: None,
            },
            Error::BatchSignatureCountMismatch => ErrorResponse {
                code: ErrorCode::SignatureCountMismatch,
                detail: err.to_string(),
                quote: None,
            },
            _ => ErrorResponse {
                code: ErrorCode::Unknown(9999),
                detail: err.to_string(),
                quote: None,
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
                state => Self::Database(crate::database::Error::InvalidStateTransition(state)),
            },
            db_error => Self::Database(db_error),
        }
    }
}

#[cfg(not(feature = "mint"))]
impl From<crate::database::Error> for Error {
    fn from(db_error: crate::database::Error) -> Self {
        Self::Database(db_error)
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
            ErrorCode::UnsupportedUnit => Self::UnsupportedUnit,
            ErrorCode::TransactionUnbalanced => Self::TransactionUnbalanced(0, 0, 0),
            ErrorCode::MintingDisabled => Self::MintingDisabled,
            ErrorCode::InvoiceAlreadyPaid => Self::RequestAlreadyPaid,
            ErrorCode::TokenNotVerified => Self::DHKE(crate::dhke::Error::TokenNotVerified),
            ErrorCode::LightningError => Self::PaymentFailed,
            ErrorCode::AmountOutofLimitRange => {
                Self::AmountOutofLimitRange(Amount::default(), Amount::default(), Amount::default())
            }
            ErrorCode::TokenPending => Self::TokenPending,
            ErrorCode::WitnessMissingOrInvalid => Self::SignatureMissingOrInvalid,
            ErrorCode::DuplicateInputs => Self::DuplicateInputs,
            ErrorCode::DuplicateOutputs => Self::DuplicateOutputs,
            ErrorCode::MultipleUnits => Self::MultipleUnits,
            ErrorCode::UnitMismatch => Self::UnitMismatch,
            ErrorCode::ClearAuthRequired => Self::ClearAuthRequired,
            ErrorCode::BlindAuthRequired => Self::BlindAuthRequired,
            ErrorCode::DuplicateSignature => Self::DuplicateSignatureError,
            ErrorCode::BatchEmpty => Self::BatchEmpty,
            ErrorCode::BatchSizeExceeded => Self::BatchSizeExceeded,
            ErrorCode::DuplicateQuoteIds => Self::DuplicatePaymentId,
            ErrorCode::UnknownQuote => Self::UnknownQuote,
            ErrorCode::PaymentMethodMismatch => Self::BatchPaymentMethodMismatch,
            ErrorCode::EndpointMethodMismatch => Self::BatchPaymentMethodEndpointMismatch,
            ErrorCode::BatchUnitMismatch => Self::MultipleUnits,
            ErrorCode::SignatureInvalid => Self::SignatureMissingOrInvalid,
            ErrorCode::SignatureMissing => Self::SignatureMissingOrInvalid,
            ErrorCode::SignatureUnexpected => Self::BatchUnexpectedSignature,
            ErrorCode::SignatureCountMismatch => Self::BatchSignatureCountMismatch,
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
    UnsupportedUnit,
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
    /// Witness missing or invalid
    WitnessMissingOrInvalid,
    /// Duplicate Inputs
    DuplicateInputs,
    /// Duplicate Outputs
    DuplicateOutputs,
    /// Batch request is empty
    BatchEmpty,
    /// Batch size exceeds limit
    BatchSizeExceeded,
    /// Duplicate quote ids in batch
    DuplicateQuoteIds,
    /// Unknown quote id
    UnknownQuote,
    /// Batch payment method mismatch
    PaymentMethodMismatch,
    /// Batch endpoint method mismatch
    EndpointMethodMismatch,
    /// Batch unit mismatch
    BatchUnitMismatch,
    /// Invalid signature in batch
    SignatureInvalid,
    /// Signature missing for locked quote
    SignatureMissing,
    /// Unexpected signature for unlocked quote
    SignatureUnexpected,
    /// Signature count mismatch for batch
    SignatureCountMismatch,
    /// Multiple Units
    MultipleUnits,
    /// Input unit does not match output
    UnitMismatch,
    /// Clear Auth Required
    ClearAuthRequired,
    /// Clear Auth Failed
    ClearAuthFailed,
    /// Blind Auth Required
    BlindAuthRequired,
    /// Blind Auth Failed
    BlindAuthFailed,
    /// Duplicate signature from same pubkey
    DuplicateSignature,
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
            11005 => Self::UnsupportedUnit,
            11006 => Self::AmountOutofLimitRange,
            11007 => Self::DuplicateInputs,
            11008 => Self::DuplicateOutputs,
            11009 => Self::MultipleUnits,
            11010 => Self::UnitMismatch,
            11012 => Self::TokenPending,
            12001 => Self::KeysetNotFound,
            12002 => Self::KeysetInactive,
            20000 => Self::LightningError,
            20001 => Self::QuoteNotPaid,
            20002 => Self::TokensAlreadyIssued,
            20003 => Self::MintingDisabled,
            20005 => Self::QuotePending,
            20006 => Self::InvoiceAlreadyPaid,
            20007 => Self::QuoteExpired,
            20008 => Self::WitnessMissingOrInvalid,
            20009 => Self::DuplicateSignature,
            21001 => Self::BatchEmpty,
            21002 => Self::BatchSizeExceeded,
            21003 => Self::DuplicateQuoteIds,
            21004 => Self::UnknownQuote,
            21005 => Self::PaymentMethodMismatch,
            21006 => Self::EndpointMethodMismatch,
            21007 => Self::BatchUnitMismatch,
            21008 => Self::SignatureInvalid,
            21009 => Self::SignatureMissing,
            21010 => Self::SignatureUnexpected,
            21011 => Self::SignatureCountMismatch,
            30001 => Self::ClearAuthRequired,
            30002 => Self::ClearAuthFailed,
            31001 => Self::BlindAuthRequired,
            31002 => Self::BlindAuthFailed,
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
            Self::UnsupportedUnit => 11005,
            Self::AmountOutofLimitRange => 11006,
            Self::DuplicateInputs => 11007,
            Self::DuplicateOutputs => 11008,
            Self::MultipleUnits => 11009,
            Self::UnitMismatch => 11010,
            Self::TokenPending => 11012,
            Self::KeysetNotFound => 12001,
            Self::KeysetInactive => 12002,
            Self::LightningError => 20000,
            Self::QuoteNotPaid => 20001,
            Self::TokensAlreadyIssued => 20002,
            Self::MintingDisabled => 20003,
            Self::QuotePending => 20005,
            Self::InvoiceAlreadyPaid => 20006,
            Self::QuoteExpired => 20007,
            Self::WitnessMissingOrInvalid => 20008,
            Self::DuplicateSignature => 20009,
            Self::BatchEmpty => 21001,
            Self::BatchSizeExceeded => 21002,
            Self::DuplicateQuoteIds => 21003,
            Self::UnknownQuote => 21004,
            Self::PaymentMethodMismatch => 21005,
            Self::EndpointMethodMismatch => 21006,
            Self::BatchUnitMismatch => 21007,
            Self::SignatureInvalid => 21008,
            Self::SignatureMissing => 21009,
            Self::SignatureUnexpected => 21010,
            Self::SignatureCountMismatch => 21011,
            Self::ClearAuthRequired => 30001,
            Self::ClearAuthFailed => 30002,
            Self::BlindAuthRequired => 31001,
            Self::BlindAuthFailed => 31002,
            Self::Unknown(code) => *code,
        }
    }

    /// Spec-friendly code string for APIs that expect symbolic codes
    pub fn as_spec_str(&self) -> Option<&'static str> {
        match self {
            Self::BatchEmpty => Some("BATCH_EMPTY"),
            Self::BatchSizeExceeded => Some("BATCH_SIZE_EXCEEDED"),
            Self::DuplicateQuoteIds => Some("DUPLICATE_QUOTE_IDS"),
            Self::UnknownQuote => Some("UNKNOWN_QUOTE"),
            Self::PaymentMethodMismatch => Some("PAYMENT_METHOD_MISMATCH"),
            Self::EndpointMethodMismatch => Some("ENDPOINT_METHOD_MISMATCH"),
            Self::BatchUnitMismatch | Self::MultipleUnits => Some("UNIT_MISMATCH"),
            Self::TransactionUnbalanced => Some("TRANSACTION_UNBALANCED"),
            Self::SignatureInvalid => Some("SIGNATURE_INVALID"),
            Self::SignatureMissing => Some("SIGNATURE_MISSING"),
            Self::SignatureUnexpected => Some("UNEXPECTED_SIGNATURE"),
            Self::SignatureCountMismatch => Some("SIGNATURE_COUNT_MISMATCH"),
            Self::QuoteNotPaid => Some("QUOTE_NOT_PAID"),
            _ => None,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(spec) = self.as_spec_str() {
            serializer.serialize_str(spec)
        } else {
            serializer.serialize_u16(self.to_code())
        }
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ErrorCode;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a numeric error code or symbolic code string")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ErrorCode::from_code(v as u16))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let normalized = v.to_ascii_uppercase();
                let code = match normalized.as_str() {
                    "BATCH_EMPTY" => ErrorCode::BatchEmpty,
                    "BATCH_SIZE_EXCEEDED" => ErrorCode::BatchSizeExceeded,
                    "DUPLICATE_QUOTE_IDS" => ErrorCode::DuplicateQuoteIds,
                    "UNKNOWN_QUOTE" => ErrorCode::UnknownQuote,
                    "PAYMENT_METHOD_MISMATCH" => ErrorCode::PaymentMethodMismatch,
                    "ENDPOINT_METHOD_MISMATCH" => ErrorCode::EndpointMethodMismatch,
                    "UNIT_MISMATCH" => ErrorCode::BatchUnitMismatch,
                    "TRANSACTION_UNBALANCED" => ErrorCode::TransactionUnbalanced,
                    "SIGNATURE_INVALID" => ErrorCode::SignatureInvalid,
                    "SIGNATURE_MISSING" => ErrorCode::SignatureMissing,
                    "UNEXPECTED_SIGNATURE" => ErrorCode::SignatureUnexpected,
                    "SIGNATURE_COUNT_MISMATCH" => ErrorCode::SignatureCountMismatch,
                    "QUOTE_NOT_PAID" => ErrorCode::QuoteNotPaid,
                    other => {
                        if let Ok(num) = other.parse::<u16>() {
                            return Ok(ErrorCode::from_code(num));
                        }
                        return Err(E::custom(format!("unknown error code `{other}`")));
                    }
                };
                Ok(code)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(spec) = self.as_spec_str() {
            write!(f, "{spec}")
        } else {
            write!(f, "{}", self.to_code())
        }
    }
}
