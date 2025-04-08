//! Bolt12
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut05::MeltRequestTrait;
use super::{BlindedMessage, CurrencyUnit, MeltOptions, PaymentMethod, Proofs};
use crate::Amount;

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
    pub options: Option<MeltOptions>,
}

/// Melt Bolt12 Request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltBolt12Request<Q> {
    /// Quote ID
    pub quote: Q,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl<Q> MeltRequestTrait<Q> for MeltBolt12Request<Q>
where
    Q: ToString,
{
    fn quote_id(&self) -> &Q {
        &self.quote
    }

    fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    fn outputs(&self) -> &Option<Vec<BlindedMessage>> {
        &self.outputs
    }

    fn inputs_amount(&self) -> Result<Amount, crate::nuts::nut05::Error> {
        Amount::try_sum(self.inputs.iter().map(|proof| proof.amount))
            .map_err(|_| crate::nut05::Error::AmountOverflow)
    }

    fn outputs_amount(&self) -> Result<Amount, crate::nut05::Error> {
        Amount::try_sum(
            self.outputs
                .as_ref()
                .unwrap_or(&vec![])
                .iter()
                .map(|proof| proof.amount),
        )
        .map_err(|_| crate::nut05::Error::AmountOverflow)
    }

    fn get_payment_method(&self) -> PaymentMethod {
        PaymentMethod::Bolt12
    }
}
