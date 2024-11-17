use std::pin::Pin;
use std::str::FromStr;

use async_trait::async_trait;
use cdk::amount::{amount_for_offer, to_unit, Amount};
use cdk::cdk_lightning::bolt12::MintBolt12Lightning;
use cdk::cdk_lightning::{
    self, Bolt12PaymentQuoteResponse, CreateOfferResponse, MintLightning, PayInvoiceResponse,
    WaitInvoiceResponse,
};
use cdk::mint;
use cdk::mint::types::PaymentRequest;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt12Request, MeltQuoteState};
use cdk::util::{hex, unix_time};
use cln_rpc::model::requests::{FetchinvoiceRequest, OfferRequest, PayRequest};
use cln_rpc::model::responses::PayStatus;
use cln_rpc::model::Request;
use cln_rpc::primitives::Amount as CLN_Amount;
use futures::Stream;
use lightning::offers::invoice::Bolt12Invoice;
use lightning::offers::offer::Offer;
use uuid::Uuid;

use super::{Cln, Error};

#[async_trait]
impl MintBolt12Lightning for Cln {
    type Err = cdk_lightning::Error;

    /// Listen for bolt12 offers to be paid
    async fn wait_any_offer(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitInvoiceResponse> + Send>>, Self::Err> {
        todo!()
    }

    async fn get_bolt12_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err> {
        let offer =
            Offer::from_str(&melt_quote_request.request).map_err(|_| Error::UnknownInvoice)?;

        let amount = match melt_quote_request.amount {
            Some(amount) => amount,
            None => amount_for_offer(&offer, &CurrencyUnit::Msat)?,
        };

        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(Request::FetchInvoice(FetchinvoiceRequest {
                amount_msat: Some(CLN_Amount::from_msat(amount.into())),
                offer: melt_quote_request.request.clone(),
                payer_note: None,
                quantity: None,
                recurrence_counter: None,
                recurrence_label: None,
                recurrence_start: None,
                timeout: None,
            }))
            .await;

        let amount = to_unit(amount, &CurrencyUnit::Msat, &melt_quote_request.unit)?;

        match cln_response {
            Ok(cln_rpc::Response::FetchInvoice(invoice_response)) => {
                let bolt12_invoice =
                    Bolt12Invoice::try_from(hex::decode(&invoice_response.invoice).unwrap())
                        .unwrap();

                Ok(Bolt12PaymentQuoteResponse {
                    request_lookup_id: bolt12_invoice.payment_hash().to_string(),
                    amount,
                    fee: Amount::ZERO,
                    state: MeltQuoteState::Unpaid,
                    invoice: Some(invoice_response.invoice),
                })
            }
            c => {
                tracing::debug!("{:?}", c);
                tracing::error!("Error attempting to pay invoice for offer",);
                Err(Error::WrongClnResponse.into())
            }
        }
    }

    async fn pay_bolt12_offer(
        &self,
        melt_quote: mint::MeltQuote,
        _amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let bolt12 = &match melt_quote.request {
            PaymentRequest::Bolt12 { offer: _, invoice } => invoice.ok_or(Error::UnknownInvoice)?,
            PaymentRequest::Bolt11 { .. } => return Err(Error::WrongPaymentType.into()),
        };

        let pay_state = self
            .check_outgoing_payment(&melt_quote.request_lookup_id)
            .await?;

        match pay_state.status {
            MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => (),
            MeltQuoteState::Paid => {
                tracing::debug!("Melt attempted on invoice already paid");
                return Err(Self::Err::InvoiceAlreadyPaid);
            }
            MeltQuoteState::Pending => {
                tracing::debug!("Melt attempted on invoice already pending");
                return Err(Self::Err::InvoicePaymentPending);
            }
        }

        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(Request::Pay(PayRequest {
                bolt11: bolt12.to_string(),
                amount_msat: None,
                label: None,
                riskfactor: None,
                maxfeepercent: None,
                retry_for: None,
                maxdelay: None,
                exemptfee: None,
                localinvreqid: None,
                exclude: None,
                maxfee: max_fee
                    .map(|a| {
                        let msat = to_unit(a, &melt_quote.unit, &CurrencyUnit::Msat)?;
                        Ok::<cln_rpc::primitives::Amount, Self::Err>(CLN_Amount::from_msat(
                            msat.into(),
                        ))
                    })
                    .transpose()?,
                description: None,
                partial_msat: None,
            }))
            .await;

        let response = match cln_response {
            Ok(cln_rpc::Response::Pay(pay_response)) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => MeltQuoteState::Paid,
                    PayStatus::PENDING => MeltQuoteState::Pending,
                    PayStatus::FAILED => MeltQuoteState::Failed,
                };
                PayInvoiceResponse {
                    payment_preimage: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_lookup_id: pay_response.payment_hash.to_string(),
                    status,
                    total_spent: to_unit(
                        pay_response.amount_sent_msat.msat(),
                        &CurrencyUnit::Msat,
                        &melt_quote.unit,
                    )?,
                    unit: melt_quote.unit,
                }
            }
            _ => {
                tracing::error!("Error attempting to pay invoice: {}", bolt12);
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(response)
    }

    /// Create bolt12 offer
    async fn create_bolt12_offer(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
        single_use: bool,
    ) -> Result<CreateOfferResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);
        let mut cln_client = self.cln_client.lock().await;

        let label = Uuid::new_v4().to_string();

        let amount = match amount {
            Some(amount) => {
                let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;

                amount.to_string()
            }
            None => "any".to_string(),
        };

        // It seems that the only way to force cln to create a unique offer
        // is to encode some random data in the offer
        let issuer = Uuid::new_v4().to_string();

        let cln_response = cln_client
            .call(cln_rpc::Request::Offer(OfferRequest {
                absolute_expiry: Some(unix_expiry),
                description: Some(description),
                label: Some(label),
                issuer: Some(issuer),
                quantity_max: None,
                recurrence: None,
                recurrence_base: None,
                recurrence_limit: None,
                recurrence_paywindow: None,
                recurrence_start_any_period: None,
                single_use: Some(single_use),
                amount,
            }))
            .await
            .map_err(Error::from)?;

        match cln_response {
            cln_rpc::Response::Offer(offer_res) => {
                let offer = Offer::from_str(&offer_res.bolt12).unwrap();
                let expiry = offer.absolute_expiry().map(|t| t.as_secs());

                Ok(CreateOfferResponse {
                    request_lookup_id: offer_res.offer_id.to_string(),
                    request: offer,
                    expiry,
                })
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                Err(Error::WrongClnResponse.into())
            }
        }
    }
}
