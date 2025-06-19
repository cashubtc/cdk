//! CDK lightning backend for lnbits

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use cdk_common::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, MakePaymentResponse, MintPayment,
    PaymentQuoteResponse,
};
use cdk_common::util::unix_time;
use cdk_common::{mint, Bolt11Invoice};
use error::Error;
use futures::Stream;
use lnbits_rs::api::invoice::CreateInvoiceRequest;
use lnbits_rs::LNBitsClient;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub mod error;

/// LNbits
#[derive(Clone)]
pub struct LNbits {
    lnbits_api: LNBitsClient,
    fee_reserve: FeeReserve,
    webhook_url: Option<String>,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    settings: Bolt11Settings,
}

impl LNbits {
    /// Create new [`LNbits`] wallet
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        admin_api_key: String,
        invoice_api_key: String,
        api_url: String,
        fee_reserve: FeeReserve,
        webhook_url: Option<String>,
    ) -> Result<Self, Error> {
        let lnbits_api = LNBitsClient::new("", &admin_api_key, &invoice_api_key, &api_url, None)?;

        Ok(Self {
            lnbits_api,
            fee_reserve,
            webhook_url,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            settings: Bolt11Settings {
                mpp: false,
                unit: CurrencyUnit::Sat,
                invoice_description: true,
                amountless: false,
            },
        })
    }

    /// Subscribe to lnbits ws
    pub async fn subscribe_ws(&self) -> Result<(), Error> {
        self.lnbits_api
            .subscribe_to_websocket()
            .await
            .map_err(|err| {
                tracing::error!("Could not subscribe to lnbits ws");
                Error::Anyhow(err)
            })
    }
}

#[async_trait]
impl MintPayment for LNbits {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<Value, Self::Err> {
        Ok(serde_json::to_value(&self.settings)?)
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let api = self.lnbits_api.clone();
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let is_active = Arc::clone(&self.wait_invoice_is_active);

        Ok(Box::pin(futures::stream::unfold(
            (api, cancel_token, is_active),
            |(api, cancel_token, is_active)| async move {
                is_active.store(true, Ordering::SeqCst);

                let receiver = api.receiver();
                let mut receiver = receiver.lock().await;

                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        // Stream is cancelled
                        is_active.store(false, Ordering::SeqCst);
                        tracing::info!("Waiting for lnbits invoice ending");
                        None
                    }
                    msg_option = receiver.recv() => {
                        match msg_option {
                            Some(msg) => {
                                let check = api.is_invoice_paid(&msg).await;

                                match check {
                                    Ok(state) => {
                                        if state {
                                            Some((msg, (api, cancel_token, is_active)))
                                        } else {
                                            Some(("".to_string(), (api, cancel_token, is_active)))
                                        }
                                    }
                                    _ => Some(("".to_string(), (api, cancel_token, is_active))),
                                }
                            }
                            None => {
                                is_active.store(false, Ordering::SeqCst);
                                None
                            },
                        }
                    }
                }
            },
        )))
    }

    async fn get_payment_quote(
        &self,
        request: &str,
        unit: &CurrencyUnit,
        options: Option<MeltOptions>,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        let bolt11 = Bolt11Invoice::from_str(request)?;

        let amount_msat = match options {
            Some(amount) => {
                if matches!(amount, MeltOptions::Mpp { mpp: _ }) {
                    return Err(payment::Error::UnsupportedPaymentOption);
                }
                amount.amount_msat()
            }
            None => bolt11
                .amount_milli_satoshis()
                .ok_or(Error::UnknownInvoiceAmount)?
                .into(),
        };

        let amount = amount_msat / MSAT_IN_SAT.into();

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
        _partial_msats: Option<Amount>,
        _max_fee_msats: Option<Amount>,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let pay_response = self
            .lnbits_api
            .pay_invoice(&melt_quote.request, None)
            .await
            .map_err(|err| {
                tracing::error!("Could not pay invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not pay invoice"))
            })?;

        let invoice_info = self
            .lnbits_api
            .get_payment_info(&pay_response.payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not find invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not find invoice"))
            })?;

        let status = match invoice_info.paid {
            true => MeltQuoteState::Paid,
            false => MeltQuoteState::Unpaid,
        };

        let total_spent = Amount::from(
            (invoice_info
                .details
                .amount
                .checked_add(invoice_info.details.fee)
                .ok_or(Error::AmountOverflow)?)
            .unsigned_abs(),
        );

        Ok(MakePaymentResponse {
            payment_lookup_id: pay_response.payment_hash,
            payment_proof: invoice_info.details.preimage,
            status,
            total_spent,
            unit: CurrencyUnit::Sat,
        })
    }

    async fn create_incoming_payment_request(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: Option<u64>,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        let time_now = unix_time();

        let expiry = unix_expiry.map(|t| t - time_now);

        let invoice_request = CreateInvoiceRequest {
            amount: to_unit(amount, unit, &CurrencyUnit::Sat)?.into(),
            memo: Some(description),
            unit: unit.to_string(),
            expiry,
            webhook: self.webhook_url.clone(),
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

        let request: Bolt11Invoice = create_invoice_response
            .bolt11()
            .ok_or_else(|| Self::Err::Anyhow(anyhow!("Missing bolt11 invoice")))?
            .parse()?;
        let expiry = request.expires_at().map(|t| t.as_secs());

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: create_invoice_response.payment_hash().to_string(),
            request: request.to_string(),
            expiry,
        })
    }

    async fn check_incoming_payment_status(
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
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment = self
            .lnbits_api
            .get_payment_info(payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        let pay_response = MakePaymentResponse {
            payment_lookup_id: payment.details.payment_hash,
            payment_proof: payment.preimage,
            status: lnbits_to_melt_status(&payment.details.status, payment.details.pending),
            total_spent: Amount::from(
                payment.details.amount.unsigned_abs()
                    + payment.details.fee.unsigned_abs() / MSAT_IN_SAT,
            ),
            unit: self.settings.unit.clone(),
        };

        Ok(pay_response)
    }
}

fn lnbits_to_melt_status(status: &str, pending: Option<bool>) -> MeltQuoteState {
    if pending.unwrap_or_default() {
        return MeltQuoteState::Pending;
    }

    match status {
        "success" => MeltQuoteState::Paid,
        "failed" => MeltQuoteState::Unpaid,
        "pending" => MeltQuoteState::Pending,
        _ => MeltQuoteState::Unknown,
    }
}

impl LNbits {
    /// Create invoice webhook
    pub async fn create_invoice_webhook_router(
        &self,
        webhook_endpoint: &str,
    ) -> anyhow::Result<Router> {
        self.lnbits_api
            .create_invoice_webhook_router(webhook_endpoint)
            .await
    }
}
