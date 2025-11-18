//! Transaction-related FFI types

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::keys::PublicKey;
use super::mint::MintUrl;
use super::proof::Proofs;
use crate::error::FfiError;

/// FFI-compatible Transaction
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Transaction {
    /// Transaction ID
    pub id: TransactionId,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Transaction direction
    pub direction: TransactionDirection,
    /// Amount
    pub amount: Amount,
    /// Fee
    pub fee: Amount,
    /// Currency Unit
    pub unit: CurrencyUnit,
    /// Proof Ys (Y values from proofs)
    pub ys: Vec<PublicKey>,
    /// Unix timestamp
    pub timestamp: u64,
    /// Memo
    pub memo: Option<String>,
    /// User-defined metadata
    pub metadata: HashMap<String, String>,
    /// Quote ID if this is a mint or melt transaction
    pub quote_id: Option<String>,
    /// Payment request (e.g., BOLT11 invoice, BOLT12 offer)
    pub payment_request: Option<String>,
    /// Payment proof (e.g., preimage for Lightning melt transactions)
    pub payment_proof: Option<String>,
}

impl From<cdk::wallet::types::Transaction> for Transaction {
    fn from(tx: cdk::wallet::types::Transaction) -> Self {
        Self {
            id: tx.id().into(),
            mint_url: tx.mint_url.into(),
            direction: tx.direction.into(),
            amount: tx.amount.into(),
            fee: tx.fee.into(),
            unit: tx.unit.into(),
            ys: tx.ys.into_iter().map(Into::into).collect(),
            timestamp: tx.timestamp,
            memo: tx.memo,
            metadata: tx.metadata,
            quote_id: tx.quote_id,
            payment_request: tx.payment_request,
            payment_proof: tx.payment_proof,
        }
    }
}

/// Convert FFI Transaction to CDK Transaction
impl TryFrom<Transaction> for cdk::wallet::types::Transaction {
    type Error = FfiError;

    fn try_from(tx: Transaction) -> Result<Self, Self::Error> {
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, _> =
            tx.ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;

        Ok(Self {
            mint_url: tx.mint_url.try_into()?,
            direction: tx.direction.into(),
            amount: tx.amount.into(),
            fee: tx.fee.into(),
            unit: tx.unit.into(),
            ys: cdk_ys,
            timestamp: tx.timestamp,
            memo: tx.memo,
            metadata: tx.metadata,
            quote_id: tx.quote_id,
            payment_request: tx.payment_request,
            payment_proof: tx.payment_proof,
        })
    }
}

impl Transaction {
    /// Convert Transaction to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Transaction from JSON string
#[uniffi::export]
pub fn decode_transaction(json: String) -> Result<Transaction, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Transaction to JSON string
#[uniffi::export]
pub fn encode_transaction(transaction: Transaction) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&transaction)?)
}

/// Check if a transaction matches the given filter conditions
#[uniffi::export]
pub fn transaction_matches_conditions(
    transaction: &Transaction,
    mint_url: Option<MintUrl>,
    direction: Option<TransactionDirection>,
    unit: Option<CurrencyUnit>,
) -> Result<bool, FfiError> {
    let cdk_transaction: cdk::wallet::types::Transaction = transaction.clone().try_into()?;
    let cdk_mint_url = mint_url.map(|url| url.try_into()).transpose()?;
    let cdk_direction = direction.map(Into::into);
    let cdk_unit = unit.map(Into::into);
    Ok(cdk_transaction.matches_conditions(&cdk_mint_url, &cdk_direction, &cdk_unit))
}

/// FFI-compatible TransactionDirection
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TransactionDirection {
    /// Incoming transaction (i.e., receive or mint)
    Incoming,
    /// Outgoing transaction (i.e., send or melt)
    Outgoing,
}

