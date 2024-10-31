use std::str::FromStr;

use anyhow::anyhow;
use async_trait::async_trait;
use cdk::amount::{amount_for_offer, Amount};
use cdk::cdk_lightning::bolt12::MintBolt12Lightning;
use cdk::cdk_lightning::{
    self, Bolt12PaymentQuoteResponse, CreateOfferResponse, MintLightning, PayInvoiceResponse,
};
use cdk::mint;
use cdk::mint::types::PaymentRequest;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt12Request, MeltQuoteState};
use cdk::util::hex;
use lightning::offers::offer::Offer;

use super::Error;
use crate::Phoenixd;

#[async_trait]
impl MintBolt12Lightning for Phoenixd {
    type Err = cdk_lightning::Error;
    async fn get_bolt12_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err> {
        if CurrencyUnit::Sat != melt_quote_request.unit {
            return Err(Error::UnsupportedUnit.into());
        }

        let offer = Offer::from_str(&melt_quote_request.request)
            .map_err(|_| Error::Anyhow(anyhow!("Invalid offer")))?;

        let amount = match melt_quote_request.amount {
            Some(amount) => amount,
            None => amount_for_offer(&offer, &CurrencyUnit::Sat)?,
        };

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let mut fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

        // Fee in phoenixd is always 0.04 + 4 sat
        fee += 4;

        Ok(Bolt12PaymentQuoteResponse {
            request_lookup_id: hex::encode(offer.id().0),
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
            invoice: None,
        })
    }

    async fn pay_bolt12_offer(
        &self,
        melt_quote: mint::MeltQuote,
        amount: Option<Amount>,
        _max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let offer = &match melt_quote.request {
            PaymentRequest::Bolt12 { offer, invoice: _ } => offer,
            PaymentRequest::Bolt11 { .. } => return Err(Error::WrongRequestType.into()),
        };

        let amount = match amount {
            Some(amount) => amount,
            None => amount_for_offer(offer, &CurrencyUnit::Sat)?,
        };

        let pay_response = self
            .phoenixd_api
            .pay_bolt12_offer(offer.to_string(), amount.into(), None)
            .await?;

        // The pay invoice response does not give the needed fee info so we have to check.
        let check_outgoing_response = self
            .check_outgoing_payment(&pay_response.payment_id)
            .await?;

        tracing::debug!(
            "Phd offer {} with amount {} with fee {} total spent {}",
            check_outgoing_response.status,
            amount,
            check_outgoing_response.total_spent - amount,
            check_outgoing_response.total_spent
        );

        Ok(PayInvoiceResponse {
            payment_lookup_id: pay_response.payment_id,
            payment_preimage: Some(pay_response.payment_preimage),
            status: check_outgoing_response.status,
            total_spent: check_outgoing_response.total_spent,
            unit: CurrencyUnit::Sat,
        })
    }

    /// Create bolt12 offer
    async fn create_bolt12_offer(
        &self,
        _amount: Option<Amount>,
        _unit: &CurrencyUnit,
        _description: String,
        _unix_expiry: u64,
        _single_use: bool,
    ) -> Result<CreateOfferResponse, Self::Err> {
        Err(Error::UnsupportedMethod.into())
    }
}
