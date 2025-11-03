//! Melt types
use cashu::{MeltQuoteBolt11Request, MeltQuoteBolt12Request, MeltQuoteCustomRequest};

/// Melt quote request enum for different types of quotes
///
/// This enum represents the different types of melt quote requests
/// that can be made, either BOLT11, BOLT12, or Custom.
#[derive(Debug, Clone, PartialEq, Eq)]
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
