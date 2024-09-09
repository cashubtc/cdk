//! CDK lightning backend for Phoenixd

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use cdk::amount::Amount;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, MintMeltSettings, PayInvoiceResponse,
    PaymentQuoteResponse, Settings, MSAT_IN_SAT,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::{mint, Bolt11Invoice};
use error::Error;
use futures::{Stream, StreamExt};
use phoenixd_rs::webhooks::WebhookResponse;
use phoenixd_rs::{InvoiceRequest, Phoenixd as PhoenixdApi};
use tokio::sync::Mutex;

pub mod error;

/// Phoenixd
#[derive(Clone)]
pub struct Phoenixd {
    mint_settings: MintMeltSettings,
    melt_settings: MintMeltSettings,
    phoenixd_api: PhoenixdApi,
    fee_reserve: FeeReserve,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<WebhookResponse>>>>,
    webhook_url: String,
}

impl Phoenixd {
    /// Create new [`Phoenixd`] wallet
    pub fn new(
        api_password: String,
        api_url: String,
        mint_settings: MintMeltSettings,
        melt_settings: MintMeltSettings,
        fee_reserve: FeeReserve,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<WebhookResponse>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let phoenixd = PhoenixdApi::new(&api_password, &api_url)?;
        Ok(Self {
            mint_settings,
            melt_settings,
            phoenixd_api: phoenixd,
            fee_reserve,
            receiver,
            webhook_url,
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
            mint_settings: self.mint_settings,
            melt_settings: self.melt_settings,
            invoice_description: true,
        }
    }

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

        Ok(futures::stream::unfold(
            (receiver, phoenixd_api),
            |(mut receiver, phoenixd_api)| async move {
                match receiver.recv().await {
                    Some(msg) => {
                        let check = phoenixd_api.get_incoming_invoice(&msg.payment_hash).await;

                        match check {
                            Ok(state) => {
                                if state.is_paid {
                                    Some((msg.payment_hash, (receiver, phoenixd_api)))
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

        // The pay response does not include the fee paided to Aciq so we check it here
        let check_outgoing_response = self
            .check_outgoing_invoice(&pay_response.payment_id)
            .await?;

        if check_outgoing_response.state != MeltQuoteState::Paid {
            return Err(anyhow!("Invoice is not paid").into());
        }

        let total_spent_sats = check_outgoing_response.fee + check_outgoing_response.amount;

        let bolt11: Bolt11Invoice = melt_quote.request.parse()?;

        Ok(PayInvoiceResponse {
            payment_hash: bolt11.payment_hash().to_string(),
            payment_preimage: Some(pay_response.payment_preimage),
            status: MeltQuoteState::Paid,
            total_spent: total_spent_sats,
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

    async fn check_invoice_status(&self, payment_hash: &str) -> Result<MintQuoteState, Self::Err> {
        let invoice = self.phoenixd_api.get_incoming_invoice(payment_hash).await?;

        let state = match invoice.is_paid {
            true => MintQuoteState::Paid,
            false => MintQuoteState::Unpaid,
        };

        Ok(state)
    }
}

impl Phoenixd {
    /// Check the status of an outgooing invoice
    // TODO: This should likely bee added to the trait. Both CLN and PhD use a form
    // of it
    async fn check_outgoing_invoice(
        &self,
        payment_hash: &str,
    ) -> Result<PaymentQuoteResponse, Error> {
        let res = self.phoenixd_api.get_outgoing_invoice(payment_hash).await?;

        // Phenixd gives fees in msats so we need to round up to the nearst sat
        let fee_sats = (res.fees + 999) / MSAT_IN_SAT;

        let state = match res.is_paid {
            true => MeltQuoteState::Paid,
            false => MeltQuoteState::Unpaid,
        };

        let quote_response = PaymentQuoteResponse {
            request_lookup_id: res.payment_hash,
            amount: res.sent.into(),
            fee: fee_sats.into(),
            state,
        };

        Ok(quote_response)
    }
}
