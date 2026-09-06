//! Token redemption requests and receipts.

use std::collections::HashMap;
use std::fmt;

use crate::nuts::Token;
use crate::wallet::advanced::ReceiveAdvancedOptions;
use crate::wallet::{ReceiveOptions, Wallet, WalletIdentity};
use crate::{Amount, Error};

/// High-level request to receive an encoded Cashu token.
#[derive(Clone)]
pub struct ReceiveRequest {
    /// Encoded Cashu token to redeem.
    pub token: String,
    /// Application metadata stored with the transaction.
    pub metadata: HashMap<String, String>,
    /// Protocol-specific redemption credentials and denomination controls.
    pub(crate) advanced: ReceiveAdvancedOptions,
}

impl ReceiveRequest {
    /// Create a receive request with no application metadata.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            metadata: HashMap::new(),
            advanced: ReceiveAdvancedOptions::default(),
        }
    }

    /// Decode a binary Cashu token into a receive request.
    pub fn from_bytes(token: &[u8]) -> Result<Self, Error> {
        Ok(Self::new(Token::try_from(&token.to_vec())?.to_string()))
    }

    /// Apply credentials or denomination controls for an expert workflow.
    pub fn with_advanced(mut self, advanced: ReceiveAdvancedOptions) -> Self {
        self.advanced = advanced;
        self
    }
}

impl fmt::Debug for ReceiveRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveRequest")
            .field("token", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

/// Receipt for a successfully redeemed token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveReceipt {
    /// Value credited to the wallet.
    pub amount: Amount,
    /// Wallet that accepted the token.
    pub wallet: WalletIdentity,
}

impl Wallet {
    /// Validate and redeem an encoded token into this wallet.
    pub async fn receive(&self, request: ReceiveRequest) -> Result<ReceiveReceipt, Error> {
        let options = ReceiveOptions {
            metadata: request.metadata,
            amount_split_target: request.advanced.amount_split_target,
            p2pk_signing_keys: request.advanced.p2pk_signing_keys,
            preimages: request.advanced.preimages,
        };
        let (operation_id, amount) = self
            .receive_token_with_operation(&request.token, options)
            .await?;
        let receipt = ReceiveReceipt {
            amount,
            wallet: self.identity(),
        };
        self.publish_balance_event().await;
        self.publish_transaction_events(operation_id).await;
        Ok(receipt)
    }
}
