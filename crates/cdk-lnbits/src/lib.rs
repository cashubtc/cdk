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
use cdk_common::amount::{to_unit, Amount};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
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
    ) -> Result<Self, Error> {
        let lnbits_api = LNBitsClient::new("", &admin_api_key, &invoice_api_key, &api_url, None)?;

        Ok(Self {
            lnbits_api,
            fee_reserve,
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
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        self.lnbits_api
            .subscribe_to_websocket()
            .await
            .map_err(|err| {
                tracing::error!("Could not subscribe to lnbits ws");
                Error::Anyhow(err)
            })
    }

    /// Process an incoming message from the websocket receiver
    async fn process_message(
        msg_option: Option<String>,
        api: &LNBitsClient,
        _is_active: &Arc<AtomicBool>,
    ) -> Option<WaitPaymentResponse> {
        let msg = msg_option?;

        let payment = match api.get_payment_info(&msg).await {
            Ok(payment) => payment,
            Err(_) => return None,
        };

        if !payment.paid {
            tracing::warn!(
                "Received payment notification but payment not paid for {}",
                msg
            );
            return None;
        }

        Self::create_payment_response(&msg, &payment).unwrap_or_else(|e| {
            tracing::error!("Failed to create payment response: {}", e);
            None
        })
    }

    /// Create a payment response from payment info
    fn create_payment_response(
        msg: &str,
        payment: &lnbits_rs::api::payment::Payment,
    ) -> Result<Option<WaitPaymentResponse>, Error> {
        let amount = payment.details.amount;

        if amount == i64::MIN {
            return Ok(None);
        }

        let hash = Self::decode_payment_hash(msg)?;

        Ok(Some(WaitPaymentResponse {
            payment_identifier: PaymentIdentifier::PaymentHash(hash),
            payment_amount: Amount::from(amount.unsigned_abs()),
            unit: CurrencyUnit::Msat,
            payment_id: msg.to_string(),
        }))
    }

    /// Decode a hex payment hash string into a byte array
    fn decode_payment_hash(hash_str: &str) -> Result<[u8; 32], Error> {
        let decoded = hex::decode(hash_str)
            .map_err(|e| Error::Anyhow(anyhow!("Failed to decode payment hash: {}", e)))?;

        decoded
            .try_into()
            .map_err(|_| Error::Anyhow(anyhow!("Invalid payment hash length")))
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

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
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
                        is_active.store(false, Ordering::SeqCst);
                        tracing::info!("Waiting for lnbits invoice ending");
                        None
                    }
                    msg_option = receiver.recv() => {
                        Self::process_message(msg_option, &api, &is_active)
                            .await
                            .map(|response| (Event::PaymentReceived(response), (api, cancel_token, is_active)))
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

                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount_msat) as f32) as u64;

                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

                let fee = max(relative_fee_reserve, absolute_fee_reserve);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    )),
                    amount: to_unit(amount_msat, &CurrencyUnit::Msat, unit)?,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
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
                    MeltQuoteState::Paid
                } else {
                    MeltQuoteState::Unpaid
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
                    unit: CurrencyUnit::Msat,
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

                let request: Bolt11Invoice = create_invoice_response.bolt11().parse()?;

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

        let amount = payment.details.amount;

        if amount == i64::MIN {
            return Err(Error::AmountOverflow.into());
        }

        match payment.paid {
            true => Ok(vec![WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: Amount::from(amount.unsigned_abs()),
                unit: CurrencyUnit::Msat,
                payment_id: payment.details.payment_hash,
            }]),
            false => Ok(vec![]),
        }
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
            status: lnbits_to_melt_status(&payment.details.status),
            total_spent: Amount::from(
                payment.details.amount.unsigned_abs() + payment.details.fee.unsigned_abs(),
            ),
            unit: CurrencyUnit::Msat,
        };

        Ok(pay_response)
    }
}

fn lnbits_to_melt_status(status: &str) -> MeltQuoteState {
    match status {
        "success" => MeltQuoteState::Paid,
        "failed" => MeltQuoteState::Unpaid,
        "pending" => MeltQuoteState::Pending,
        _ => MeltQuoteState::Unknown,
    }
}
