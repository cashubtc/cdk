//! Melting Tokens
// https://github.com/cashubtc/nuts/blob/main/05.md

use serde::{Deserialize, Serialize};

use super::nut00::Proofs;
use crate::error::Error;
use crate::{Amount, Bolt11Invoice};

/// Check Fees Response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFeesResponse {
    /// Expected Mac Fee in satoshis    
    pub fee: Amount,
}

/// Check Fees request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFeesRequest {
    /// Lighting Invoice
    pub pr: Bolt11Invoice,
}

/// Melt Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltRequest {
    pub proofs: Proofs,
    /// bollt11
    pub pr: Bolt11Invoice,
}

impl MeltRequest {
    pub fn proofs_amount(&self) -> Amount {
        self.proofs.iter().map(|proof| proof.amount).sum()
    }

    pub fn invoice_amount(&self) -> Result<Amount, Error> {
        match self.pr.amount_milli_satoshis() {
            Some(value) => Ok(Amount::from_sat(value)),
            None => Err(Error::InvoiceAmountUndefined),
        }
    }
}

/// Melt Response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltResponse {
    pub paid: bool,
    pub preimage: Option<String>,
}
