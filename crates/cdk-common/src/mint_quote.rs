//! Unified Mint Quote types for mint use-cases.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut04::{MintQuoteCustomRequest, MintQuoteCustomResponse};
use crate::nuts::nut23::{MintQuoteBolt11Request, MintQuoteBolt11Response};
use crate::nuts::nut25::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use crate::{Amount, CurrencyUnit, PaymentMethod, PublicKey};

/// Unified mint quote request for all payment methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MintQuoteRequest {
    /// Bolt11 (Lightning invoice)
    Bolt11(MintQuoteBolt11Request),
    /// Bolt12 (Offers)
    Bolt12(MintQuoteBolt12Request),
    /// Custom payment method
    Custom {
        /// Payment method identifier
        method: PaymentMethod,
        /// Payment method specific request
        request: MintQuoteCustomRequest,
    },
}

impl From<MintQuoteBolt11Request> for MintQuoteRequest {
    fn from(request: MintQuoteBolt11Request) -> Self {
        MintQuoteRequest::Bolt11(request)
    }
}

impl From<MintQuoteBolt12Request> for MintQuoteRequest {
    fn from(request: MintQuoteBolt12Request) -> Self {
        MintQuoteRequest::Bolt12(request)
    }
}

impl MintQuoteRequest {
    /// Returns the payment method for this request.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Custom { method, .. } => method.clone(),
        }
    }

    /// Returns the amount for this request when present.
    pub fn amount(&self) -> Option<Amount> {
        match self {
            Self::Bolt11(request) => Some(request.amount),
            Self::Bolt12(request) => request.amount,
            Self::Custom { request, .. } => Some(request.amount),
        }
    }

    /// Returns the unit for this request.
    pub fn unit(&self) -> CurrencyUnit {
        match self {
            Self::Bolt11(request) => request.unit.clone(),
            Self::Bolt12(request) => request.unit.clone(),
            Self::Custom { request, .. } => request.unit.clone(),
        }
    }

    /// Returns the payment method for this request.
    pub fn payment_method(&self) -> PaymentMethod {
        self.method()
    }

    /// Returns the pubkey for this request when present.
    pub fn pubkey(&self) -> Option<PublicKey> {
        match self {
            Self::Bolt11(request) => request.pubkey,
            Self::Bolt12(request) => Some(request.pubkey),
            Self::Custom { request, .. } => request.pubkey,
        }
    }
}

/// Unified mint quote response for all payment methods
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub enum MintQuoteResponse<Q> {
    /// Bolt11 (Lightning invoice)
    Bolt11(MintQuoteBolt11Response<Q>),
    /// Bolt12 (Offers)
    Bolt12(MintQuoteBolt12Response<Q>),
    /// Custom payment method
    Custom {
        /// Payment method identifier
        method: PaymentMethod,
        /// Quote state, when the response source provides it
        state: Option<crate::nuts::nut23::QuoteState>,
        /// Payment method specific response
        response: MintQuoteCustomResponse<Q>,
    },
}

impl<Q> MintQuoteResponse<Q> {
    /// Returns the payment method for this response.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Custom { method, .. } => method.clone(),
        }
    }

    /// Returns the quote ID.
    pub fn quote(&self) -> &Q {
        match self {
            Self::Bolt11(r) => &r.quote,
            Self::Bolt12(r) => &r.quote,
            Self::Custom { response: r, .. } => &r.quote,
        }
    }

    /// Returns the payment request string.
    pub fn request(&self) -> &str {
        match self {
            Self::Bolt11(r) => &r.request,
            Self::Bolt12(r) => &r.request,
            Self::Custom { response: r, .. } => &r.request,
        }
    }

    /// Returns the quote state when available.
    pub fn state(&self) -> Option<crate::nuts::nut23::QuoteState> {
        match self {
            Self::Bolt11(r) => Some(r.state),
            Self::Bolt12(r) => Some({
                if r.amount_issued > Amount::ZERO {
                    crate::nuts::nut23::QuoteState::Issued
                } else if r.amount_paid >= r.amount.unwrap_or(Amount::ZERO) {
                    crate::nuts::nut23::QuoteState::Paid
                } else {
                    crate::nuts::nut23::QuoteState::Unpaid
                }
            }),
            Self::Custom { state, .. } => *state,
        }
    }

    /// Returns the quote expiry timestamp.
    pub fn expiry(&self) -> Option<u64> {
        match self {
            Self::Bolt11(r) => r.expiry,
            Self::Bolt12(r) => r.expiry,
            Self::Custom { response: r, .. } => r.expiry,
        }
    }
}
