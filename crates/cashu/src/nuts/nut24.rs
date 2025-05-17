//! Bolt12
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BlindedMessage, CurrencyUnit, MeltOptions, Proofs};

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
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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

impl<Q> MeltBolt12Request<Q> {
    /// Get quote
    pub fn quote(&self) -> &Q {
        &self.quote
    }

    /// Get inputs (proofs)
    pub fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    /// Get outputs (blinded messages for change)
    pub fn outputs(&self) -> &Option<Vec<BlindedMessage>> {
        &self.outputs
    }
}
