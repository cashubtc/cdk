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

impl<Q: ToString> MeltQuoteOnchainOptions<Q> {
    /// Convert the MeltQuoteOnchainOptions with a quote type Q to a String
    pub fn to_string_id(&self) -> MeltQuoteOnchainOptions<String> {
        MeltQuoteOnchainOptions {
            quotes: self.quotes.iter().map(|q| q.to_string_id()).collect(),
        }
    }
}

impl From<MeltQuoteOnchainOptions<crate::QuoteId>> for MeltQuoteOnchainOptions<String> {
    fn from(value: MeltQuoteOnchainOptions<crate::QuoteId>) -> Self {
        value.to_string_id()
    }
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
}

impl<Q: ToString> MeltQuoteResponse<Q> {
    /// Convert the MeltQuoteResponse with a quote type Q to a String
    pub fn to_string_id(self) -> MeltQuoteResponse<String> {
        match self {
            Self::Bolt11(r) => MeltQuoteResponse::Bolt11(r.to_string_id()),
            Self::Bolt12(r) => MeltQuoteResponse::Bolt12(r.to_string_id()),
            Self::Onchain(r) => MeltQuoteResponse::Onchain(r.to_string_id()),
            Self::Custom((method, r)) => MeltQuoteResponse::Custom((method, r.to_string_id())),
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

    /// Returns the payment proof.
    ///
    /// For Bolt11/Bolt12/Custom methods this is the Lightning payment
    /// preimage. For Onchain, the "proof" is the broadcast outpoint
    /// (`txid:vout`) — it plays the same role of a canonical,
    /// method-specific artifact proving the mint executed the payment.
    /// Callers inspecting `payment_proof` to decide whether an irreversible
    /// settlement has occurred can treat Onchain uniformly with the other
    /// methods.
    pub fn payment_proof(&self) -> Option<&str> {
        match self {
            Self::Bolt11(r) => r.payment_preimage.as_deref(),
            Self::Bolt12(r) => r.payment_preimage.as_deref(),
            Self::Onchain(r) => r.outpoint.as_deref(),
            Self::Custom((_, r)) => r.payment_preimage.as_deref(),
        }
    }

    /// Returns the change signatures when present.
    ///
    /// Onchain melts never return NUT-08 change.
    pub fn change(&self) -> Option<&Vec<crate::BlindSignature>> {
        match self {
            Self::Bolt11(r) => r.change.as_ref(),
            Self::Bolt12(r) => r.change.as_ref(),
            Self::Onchain(_) => None,
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

impl From<MeltQuoteResponse<crate::QuoteId>> for MeltQuoteResponse<String> {
    fn from(value: MeltQuoteResponse<crate::QuoteId>) -> Self {
        value.to_string_id()
    }
}

impl<Q: ToString> MeltQuoteCreateResponse<Q> {
    /// Convert the MeltQuoteCreateResponse with a quote type Q to a String
    pub fn to_string_id(self) -> MeltQuoteCreateResponse<String> {
        match self {
            Self::Bolt11(r) => MeltQuoteCreateResponse::Bolt11(r.to_string_id()),
            Self::Bolt12(r) => MeltQuoteCreateResponse::Bolt12(r.to_string_id()),
            Self::Onchain(r) => MeltQuoteCreateResponse::Onchain(r.to_string_id()),
            Self::Custom((method, r)) => {
                MeltQuoteCreateResponse::Custom((method, r.to_string_id()))
            }
        }
    }

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

impl From<MeltQuoteCreateResponse<crate::QuoteId>> for MeltQuoteCreateResponse<String> {
    fn from(value: MeltQuoteCreateResponse<crate::QuoteId>) -> Self {
        value.to_string_id()
    }
}

impl<Q> From<crate::mint::MeltQuote> for MeltQuoteResponse<Q>
where
    Q: From<crate::QuoteId>,
{
    fn from(value: crate::mint::MeltQuote) -> Self {
        match value.payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                Self::Bolt11(crate::nuts::nut23::MeltQuoteBolt11Response {
                    quote: value.id.clone().into(),
                    amount: value.amount().into(),
                    fee_reserve: value.fee_reserve().into(),
                    state: value.state,
                    expiry: value.expiry,
                    payment_preimage: value.payment_proof.clone(),
                    change: None,
                    request: Some(value.request.to_string()),
                    unit: Some(value.unit.clone()),
                })
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                Self::Bolt12(crate::nuts::nut25::MeltQuoteBolt12Response {
                    quote: value.id.clone().into(),
                    amount: value.amount().into(),
                    fee_reserve: value.fee_reserve().into(),
                    state: value.state,
                    expiry: value.expiry,
                    payment_preimage: value.payment_proof.clone(),
                    change: None,
                    request: Some(value.request.to_string()),
                    unit: Some(value.unit.clone()),
                })
            }
            PaymentMethod::Known(KnownMethod::Onchain) => {
                Self::Onchain(crate::nuts::nut_onchain::MeltQuoteOnchainResponse {
                    quote: value.id.clone().into(),
                    request: value.request.to_string(),
                    amount: value.amount().into(),
                    unit: value.unit.clone(),
                    fee: value.fee_reserve().into(),
                    estimated_blocks: value.estimated_blocks.unwrap_or_default(),
                    state: value.state,
                    expiry: value.expiry,
                    outpoint: value.payment_proof.clone(),
                })
            }
            ref method => Self::Custom((
                method.clone(),
                crate::nuts::nut05::MeltQuoteCustomResponse {
                    quote: value.id.clone().into(),
                    amount: value.amount().into(),
                    fee_reserve: value.fee_reserve().into(),
                    state: value.state,
                    expiry: value.expiry,
                    payment_preimage: value.payment_proof.clone(),
                    change: None,
                    request: Some(value.request.to_string()),
                    unit: Some(value.unit.clone()),
                    extra: serde_json::Value::Null,
                },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut05::MeltQuoteCustomResponse;
    use crate::nuts::nut23::MeltQuoteBolt11Response;
    use crate::nuts::nut_onchain::MeltQuoteOnchainResponse;
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

    fn onchain_response(quote: &str) -> MeltQuoteOnchainResponse<String> {
        MeltQuoteOnchainResponse {
            quote: quote.to_string(),
            request: "bc1qonchainaddress".to_string(),
            amount: Amount::from(400),
            unit: CurrencyUnit::Sat,
            fee: Amount::from(4),
            estimated_blocks: 6,
            state: MeltQuoteState::Paid,
            expiry: 4000,
            outpoint: Some("abcd...ef:0".to_string()),
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
    fn melt_quote_response_accessors_onchain() {
        let r = MeltQuoteResponse::Onchain(onchain_response("qoc"));
        assert_eq!(r.method(), PaymentMethod::Known(KnownMethod::Onchain));
        assert_eq!(r.quote(), "qoc");
        assert_eq!(r.amount(), Amount::from(400));
        // fee_reserve() reads `fee` on onchain (field name differs from Bolt11/Bolt12)
        assert_eq!(r.fee_reserve(), Amount::from(4));
        assert_eq!(r.state(), MeltQuoteState::Paid);
        assert_eq!(r.expiry(), 4000);
        // payment_proof() is the outpoint, not a Lightning preimage
        assert_eq!(r.payment_proof(), Some("abcd...ef:0"));
        // Onchain melts never carry NUT-08 change
        assert!(r.change().is_none());
        // `request` is non-Option on onchain; accessor wraps in Some
        assert_eq!(r.request(), Some("bc1qonchainaddress"));
        // `unit` is non-Option on onchain; accessor wraps in Some
        assert_eq!(r.unit(), Some(CurrencyUnit::Sat));
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
        assert_eq!(r11.quote().map(String::as_str), Some("c11"));

        let r12 = MeltQuoteCreateResponse::Bolt12(bolt12_response("c12"));
        assert_eq!(r12.method(), PaymentMethod::Known(KnownMethod::Bolt12));
        assert_eq!(r12.quote().map(String::as_str), Some("c12"));

        let method = PaymentMethod::from("venmo");
        let rc = MeltQuoteCreateResponse::Custom((method.clone(), custom_response("cc")));
        assert_eq!(rc.method(), method);
        assert_eq!(rc.quote().map(String::as_str), Some("cc"));
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
