//! Receive saga types

use core::fmt;

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::{Amount, Error};

/// States specific to receive saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveSagaState {
    /// Input proofs validated and stored as pending, ready to swap for new proofs
    ProofsPending,
    /// Swap request sent to mint, awaiting signatures for new proofs
    SwapRequested,
    /// Proofs accepted offline (DLEQ-verified), stored as PendingReceive awaiting
    /// `finalize_pending_receives`. This saga holds the original token string so
    /// the memo and proof grouping survive until the wallet comes back online.
    /// Recovery must NOT compensate this state — `finalize_pending_receives` owns it.
    OfflinePendingReceive,
}

impl std::fmt::Display for ReceiveSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReceiveSagaState::ProofsPending => write!(f, "proofs_pending"),
            ReceiveSagaState::SwapRequested => write!(f, "swap_requested"),
            ReceiveSagaState::OfflinePendingReceive => write!(f, "offline_pending_receive"),
        }
    }
}

impl std::str::FromStr for ReceiveSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_pending" => Ok(ReceiveSagaState::ProofsPending),
            "swap_requested" => Ok(ReceiveSagaState::SwapRequested),
            "offline_pending_receive" => Ok(ReceiveSagaState::OfflinePendingReceive),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Receive operations
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveOperationData {
    /// Token to receive
    pub token: Option<String>,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Amount received
    pub amount: Option<Amount>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}

impl fmt::Debug for ReceiveOperationData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveOperationData")
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("counter_start", &self.counter_start)
            .field("counter_end", &self.counter_end)
            .field("amount", &self.amount)
            .field(
                "blinded_message_count",
                &self.blinded_messages.as_ref().map(Vec::len),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cashu::MintUrl;
    use uuid::Uuid;

    use super::{ReceiveOperationData, ReceiveSagaState};
    use crate::wallet::{OperationData, WalletSaga, WalletSagaState};
    use crate::{Amount, CurrencyUnit};

    const TOKEN_MARKER: &str = "cashuB_super_secret_bearer_token_marker";

    #[allow(clippy::use_debug)]
    #[test]
    fn receive_operation_data_and_wallet_saga_debug_redact_token() {
        let data = ReceiveOperationData {
            token: Some(TOKEN_MARKER.to_string()),
            counter_start: Some(4),
            counter_end: Some(5),
            amount: Some(Amount::from(1)),
            blinded_messages: None,
        };

        let data_debug = format!("{data:?}");
        assert!(!data_debug.contains(TOKEN_MARKER));
        assert!(data_debug.contains("[redacted]"));

        let saga = WalletSaga::new(
            Uuid::nil(),
            WalletSagaState::Receive(ReceiveSagaState::ProofsPending),
            Amount::from(1),
            MintUrl::from_str("https://example.com").expect("valid mint URL"),
            CurrencyUnit::Sat,
            OperationData::Receive(data),
        );
        let saga_debug = format!("{saga:?}");
        assert!(!saga_debug.contains(TOKEN_MARKER));
        assert!(saga_debug.contains("[redacted]"));
    }
}
