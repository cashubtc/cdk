//! CDK lightning backend for LND

// Copyright (c) 2023 Steffen (MIT)

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use cdk_common::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, MakePaymentResponse, MintPayment,
    PaymentQuoteResponse,
};
use cdk_common::util::hex;
use cdk_common::{mint, Bolt11Invoice};
use error::Error;
use futures::{Stream, StreamExt};
use lnrpc::fee_limit::Limit;
use lnrpc::payment::PaymentStatus;
use lnrpc::{FeeLimit, Hop, MppRecord};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

mod client;
pub mod error;

mod proto;
pub(crate) use proto::{lnrpc, routerrpc};

/// Lnd mint backend
#[derive(Clone)]
pub struct Lnd {
    _address: String,
    _cert_file: PathBuf,
    _macaroon_file: PathBuf,
    lnd_client: client::Client,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    settings: Bolt11Settings,
}

impl Lnd {
    /// Maximum number of attempts at a partial payment
    pub const MAX_ROUTE_RETRIES: usize = 50;

    /// Create new [`Lnd`]
    pub async fn new(
        address: String,
        cert_file: PathBuf,
        macaroon_file: PathBuf,
        fee_reserve: FeeReserve,
    ) -> Result<Self, Error> {
        // Validate address is not empty
        if address.is_empty() {
            return Err(Error::InvalidConfig("LND address cannot be empty".into()));
        }

        // Validate cert_file exists and is not empty
        if !cert_file.exists() || cert_file.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            return Err(Error::InvalidConfig(format!(
                "LND certificate file not found or empty: {cert_file:?}"
            )));
        }

        // Validate macaroon_file exists and is not empty
        if !macaroon_file.exists()
            || macaroon_file
                .metadata()
                .map(|m| m.len() == 0)
                .unwrap_or(true)
        {
            return Err(Error::InvalidConfig(format!(
                "LND macaroon file not found or empty: {macaroon_file:?}"
            )));
        }

        let lnd_client = client::connect(&address, &cert_file, &macaroon_file)
            .await
            .map_err(|err| {
                tracing::error!("Connection error: {}", err.to_string());
                Error::Connection
            })
            .unwrap();

        Ok(Self {
            _address: address,
            _cert_file: cert_file,
            _macaroon_file: macaroon_file,
            lnd_client,
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            settings: Bolt11Settings {
                mpp: true,
                unit: CurrencyUnit::Msat,
                invoice_description: true,
                amountless: true,
            },
        })
    }
}

#[async_trait]
impl MintPayment for Lnd {
    type Err = payment::Error;

    #[instrument(skip_all)]
    async fn get_settings(&self) -> Result<serde_json::Value, Self::Err> {
        Ok(serde_json::to_value(&self.settings)?)
    }

    #[instrument(skip_all)]
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    #[instrument(skip_all)]
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    #[instrument(skip_all)]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let mut lnd_client = self.lnd_client.clone();

        let stream_req = lnrpc::InvoiceSubscription {
            add_index: 0,
            settle_index: 0,
        };

        let stream = lnd_client
            .lightning()
            .subscribe_invoices(stream_req)
            .await
            .map_err(|_err| {
                tracing::error!("Could not subscribe to invoice");
                Error::Connection
            })?
            .into_inner();

        let cancel_token = self.wait_invoice_cancel_token.clone();

