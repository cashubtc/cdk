//! Unified Melt Quote types for melt use-cases.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut05::{MeltQuoteCustomRequest, MeltQuoteCustomResponse};
use crate::nuts::nut23::{MeltQuoteBolt11Request, MeltQuoteBolt11Response};
use crate::nuts::nut25::{MeltQuoteBolt12Request, MeltQuoteBolt12Response};
use crate::{Amount, CurrencyUnit, MeltQuoteState, PaymentMethod};

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
    /// Custom payment method
    Custom((PaymentMethod, MeltQuoteCustomResponse<Q>)),
}

impl<Q> MeltQuoteResponse<Q> {
    /// Returns the payment method for this response.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Custom((method, _)) => method.clone(),
        }
    }

    /// Returns the quote ID.
    pub fn quote(&self) -> &Q {
        match self {
            Self::Bolt11(r) => &r.quote,
            Self::Bolt12(r) => &r.quote,
            Self::Custom((_, r)) => &r.quote,
        }
    }

    /// Returns the quoted amount.
    pub fn amount(&self) -> Amount {
        match self {
            Self::Bolt11(r) => r.amount,
            Self::Bolt12(r) => r.amount,
            Self::Custom((_, r)) => r.amount,
        }
    }

    /// Returns the fee reserve.
    pub fn fee_reserve(&self) -> Amount {
        match self {
            Self::Bolt11(r) => r.fee_reserve,
            Self::Bolt12(r) => r.fee_reserve,
            Self::Custom((_, r)) => r.fee_reserve,
        }
    }

    /// Returns the quote state.
    pub fn state(&self) -> MeltQuoteState {
        match self {
            Self::Bolt11(r) => r.state,
            Self::Bolt12(r) => r.state,
            Self::Custom((_, r)) => r.state,
        }
    }

    /// Returns the quote expiry timestamp.
    pub fn expiry(&self) -> u64 {
        match self {
            Self::Bolt11(r) => r.expiry,
            Self::Bolt12(r) => r.expiry,
            Self::Custom((_, r)) => r.expiry,
        }
    }

    /// Returns the payment preimage.
    pub fn payment_proof(&self) -> Option<&str> {
        match self {
            Self::Bolt11(r) => r.payment_preimage.as_deref(),
            Self::Bolt12(r) => r.payment_preimage.as_deref(),
            Self::Custom((_, r)) => r.payment_preimage.as_deref(),
        }
    }

    /// Returns the change signatures when present.
    pub fn change(&self) -> Option<&Vec<crate::BlindSignature>> {
        match self {
            Self::Bolt11(r) => r.change.as_ref(),
            Self::Bolt12(r) => r.change.as_ref(),
            Self::Custom((_, r)) => r.change.as_ref(),
        }
    }

    /// Returns the payment request string when present.
    pub fn request(&self) -> Option<&str> {
        match self {
            Self::Bolt11(r) => r.request.as_deref(),
            Self::Bolt12(r) => r.request.as_deref(),
            Self::Custom((_, r)) => r.request.as_deref(),
        }
    }

    /// Returns the unit when present.
    pub fn unit(&self) -> Option<CurrencyUnit> {
        match self {
            Self::Bolt11(r) => r.unit.clone(),
            Self::Bolt12(r) => r.unit.clone(),
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
            Self::Custom((method, _)) => method.clone(),
        }
    }

    /// Returns the quote ID for single-quote methods.
    pub fn quote(&self) -> &Q {
        match self {
            Self::Bolt11(r) => &r.quote,
            Self::Bolt12(r) => &r.quote,
            Self::Custom((_, r)) => &r.quote,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut05::MeltQuoteCustomResponse;
    use crate::nuts::nut23::MeltQuoteBolt11Response;
    use crate::{Amount, CurrencyUnit, MeltQuoteState};

    fn bolt11_response(quote: &str) -> MeltQuoteBolt11Response<String> {
        MeltQuoteBolt11Response {
            quote: quote.to_string(),
            amount: Amount::from(100),
            fee_reserve: Amount::from(1),
            state: MeltQuoteState::Unpaid,
            expiry: 1000,
            payment_preimage: Some("preimage-11".to_string()),
            change: None,
            request: Some("lnbc100".to_string()),
            unit: Some(CurrencyUnit::Sat),
        }
    }

    fn bolt12_response(quote: &str) -> MeltQuoteBolt12Response<String> {
        MeltQuoteBolt12Response {
            quote: quote.to_string(),
            amount: Amount::from(200),
            fee_reserve: Amount::from(2),
            state: MeltQuoteState::Pending,
            expiry: 2000,
            payment_preimage: Some("preimage-12".to_string()),
            change: None,
            request: Some("lno200".to_string()),
            unit: Some(CurrencyUnit::Sat),
        }
    }

    fn custom_response(quote: &str) -> MeltQuoteCustomResponse<String> {
        MeltQuoteCustomResponse {
            quote: quote.to_string(),
            amount: Amount::from(300),
            fee_reserve: Amount::from(3),
            state: MeltQuoteState::Paid,
            expiry: 3000,
            payment_preimage: Some("outpoint-abc".to_string()),
            change: None,
            request: Some("bc1qaddress".to_string()),
            unit: Some(CurrencyUnit::Sat),
            extra: serde_json::Value::Null,
        }
    }

    #[test]
    fn melt_quote_response_accessors_bolt11() {
        let r = MeltQuoteResponse::Bolt11(bolt11_response("q11"));
        assert_eq!(r.method(), PaymentMethod::Known(KnownMethod::Bolt11));
        assert_eq!(r.quote(), "q11");
        assert_eq!(r.amount(), Amount::from(100));
        assert_eq!(r.fee_reserve(), Amount::from(1));
        assert_eq!(r.state(), MeltQuoteState::Unpaid);
        assert_eq!(r.expiry(), 1000);
        assert_eq!(r.payment_proof(), Some("preimage-11"));
        assert!(r.change().is_none());
        assert_eq!(r.request(), Some("lnbc100"));
        assert_eq!(r.unit(), Some(CurrencyUnit::Sat));
    }

    #[test]
    fn melt_quote_response_accessors_bolt12() {
        let r = MeltQuoteResponse::Bolt12(bolt12_response("q12"));
        assert_eq!(r.method(), PaymentMethod::Known(KnownMethod::Bolt12));
        assert_eq!(r.quote(), "q12");
        assert_eq!(r.amount(), Amount::from(200));
        assert_eq!(r.fee_reserve(), Amount::from(2));
        assert_eq!(r.state(), MeltQuoteState::Pending);
        assert_eq!(r.expiry(), 2000);
        assert_eq!(r.payment_proof(), Some("preimage-12"));
        assert_eq!(r.request(), Some("lno200"));
    }

    #[test]
    fn melt_quote_response_accessors_custom() {
        let method = PaymentMethod::from("paypal");
        let r = MeltQuoteResponse::Custom((method.clone(), custom_response("qc")));
        assert_eq!(r.method(), method);
        assert_eq!(r.quote(), "qc");
        assert_eq!(r.amount(), Amount::from(300));
        assert_eq!(r.fee_reserve(), Amount::from(3));
        assert_eq!(r.state(), MeltQuoteState::Paid);
        assert_eq!(r.expiry(), 3000);
        assert_eq!(r.payment_proof(), Some("outpoint-abc"));
        assert_eq!(r.request(), Some("bc1qaddress"));
    }

    #[test]
    fn melt_quote_create_response_accessors() {
        let r11 = MeltQuoteCreateResponse::Bolt11(bolt11_response("c11"));
        assert_eq!(r11.method(), PaymentMethod::Known(KnownMethod::Bolt11));
        assert_eq!(r11.quote(), "c11");

        let r12 = MeltQuoteCreateResponse::Bolt12(bolt12_response("c12"));
        assert_eq!(r12.method(), PaymentMethod::Known(KnownMethod::Bolt12));
        assert_eq!(r12.quote(), "c12");

        let method = PaymentMethod::from("venmo");
        let rc = MeltQuoteCreateResponse::Custom((method.clone(), custom_response("cc")));
        assert_eq!(rc.method(), method);
        assert_eq!(rc.quote(), "cc");
    }

    #[test]
    fn melt_quote_request_method_dispatch() {
        use crate::nuts::nut05::MeltQuoteCustomRequest;

        let custom_req = MeltQuoteCustomRequest {
            method: "cashapp".to_string(),
            unit: CurrencyUnit::Sat,
            request: "$tag".to_string(),
            extra: serde_json::Value::Null,
        };
        let req: MeltQuoteRequest = custom_req.into();
        assert_eq!(req.method(), PaymentMethod::from("cashapp"));
    }
}