impl From<cdk::wallet::types::TransactionDirection> for TransactionDirection {
    fn from(direction: cdk::wallet::types::TransactionDirection) -> Self {
        match direction {
            cdk::wallet::types::TransactionDirection::Incoming => TransactionDirection::Incoming,
            cdk::wallet::types::TransactionDirection::Outgoing => TransactionDirection::Outgoing,
        }
    }
}

impl From<TransactionDirection> for cdk::wallet::types::TransactionDirection {
    fn from(direction: TransactionDirection) -> Self {
        match direction {
            TransactionDirection::Incoming => cdk::wallet::types::TransactionDirection::Incoming,
            TransactionDirection::Outgoing => cdk::wallet::types::TransactionDirection::Outgoing,
        }
    }
}

/// FFI-compatible TransactionId
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct TransactionId {
    /// Hex-encoded transaction ID (64 characters)
    pub hex: String,
}

impl TransactionId {
    /// Create a new TransactionId from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::InvalidHex {
                msg: "Transaction ID hex must be exactly 64 characters (32 bytes)".to_string(),
            });
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::InvalidHex {
                msg: "Transaction ID hex contains invalid characters".to_string(),
            });
        }

        Ok(Self { hex })
    }

    /// Create from proofs
    pub fn from_proofs(proofs: &Proofs) -> Result<Self, FfiError> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.iter().map(|p| p.clone().try_into()).collect();
        let cdk_proofs = cdk_proofs?;
        let id = cdk::wallet::types::TransactionId::from_proofs(cdk_proofs)?;
        Ok(Self {
            hex: id.to_string(),
        })
    }
}

impl From<cdk::wallet::types::TransactionId> for TransactionId {
    fn from(id: cdk::wallet::types::TransactionId) -> Self {
        Self {
            hex: id.to_string(),
        }
    }
}

impl TryFrom<TransactionId> for cdk::wallet::types::TransactionId {
    type Error = FfiError;

    fn try_from(id: TransactionId) -> Result<Self, Self::Error> {
        cdk::wallet::types::TransactionId::from_hex(&id.hex)
            .map_err(|e| FfiError::InvalidHex { msg: e.to_string() })
    }
}

/// FFI-compatible AuthProof
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct AuthProof {
    /// Keyset ID
    pub keyset_id: String,
    /// Secret message
    pub secret: String,
    /// Unblinded signature (C)
    pub c: String,
    /// Y value (hash_to_curve of secret)
    pub y: String,
}

impl From<cdk::nuts::AuthProof> for AuthProof {
    fn from(auth_proof: cdk::nuts::AuthProof) -> Self {
        Self {
            keyset_id: auth_proof.keyset_id.to_string(),
            secret: auth_proof.secret.to_string(),
            c: auth_proof.c.to_string(),
            y: auth_proof
                .y()
                .map(|y| y.to_string())
                .unwrap_or_else(|_| "".to_string()),
        }
    }
}

impl TryFrom<AuthProof> for cdk::nuts::AuthProof {
    type Error = FfiError;

    fn try_from(auth_proof: AuthProof) -> Result<Self, Self::Error> {
        use std::str::FromStr;
        Ok(Self {
            keyset_id: cdk::nuts::Id::from_str(&auth_proof.keyset_id)
                .map_err(|e| FfiError::Serialization { msg: e.to_string() })?,
            secret: {
                use std::str::FromStr;
                cdk::secret::Secret::from_str(&auth_proof.secret)
                    .map_err(|e| FfiError::Serialization { msg: e.to_string() })?
            },
            c: cdk::nuts::PublicKey::from_str(&auth_proof.c)
                .map_err(|e| FfiError::InvalidCryptographicKey { msg: e.to_string() })?,
            dleq: None, // FFI doesn't expose DLEQ proofs for simplicity
        })
    }
}

impl AuthProof {
    /// Convert AuthProof to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode AuthProof from JSON string
#[uniffi::export]
pub fn decode_auth_proof(json: String) -> Result<AuthProof, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode AuthProof to JSON string
#[uniffi::export]
pub fn encode_auth_proof(proof: AuthProof) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&proof)?)
}
