//! Melt types

use cashu::{
    Amount, BlindedMessage, MeltBolt11Request, MeltBolt12Request, MeltQuoteBolt11Request,
    MeltQuoteBolt12Request, PaymentMethod, Proofs,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Error;

/// Melt quote request enum for different types of quotes
///
/// This enum represents the different types of melt quote requests
/// that can be made, either BOLT11 or BOLT12.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeltQuoteRequest {
    /// Lightning Network BOLT11 invoice request
    Bolt11(MeltQuoteBolt11Request),
    /// Lightning Network BOLT12 offer request
    Bolt12(MeltQuoteBolt12Request),
}

impl From<MeltQuoteBolt11Request> for MeltQuoteRequest {
    fn from(request: MeltQuoteBolt11Request) -> Self {
        MeltQuoteRequest::Bolt11(request)
    }
}

impl From<MeltQuoteBolt12Request> for MeltQuoteRequest {
    fn from(request: MeltQuoteBolt12Request) -> Self {
        MeltQuoteRequest::Bolt12(request)
    }
}

/// A request to melt proofs in exchange for an external payment
///
/// This enum represents different types of melt requests based on the payment method.
/// It can handle both Lightning BOLT11 and BOLT12 invoice payments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeltRequest {
    /// A melt request for a BOLT11 Lightning invoice payment
    Bolt11(MeltBolt11Request<Uuid>),
    /// A melt request for a BOLT12 Lightning offer payment
    Bolt12(MeltBolt12Request<Uuid>),
}

/// Conversion from a BOLT11 melt request to a generic melt request
impl From<MeltBolt11Request<Uuid>> for MeltRequest {
    fn from(request: MeltBolt11Request<Uuid>) -> Self {
        Self::Bolt11(request)
    }
}

/// Conversion from a BOLT12 melt request to a generic melt request
impl From<MeltBolt12Request<Uuid>> for MeltRequest {
    fn from(request: MeltBolt12Request<Uuid>) -> Self {
        Self::Bolt12(request)
    }
}

impl MeltRequest {
    /// Get the quote ID
    pub fn quote_id(&self) -> &Uuid {
        match self {
            Self::Bolt11(request) => request.quote(),
            Self::Bolt12(request) => request.quote(),
        }
    }

    /// Get inputs (proofs)
    pub fn inputs(&self) -> &Proofs {
        match self {
            Self::Bolt11(request) => request.inputs(),
            Self::Bolt12(request) => request.inputs(),
        }
    }

    /// Get outputs (blinded messages for change)
    pub fn outputs(&self) -> &Option<Vec<BlindedMessage>> {
        match self {
            Self::Bolt11(request) => request.outputs(),
            Self::Bolt12(request) => request.outputs(),
        }
    }

    /// Total amount of inputs
    pub fn inputs_amount(&self) -> Result<Amount, Error> {
        match self {
            Self::Bolt11(request) => {
                Amount::try_sum(request.inputs().iter().map(|proof| proof.amount))
                    .map_err(|_| Error::AmountOverflow)
            }
            Self::Bolt12(request) => {
                Amount::try_sum(request.inputs().iter().map(|proof| proof.amount))
                    .map_err(|_| Error::AmountOverflow)
            }
        }
    }

    /// Total amount of outputs
    pub fn outputs_amount(&self) -> Result<Amount, Error> {
        match self {
            Self::Bolt11(request) => Amount::try_sum(
                request
                    .outputs()
                    .as_ref()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|proof| proof.amount),
            )
            .map_err(|_| Error::AmountOverflow),
            Self::Bolt12(request) => Amount::try_sum(
                request
                    .outputs()
                    .as_ref()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|proof| proof.amount),
            )
            .map_err(|_| Error::AmountOverflow),
        }
    }

    /// Get payment method
    pub fn get_payment_method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Bolt11,
            Self::Bolt12(_) => PaymentMethod::Bolt12,
        }
    }
}
