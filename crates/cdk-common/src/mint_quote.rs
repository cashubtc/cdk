//! Unified Mint Quote types for mint use-cases.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut04::{MintQuoteCustomRequest, MintQuoteCustomResponse};
use crate::nuts::nut23::{MintQuoteBolt11Request, MintQuoteBolt11Response, QuoteState};
use crate::nuts::nut25::{MintQuoteBolt12Request, MintQuoteBolt12Response};
use crate::nuts::nut30::{MintQuoteOnchainRequest, MintQuoteOnchainResponse};
use crate::{Amount, CurrencyUnit, PaymentMethod, PublicKey};

/// Unified mint quote request for all payment methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MintQuoteRequest {
    /// Bolt11 (Lightning invoice)
    Bolt11(MintQuoteBolt11Request),
    /// Bolt12 (Offers)
    Bolt12(MintQuoteBolt12Request),
    /// Onchain
    Onchain(MintQuoteOnchainRequest),
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

impl From<MintQuoteOnchainRequest> for MintQuoteRequest {
    fn from(request: MintQuoteOnchainRequest) -> Self {
        MintQuoteRequest::Onchain(request)
    }
}

impl MintQuoteRequest {
    /// Returns the payment method for this request.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Onchain(_) => PaymentMethod::Known(KnownMethod::Onchain),
            Self::Custom { method, .. } => method.clone(),
        }
    }

    /// Returns the amount for this request when present.
    pub fn amount(&self) -> Option<Amount> {
        match self {
            Self::Bolt11(request) => Some(request.amount),
            Self::Bolt12(request) => request.amount,
            Self::Onchain(_) => None,
            Self::Custom { request, .. } => Some(request.amount),
        }
    }

    /// Returns the unit for this request.
    pub fn unit(&self) -> CurrencyUnit {
        match self {
            Self::Bolt11(request) => request.unit.clone(),
            Self::Bolt12(request) => request.unit.clone(),
            Self::Onchain(request) => request.unit.clone(),
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
            Self::Onchain(request) => Some(request.pubkey),
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
    /// Onchain
    Onchain(MintQuoteOnchainResponse<Q>),
    /// Custom payment method
    Custom {
        /// Payment method identifier
        method: PaymentMethod,
        /// Payment method specific response
        response: MintQuoteCustomResponse<Q>,
    },
}

/// Errors from mint quote accounting validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MintQuoteAccountingError {
    /// The response reports more issued ecash than paid amount.
    #[error("mint quote amount_issued ({amount_issued}) exceeds amount_paid ({amount_paid})")]
    AmountIssuedExceedsAmountPaid {
        /// Amount paid to the mint.
        amount_paid: Amount,
        /// Amount of ecash issued by the mint.
        amount_issued: Amount,
    },
}

impl<Q> MintQuoteResponse<Q> {
    /// Returns the payment method for this response.
    pub fn method(&self) -> PaymentMethod {
        match self {
            Self::Bolt11(_) => PaymentMethod::Known(KnownMethod::Bolt11),
            Self::Bolt12(_) => PaymentMethod::Known(KnownMethod::Bolt12),
            Self::Onchain(_) => PaymentMethod::Known(KnownMethod::Onchain),
            Self::Custom { method, .. } => method.clone(),
        }
    }

    /// Returns the quote ID.
    pub fn quote(&self) -> &Q {
        match self {
            Self::Bolt11(r) => &r.quote,
            Self::Bolt12(r) => &r.quote,
            Self::Onchain(r) => &r.quote,
            Self::Custom { response: r, .. } => &r.quote,
        }
    }

    /// Returns the payment request string.
    pub fn request(&self) -> &str {
        match self {
            Self::Bolt11(r) => &r.request,
            Self::Bolt12(r) => &r.request,
            Self::Onchain(r) => &r.request,
            Self::Custom { response: r, .. } => &r.request,
        }
    }

    /// Returns the quote state derived from the response data.
    pub fn state(&self) -> Option<QuoteState> {
        self.try_state().ok()
    }

    /// Returns the quote state derived from the response data, validating quote accounting.
    pub fn try_state(&self) -> Result<QuoteState, MintQuoteAccountingError> {
        match self {
            Self::Bolt11(r) => {
                if r.amount_paid > Amount::ZERO || r.amount_issued > Amount::ZERO {
                    quote_state_from_amounts(r.amount_paid, r.amount_issued)
                } else {
                    Ok(r.state)
                }
            }
            Self::Bolt12(r) => quote_state_from_amounts(r.amount_paid, r.amount_issued),
            Self::Onchain(r) => quote_state_from_amounts(r.amount_paid, r.amount_issued),
            Self::Custom { response, .. } => {
                quote_state_from_amounts(response.amount_paid, response.amount_issued)
            }
        }
    }

    /// Returns the quote expiry timestamp.
    pub fn expiry(&self) -> Option<u64> {
        match self {
            Self::Bolt11(r) => r.expiry,
            Self::Bolt12(r) => r.expiry,
            Self::Onchain(r) => r.expiry,
            Self::Custom { response: r, .. } => r.expiry,
        }
    }
}

/// Derive the deprecated single-use mint quote state from canonical quote counters.
pub fn quote_state_from_amounts(
    amount_paid: Amount,
    amount_issued: Amount,
) -> Result<QuoteState, MintQuoteAccountingError> {
    if amount_issued > amount_paid {
        return Err(MintQuoteAccountingError::AmountIssuedExceedsAmountPaid {
            amount_paid,
            amount_issued,
        });
    }

    if amount_paid == Amount::ZERO && amount_issued == Amount::ZERO {
        return Ok(QuoteState::Unpaid);
    }

    if amount_paid == amount_issued {
        return Ok(QuoteState::Issued);
    }

    Ok(QuoteState::Paid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_response(amount_paid: Amount, amount_issued: Amount) -> MintQuoteResponse<String> {
        MintQuoteResponse::Custom {
            method: PaymentMethod::Custom("custom".to_string()),
            response: MintQuoteCustomResponse {
                quote: "quote".to_string(),
                request: "custom-request".to_string(),
                amount: Some(Amount::from(100)),
                amount_paid,
                amount_issued,
                updated_at: 0,
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
            custom_response(Amount::ZERO, Amount::ZERO).state(),
            Some(QuoteState::Unpaid)
        );
        assert_eq!(
            custom_response(Amount::from(100), Amount::ZERO).state(),
            Some(QuoteState::Paid)
        );
        assert_eq!(
            custom_response(Amount::from(100), Amount::from(100)).state(),
            Some(QuoteState::Issued)
        );
        assert_eq!(
            custom_response(Amount::from(50), Amount::from(100)).state(),
            None
        );
        assert!(matches!(
            custom_response(Amount::from(50), Amount::from(100)).try_state(),
            Err(MintQuoteAccountingError::AmountIssuedExceedsAmountPaid { .. })
        ));
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
            updated_at: 0,
        });

        assert_eq!(response.state(), Some(QuoteState::Paid));
    }
}
