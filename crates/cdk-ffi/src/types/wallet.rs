//! Wallet-related FFI types

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, SplitTarget};
use super::proof::{Proofs, SpendingConditions};
use crate::error::FfiError;
use crate::token::Token;

/// FFI-compatible SendMemo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SendMemo {
    /// Memo text
    pub memo: String,
    /// Include memo in token
    pub include_memo: bool,
}

impl From<SendMemo> for cdk::wallet::SendMemo {
    fn from(memo: SendMemo) -> Self {
        cdk::wallet::SendMemo {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl From<cdk::wallet::SendMemo> for SendMemo {
    fn from(memo: cdk::wallet::SendMemo) -> Self {
        Self {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl SendMemo {
    /// Convert SendMemo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendMemo from JSON string
#[uniffi::export]
pub fn decode_send_memo(json: String) -> Result<SendMemo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendMemo to JSON string
#[uniffi::export]
pub fn encode_send_memo(memo: SendMemo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&memo)?)
}

/// FFI-compatible SendKind
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum SendKind {
    /// Allow online swap before send if wallet does not have exact amount
    OnlineExact,
    /// Prefer offline send if difference is less than tolerance
    OnlineTolerance { tolerance: Amount },
    /// Wallet cannot do an online swap and selected proof must be exactly send amount
    OfflineExact,
    /// Wallet must remain offline but can over pay if below tolerance
    OfflineTolerance { tolerance: Amount },
}

impl From<SendKind> for cdk::wallet::SendKind {
    fn from(kind: SendKind) -> Self {
        match kind {
            SendKind::OnlineExact => cdk::wallet::SendKind::OnlineExact,
            SendKind::OnlineTolerance { tolerance } => {
                cdk::wallet::SendKind::OnlineTolerance(tolerance.into())
            }
            SendKind::OfflineExact => cdk::wallet::SendKind::OfflineExact,
            SendKind::OfflineTolerance { tolerance } => {
                cdk::wallet::SendKind::OfflineTolerance(tolerance.into())
            }
        }
    }
}

impl From<cdk::wallet::SendKind> for SendKind {
    fn from(kind: cdk::wallet::SendKind) -> Self {
        match kind {
            cdk::wallet::SendKind::OnlineExact => SendKind::OnlineExact,
            cdk::wallet::SendKind::OnlineTolerance(tolerance) => SendKind::OnlineTolerance {
                tolerance: tolerance.into(),
            },
            cdk::wallet::SendKind::OfflineExact => SendKind::OfflineExact,
            cdk::wallet::SendKind::OfflineTolerance(tolerance) => SendKind::OfflineTolerance {
                tolerance: tolerance.into(),
            },
        }
    }
}

/// FFI-compatible Send options
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<SendMemo>,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    pub include_fee: bool,
    /// Maximum number of proofs to include in the token
    pub max_proofs: Option<u32>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind: SendKind::OnlineExact,
            include_fee: false,
            max_proofs: None,
            metadata: HashMap::new(),
        }
    }
}

impl From<SendOptions> for cdk::wallet::SendOptions {
    fn from(opts: SendOptions) -> Self {
        cdk::wallet::SendOptions {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.and_then(|c| c.try_into().ok()),
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as usize),
            metadata: opts.metadata,
        }
    }
}

impl From<cdk::wallet::SendOptions> for SendOptions {
    fn from(opts: cdk::wallet::SendOptions) -> Self {
        Self {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.map(Into::into),
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as u32),
            metadata: opts.metadata,
        }
    }
}

impl SendOptions {
    /// Convert SendOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendOptions from JSON string
#[uniffi::export]
pub fn decode_send_options(json: String) -> Result<SendOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendOptions to JSON string
#[uniffi::export]
pub fn encode_send_options(options: SendOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible SecretKey
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct SecretKey {
    /// Hex-encoded secret key (64 characters)
    pub hex: String,
}

impl SecretKey {
    /// Create a new SecretKey from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::InvalidHex {
                msg: "Secret key hex must be exactly 64 characters (32 bytes)".to_string(),
            });
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::InvalidHex {
                msg: "Secret key hex contains invalid characters".to_string(),
            });
        }

        Ok(Self { hex })
    }

    /// Generate a random secret key
    pub fn random() -> Self {
        use cdk::nuts::SecretKey as CdkSecretKey;
        let secret_key = CdkSecretKey::generate();
        Self {
            hex: secret_key.to_secret_hex(),
        }
    }
}

impl From<SecretKey> for cdk::nuts::SecretKey {
    fn from(key: SecretKey) -> Self {
        // This will panic if hex is invalid, but we validate in from_hex()
        cdk::nuts::SecretKey::from_hex(&key.hex).expect("Invalid secret key hex")
    }
}

impl From<cdk::nuts::SecretKey> for SecretKey {
    fn from(key: cdk::nuts::SecretKey) -> Self {
        Self {
            hex: key.to_secret_hex(),
        }
    }
}

