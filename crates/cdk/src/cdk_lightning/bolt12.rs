//! CDK Mint Bolt12

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use super::{
    Bolt12PaymentQuoteResponse, CreateOfferResponse, Error, PayInvoiceResponse, WaitInvoiceResponse,
};
use crate::nuts::nut20::MeltQuoteBolt12Request;
use crate::nuts::CurrencyUnit;
use crate::{mint, Amount};

/// MintLighting Bolt12 Trait
#[async_trait]
pub trait MintBolt12Lightning {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Listen for bolt12 offers to be paid
    async fn wait_any_offer(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitInvoiceResponse> + Send>>, Self::Err>;

    /// Bolt12 Payment quote
    async fn get_bolt12_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err>;

    /// Pay a bolt12 offer
    async fn pay_bolt12_offer(
        &self,
        melt_quote: mint::MeltQuote,
        amount: Option<Amount>,
        max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err>;

    /// Create bolt12 offer
    async fn create_bolt12_offer(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
        single_use: bool,
    ) -> Result<CreateOfferResponse, Self::Err>;
}
