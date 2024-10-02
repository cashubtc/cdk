//! CDK lightning backend for Phoenixd

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use cdk::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk::cdk_lightning::{
    self, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse, Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::{mint, Bolt11Invoice};
use error::Error;
use futures::{Stream, StreamExt};
use phoenixd_rs::webhooks::WebhookResponse;
use phoenixd_rs::{InvoiceRequest, Phoenixd as PhoenixdApi};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub mod error;

/// Phoenixd
#[derive(Clone)]
pub struct Phoenixd {
    phoenixd_api: PhoenixdApi,
    fee_reserve: FeeReserve,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<WebhookResponse>>>>,
    webhook_url: String,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl Phoenixd {
    /// Create new [`Phoenixd`] wallet
    pub fn new(
        api_password: String,
        api_url: String,
        fee_reserve: FeeReserve,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<WebhookResponse>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let phoenixd = PhoenixdApi::new(&api_password, &api_url)?;
        Ok(Self {
            phoenixd_api: phoenixd,
            fee_reserve,
            receiver,
            webhook_url,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create invoice webhook
    pub async fn create_invoice_webhook(
        &self,
        webhook_endpoint: &str,
        sender: tokio::sync::mpsc::Sender<WebhookResponse>,
    ) -> anyhow::Result<Router> {
        self.phoenixd_api
            .create_invoice_webhook_router(webhook_endpoint, sender)
            .await
    }
}

#[async_trait]
impl MintLightning for Phoenixd {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: false,
            unit: CurrencyUnit::Sat,
            invoice_description: true,
        }
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    #[allow(clippy::incompatible_msrv)]
    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(anyhow!("No receiver"))?;

        let phoenixd_api = self.phoenixd_api.clone();

        let cancel_token = self.wait_invoice_cancel_token.clone();

        Ok(futures::stream::unfold(
        (receiver, phoenixd_api, cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
        ),
        |(mut receiver, phoenixd_api, cancel_token, is_active)| async move {

                is_active.store(true, Ordering::SeqCst);
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    // Stream is cancelled
                    is_active.store(false, Ordering::SeqCst);
                    tracing::info!("Waiting for phonixd invoice ending");
                    None
                }
                msg_option = receiver.recv() => {
                    match msg_option {
                        Some(msg) => {
                            let check = phoenixd_api.get_incoming_invoice(&msg.payment_hash).await;

                            match check {
                                Ok(state) => {
                                    if state.is_paid {
                                        // Yield the payment hash and continue the stream
                                        Some((msg.payment_hash, (receiver, phoenixd_api, cancel_token, is_active)))
                                    } else {
                                        // Invoice not paid yet, continue waiting
                                        // We need to continue the stream, so we return the same state
                                        None
                                    }
                                }
                                Err(e) => {
                                    // Log the error and continue
                                    tracing::warn!("Error checking invoice state: {:?}", e);
                                    None
                                }
                            }
                        }
                        None => {
                            // The receiver stream has ended
                            None
                        }
                    }
                }
            }
        },
    )
    .boxed())
    }

    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if CurrencyUnit::Sat != melt_quote_request.unit {
            return Err(Error::UnsupportedUnit.into());
        }

        let invoice_amount_msat = melt_quote_request
            .request
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount)?;

        let amount = to_unit(
            invoice_amount_msat,
            &CurrencyUnit::Msat,
            &melt_quote_request.unit,
        )?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let mut fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

        // Fee in phoenixd is always 0.04 + 4 sat
        fee += 4;

        Ok(PaymentQuoteResponse {
            request_lookup_id: melt_quote_request.request.payment_hash().to_string(),
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        _max_fee_msats: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let pay_response = self
            .phoenixd_api
            .pay_bolt11_invoice(&melt_quote.request, partial_amount.map(|a| a.into()))
            .await?;

        // The pay invoice response does not give the needed fee info so we have to check.
        let check_outgoing_response = self
            .check_outgoing_payment(&pay_response.payment_id)
            .await?;

        let bolt11: Bolt11Invoice = melt_quote.request.parse()?;

        Ok(PayInvoiceResponse {
            payment_lookup_id: bolt11.payment_hash().to_string(),
            payment_preimage: Some(pay_response.payment_preimage),
            status: MeltQuoteState::Paid,
            total_spent: check_outgoing_response.total_spent,
            unit: CurrencyUnit::Sat,
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        _unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let amount_sat = to_unit(amount, unit, &CurrencyUnit::Sat)?;

        let invoice_request = InvoiceRequest {
            external_id: None,
            description: Some(description),
            description_hash: None,
            amount_sat: amount_sat.into(),
            webhook_url: Some(self.webhook_url.clone()),
        };

        let create_invoice_response = self.phoenixd_api.create_invoice(invoice_request).await?;

        let bolt11: Bolt11Invoice = create_invoice_response.serialized.parse()?;
        let expiry = bolt11.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: create_invoice_response.payment_hash,
            request: bolt11.clone(),
            expiry,
        })
    }

    async fn check_incoming_invoice_status(
        &self,
        payment_hash: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let invoice = self.phoenixd_api.get_incoming_invoice(payment_hash).await?;

        let state = match invoice.is_paid {
            true => MintQuoteState::Paid,
            false => MintQuoteState::Unpaid,
        };

        Ok(state)
    }

    /// Check the status of an outgoing invoice
    async fn check_outgoing_payment(
        &self,
        payment_id: &str,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        // We can only check the status of the payment if we have the payment id not if we only have a payment hash.
        // In phd this is a uuid, that we get after getting a response from the pay invoice
        if let Err(_err) = uuid::Uuid::from_str(payment_id) {
            tracing::warn!("Could not check status of payment, no payment id");
            return Ok(PayInvoiceResponse {
                payment_lookup_id: payment_id.to_string(),
                payment_preimage: None,
                status: MeltQuoteState::Unknown,
                total_spent: Amount::ZERO,
                unit: CurrencyUnit::Sat,
            });
        }

        let res = self.phoenixd_api.get_outgoing_invoice(payment_id).await;

        let state = match res {
            Ok(res) => {
                let status = match res.is_paid {
                    true => MeltQuoteState::Paid,
                    false => MeltQuoteState::Unpaid,
                };

                let total_spent = res.sent + (res.fees + 999) / MSAT_IN_SAT;

                PayInvoiceResponse {
                    payment_lookup_id: res.payment_hash,
                    payment_preimage: Some(res.preimage),
                    status,
                    total_spent: total_spent.into(),
                    unit: CurrencyUnit::Sat,
                }
            }
            Err(err) => match err {
                phoenixd_rs::Error::NotFound => PayInvoiceResponse {
                    payment_lookup_id: payment_id.to_string(),
                    payment_preimage: None,
                    status: MeltQuoteState::Unknown,
                    total_spent: Amount::ZERO,
                    unit: CurrencyUnit::Sat,
                },
                _ => {
                    return Err(Error::from(err).into());
                }
            },
        };

        Ok(state)
    }
}
