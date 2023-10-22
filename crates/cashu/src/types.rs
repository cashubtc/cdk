//! Types for `cashu-crab`

use serde::{Deserialize, Serialize};

use crate::nuts::nut00::{mint, Proofs};
use crate::nuts::nut02::Id;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofsStatus {
    pub spendable: mint::Proofs,
    pub spent: mint::Proofs,
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
    pub valid_from: u64,
    pub valid_to: Option<u64>,
    pub secret: String,
    pub derivation_path: String,
    pub max_order: u8,
}
