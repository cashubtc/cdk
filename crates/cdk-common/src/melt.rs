//! Unified Melt Quote types for melt use-cases.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut05::{MeltQuoteCustomRequest, MeltQuoteCustomResponse};
use crate::nuts::nut23::{MeltQuoteBolt11Request, MeltQuoteBolt11Response};
use crate::nuts::nut25::{MeltQuoteBolt12Request, MeltQuoteBolt12Response};
use crate::nuts::nut_onchain::{MeltQuoteOnchainRequest, MeltQuoteOnchainResponse};
use crate::{Amount, CurrencyUnit, MeltQuoteState, PaymentMethod};

/// Onchain melt quote creation response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteOnchainOptions<Q> {
    /// Available onchain quote options.
    pub quotes: Vec<MeltQuoteOnchainResponse<Q>>,
}

/// Melt quote request enum for different types of quotes
///
/// This enum represents the different types of melt quote requests
/// that can be made, either BOLT11, BOLT12, or Custom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeltQuoteRequest {
    /// Lightning Network BOLT11 invoice request
    Bolt11(MeltQuoteBolt11Request),
    /// Lightning Network BOLT12 offer request
    Bolt12(MeltQuoteBolt12Request),
    /// Onchain request
    Onchain(MeltQuoteOnchainRequest),
    /// Custom payment method request
    Custom(MeltQuoteCustomRequest),
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

impl From<MeltQuoteOnchainRequest> for MeltQuoteRequest {
    fn from(request: MeltQuoteOnchainRequest) -> Self {
        MeltQuoteRequest::Onchain(request)
    }
}

impl From<MeltQuoteCustomRequest> for MeltQuoteRequest {
    fn from(request: MeltQuoteCustomRequest) -> Self {
        MeltQuoteRequest::Custom(request)
    }
}

impl MeltQuoteRequest {
    /// Returns the payment method for this request.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Onchain(_) => PaymentMethod::Known(KnownMethod::Onchain),
            Self::Custom(request) => PaymentMethod::from(request.method.as_str()),
        }
    }
}

/// Unified melt quote response for all payment methods
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub enum MeltQuoteResponse<Q> {
    /// Bolt11 (Lightning invoice)
    Bolt11(MeltQuoteBolt11Response<Q>),
    /// Bolt12 (Offers)
    Bolt12(MeltQuoteBolt12Response<Q>),
    /// Onchain
    Onchain(MeltQuoteOnchainResponse<Q>),
    /// Custom payment method
    Custom((PaymentMethod, MeltQuoteCustomResponse<Q>)),
}

/// Melt quote creation response for all payment methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub enum MeltQuoteCreateResponse<Q> {
    /// Bolt11 (Lightning invoice)
    Bolt11(MeltQuoteBolt11Response<Q>),
    /// Bolt12 (Offers)
    Bolt12(MeltQuoteBolt12Response<Q>),
    /// Onchain quote options
    Onchain(MeltQuoteOnchainOptions<Q>),
    /// Custom payment method
    Custom((PaymentMethod, MeltQuoteCustomResponse<Q>)),
}

impl<Q> MeltQuoteResponse<Q> {
    /// Returns the payment method for this response.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Onchain(_) => PaymentMethod::Known(KnownMethod::Onchain),
            Self::Custom((method, _)) => method.clone(),
        }
    }

    /// Returns the quote ID.
    pub fn quote(&self) -> &Q {
        match self {
            Self::Bolt11(r) => &r.quote,
            Self::Bolt12(r) => &r.quote,
            Self::Onchain(r) => &r.quote,
            Self::Custom((_, r)) => &r.quote,
        }
    }

    /// Returns the quoted amount.
    pub fn amount(&self) -> Amount {
        match self {
            Self::Bolt11(r) => r.amount,
            Self::Bolt12(r) => r.amount,
            Self::Onchain(r) => r.amount,
            Self::Custom((_, r)) => r.amount,
        }
    }

    /// Returns the fee reserve.
    pub fn fee_reserve(&self) -> Amount {
        match self {
            Self::Bolt11(r) => r.fee_reserve,
            Self::Bolt12(r) => r.fee_reserve,
            Self::Onchain(r) => r.fee,
            Self::Custom((_, r)) => r.fee_reserve,
        }
    }

    /// Returns the quote state.
    pub fn state(&self) -> MeltQuoteState {
        match self {
            Self::Bolt11(r) => r.state,
            Self::Bolt12(r) => r.state,
            Self::Onchain(r) => r.state,
            Self::Custom((_, r)) => r.state,
        }
    }

    /// Returns the quote expiry timestamp.
    pub fn expiry(&self) -> u64 {
        match self {
            Self::Bolt11(r) => r.expiry,
            Self::Bolt12(r) => r.expiry,
            Self::Onchain(r) => r.expiry,
            Self::Custom((_, r)) => r.expiry,
        }
    }

    /// Returns the payment preimage.
    pub fn payment_preimage(&self) -> Option<&str> {
        match self {
            Self::Bolt11(r) => r.payment_preimage.as_deref(),
            Self::Bolt12(r) => r.payment_preimage.as_deref(),
            Self::Onchain(_) => None,
            Self::Custom((_, r)) => r.payment_preimage.as_deref(),
        }
    }

    /// Returns the change signatures when present.
    pub fn change(&self) -> Option<&Vec<crate::BlindSignature>> {
        match self {
            Self::Bolt11(r) => r.change.as_ref(),
            Self::Bolt12(r) => r.change.as_ref(),
            Self::Onchain(r) => r.change.as_ref(),
            Self::Custom((_, r)) => r.change.as_ref(),
        }
    }

    /// Returns the payment request string when present.
    pub fn request(&self) -> Option<&str> {
        match self {
            Self::Bolt11(r) => r.request.as_deref(),
            Self::Bolt12(r) => r.request.as_deref(),
            Self::Onchain(r) => Some(r.request.as_str()),
            Self::Custom((_, r)) => r.request.as_deref(),
        }
    }

    /// Returns the unit when present.
    pub fn unit(&self) -> Option<CurrencyUnit> {
        match self {
            Self::Bolt11(r) => r.unit.clone(),
            Self::Bolt12(r) => r.unit.clone(),
            Self::Onchain(r) => Some(r.unit.clone()),
            Self::Custom((_, r)) => r.unit.clone(),
        }
    }
}

impl<Q> MeltQuoteCreateResponse<Q> {
    /// Returns the payment method for this response.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Onchain(_) => PaymentMethod::Known(KnownMethod::Onchain),
            Self::Custom((method, _)) => method.clone(),
        }
    }

    /// Returns the quote ID for single-quote methods.
    pub fn quote(&self) -> Option<&Q> {
        match self {
            Self::Bolt11(r) => Some(&r.quote),
            Self::Bolt12(r) => Some(&r.quote),
            Self::Onchain(_) => None,
            Self::Custom((_, r)) => Some(&r.quote),
        }
    }
}
