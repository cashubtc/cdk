//! Issue (mint) saga types

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::Error;

/// States specific to mint (issue) saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSagaState {
    /// Pre-mint secrets created and counter incremented, ready to request signatures
    SecretsPrepared,
    /// Mint request sent to mint, awaiting signatures for new proofs
    MintRequested,
}

impl std::fmt::Display for IssueSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueSagaState::SecretsPrepared => write!(f, "secrets_prepared"),
            IssueSagaState::MintRequested => write!(f, "mint_requested"),
        }
    }
}

impl std::str::FromStr for IssueSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "secrets_prepared" => Ok(IssueSagaState::SecretsPrepared),
            "mint_requested" => Ok(IssueSagaState::MintRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Mint operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintOperationData {
    /// Quote ID (for single mint, or first quote in batch)
    quote_id: String,
    /// Quote IDs for batch operations
    ///
    /// If present, this is a batch operation. The batch may have one or more quotes.
    /// For backward compatibility with existing sagas, check this field first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quote_ids: Option<Vec<String>>,
    /// Whether this is a batch operation
    ///
    /// True if this was created by batch_mint, false for single mint.
    /// Used to determine which endpoint to use for replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_batch: Option<bool>,
    /// Amount to mint (total for batch)
    pub amount: crate::Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the request,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}

impl MintOperationData {
    /// Create operation data for a single-quote mint operation.
    pub fn new_single(
        quote_id: String,
        amount: crate::Amount,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
        blinded_messages: Option<Vec<BlindedMessage>>,
    ) -> Self {
        Self {
            quote_ids: Some(vec![quote_id.clone()]),
            quote_id,
            is_batch: Some(false),
            amount,
            counter_start,
            counter_end,
            blinded_messages,
        }
    }

    /// Create operation data for a batch mint operation.
    pub fn new_batch(
        quote_ids: Vec<String>,
        amount: crate::Amount,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
        blinded_messages: Option<Vec<BlindedMessage>>,
    ) -> Self {
        let quote_id = quote_ids.first().cloned().unwrap_or_default();

        Self {
            quote_id,
            quote_ids: Some(quote_ids),
            is_batch: Some(true),
            amount,
            counter_start,
            counter_end,
            blinded_messages,
        }
    }

    /// Get the representative quote ID for this operation.
    pub fn primary_quote_id(&self) -> &str {
        &self.quote_id
    }

    /// Get all quote IDs for this operation.
    ///
    /// Returns quote_ids if this is a batch, otherwise wraps quote_id in a vec.
    pub fn quote_ids(&self) -> Vec<String> {
        if let Some(ref ids) = self.quote_ids {
            ids.clone()
        } else {
            vec![self.quote_id.clone()]
        }
    }

    /// Check if this is a batch operation.
    pub fn is_batch(&self) -> bool {
        self.is_batch.unwrap_or(false)
    }
}