/// FFI-compatible Receive options
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ReceiveOptions {
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages for HTLC conditions
    pub preimages: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl Default for ReceiveOptions {
    fn default() -> Self {
        Self {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: Vec::new(),
            preimages: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

impl From<ReceiveOptions> for cdk::wallet::ReceiveOptions {
    fn from(opts: ReceiveOptions) -> Self {
        cdk::wallet::ReceiveOptions {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            preimages: opts.preimages,
            metadata: opts.metadata,
        }
    }
}

impl From<cdk::wallet::ReceiveOptions> for ReceiveOptions {
    fn from(opts: cdk::wallet::ReceiveOptions) -> Self {
        Self {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            preimages: opts.preimages,
            metadata: opts.metadata,
        }
    }
}

impl ReceiveOptions {
    /// Convert ReceiveOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ReceiveOptions from JSON string
#[uniffi::export]
pub fn decode_receive_options(json: String) -> Result<ReceiveOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ReceiveOptions to JSON string
#[uniffi::export]
pub fn encode_receive_options(options: ReceiveOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible PreparedSend
#[derive(Debug, uniffi::Object)]
pub struct PreparedSend {
    inner: Mutex<Option<cdk::wallet::PreparedSend>>,
    id: String,
    amount: Amount,
    proofs: Proofs,
}

impl From<cdk::wallet::PreparedSend> for PreparedSend {
    fn from(prepared: cdk::wallet::PreparedSend) -> Self {
        let id = format!("{:?}", prepared); // Use debug format as ID
        let amount = prepared.amount().into();
        let proofs = prepared
            .proofs()
            .iter()
            .cloned()
            .map(|p| p.into())
            .collect();
        Self {
            inner: Mutex::new(Some(prepared)),
            id,
            amount,
            proofs,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PreparedSend {
    /// Get the prepared send ID
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Get the amount to send
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> Proofs {
        self.proofs.clone()
    }

    /// Get the total fee for this send operation
    pub fn fee(&self) -> Amount {
        if let Ok(guard) = self.inner.lock() {
            if let Some(ref inner) = *guard {
                inner.fee().into()
            } else {
                Amount::new(0)
            }
        } else {
            Amount::new(0)
        }
    }

    /// Confirm the prepared send and create a token
    pub async fn confirm(
        self: std::sync::Arc<Self>,
        memo: Option<String>,
    ) -> Result<Token, FfiError> {
        let inner = {
            if let Ok(mut guard) = self.inner.lock() {
                guard.take()
            } else {
                return Err(FfiError::Generic {
                    msg: "Failed to acquire lock on PreparedSend".to_string(),
                });
            }
        };

        if let Some(inner) = inner {
            let send_memo = memo.map(|m| cdk::wallet::SendMemo::for_token(&m));
            let token = inner.confirm(send_memo).await?;
            Ok(token.into())
        } else {
            Err(FfiError::Generic {
                msg: "PreparedSend has already been consumed or cancelled".to_string(),
            })
        }
    }

    /// Cancel the prepared send operation
    pub async fn cancel(self: std::sync::Arc<Self>) -> Result<(), FfiError> {
        let inner = {
            if let Ok(mut guard) = self.inner.lock() {
                guard.take()
            } else {
                return Err(FfiError::Generic {
                    msg: "Failed to acquire lock on PreparedSend".to_string(),
                });
            }
        };

        if let Some(inner) = inner {
            inner.cancel().await?;
            Ok(())
        } else {
            Err(FfiError::Generic {
                msg: "PreparedSend has already been consumed or cancelled".to_string(),
            })
        }
    }
}

/// FFI-compatible Melted result
#[derive(Debug, Clone, uniffi::Record)]
pub struct Melted {
    pub state: super::quote::QuoteState,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
    pub amount: Amount,
    pub fee_paid: Amount,
}

// MeltQuoteState is just an alias for nut05::QuoteState, so we don't need a separate implementation

impl From<cdk::types::Melted> for Melted {
    fn from(melted: cdk::types::Melted) -> Self {
        Self {
            state: melted.state.into(),
            preimage: melted.preimage,
            change: melted
                .change
                .map(|proofs| proofs.into_iter().map(|p| p.into()).collect()),
            amount: melted.amount.into(),
            fee_paid: melted.fee_paid.into(),
        }
    }
}

/// FFI-compatible MeltOptions
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum MeltOptions {
    /// MPP (Multi-Part Payments) options
    Mpp { amount: Amount },
    /// Amountless options
    Amountless { amount_msat: Amount },
}

impl From<MeltOptions> for cdk::nuts::MeltOptions {
    fn from(opts: MeltOptions) -> Self {
        match opts {
            MeltOptions::Mpp { amount } => {
                let cdk_amount: cdk::Amount = amount.into();
                cdk::nuts::MeltOptions::new_mpp(cdk_amount)
            }
            MeltOptions::Amountless { amount_msat } => {
                let cdk_amount: cdk::Amount = amount_msat.into();
                cdk::nuts::MeltOptions::new_amountless(cdk_amount)
            }
        }
    }
}

impl From<cdk::nuts::MeltOptions> for MeltOptions {
    fn from(opts: cdk::nuts::MeltOptions) -> Self {
        match opts {
            cdk::nuts::MeltOptions::Mpp { mpp } => MeltOptions::Mpp {
                amount: mpp.amount.into(),
            },
            cdk::nuts::MeltOptions::Amountless { amountless } => MeltOptions::Amountless {
                amount_msat: amountless.amount_msat.into(),
            },
        }
    }
}
