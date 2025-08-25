//! Melt types
use cashu::quote_id::QuoteId;
use cashu::{
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltQuoteBolt12Request,
    MeltQuoteOnchainRequest, MeltQuoteOnchainResponse,
};

use crate::mint::MeltQuote;
use crate::Error;

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
    /// Onchain request
    Onchain(MeltQuoteOnchainRequest),
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

/// Melt quote response enum for different types of quotes
///
/// This enum represents the different types of melt quote responses
/// that can be returned from creating a melt quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeltQuoteResponse {
    /// Lightning Network BOLT11 invoice response
    Bolt11(MeltQuoteBolt11Response<QuoteId>),
    /// Lightning Network BOLT12 offer response
    Bolt12(MeltQuoteBolt11Response<QuoteId>),
    /// Onchain response
    Onchain(MeltQuoteOnchainResponse<QuoteId>),
}

impl TryFrom<MeltQuoteResponse> for MeltQuoteBolt11Response<String> {
    type Error = Error;

    fn try_from(response: MeltQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MeltQuoteResponse::Bolt11(bolt11_response) => Ok(bolt11_response.into()),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}
impl TryFrom<MeltQuoteResponse> for MeltQuoteBolt11Response<QuoteId> {
    type Error = Error;

    fn try_from(response: MeltQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MeltQuoteResponse::Bolt11(bolt11_response) => Ok(bolt11_response),
            MeltQuoteResponse::Bolt12(bolt12_response) => Ok(bolt12_response),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MeltQuoteResponse> for MeltQuoteOnchainResponse<QuoteId> {
    type Error = Error;

    fn try_from(response: MeltQuoteResponse) -> Result<Self, Self::Error> {
        match response {
            MeltQuoteResponse::Onchain(onchain_response) => Ok(onchain_response),
            _ => Err(Error::InvalidPaymentMethod),
        }
    }
}

impl TryFrom<MeltQuote> for MeltQuoteResponse {
    type Error = Error;

    fn try_from(quote: MeltQuote) -> Result<Self, Self::Error> {
        match quote.payment_method {
            crate::PaymentMethod::Bolt11 => {
                let bolt11_response: MeltQuoteBolt11Response<QuoteId> = quote.into();
                Ok(MeltQuoteResponse::Bolt11(bolt11_response))
            }
            crate::PaymentMethod::Bolt12 => {
                let bolt12_response: MeltQuoteBolt11Response<QuoteId> = quote.into();
                Ok(MeltQuoteResponse::Bolt12(bolt12_response))
            }
            crate::PaymentMethod::Onchain => {
                let onchain_response: MeltQuoteOnchainResponse<QuoteId> = quote.into();
                Ok(MeltQuoteResponse::Onchain(onchain_response))
            }
            crate::PaymentMethod::Custom(_) => Err(Error::InvalidPaymentMethod),
        }
    }
}
