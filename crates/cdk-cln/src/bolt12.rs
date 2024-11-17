use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

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
use cln_rpc::model::requests::{
    FetchinvoiceRequest, OfferRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{PayStatus, WaitanyinvoiceResponse, WaitanyinvoiceStatus};
use cln_rpc::model::Request;
use cln_rpc::primitives::Amount as CLN_Amount;
use futures::{Stream, StreamExt};
use lightning::offers::invoice::Bolt12Invoice;
use lightning::offers::offer::Offer;
use uuid::Uuid;

use super::{Cln, Error};
use crate::fetch_invoice_by_payment_hash;

#[async_trait]
impl MintBolt12Lightning for Cln {
    type Err = cdk_lightning::Error;

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    /// Listen for bolt12 offers to be paid
    async fn wait_any_offer(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitInvoiceResponse> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?;
        let cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        let stream = futures::stream::unfold(
            (
                cln_client,
                last_pay_index,
                self.wait_invoice_cancel_token.clone(),
                Arc::clone(&self.bolt12_wait_invoice_is_active),
            ),
            |(mut cln_client, mut last_pay_idx, cancel_token, is_active)| async move {
                // Set the stream as active
                is_active.store(true, Ordering::SeqCst);

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Set the stream as inactive
                            is_active.store(false, Ordering::SeqCst);
                            // End the stream
                            return None;
                        }
                        result = cln_client.call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        })) => {
                            match result {
                                Ok(invoice) => {

                                        // Try to convert the invoice to WaitanyinvoiceResponse
                            let wait_any_response_result: Result<WaitanyinvoiceResponse, _> =
                                invoice.try_into();

                            let wait_any_response = match wait_any_response_result {
                                Ok(response) => response,
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to parse WaitAnyInvoice response: {:?}",
                                        e
                                    );
                                    // Continue to the next iteration without panicking
                                    continue;
                                }
                            };

                            // Check the status of the invoice
                            // We only want to yield invoices that have been paid
                            match wait_any_response.status {
                                WaitanyinvoiceStatus::PAID => (),
                                WaitanyinvoiceStatus::EXPIRED => continue,
                            }

                            last_pay_idx = wait_any_response.pay_index;

                            let payment_hash = wait_any_response.payment_hash.to_string();


                            // TODO: Handle unit conversion
                            let amount_msats = wait_any_response.amount_received_msat.expect("status is paid there should be an amount");
                            let amount_sats =  amount_msats.msat() / 1000;

                            let request_lookup_id = match wait_any_response.bolt12 {
                                // If it is a bolt12 payment we need to get the offer_id as this is what we use as the request look up.
                                // Since this is not returned in the wait any response,
                                // we need to do a second query for it.
                                Some(_) => {
                                    match fetch_invoice_by_payment_hash(
                                        &mut cln_client,
                                        &payment_hash,
                                    )
                                    .await
                                    {
                                        Ok(Some(invoice)) => {
                                            if let Some(local_offer_id) = invoice.local_offer_id {
                                                local_offer_id.to_string()
                                            } else {
                                                continue;
                                            }
                                        }
                                        Ok(None) => continue,
                                        Err(e) => {
                                            tracing::warn!(
                                                "Error fetching invoice by payment hash: {e}"
                                            );
                                            continue;
                                        }
                                    }
                                }
                                None => payment_hash.clone(),
                            };

                            let response = WaitInvoiceResponse {
                                request_lookup_id,
                                payment_amount: amount_sats.into(),
                                unit: CurrencyUnit::Sat,
                                payment_id: payment_hash
                            };

                            break Some((response, (cln_client, last_pay_idx, cancel_token, is_active)));
                                }
                                Err(e) => {
                                    tracing::warn!("Error fetching invoice: {e}");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
                            }
                        }
                    }
                }
            },
        )
        .boxed();

        Ok(stream)
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
