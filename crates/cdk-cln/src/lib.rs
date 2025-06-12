//! CDK lightning backend for CLN

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::amount::{to_unit, Amount};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, MakePaymentResponse, MintPayment,
    PaymentQuoteResponse,
};
use cdk_common::util::{hex, unix_time};
use cdk_common::{mint, Bolt11Invoice};
use cln_rpc::model::requests::{
    InvoiceRequest, ListinvoicesRequest, ListpaysRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{
    ListinvoicesInvoices, ListinvoicesInvoicesStatus, ListpaysPaysStatus, PayStatus,
    WaitanyinvoiceStatus,
};
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny};
use connection::ClnConnection;
use error::Error;
use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod connection;
pub mod error;

/// CLN mint backend
#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    cln_connection: Arc<ClnConnection>,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl Cln {
    /// Create new [`Cln`]
    pub async fn new(rpc_socket: PathBuf, fee_reserve: FeeReserve) -> Result<Self, Error> {
        Ok(Self {
            rpc_socket: rpc_socket.clone(),
            cln_connection: Arc::new(ClnConnection::new(rpc_socket)),
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl MintPayment for Cln {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<Value, Self::Err> {
        Ok(serde_json::to_value(Bolt11Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            invoice_description: true,
            amountless: true,
        })?)
    }

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?;
        let cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        let stream = futures::stream::unfold(
            (
                cln_client,
                last_pay_index,
                self.wait_invoice_cancel_token.clone(),
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut cln_client, mut last_pay_idx, cancel_token, is_active)| async move {
                // Set the stream as active
                is_active.store(true, Ordering::SeqCst);

                loop {
                    let request = WaitanyinvoiceRequest {
                        timeout: None,
                        lastpay_index: last_pay_idx,
                    };
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Set the stream as inactive
                            is_active.store(false, Ordering::SeqCst);
                            // End the stream
                            return None;
                        }
                        result = cln_client.call_typed(&request) => {
                            match result {
                                Ok(invoice) => {

                            // Check the status of the invoice
                            // We only want to yield invoices that have been paid
                            match invoice.status {
                                WaitanyinvoiceStatus::PAID => (),
                                WaitanyinvoiceStatus::EXPIRED => continue,
                            }

                            last_pay_idx = invoice.pay_index;

                            let payment_hash = invoice.payment_hash.to_string();

                            let request_look_up = match invoice.bolt12 {
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
                                None => payment_hash,
                            };

                            return Some((request_look_up, (cln_client, last_pay_idx, cancel_token, is_active)));
                                }
                                Err(e) => {
                                    tracing::warn!("Error fetching invoice: {e}");
                                    is_active.store(false, Ordering::SeqCst);
                                    return None;
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

    async fn get_payment_quote(
        &self,
        request: &str,
        unit: &CurrencyUnit,
        options: Option<MeltOptions>,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let bolt11 = Bolt11Invoice::from_str(request)?;

        let amount_msat = match options {
            Some(amount) => amount.amount_msat(),
            None => bolt11
                .amount_milli_satoshis()
                .ok_or(Error::UnknownInvoiceAmount)?
                .into(),
        };

        let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = max(relative_fee_reserve, absolute_fee_reserve);

        Ok(PaymentQuoteResponse {
            request_lookup_id: bolt11.payment_hash().to_string(),
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
        })
    }

    async fn make_payment(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let bolt11 = Bolt11Invoice::from_str(&melt_quote.request)?;
        let pay_state = self
            .check_outgoing_payment(&bolt11.payment_hash().to_string())
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

        let amount_msat = partial_amount
            .is_none()
            .then(|| {
                melt_quote
                    .msat_to_pay
                    .map(|a| CLN_Amount::from_msat(a.into()))
            })
            .flatten();

        let (tx, rx) = oneshot::channel();

        self.cln_connection
            .pipeline
            .send(connection::Request::Pay(
                PayRequest {
                    bolt11: melt_quote.request.to_string(),
                    amount_msat,
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
                            Ok::<CLN_Amount, Self::Err>(CLN_Amount::from_msat(msat.into()))
                        })
                        .transpose()?,
                    description: None,
                    partial_msat: partial_amount
                        .map(|a| {
                            let msat = to_unit(a, &melt_quote.unit, &CurrencyUnit::Msat)?;

                            Ok::<cln_rpc::primitives::Amount, Self::Err>(CLN_Amount::from_msat(
                                msat.into(),
                            ))
                        })
                        .transpose()?,
                },
                tx,
            ))
            .await
            .map_err(Error::from)?;

        let cln_response = rx.await.unwrap();

        let response = match cln_response {
            Ok(pay_response) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => MeltQuoteState::Paid,
                    PayStatus::PENDING => MeltQuoteState::Pending,
                    PayStatus::FAILED => MeltQuoteState::Failed,
                };

                MakePaymentResponse {
                    payment_proof: Some(hex::encode(pay_response.payment_preimage.to_vec())),
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
            Err(err) => {
                tracing::error!("Could not pay invoice: {}", err);
                return Err(Error::ClnRpc(err).into());
            }
        };

        Ok(response)
    }

    async fn create_incoming_payment_request(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: Option<u64>,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let time_now = unix_time();

        let label = Uuid::new_v4().to_string();

        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;
        let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount.into()));

        let (tx, rx) = oneshot::channel();

        self.cln_connection
            .pipeline
            .send(connection::Request::Invoice(
                InvoiceRequest {
                    amount_msat,
                    description,
                    label: label.clone(),
                    expiry: unix_expiry.map(|t| t - time_now),
                    fallbacks: None,
                    preimage: None,
                    cltv: None,
                    deschashonly: None,
                    exposeprivatechannels: None,
                },
                tx,
            ))
            .await
            .map_err(Error::from)?;

        let invoice_response = rx.await.map_err(Error::from)?.map_err(Error::from)?;

        let request = Bolt11Invoice::from_str(&invoice_response.bolt11)?;
        let expiry = request.expires_at().map(|t| t.as_secs());
        let payment_hash = request.payment_hash();

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: payment_hash.to_string(),
            request: request.to_string(),
            expiry,
        })
    }

    async fn check_incoming_payment_status(
        &self,
        payment_hash: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let (tx, rx) = oneshot::channel();
        self.cln_connection
            .pipeline
            .send(connection::Request::ListInvoices(
                ListinvoicesRequest {
                    payment_hash: Some(payment_hash.to_string()),
                    label: None,
                    invstring: None,
                    offer_id: None,
                    index: None,
                    limit: None,
                    start: None,
                },
                tx,
            ))
            .await
            .map_err(Error::from)?;

        let cln_response = rx.await.map_err(Error::from)?.map_err(Error::from)?;

        let status = match cln_response.invoices.first() {
            Some(invoice_response) => cln_invoice_status_to_mint_state(invoice_response.status),
            None => {
                tracing::info!(
                    "Check invoice called on unknown look up id: {}",
                    payment_hash
                );
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(status)
    }

    async fn check_outgoing_payment(
        &self,
        payment_hash: &str,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let (tx, rx) = oneshot::channel();

        self.cln_connection
            .pipeline
            .send(connection::Request::ListPays(
                ListpaysRequest {
                    payment_hash: Some(payment_hash.parse().map_err(|_| Error::InvalidHash)?),
                    bolt11: None,
                    status: None,
                    start: None,
                    index: None,
                    limit: None,
                },
                tx,
            ))
            .await
            .map_err(Error::from)?;

        let cln_response = rx.await.map_err(Error::from)?.map_err(Error::from)?;

        match cln_response.pays.first() {
            Some(pays_response) => {
                let status = cln_pays_status_to_mint_state(pays_response.status);

                Ok(MakePaymentResponse {
                    payment_lookup_id: pays_response.payment_hash.to_string(),
                    payment_proof: pays_response.preimage.map(|p| hex::encode(p.to_vec())),
                    status,
                    total_spent: pays_response
                        .amount_sent_msat
                        .map_or(Amount::ZERO, |a| a.msat().into()),
                    unit: CurrencyUnit::Msat,
                })
            }
            None => Ok(MakePaymentResponse {
                payment_lookup_id: payment_hash.to_string(),
                payment_proof: None,
                status: MeltQuoteState::Unknown,
                total_spent: Amount::ZERO,
                unit: CurrencyUnit::Msat,
            }),
        }
    }
}

impl Cln {
    /// Get last pay index for cln
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        let (tx, rx) = oneshot::channel();
        self.cln_connection
            .pipeline
            .send(connection::Request::ListInvoices(
                ListinvoicesRequest {
                    payment_hash: None,
                    label: None,
                    invstring: None,
                    offer_id: None,
                    index: None,
                    limit: None,
                    start: None,
                },
                tx,
            ))
            .await?;

        let cln_response = rx.await??;

        match cln_response.invoices.last() {
            Some(last_invoice) => Ok(last_invoice.pay_index),
            None => Ok(None),
        }
    }
}

fn cln_invoice_status_to_mint_state(status: ListinvoicesInvoicesStatus) -> MintQuoteState {
    match status {
        ListinvoicesInvoicesStatus::UNPAID => MintQuoteState::Unpaid,
        ListinvoicesInvoicesStatus::PAID => MintQuoteState::Paid,
        ListinvoicesInvoicesStatus::EXPIRED => MintQuoteState::Unpaid,
    }
}

fn cln_pays_status_to_mint_state(status: ListpaysPaysStatus) -> MeltQuoteState {
    match status {
        ListpaysPaysStatus::PENDING => MeltQuoteState::Pending,
        ListpaysPaysStatus::COMPLETE => MeltQuoteState::Paid,
        ListpaysPaysStatus::FAILED => MeltQuoteState::Failed,
    }
}

async fn fetch_invoice_by_payment_hash(
    cln_client: &mut cln_rpc::ClnRpc,
    payment_hash: &str,
) -> Result<Option<ListinvoicesInvoices>, Error> {
    match cln_client
        .call_typed(&ListinvoicesRequest {
            payment_hash: Some(payment_hash.to_string()),
            index: None,
            invstring: None,
            label: None,
            limit: None,
            offer_id: None,
            start: None,
        })
        .await
    {
        Ok(invoice_response) => Ok(invoice_response.invoices.first().cloned()),
        Err(e) => {
            tracing::warn!("Error fetching invoice: {e}");
            Err(Error::from(e))
        }
    }
}
