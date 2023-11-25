//! Types for `cashu-crab`

use serde::{Deserialize, Serialize};

use crate::nuts::{Id, KeySetInfo, Proofs};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofsStatus {
    pub spendable: Proofs,
    pub spent: Proofs,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendProofs {
    pub change_proofs: Proofs,
    pub send_proofs: Proofs,
}

/// Melt response with proofs
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Melted {
    pub paid: bool,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
}

/// Possible states of an invoice
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum InvoiceStatus {
    Unpaid,
    Paid,
    Expired,
    InFlight,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysetInfo {
    pub id: Id,
    pub symbol: String,
    pub valid_from: u64,
    pub valid_to: Option<u64>,
    pub derivation_path: String,
    pub max_order: u8,
}

impl From<KeysetInfo> for KeySetInfo {
    fn from(keyset_info: KeysetInfo) -> Self {
        Self {
            id: keyset_info.id,
            symbol: keyset_info.symbol,
        }
    }
}