        Ok(futures::stream::unfold(
            (
                stream,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut stream, cancel_token, is_active)| async move {
                is_active.store(true, Ordering::SeqCst);

                tokio::select! {
                    _ = cancel_token.cancelled() => {
                    // Stream is cancelled
                    is_active.store(false, Ordering::SeqCst);
                    tracing::info!("Waiting for lnd invoice ending");
                    None

                    }
                    msg = stream.message() => {

                match msg {
                    Ok(Some(msg)) => {
                        if msg.state == 1 {
                            Some((hex::encode(msg.r_hash), (stream, cancel_token, is_active)))
                        } else {
                            None
                        }
                    }
                    Ok(None) => {
                    is_active.store(false, Ordering::SeqCst);
                    tracing::info!("LND invoice stream ended.");
                        None
                    }, // End of stream
                    Err(err) => {
                    is_active.store(false, Ordering::SeqCst);
                    tracing::warn!("Encountered error in LND invoice stream. Stream ending");
                    tracing::error!("{:?}", err);
                    None

                    },   // Handle errors gracefully, ends the stream on error
                }
                    }
                }
            },
        )
        .boxed())
    }

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment_request = melt_quote.request;
        let bolt11 = Bolt11Invoice::from_str(&payment_request)?;

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

        let bolt11 = Bolt11Invoice::from_str(&payment_request)?;
        let amount_msat: u64 = match bolt11.amount_milli_satoshis() {
            Some(amount_msat) => amount_msat,
            None => melt_quote
                .msat_to_pay
                .ok_or(Error::UnknownInvoiceAmount)?
                .into(),
        };

        // Detect partial payments
        match partial_amount {
            Some(part_amt) => {
                let partial_amount_msat = to_unit(part_amt, &melt_quote.unit, &CurrencyUnit::Msat)?;
                let invoice = Bolt11Invoice::from_str(&payment_request)?;

                // Extract information from invoice
                let pub_key = invoice.get_payee_pub_key();
                let payer_addr = invoice.payment_secret().0.to_vec();
                let payment_hash = invoice.payment_hash();

                let mut lnd_client = self.lnd_client.clone();

                for attempt in 0..Self::MAX_ROUTE_RETRIES {
                    // Create a request for the routes
                    let route_req = lnrpc::QueryRoutesRequest {
                        pub_key: hex::encode(pub_key.serialize()),
                        amt_msat: u64::from(partial_amount_msat) as i64,
                        fee_limit: max_fee.map(|f| {
                            let limit = Limit::Fixed(u64::from(f) as i64);
                            FeeLimit { limit: Some(limit) }
                        }),
                        use_mission_control: true,
                        ..Default::default()
                    };

                    // Query the routes
                    let mut routes_response = lnd_client
                        .lightning()
                        .query_routes(route_req)
                        .await
                        .map_err(Error::LndError)?
                        .into_inner();

                    // update its MPP record,
                    // attempt it and check the result
                    let last_hop: &mut Hop = routes_response.routes[0]
                        .hops
                        .last_mut()
                        .ok_or(Error::MissingLastHop)?;
                    let mpp_record = MppRecord {
                        payment_addr: payer_addr.clone(),
                        total_amt_msat: amount_msat as i64,
                    };
                    last_hop.mpp_record = Some(mpp_record);

                    let payment_response = lnd_client
                        .router()
                        .send_to_route_v2(routerrpc::SendToRouteRequest {
                            payment_hash: payment_hash.to_byte_array().to_vec(),
                            route: Some(routes_response.routes[0].clone()),
                            ..Default::default()
                        })
                        .await
                        .map_err(Error::LndError)?
                        .into_inner();

                    if let Some(failure) = payment_response.failure {
                        if failure.code == 15 {
                            tracing::debug!(
                                "Attempt number {}: route has failed. Re-querying...",
                                attempt + 1
                            );
                            continue;
                        }
                    }

                    // Get status and maybe the preimage
                    let (status, payment_preimage) = match payment_response.status {
                        0 => (MeltQuoteState::Pending, None),
                        1 => (
                            MeltQuoteState::Paid,
                            Some(hex::encode(payment_response.preimage)),
                        ),
                        2 => (MeltQuoteState::Unpaid, None),
                        _ => (MeltQuoteState::Unknown, None),
                    };

                    // Get the actual amount paid in sats
                    let mut total_amt: u64 = 0;
                    if let Some(route) = payment_response.route {
                        total_amt = (route.total_amt_msat / 1000) as u64;
                    }

                    return Ok(MakePaymentResponse {
                        payment_lookup_id: hex::encode(payment_hash),
                        payment_proof: payment_preimage,
                        status,
                        total_spent: total_amt.into(),
                        unit: CurrencyUnit::Sat,
                    });
                }

                // "We have exhausted all tactical options" -- STEM, Upgrade (2018)
                // The payment was not possible within 50 retries.
                tracing::error!("Limit of retries reached, payment couldn't succeed.");
                Err(Error::PaymentFailed.into())
            }
            None => {
                let mut lnd_client = self.lnd_client.clone();

                let pay_req = lnrpc::SendRequest {
                    payment_request,
                    fee_limit: max_fee.map(|f| {
                        let limit = Limit::Fixed(u64::from(f) as i64);
                        FeeLimit { limit: Some(limit) }
                    }),
                    amt_msat: amount_msat as i64,
                    ..Default::default()
                };

                let payment_response = lnd_client
                    .lightning()
                    .send_payment_sync(tonic::Request::new(pay_req))
                    .await
                    .map_err(|err| {
                        tracing::warn!("Lightning payment failed: {}", err);
                        Error::PaymentFailed
                    })?
                    .into_inner();

                let total_amount = payment_response
                    .payment_route
                    .map_or(0, |route| route.total_amt_msat / MSAT_IN_SAT as i64)
                    as u64;

                let (status, payment_preimage) = match total_amount == 0 {
                    true => (MeltQuoteState::Unpaid, None),
                    false => (
                        MeltQuoteState::Paid,
                        Some(hex::encode(payment_response.payment_preimage)),
                    ),
                };

                Ok(MakePaymentResponse {
                    payment_lookup_id: hex::encode(payment_response.payment_hash),
                    payment_proof: payment_preimage,
                    status,
                    total_spent: total_amount.into(),
                    unit: CurrencyUnit::Sat,
                })
            }
        }
    }

    #[instrument(skip(self, description))]
    async fn create_incoming_payment_request(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: Option<u64>,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;

        let invoice_request = lnrpc::Invoice {
            value_msat: u64::from(amount) as i64,
            memo: description,
            ..Default::default()
        };

        let mut lnd_client = self.lnd_client.clone();

        let invoice = lnd_client
            .lightning()
            .add_invoice(tonic::Request::new(invoice_request))
            .await
            .map_err(|e| payment::Error::Anyhow(anyhow!(e)))?
            .into_inner();

        let bolt11 = Bolt11Invoice::from_str(&invoice.payment_request)?;

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: bolt11.payment_hash().to_string(),
            request: bolt11.to_string(),
            expiry: unix_expiry,
        })
    }

    #[instrument(skip(self))]
    async fn check_incoming_payment_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut lnd_client = self.lnd_client.clone();

        let invoice_request = lnrpc::PaymentHash {
            r_hash: hex::decode(request_lookup_id).unwrap(),
            ..Default::default()
        };

        let invoice = lnd_client
            .lightning()
            .lookup_invoice(tonic::Request::new(invoice_request))
            .await
            .map_err(|e| payment::Error::Anyhow(anyhow!(e)))?
            .into_inner();

        match invoice.state {
            // Open
            0 => Ok(MintQuoteState::Unpaid),
            // Settled
            1 => Ok(MintQuoteState::Paid),
            // Canceled
            2 => Ok(MintQuoteState::Unpaid),
            // Accepted
            3 => Ok(MintQuoteState::Unpaid),
            _ => Err(Self::Err::Anyhow(anyhow!("Invalid status"))),
        }
    }

    #[instrument(skip(self))]
    async fn check_outgoing_payment(
        &self,
        payment_hash: &str,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let mut lnd_client = self.lnd_client.clone();

        let track_request = routerrpc::TrackPaymentRequest {
            payment_hash: hex::decode(payment_hash).map_err(|_| Error::InvalidHash)?,
            no_inflight_updates: true,
        };

        let payment_response = lnd_client.router().track_payment_v2(track_request).await;

        let mut payment_stream = match payment_response {
            Ok(stream) => stream.into_inner(),
            Err(err) => {
                let err_code = err.code();
                if err_code == tonic::Code::NotFound {
                    return Ok(MakePaymentResponse {
                        payment_lookup_id: payment_hash.to_string(),
                        payment_proof: None,
                        status: MeltQuoteState::Unknown,
                        total_spent: Amount::ZERO,
                        unit: self.settings.unit.clone(),
                    });
                } else {
                    return Err(payment::Error::UnknownPaymentState);
                }
            }
        };

        while let Some(update_result) = payment_stream.next().await {
            match update_result {
                Ok(update) => {
                    let status = update.status();

                    let response = match status {
                        PaymentStatus::Unknown => MakePaymentResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_proof: Some(update.payment_preimage),
                            status: MeltQuoteState::Unknown,
                            total_spent: Amount::ZERO,
                            unit: self.settings.unit.clone(),
                        },
                        PaymentStatus::InFlight | PaymentStatus::Initiated => {
                            // Continue waiting for the next update
                            continue;
                        }
                        PaymentStatus::Succeeded => MakePaymentResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_proof: Some(update.payment_preimage),
                            status: MeltQuoteState::Paid,
                            total_spent: Amount::from(
                                (update
                                    .value_sat
                                    .checked_add(update.fee_sat)
                                    .ok_or(Error::AmountOverflow)?)
                                    as u64,
                            ),
                            unit: CurrencyUnit::Sat,
                        },
                        PaymentStatus::Failed => MakePaymentResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_proof: Some(update.payment_preimage),
                            status: MeltQuoteState::Failed,
                            total_spent: Amount::ZERO,
                            unit: self.settings.unit.clone(),
                        },
                    };

                    return Ok(response);
                }
                Err(_) => {
                    // Handle the case where the update itself is an error (e.g., stream failure)
                    return Err(Error::UnknownPaymentStatus.into());
                }
            }
        }

        // If the stream is exhausted without a final status
        Err(Error::UnknownPaymentStatus.into())
    }
}
