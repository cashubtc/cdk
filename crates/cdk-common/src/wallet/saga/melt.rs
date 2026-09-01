//! Melt saga types

use std::collections::HashMap;

use cashu::{BlindedMessage, PublicKey};
use serde::{Deserialize, Serialize};

use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
use crate::wallet::MeltQuote;
use crate::{Amount, Error, Proofs};

/// States specific to melt saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeltSagaState {
    /// Durable owner record created before the quote and proofs are reserved.
    Preparing,
    /// A durable payment plan awaiting an explicit confirm or cancel decision.
    Prepared,
    /// Confirmation started after proofs were reserved and the quote was locked.
    ProofsReserved,
    /// Melt request durably staged and potentially still in flight.
    MeltRequested,
    /// Mint acknowledged the request but payment still awaits a terminal state.
    PaymentPending,
}

/// Application-level purpose of a prepared melt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreparedMeltPurpose {
    /// A regular outgoing payment.
    #[default]
    Payment,
    /// A payment whose destination invoice funds another wallet.
    CrossMintTransfer {
        /// Destination mint.
        destination_mint_url: MintUrl,
        /// Destination currency unit.
        destination_unit: CurrencyUnit,
        /// Quote that will issue the received value at the destination.
        destination_quote_id: String,
    },
}

impl std::fmt::Display for MeltSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeltSagaState::Preparing => write!(f, "preparing"),
            MeltSagaState::Prepared => write!(f, "prepared"),
            MeltSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            MeltSagaState::MeltRequested => write!(f, "melt_requested"),
            MeltSagaState::PaymentPending => write!(f, "payment_pending"),
        }
    }
}

impl std::str::FromStr for MeltSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "preparing" => Ok(MeltSagaState::Preparing),
            "prepared" => Ok(MeltSagaState::Prepared),
            "proofs_reserved" => Ok(MeltSagaState::ProofsReserved),
            "melt_requested" => Ok(MeltSagaState::MeltRequested),
            "payment_pending" => Ok(MeltSagaState::PaymentPending),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Complete, persisted plan for a melt that is ready for confirmation.
///
/// The plan is stored with the saga so an owned operation handle only needs a
/// wallet and operation ID. Proof material is intentionally redacted from its
/// debug representation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedMeltOperationData {
    /// Quote selected for the payment.
    pub quote: MeltQuote,
    /// Proofs selected for the melt after any required pre-melt swap.
    pub proofs: Proofs,
    /// Proofs that need to be swapped first.
    pub proofs_to_swap: Proofs,
    /// Fee for the pre-melt swap.
    pub swap_fee: Amount,
    /// Input fee after the pre-melt swap.
    pub input_fee: Amount,
    /// Input fee when the caller elects to skip the pre-melt swap.
    pub input_fee_without_swap: Amount,
    /// User-defined transaction metadata.
    pub metadata: HashMap<String, String>,
    /// Why this melt was prepared.
    pub purpose: PreparedMeltPurpose,
}

impl std::fmt::Debug for PreparedMeltOperationData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMeltOperationData")
            .field("operation_quote_id", &self.quote.id)
            .field("amount", &self.quote.amount)
            .field(
                "proofs",
                &self
                    .proofs
                    .iter()
                    .map(|proof| proof.amount)
                    .collect::<Vec<_>>(),
            )
            .field(
                "proofs_to_swap",
                &self
                    .proofs_to_swap
                    .iter()
                    .map(|proof| proof.amount)
                    .collect::<Vec<_>>(),
            )
            .field("swap_fee", &self.swap_fee)
            .field("input_fee", &self.input_fee)
            .field("input_fee_without_swap", &self.input_fee_without_swap)
            .field("metadata", &self.metadata)
            .field("purpose", &self.purpose)
            .finish()
    }
}

/// Operation-specific data for Melt operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltOperationData {
    /// Quote ID
    pub quote_id: String,
    /// Amount to melt
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Change amount (if any)
    pub change_amount: Option<Amount>,
    /// User-defined metadata for the outgoing melt transaction.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Proof Ys actually sent to the melt request.
    ///
    /// Stored separately from `used_by_operation` so recovery can record the
    /// correct transaction inputs even after a pre-melt swap or after inputs
    /// were already marked spent before a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_proof_ys: Option<Vec<PublicKey>>,
    /// Blinded messages for change recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the melt,
    /// we can use these to query the mint for change signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_blinded_messages: Option<Vec<BlindedMessage>>,
}
