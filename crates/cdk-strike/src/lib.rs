//! CDK lightning backend for Strike

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail};
use async_trait::async_trait;
use axum::Router;
use cdk::amount::Amount;
use cdk::cdk_lightning::{
    self, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse, Settings,
};
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::util::unix_time;
use cdk::{mint, Bolt11Invoice};
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use strike_rs::{
    Amount as StrikeAmount, Currency as StrikeCurrencyUnit, InvoiceRequest, InvoiceState,
    PayInvoiceQuoteRequest, Strike as StrikeApi,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod error;

/// Strike
#[derive(Clone)]
pub struct Strike {
    strike_api: StrikeApi,
    unit: CurrencyUnit,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    webhook_url: String,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl Strike {
    /// Create new [`Strike`] wallet
    pub async fn new(
        api_key: String,
        unit: CurrencyUnit,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let strike = StrikeApi::new(&api_key, None)?;
        Ok(Self {
            strike_api: strike,
            receiver,
            unit,
            webhook_url,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl MintLightning for Strike {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: false,
            unit: self.unit.clone(),
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
        self.strike_api
            .subscribe_to_invoice_webhook(self.webhook_url.clone())
            .await?;

        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(anyhow!("No receiver"))?;

        let strike_api = self.strike_api.clone();
        let cancel_token = self.wait_invoice_cancel_token.clone();

        Ok(futures::stream::unfold(
            (
                receiver,
                strike_api,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut receiver, strike_api, cancel_token, is_active)| async move {
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
                        let check = strike_api.get_incoming_invoice(&msg).await;

                        match check {
                            Ok(state) => {
                                if state.state == InvoiceState::Paid {
                                    Some((msg, (receiver, strike_api, cancel_token, is_active)))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    None => None,
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
        if melt_quote_request.unit != self.unit {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        let source_currency = match melt_quote_request.unit {
            CurrencyUnit::Sat => StrikeCurrencyUnit::BTC,
            CurrencyUnit::Msat => StrikeCurrencyUnit::BTC,
            CurrencyUnit::Usd => StrikeCurrencyUnit::USD,
            CurrencyUnit::Eur => StrikeCurrencyUnit::EUR,
            _ => return Err(Self::Err::UnsupportedUnit),
        };

        let payment_quote_request = PayInvoiceQuoteRequest {
            ln_invoice: melt_quote_request.request.to_string(),
            source_currency,
        };

        let quote = self.strike_api.payment_quote(payment_quote_request).await?;

        let fee = from_strike_amount(quote.lightning_network_fee, &melt_quote_request.unit)?;

        Ok(PaymentQuoteResponse {
            request_lookup_id: quote.payment_quote_id,
            amount: from_strike_amount(quote.amount, &melt_quote_request.unit)?.into(),
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
            .strike_api
            .pay_quote(&melt_quote.request_lookup_id)
            .await?;

        let state = match pay_response.state {
            InvoiceState::Paid => MeltQuoteState::Paid,
            InvoiceState::Unpaid => MeltQuoteState::Unpaid,
            InvoiceState::Completed => MeltQuoteState::Paid,
            InvoiceState::Pending => MeltQuoteState::Pending,
        };

        let total_spent = from_strike_amount(pay_response.total_amount, &melt_quote.unit)?.into();

        Ok(PayInvoiceResponse {
            payment_lookup_id: pay_response.payment_id,
            payment_preimage: None,
            status: state,
            total_spent,
            unit: melt_quote.unit,
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        _unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);
        let request_lookup_id = Uuid::new_v4();

        let invoice_request = InvoiceRequest {
            correlation_id: Some(request_lookup_id.to_string()),
            amount: to_strike_unit(amount, &self.unit)?,
            description: Some(description),
        };

        let create_invoice_response = self.strike_api.create_invoice(invoice_request).await?;

        let quote = self
            .strike_api
            .invoice_quote(&create_invoice_response.invoice_id)
            .await?;

        let request: Bolt11Invoice = quote.ln_invoice.parse()?;
        let expiry = request.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: create_invoice_response.invoice_id,
            request: quote.ln_invoice.parse()?,
            expiry,
        })
    }

    async fn check_incoming_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let invoice = self
            .strike_api
            .get_incoming_invoice(request_lookup_id)
            .await?;

        let state = match invoice.state {
            InvoiceState::Paid => MintQuoteState::Paid,
            InvoiceState::Unpaid => MintQuoteState::Unpaid,
            InvoiceState::Completed => MintQuoteState::Paid,
            InvoiceState::Pending => MintQuoteState::Pending,
        };

        Ok(state)
    }

    async fn check_outgoing_payment(
        &self,
        payment_id: &str,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let invoice = self.strike_api.get_outgoing_payment(payment_id).await;

        let pay_invoice_response = match invoice {
            Ok(invoice) => {
                let state = match invoice.state {
                    InvoiceState::Paid => MeltQuoteState::Paid,
                    InvoiceState::Unpaid => MeltQuoteState::Unpaid,
                    InvoiceState::Completed => MeltQuoteState::Paid,
                    InvoiceState::Pending => MeltQuoteState::Pending,
                };

                PayInvoiceResponse {
                    payment_lookup_id: invoice.payment_id,
                    payment_preimage: None,
                    status: state,
                    total_spent: from_strike_amount(invoice.total_amount, &self.unit)?.into(),
                    unit: self.unit.clone(),
                }
            }
            Err(err) => match err {
                strike_rs::Error::NotFound => PayInvoiceResponse {
                    payment_lookup_id: payment_id.to_string(),
                    payment_preimage: None,
                    status: MeltQuoteState::Unknown,
                    total_spent: Amount::ZERO,
                    unit: self.unit.clone(),
                },
                _ => {
                    return Err(Error::from(err).into());
                }
            },
        };

        Ok(pay_invoice_response)
    }
}

impl Strike {
    /// Create invoice webhook
    pub async fn create_invoice_webhook(
        &self,
        webhook_endpoint: &str,
        sender: tokio::sync::mpsc::Sender<String>,
    ) -> anyhow::Result<Router> {
        let subs = self.strike_api.get_current_subscriptions().await?;

        tracing::debug!("Got {} current subscriptions", subs.len());

        for sub in subs {
            tracing::info!("Deleting webhook: {}", &sub.id);
            if let Err(err) = self.strike_api.delete_subscription(&sub.id).await {
                tracing::error!("Error deleting webhook subscription: {} {}", sub.id, err);
            }
        }

        self.strike_api
            .create_invoice_webhook_router(webhook_endpoint, sender)
            .await
    }
}

pub(crate) fn from_strike_amount(
    strike_amount: StrikeAmount,
    target_unit: &CurrencyUnit,
) -> anyhow::Result<u64> {
    match target_unit {
        CurrencyUnit::Sat => strike_amount.to_sats(),
        CurrencyUnit::Msat => Ok(strike_amount.to_sats()? * 1000),
        CurrencyUnit::Usd => {
            if strike_amount.currency == StrikeCurrencyUnit::USD {
                Ok((strike_amount.amount * 100.0).round() as u64)
            } else {
                bail!("Could not convert strike USD");
            }
        }
        CurrencyUnit::Eur => {
            if strike_amount.currency == StrikeCurrencyUnit::EUR {
                Ok((strike_amount.amount * 100.0).round() as u64)
            } else {
                bail!("Could not convert to EUR");
            }
        }
        _ => bail!("Unsupported unit"),
    }
}

pub(crate) fn to_strike_unit<T>(
    amount: T,
    current_unit: &CurrencyUnit,
) -> anyhow::Result<StrikeAmount>
where
    T: Into<u64>,
{
    let amount = amount.into();
    match current_unit {
        CurrencyUnit::Sat => Ok(StrikeAmount::from_sats(amount)),
        CurrencyUnit::Msat => Ok(StrikeAmount::from_sats(amount / 1000)),
        CurrencyUnit::Usd => {
            let dollars = (amount as f64 / 100_f64) * 100.0;

            Ok(StrikeAmount {
                currency: StrikeCurrencyUnit::USD,
                amount: dollars.round() / 100.0,
            })
        }
        CurrencyUnit::Eur => {
            let euro = (amount as f64 / 100_f64) * 100.0;

            Ok(StrikeAmount {
                currency: StrikeCurrencyUnit::EUR,
                amount: euro.round() / 100.0,
            })
        }
        _ => bail!("Unsupported unit"),
    }
}
