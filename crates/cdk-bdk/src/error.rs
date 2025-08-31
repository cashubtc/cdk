//! BDK Node Errors

use thiserror::Error;

/// BDK Node Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid description
    #[error("Invalid description")]
    InvalidDescription,

    /// Invalid payment hash
    #[error("Invalid payment hash")]
    InvalidPaymentHash,

    /// Invalid payment hash length
    #[error("Invalid payment hash length")]
    InvalidPaymentHashLength,

    /// Invalid payment ID length
    #[error("Invalid payment ID length")]
    InvalidPaymentIdLength,

    /// Unknown invoice amount
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,

    /// Payment not found
    #[error("Payment not found")]
    PaymentNotFound,

    /// Could not get amount spent
    #[error("Could not get amount spent")]
    CouldNotGetAmountSpent,

    /// Could not get payment amount
    #[error("Could not get payment amount")]
    CouldNotGetPaymentAmount,

    /// Unexpected payment kind
    #[error("Unexpected payment kind")]
    UnexpectedPaymentKind,

    /// Unsupported payment identifier type
    #[error("Unsupported payment identifier type")]
    UnsupportedPaymentIdentifierType,

    /// Invalid payment direction
    #[error("Invalid payment direction")]
    InvalidPaymentDirection,

    /// Hex decode error
    #[error("Hex decode error: {0}")]
    HexDecode(#[from] cdk_common::util::hex::Error),

    /// JSON error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Amount conversion error
    #[error("Amount conversion error: {0}")]
    AmountConversion(#[from] cdk_common::amount::Error),

    /// Invalid hex
    #[error("Invalid hex")]
    InvalidHex,

    /// Unsupported onchain
    #[error("Unsupported onchain")]
    UnsupportedOnchain,

    /// Database error
    #[error("Database error: {0}")]
    Database(#[from] bdk_wallet::rusqlite::Error),

    /// Wallet error
    #[error("Wallet error: {0}")]
    Wallet(String),

    /// Bitcoin RPC error
    #[error("Bitcoin RPC error: {0}")]
    BitcoinRpc(#[from] bdk_bitcoind_rpc::bitcoincore_rpc::Error),

    /// Bip32 key derivation error
    #[error("Bip32 key derivation error: {0}")]
    Bip32(#[from] bdk_wallet::bitcoin::bip32::Error),

    /// Key derivation error
    #[error("Key derivation error: {0}")]
    KeyDerivation(#[from] bdk_wallet::keys::KeyError),

    /// Channel send error
    #[error("Channel send error")]
    ChannelSend,

    /// Channel receive error
    #[error("Channel receive error: {0}")]
    ChannelRecv(#[from] tokio::sync::oneshot::error::RecvError),

    /// Fee too high
    #[error("Fee too high: {fee} sats exceeds maximum {max_fee} sats")]
    FeeTooHigh { fee: u64, max_fee: u64 },

    /// Could not sign transaction
    #[error("Could not sign transaction")]
    CouldNotSign,

    /// Path error
    #[error("Path error")]
    Path,
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(_: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Self::ChannelSend
    }
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
