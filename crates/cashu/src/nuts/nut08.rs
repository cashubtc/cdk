//! Lightning fee return
// https://github.com/cashubtc/nuts/blob/main/08.md

use lightning_invoice::Bolt11Invoice;
use serde::{Deserialize, Serialize};

use super::nut00::{BlindedMessage, BlindedSignature, Proofs};
use crate::error::Error;
use crate::Amount;

/// Melt Request [NUT-08]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltRequest {
    pub proofs: Proofs,
    /// bollt11
    pub pr: Bolt11Invoice,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of blindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl MeltRequest {
    pub fn proofs_amount(&self) -> Amount {
        self.proofs.iter().map(|proof| proof.amount).sum()
    }

    pub fn invoice_amount(&self) -> Result<Amount, Error> {
        match self.pr.amount_milli_satoshis() {
            Some(value) => Ok(Amount::from_msat(value)),
            None => Err(Error::InvoiceAmountUndefined),
        }
    }
}

/// Melt Response [NUT-08]
/// Lightning fee return [NUT-08] if change is defined
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltResponse {
    pub paid: bool,
    pub preimage: Option<String>,
    pub change: Option<Vec<BlindedSignature>>,
}

impl MeltResponse {
    pub fn change_amount(&self) -> Amount {
        match &self.change {
            Some(change) => change.iter().map(|c| c.amount).sum(),
            None => Amount::ZERO,
        }
    }
}
