//! NUT-XX: Transactions
//!
//! One endpoint carrying any NUT-10 transaction: proofs and paid mint quotes in;
//! blinded messages, one melt quote and change quotes out.

use core::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::nut00::{BlindedMessage, Proofs};
use crate::Amount;

/// Method name reserved for change quotes created by a transaction.
pub const CHANGE_METHOD: &str = "change";

/// NUT-XX settings for mint info
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Settings {
    /// `POST /v1/transaction` is served
    pub supported: bool,
    /// Fee per thousand a quote input's minimal split would carry as proofs
    #[serde(default)]
    pub quote_input_fee_ppk: u64,
}

/// A paid mint quote spent as a transaction input
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionQuoteInput {
    /// Mint quote id
    pub quote: String,
    /// Amount this transaction issues against the quote
    pub amount: Amount,
    /// Key-path signature or script-path witness over the quote input digest
    pub witness: String,
}

/// A melt quote paid by a transaction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionMeltOutput {
    /// Melt quote id
    pub quote: String,
    /// Fee reserve this transaction commits to the quote
    pub fee_reserve: Amount,
    /// Selected `fee_options` entry, only for quotes offering them (NUT-30)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_index: Option<u32>,
}

/// A change quote created by a transaction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionChangeOutput {
    /// Lock key of the change quote, a 33-byte compressed secp256k1 key
    pub pubkey: String,
    /// Fixed amount; omitted on the remainder quote, which takes the balance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
}

/// `POST /v1/transaction` request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransactionRequest {
    /// Proofs spent in full
    #[serde(default)]
    pub proof_inputs: Proofs,
    /// Paid mint quotes drawn on
    #[serde(default)]
    pub mint_quote_inputs: Vec<TransactionQuoteInput>,
    /// Outputs to sign, one keyset, no blank amounts
    #[serde(default)]
    pub blinded_outputs: Vec<BlindedMessage>,
    /// At most one melt quote
    #[serde(default)]
    pub melt_quote_outputs: Vec<TransactionMeltOutput>,
    /// Change quotes to create, at most one without an amount
    #[serde(default)]
    pub change_quote_outputs: Vec<TransactionChangeOutput>,
    /// Return once the inputs are reserved rather than waiting for the melt
    #[serde(default)]
    pub prefer_async: bool,
}

impl super::nut10::SpendingConditionVerification for TransactionRequest {
    fn inputs(&self) -> &Proofs {
        &self.proof_inputs
    }

    /// NUT-11 defines no `SIG_ALL` message for this endpoint; such inputs are rejected.
    fn sig_all_msg_to_sign(&self) -> String {
        String::new()
    }
}

/// Transaction state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionState {
    /// A melt payment is in flight
    Pending,
    /// Settled: outputs signed, quotes issued, change quotes created
    Paid,
    /// The payment failed and the inputs were released
    Failed,
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "PENDING",
            Self::Paid => "PAID",
            Self::Failed => "FAILED",
        })
    }
}

impl FromStr for TransactionState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "PAID" => Ok(Self::Paid),
            "FAILED" => Ok(Self::Failed),
            other => Err(format!("unknown transaction state: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_arrays_default_to_empty() {
        let request: TransactionRequest = serde_json::from_str(
            r#"{"change_quote_outputs":[{"pubkey":"02aa","amount":3},{"pubkey":"02bb"}]}"#,
        )
        .unwrap();
        assert!(request.proof_inputs.is_empty() && request.melt_quote_outputs.is_empty());
        assert_eq!(
            request.change_quote_outputs[0].amount,
            Some(Amount::from(3))
        );
        assert_eq!(request.change_quote_outputs[1].pubkey, "02bb");
        assert_eq!(request.change_quote_outputs[1].amount, None);
        assert!(!request.prefer_async);
        assert_eq!(
            serde_json::to_string(&TransactionState::Pending).unwrap(),
            r#""PENDING""#
        );
    }
}
