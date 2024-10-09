//! Bolt12
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Amount;

use super::{nut05::MeltRequestTrait, BlindedMessage, CurrencyUnit, PaymentMethod, Proofs};

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown quote state")]
    UnknownState,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
}

/// Melt quote request [NUT-18]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt12Request {
    /// Bolt12 invoice to be paid
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub amount: Option<Amount>,
}

/// Melt Bolt12 Request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt12Request {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl MeltRequestTrait for MeltBolt12Request {
    type Err = Error;

    fn get_quote_id(&self) -> &str {
        &self.quote
    }

    fn get_inputs(&self) -> &Proofs {
        &self.inputs
    }

    fn get_outputs(&self) -> &Option<Vec<BlindedMessage>> {
        &self.outputs
    }

    fn inputs_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(self.inputs.iter().map(|proof| proof.amount))
            .map_err(|_| Error::AmountOverflow)
    }

    fn outputs_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(
            self.outputs
                .as_ref()
                .unwrap_or(&vec![])
                .iter()
                .map(|proof| proof.amount),
        )
        .map_err(|_| Error::AmountOverflow)
    }

    fn get_payment_method(&self) -> PaymentMethod {
        PaymentMethod::Bolt12
    }
}
