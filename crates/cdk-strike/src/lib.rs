//! CDK lightning backend for Strike

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, bail};
use async_trait::async_trait;
use axum::Router;
use cdk::cdk_lightning::{
    self, CreateInvoiceResponse, MintLightning, MintMeltSettings, PayInvoiceResponse,
    PaymentQuoteResponse, Settings,
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
use uuid::Uuid;

pub mod error;

/// Strike
#[derive(Clone)]
pub struct Strike {
    strike_api: StrikeApi,
    mint_settings: MintMeltSettings,
    melt_settings: MintMeltSettings,
    unit: CurrencyUnit,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    webhook_url: String,
}

impl Strike {
    /// Create new [`Strike`] wallet
    pub async fn new(
        api_key: String,
        mint_settings: MintMeltSettings,
        melt_settings: MintMeltSettings,
        unit: CurrencyUnit,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let strike = StrikeApi::new(&api_key, None)?;
        Ok(Self {
            strike_api: strike,
            mint_settings,
            melt_settings,
            receiver,
            unit,
            webhook_url,
        })
    }
}

#[async_trait]
impl MintLightning for Strike {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: false,
            unit: self.unit,
            mint_settings: self.mint_settings,
            melt_settings: self.melt_settings,
        }
    }

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

        Ok(futures::stream::unfold(
            (receiver, strike_api),
            |(mut receiver, strike_api)| async move {
                match receiver.recv().await {
                    Some(msg) => {
                        let check = strike_api.find_invoice(&msg).await;

                        match check {
                            Ok(state) => {
                                if state.state == InvoiceState::Paid {
                                    Some((msg, (receiver, strike_api)))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    None => None,
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

        let payment_quote_request = PayInvoiceQuoteRequest {
            ln_invoice: melt_quote_request.request.to_string(),
            source_currency: strike_rs::Currency::BTC,
        };
        let quote = self.strike_api.payment_quote(payment_quote_request).await?;

        let fee = from_strike_amount(quote.lightning_network_fee, &melt_quote_request.unit)?;

        Ok(PaymentQuoteResponse {
            request_lookup_id: quote.payment_quote_id,
            amount: from_strike_amount(quote.amount, &melt_quote_request.unit)?,
            fee,
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        _partial_msats: Option<u64>,
        _max_fee_msats: Option<u64>,
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

        let total_spent_msats = from_strike_amount(pay_response.total_amount, &melt_quote.unit)?;

        let bolt11: Bolt11Invoice = melt_quote.request.parse()?;

        Ok(PayInvoiceResponse {
            payment_hash: bolt11.payment_hash().to_string(),
            payment_preimage: None,
            status: state,
            total_spent_msats,
        })
    }

    async fn create_invoice(
        &self,
        amount: u64,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);
        let request_lookup_id = Uuid::new_v4();

        let invoice_request = InvoiceRequest {
            correlation_id: Some(request_lookup_id.to_string()),
            amount: to_strike_unit(amount, &self.unit),
            description: Some(description),
        };

        let create_invoice_response = self.strike_api.create_invoice(invoice_request).await?;

        let quote = self
            .strike_api
            .invoice_quote(&create_invoice_response.invoice_id)
            .await?;

        Ok(CreateInvoiceResponse {
            request_lookup_id: create_invoice_response.invoice_id,
            request: quote.ln_invoice.parse()?,
        })
    }

    async fn check_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let invoice = self.strike_api.find_invoice(request_lookup_id).await?;

        let state = match invoice.state {
            InvoiceState::Paid => MintQuoteState::Paid,
            InvoiceState::Unpaid => MintQuoteState::Unpaid,
            InvoiceState::Completed => MintQuoteState::Paid,
            InvoiceState::Pending => MintQuoteState::Pending,
        };

        Ok(state)
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
                bail!("Could not convert ");
            }
        }
        CurrencyUnit::Eur => {
            if strike_amount.currency == StrikeCurrencyUnit::EUR {
                Ok((strike_amount.amount * 100.0).round() as u64)
            } else {
                bail!("Could not convert ");
            }
        }
    }
}

pub(crate) fn to_strike_unit<T>(amount: T, current_unit: &CurrencyUnit) -> StrikeAmount
where
    T: Into<u64>,
{
    let amount = amount.into();
    match current_unit {
        CurrencyUnit::Sat => StrikeAmount::from_sats(amount),
        CurrencyUnit::Msat => StrikeAmount::from_sats(amount / 1000),
        CurrencyUnit::Usd => {
            let dollars = (amount as f64 / 100_f64) * 100.0;

            StrikeAmount {
                currency: StrikeCurrencyUnit::USD,
                amount: dollars.round() / 100.0,
            }
        }
        CurrencyUnit::Eur => {
            let euro = (amount as f64 / 100_f64) * 100.0;

            StrikeAmount {
                currency: StrikeCurrencyUnit::EUR,
                amount: euro.round() / 100.0,
            }
        }
    }
}
