//! Wallet saga types for crash-tolerant recovery
//!
//! Sagas represent in-progress wallet operations that need to survive crashes.
//! They use **optimistic locking** via the `version` field to handle concurrent
//! access from multiple wallet instances safely.
//!
//! # Optimistic Locking
//!
//! When multiple wallet instances share the same database (e.g., mobile app
//! backgrounded while desktop app runs), they might both try to recover the
//! same incomplete saga. Optimistic locking prevents conflicts:
//!
//! 1. Each saga has a `version` number starting at 0
//! 2. When updating, the database checks: `WHERE id = ? AND version = ?`
//! 3. If the version matches, the update succeeds and `version` increments
//! 4. If the version doesn't match, another instance modified it first
//!
//! This is preferable to pessimistic locking (mutexes) because:
//! - Works across process boundaries (multiple wallet instances)
//! - No deadlock risk
//! - No lock expiration/cleanup needed
//! - Conflicts are rare in practice (sagas are short-lived)
//!
//! Instance A reads saga with version=1
//! Instance B reads saga with version=1
//! Instance A updates successfully, version becomes 2
//! Instance B's update fails (version mismatch) - it knows to skip

use serde::{Deserialize, Serialize};

use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
use crate::wallet::OperationKind;
use crate::Amount;

mod issue;
mod melt;
mod receive;
mod send;
mod swap;

pub use issue::{IssueSagaState, MintOperationData};
pub use melt::{MeltOperationData, MeltSagaState};
pub use receive::{ReceiveOperationData, ReceiveSagaState};
pub use send::{SendOperationData, SendSagaState};
pub use swap::{SwapOperationData, SwapSagaState};

/// Wallet saga state for different operation types
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum WalletSagaState {
    /// Send saga states
    Send(SendSagaState),
    /// Receive saga states
    Receive(ReceiveSagaState),
    /// Swap saga states
    Swap(SwapSagaState),
    /// Mint (issue) saga states
    Issue(IssueSagaState),
    /// Melt saga states
    Melt(MeltSagaState),
}

impl WalletSagaState {
    /// Get the operation kind
    pub fn kind(&self) -> OperationKind {
        match self {
            WalletSagaState::Send(_) => OperationKind::Send,
            WalletSagaState::Receive(_) => OperationKind::Receive,
            WalletSagaState::Swap(_) => OperationKind::Swap,
            WalletSagaState::Issue(_) => OperationKind::Mint,
            WalletSagaState::Melt(_) => OperationKind::Melt,
        }
    }

    /// Get string representation of the inner state
    pub fn state_str(&self) -> &'static str {
        match self {
            WalletSagaState::Send(s) => match s {
                SendSagaState::ProofsReserved => "proofs_reserved",
                SendSagaState::TokenCreated => "token_created",
                SendSagaState::RollingBack => "rolling_back",
            },
            WalletSagaState::Receive(s) => match s {
                ReceiveSagaState::ProofsPending => "proofs_pending",
                ReceiveSagaState::SwapRequested => "swap_requested",
            },
            WalletSagaState::Swap(s) => match s {
                SwapSagaState::ProofsReserved => "proofs_reserved",
                SwapSagaState::SwapRequested => "swap_requested",
            },
            WalletSagaState::Issue(s) => match s {
                IssueSagaState::SecretsPrepared => "secrets_prepared",
                IssueSagaState::MintRequested => "mint_requested",
            },
            WalletSagaState::Melt(s) => match s {
                MeltSagaState::ProofsReserved => "proofs_reserved",
                MeltSagaState::MeltRequested => "melt_requested",
                MeltSagaState::PaymentPending => "payment_pending",
            },
        }
    }
}

/// Operation data enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum OperationData {
    /// Send operation data
    Send(SendOperationData),
    /// Receive operation data
    Receive(ReceiveOperationData),
    /// Swap operation data
    Swap(SwapOperationData),
    /// Mint operation data
    Mint(MintOperationData),
    /// Melt operation data
    Melt(MeltOperationData),
}

impl OperationData {
    /// Get the operation kind
    pub fn kind(&self) -> OperationKind {
        match self {
            OperationData::Send(_) => OperationKind::Send,
            OperationData::Receive(_) => OperationKind::Receive,
            OperationData::Swap(_) => OperationKind::Swap,
            OperationData::Mint(_) => OperationKind::Mint,
            OperationData::Melt(_) => OperationKind::Melt,
        }
    }
}

/// Wallet saga for crash-tolerant recovery.
///
/// Sagas represent in-progress wallet operations that need to survive crashes.
/// They use **optimistic locking** via the `version` field to handle concurrent
/// access from multiple wallet instances safely.
///
/// # Optimistic Locking
///
/// When multiple wallet instances share the same database (e.g., mobile app
/// backgrounded while desktop app runs), they might both try to recover the
/// same incomplete saga. Optimistic locking prevents conflicts:
///
/// 1. Each saga has a `version` number starting at 0
/// 2. When updating, the database checks: `WHERE id = ? AND version = ?`
/// 3. If the version matches, the update succeeds and `version` increments
/// 4. If the version doesn't match, another instance modified it first
///
/// This is preferable to pessimistic locking (mutexes) because:
/// - Works across process boundaries (multiple wallet instances)
/// - No deadlock risk
/// - No lock expiration/cleanup needed
/// - Conflicts are rare in practice (sagas are short-lived)
///
/// Instance A reads saga with version=1
/// Instance B reads saga with version=1
/// Instance A updates successfully, version becomes 2
/// Instance B's update fails (version mismatch) - it knows to skip
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletSaga {
    /// Unique operation ID
    pub id: uuid::Uuid,
    /// Operation kind (derived from state)
    pub kind: OperationKind,
    /// Saga state (operation-specific)
    pub state: WalletSagaState,
    /// Amount involved in the operation
    pub amount: Amount,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Quote ID (for mint/melt operations)
    pub quote_id: Option<String>,
    /// Creation timestamp (unix seconds)
    pub created_at: u64,
    /// Last update timestamp (unix seconds)
    pub updated_at: u64,
    /// Operation-specific data
    pub data: OperationData,
    /// Version number for optimistic locking.
    ///
    /// Incremented on each update. Used to detect concurrent modifications:
    /// - If update succeeds: this instance "won" the race
    /// - If update fails (version mismatch): another instance modified it
    ///
    /// Recovery code should treat version conflicts as "someone else handled it"
    /// and skip to the next saga rather than retrying.
    pub version: u32,
}

impl WalletSaga {
    /// Create a new wallet saga.
    ///
    /// The saga is created with `version = 0`. Each successful update
    /// will increment the version for optimistic locking.
    pub fn new(
        id: uuid::Uuid,
        state: WalletSagaState,
        amount: Amount,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        data: OperationData,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let quote_id = match &data {
            OperationData::Mint(d) => Some(d.quote_id.clone()),
            OperationData::Melt(d) => Some(d.quote_id.clone()),
            _ => None,
        };

        Self {
            id,
            kind: state.kind(),
            state,
            amount,
            mint_url,
            unit,
            quote_id,
            created_at: now,
            updated_at: now,
            data,
            version: 0,
        }
    }

    /// Update the saga state and increment the version.
    ///
    /// This prepares the saga for an optimistic locking update.
    /// The database layer will verify the previous version matches
    /// before applying the update.
    pub fn update_state(&mut self, state: WalletSagaState) {
        self.state = state;
        self.kind = state.kind();
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.version += 1;
    }
}
