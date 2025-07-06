//! CDK lightning backend for lnbits

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use cdk_common::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::{hex, unix_time};
use cdk_common::Bolt11Invoice;
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
                bolt12: false,
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
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
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
                                let check = api.get_payment_info(&msg).await;
                                match check {
                                    Ok(payment) => {
                                        if payment.paid {
                                            match hex::decode(msg.clone()) {
                                                Ok(decoded) => {
                                                    match decoded.try_into() {
                                                        Ok(hash) => {
                                                            let response = WaitPaymentResponse {
                                                                payment_identifier: PaymentIdentifier::PaymentHash(hash),
                                                                payment_amount: Amount::from(payment.details.amount as u64),
                                                                unit: CurrencyUnit::Sat,
                                                                payment_id: msg.clone()
                                                            };
                                                            Some((response, (api, cancel_token, is_active)))
                                                        },
                                                        Err(e) => {
                                                            tracing::error!("Failed to convert payment hash bytes to array: {:?}", e);
                                                            None
                                                        }
                                                    }
                                                },
                                                Err(e) => {
                                                    tracing::error!("Failed to decode payment hash hex string: {}", e);
                                                    None
                                                }
                                            }
                                        } else {
                                            tracing::warn!("Received payment notification but could not check payment for {}", msg);
                                            None
                                        }
                                    },
                                    Err(_) => None
                                }
                            },
                            None => {
                                is_active.store(false, Ordering::SeqCst);
                                None
                            }
                        }
                    }
                }
            },
        )))
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = match bolt11_options.melt_options {
                    Some(amount) => {
                        if matches!(amount, MeltOptions::Mpp { mpp: _ }) {
                            return Err(payment::Error::UnsupportedPaymentOption);
                        }
                        amount.amount_msat()
                    }
                    None => bolt11_options
                        .bolt11
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
                    request_lookup_id: PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    ),
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    options: None,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(_bolt12_options) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LNbits")))
            }
        }
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let pay_response = self
                    .lnbits_api
                    .pay_invoice(&bolt11_options.bolt11.to_string(), None)
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

                let status = if invoice_info.paid {
                    MeltQuoteState::Unpaid
                } else {
                    MeltQuoteState::Paid
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
                    payment_lookup_id: PaymentIdentifier::PaymentHash(
                        hex::decode(pay_response.payment_hash)
                            .map_err(|_| Error::InvalidPaymentHash)?
                            .try_into()
                            .map_err(|_| Error::InvalidPaymentHash)?,
                    ),
                    payment_proof: Some(invoice_info.details.payment_hash),
                    status,
                    total_spent,
                    unit: CurrencyUnit::Sat,
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LNbits")))
            }
        }
    }

    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let description = bolt11_options.description.unwrap_or_default();
                let amount = bolt11_options.amount;
                let unix_expiry = bolt11_options.unix_expiry;

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
                    request_lookup_id: PaymentIdentifier::PaymentHash(
                        *request.payment_hash().as_ref(),
                    ),
                    request: request.to_string(),
                    expiry,
                })
            }
            IncomingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LNbits")))
            }
        }
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let payment = self
            .lnbits_api
            .get_payment_info(&payment_identifier.to_string())
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        Ok(vec![WaitPaymentResponse {
            payment_identifier: payment_identifier.clone(),
            payment_amount: Amount::from(payment.details.amount as u64),
            unit: CurrencyUnit::Sat,
            payment_id: payment.details.payment_hash,
        }])
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment = self
            .lnbits_api
            .get_payment_info(&payment_identifier.to_string())
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        let pay_response = MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
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
