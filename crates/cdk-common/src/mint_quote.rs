//! Unified Mint Quote types for mint use-cases.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut04::{MintQuoteCustomRequest, MintQuoteCustomResponse};
use crate::nuts::nut23::{MintQuoteBolt11Request, MintQuoteBolt11Response, QuoteState};
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
        /// Legacy quote state, when older custom responses provide it
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<QuoteState>,
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
    pub fn state(&self) -> Option<QuoteState> {
        match self {
            Self::Bolt11(r) => Some(r.state),
            Self::Bolt12(r) => Some(quote_state_from_amounts(r.amount_paid, r.amount_issued)),
            Self::Custom {
                response, state, ..
            } => match (response.amount_paid, response.amount_issued) {
                (Some(amount_paid), Some(amount_issued)) => {
                    Some(quote_state_from_amounts(amount_paid, amount_issued))
                }
                _ => *state,
            },
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

pub(crate) fn quote_state_from_amounts(amount_paid: Amount, amount_issued: Amount) -> QuoteState {
    if amount_paid == Amount::ZERO && amount_issued == Amount::ZERO {
        return QuoteState::Unpaid;
    }

    match amount_paid.cmp(&amount_issued) {
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => QuoteState::Issued,
        std::cmp::Ordering::Greater => QuoteState::Paid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_response(
        amount_paid: Option<Amount>,
        amount_issued: Option<Amount>,
    ) -> MintQuoteResponse<String> {
        MintQuoteResponse::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            state: None,
            response: MintQuoteCustomResponse {
                quote: "quote".to_string(),
                request: "custom-request".to_string(),
                amount: Some(Amount::from(100)),
                amount_paid,
                amount_issued,
                unit: Some(CurrencyUnit::Sat),
                expiry: None,
                pubkey: None,
                extra: serde_json::Value::Null,
            },
        }
    }

    #[test]
    fn custom_state_is_derived_from_amount_counters() {
        assert_eq!(
            custom_response(Some(Amount::ZERO), Some(Amount::ZERO)).state(),
            Some(QuoteState::Unpaid)
        );
        assert_eq!(
            custom_response(Some(Amount::from(100)), Some(Amount::ZERO)).state(),
            Some(QuoteState::Paid)
        );
        assert_eq!(
            custom_response(Some(Amount::from(100)), Some(Amount::from(100))).state(),
            Some(QuoteState::Issued)
        );
    }

    #[test]
    fn custom_state_falls_back_to_legacy_state_when_amounts_are_missing() {
        let response = MintQuoteResponse::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            state: Some(QuoteState::Paid),
            response: MintQuoteCustomResponse {
                quote: "quote".to_string(),
                request: "custom-request".to_string(),
                amount: Some(Amount::from(100)),
                amount_paid: None,
                amount_issued: None,
                unit: Some(CurrencyUnit::Sat),
                expiry: None,
                pubkey: None,
                extra: serde_json::Value::Null,
            },
        };

        assert_eq!(response.state(), Some(QuoteState::Paid));
    }

    #[test]
    fn bolt12_state_uses_unissued_amount() {
        let response = MintQuoteResponse::Bolt12(MintQuoteBolt12Response {
            quote: "quote".to_string(),
            request: "bolt12-request".to_string(),
            amount: Some(Amount::from(100)),
            unit: CurrencyUnit::Sat,
            expiry: None,
            pubkey: PublicKey::from_hex(
                "02a8cda4cf448bfce9a9e46e588c06ea1780fcb94e3bbdf3277f42995d403a8b0c",
            )
            .expect("valid public key"),
            amount_paid: Amount::from(100),
            amount_issued: Amount::from(40),
        });

        assert_eq!(response.state(), Some(QuoteState::Paid));
    }
}
