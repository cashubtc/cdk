//! CDK lightning backend for lnbits

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
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
use cdk::util::unix_time;
use cdk::{mint, Bolt11Invoice};
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lnbits_rs::api::invoice::CreateInvoiceRequest;
use lnbits_rs::LNBitsClient;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub mod error;

/// LNbits
#[derive(Clone)]
pub struct LNbits {
    lnbits_api: LNBitsClient,
    fee_reserve: FeeReserve,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    webhook_url: String,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl LNbits {
    /// Create new [`LNbits`] wallet
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        admin_api_key: String,
        invoice_api_key: String,
        api_url: String,
        fee_reserve: FeeReserve,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let lnbits_api = LNBitsClient::new("", &admin_api_key, &invoice_api_key, &api_url, None)?;

        Ok(Self {
            lnbits_api,
            receiver,
            fee_reserve,
            webhook_url,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl MintLightning for LNbits {
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

        let lnbits_api = self.lnbits_api.clone();

        let cancel_token = self.wait_invoice_cancel_token.clone();

        Ok(futures::stream::unfold(
            (
                receiver,
                lnbits_api,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut receiver, lnbits_api, cancel_token, is_active)| async move {
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
                            let check = lnbits_api.is_invoice_paid(&msg).await;

                            match check {
                                Ok(state) => {
                                    if state {
                                        Some((msg, (receiver, lnbits_api, cancel_token, is_active)))
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            }
                        }
                        None => {
                            is_active.store(true, Ordering::SeqCst);
                            None
                        },
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
        if melt_quote_request.unit != CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
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

        let fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

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
        _partial_msats: Option<Amount>,
        _max_fee_msats: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let pay_response = self
            .lnbits_api
            .pay_invoice(&melt_quote.request)
            .await
            .map_err(|err| {
                tracing::error!("Could not pay invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not pay invoice"))
            })?;

        let invoice_info = self
            .lnbits_api
            .find_invoice(&pay_response.payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not find invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not find invoice"))
            })?;

        let status = match invoice_info.pending {
            true => MeltQuoteState::Unpaid,
            false => MeltQuoteState::Paid,
        };

        let total_spent = Amount::from((invoice_info.amount + invoice_info.fee).unsigned_abs());

        Ok(PayInvoiceResponse {
            payment_lookup_id: pay_response.payment_hash,
            payment_preimage: Some(invoice_info.payment_hash),
            status,
            total_spent,
            unit: CurrencyUnit::Sat,
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let expiry = unix_expiry - time_now;

        let invoice_request = CreateInvoiceRequest {
            amount: to_unit(amount, unit, &CurrencyUnit::Sat)?.into(),
            memo: Some(description),
            unit: unit.to_string(),
            expiry: Some(expiry),
            webhook: Some(self.webhook_url.clone()),
            internal: None,
            out: false,
        };

        let create_invoice_response = self
            .lnbits_api
            .create_invoice(&invoice_request)
            .await
            .map_err(|err| {
                tracing::error!("Could not create invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not create invoice"))
            })?;

        let request: Bolt11Invoice = create_invoice_response.payment_request.parse()?;
        let expiry = request.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: create_invoice_response.payment_hash,
            request,
            expiry,
        })
    }

    async fn check_incoming_invoice_status(
        &self,
        payment_hash: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let paid = self
            .lnbits_api
            .is_invoice_paid(payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        let state = match paid {
            true => MintQuoteState::Paid,
            false => MintQuoteState::Unpaid,
        };

        Ok(state)
    }

    async fn check_outgoing_payment(
        &self,
        payment_hash: &str,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let payment = self
            .lnbits_api
            .get_payment_info(payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        let pay_response = PayInvoiceResponse {
            payment_lookup_id: payment.details.payment_hash,
            payment_preimage: Some(payment.preimage),
            status: lnbits_to_melt_status(&payment.details.status, payment.details.pending),
            total_spent: Amount::from(
                payment.details.amount.unsigned_abs()
                    + payment.details.fee.unsigned_abs() / MSAT_IN_SAT,
            ),
            unit: self.get_settings().unit,
        };

        Ok(pay_response)
    }
}

fn lnbits_to_melt_status(status: &str, pending: bool) -> MeltQuoteState {
    match (status, pending) {
        ("success", false) => MeltQuoteState::Paid,
        ("failed", false) => MeltQuoteState::Unpaid,
        (_, false) => MeltQuoteState::Unknown,
        (_, true) => MeltQuoteState::Pending,
    }
}

impl LNbits {
    /// Create invoice webhook
    pub async fn create_invoice_webhook_router(
        &self,
        webhook_endpoint: &str,
        sender: tokio::sync::mpsc::Sender<String>,
    ) -> anyhow::Result<Router> {
        self.lnbits_api
            .create_invoice_webhook_router(webhook_endpoint, sender)
            .await
    }
}
