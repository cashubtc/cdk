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
use bitcoin::hashes::Hash;
use cdk_common::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::hex;
use cdk_common::Bolt11Invoice;
use error::Error;
use fedimint_tonic_lnd::lnrpc::fee_limit::Limit;
use fedimint_tonic_lnd::lnrpc::invoice::InvoiceState;
use fedimint_tonic_lnd::lnrpc::payment::PaymentStatus;
use fedimint_tonic_lnd::lnrpc::{FeeLimit, Hop, MppRecord};
use fedimint_tonic_lnd::tonic::Code;
use fedimint_tonic_lnd::Client;
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

pub mod error;

/// Lnd mint backend
#[derive(Clone)]
pub struct Lnd {
    address: String,
    cert_file: PathBuf,
    macaroon_file: PathBuf,
    client: Arc<Mutex<Client>>,
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

        let client = fedimint_tonic_lnd::connect(address.to_string(), &cert_file, &macaroon_file)
            .await
            .map_err(|err| {
                tracing::error!("Connection error: {}", err.to_string());
                Error::Connection
            })?;

        Ok(Self {
            address,
            cert_file,
            macaroon_file,
            client: Arc::new(Mutex::new(client)),
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
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
        tracing::info!(
            "LND: Starting wait_any_incoming_payment with address: {}",
            self.address
        );

        let mut client =
            fedimint_tonic_lnd::connect(self.address.clone(), &self.cert_file, &self.macaroon_file)
                .await
                .map_err(|err| {
                    tracing::error!(
                        "LND: Connection error in wait_any_incoming_payment: {}",
                        err
                    );
                    Error::Connection
                })?;
        tracing::debug!("LND: Connected to LND node successfully");

        let stream_req = fedimint_tonic_lnd::lnrpc::InvoiceSubscription {
            add_index: 0,
            settle_index: 0,
        };
        tracing::debug!(
            "LND: Created invoice subscription request with add_index: {}, settle_index: {}",
            stream_req.add_index,
            stream_req.settle_index
        );

        tracing::debug!("LND: Attempting to subscribe to invoices...");
        let stream = client
            .lightning()
            .subscribe_invoices(stream_req)
            .await
            .map_err(|err| {
                tracing::error!("LND: Could not subscribe to invoices: {}", err);
                Error::Connection
            })?
            .into_inner();
        tracing::info!("LND: Successfully subscribed to invoice stream");

        let cancel_token = self.wait_invoice_cancel_token.clone();

        tracing::debug!("LND: Creating stream processing pipeline");
        Ok(futures::stream::unfold(
            (
                stream,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut stream, cancel_token, is_active)| async move {
                is_active.store(true, Ordering::SeqCst);
                tracing::debug!("LND: Stream is now active, waiting for invoice events");

                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        // Stream is cancelled
                        is_active.store(false, Ordering::SeqCst);
                        tracing::info!("LND: Invoice stream cancelled");
                        None
                    }
                    msg = stream.message() => {
                        tracing::debug!("LND: Received message from invoice stream");                        
                        match msg {
                            Ok(Some(msg)) => {
                                tracing::debug!("LND: Invoice message - state: {:?}, memo: {}, amt_paid_msat: {}", 
                                              msg.state(), msg.memo, msg.amt_paid_msat);
                                if msg.state() == InvoiceState::Settled {
                                    tracing::info!("LND: Received settled invoice with memo: '{}', amount: {} msat", 
                                                 msg.memo, msg.amt_paid_msat);
                                    match msg.r_hash.clone().try_into() {
                                        Ok(hash) => {
                                            let hash_hex = hex::encode(hash);
                                            tracing::info!("LND: Processing payment with hash: {}", hash_hex);
                                            let wait_response = WaitPaymentResponse {
                                                payment_identifier: PaymentIdentifier::PaymentHash(hash), payment_amount: Amount::from(msg.amt_paid_msat as u64),
                                                unit: CurrencyUnit::Msat,
                                                payment_id: hex::encode(msg.r_hash),
                                            };
                                            tracing::info!("LND: Created WaitPaymentResponse with amount {} msat", 
                                                         msg.amt_paid_msat);
                                            Some((wait_response, (stream, cancel_token, is_active)))
                                        },
                                        Err(err) => {
                                            tracing::error!("LND: Failed to convert r_hash to payment hash: {:?}", err);
                                            tracing::debug!("LND: Continuing to wait for next invoice");
                                            None
                                        }
                                    }
                                } else {
                                    tracing::debug!("LND: Ignoring non-settled invoice with state: {:?}", msg.state());
                                    None
                                }
                            }
                            Ok(None) => {
                                is_active.store(false, Ordering::SeqCst);
                                tracing::info!("LND: Invoice stream ended (received None)");
                                None
                            }, // End of stream
                            Err(err) => {
                                is_active.store(false, Ordering::SeqCst);
                                tracing::error!("LND: Error in invoice stream: {:?}", err);
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
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = match bolt11_options.melt_options {
                    Some(amount) => amount.amount_msat(),
                    None => bolt11_options
                        .bolt11
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
                    request_lookup_id: PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    ),
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    options: None,
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
        }
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;

                let pay_state = self
                    .check_outgoing_payment(&PaymentIdentifier::PaymentHash(
                        *bolt11.payment_hash().as_ref(),
                    ))
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

                match bolt11_options.melt_options {
                    Some(MeltOptions::Mpp { mpp }) => {
                        let amount_msat: u64 = bolt11
                            .amount_milli_satoshis()
                            .ok_or(Error::UnknownInvoiceAmount)?;

                        tracing::debug!("Attempting lnd mpp payment of {} sats.", mpp.amount);
                        let max_fee: Option<Amount> = bolt11_options.max_fee_amount;
                        let part_amt = mpp.amount;
                        let invoice = bolt11.clone();

                        // Extract information from invoice
                        let pub_key = invoice.get_payee_pub_key();
                        let payer_addr = invoice.payment_secret().0.to_vec();
                        let payment_hash = invoice.payment_hash();

                        for attempt in 0..Self::MAX_ROUTE_RETRIES {
                            // Create a request for the routes
                            let route_req = fedimint_tonic_lnd::lnrpc::QueryRoutesRequest {
                                pub_key: hex::encode(pub_key.serialize()),
                                amt_msat: u64::from(part_amt) as i64,
                                fee_limit: max_fee.map(|f| {
                                    let limit = Limit::Fixed(u64::from(f) as i64);
                                    FeeLimit { limit: Some(limit) }
                                }),
                                use_mission_control: true,
                                ..Default::default()
                            };

                            // Query the routes
                            let mut routes_response: fedimint_tonic_lnd::lnrpc::QueryRoutesResponse = self
                        .client
                        .lock()
                        .await
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

                            let payment_response = self
                                .client
                                .lock()
                                .await
                                .router()
                                .send_to_route_v2(
                                    fedimint_tonic_lnd::routerrpc::SendToRouteRequest {
                                        payment_hash: payment_hash.to_byte_array().to_vec(),
                                        route: Some(routes_response.routes[0].clone()),
                                        ..Default::default()
                                    },
                                )
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
                                payment_lookup_id: PaymentIdentifier::PaymentHash(
                                    *payment_hash.as_ref(),
                                ),
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
                    _ => {
                        let amount_msat: u64 = match bolt11.amount_milli_satoshis() {
                            Some(amount_msat) => amount_msat,
                            None => bolt11_options
                                .melt_options
                                .ok_or(Error::UnknownInvoiceAmount)?
                                .amount_msat()
                                .into(),
                        };
                        let max_fee: Option<Amount> = bolt11_options.max_fee_amount;
                        let pay_req = fedimint_tonic_lnd::lnrpc::SendRequest {
                            payment_request: bolt11.to_string(),
                            fee_limit: max_fee.map(|f| {
                                let limit = Limit::Fixed(u64::from(f) as i64);

                                FeeLimit { limit: Some(limit) }
                            }),
                            amt_msat: amount_msat as i64,
                            ..Default::default()
                        };

                        let payment_response = self
                            .client
                            .lock()
                            .await
                            .lightning()
                            .send_payment_sync(fedimint_tonic_lnd::tonic::Request::new(pay_req))
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
                        let payment_identifier =
                            PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref());

                        Ok(MakePaymentResponse {
                            payment_lookup_id: payment_identifier,
                            payment_proof: payment_preimage,
                            status,
                            total_spent: total_amount.into(),
                            unit: CurrencyUnit::Sat,
                        })
                    }
                }
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
        }
    }

    #[instrument(skip(self))]
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

                let amount_msat = to_unit(amount, unit, &CurrencyUnit::Msat)?;

                let invoice_request = fedimint_tonic_lnd::lnrpc::Invoice {
                    value_msat: u64::from(amount_msat) as i64,
                    memo: description,
                    ..Default::default()
                };

                let invoice = self
                    .client
                    .lock()
                    .await
                    .lightning()
                    .add_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
                    .await
                    .unwrap()
                    .into_inner();

                let bolt11 = Bolt11Invoice::from_str(&invoice.payment_request)?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(
                        *bolt11.payment_hash().as_ref(),
                    ),
                    request: bolt11.to_string(),
                    expiry: unix_expiry,
                })
            }
            IncomingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
        }
    }

    #[instrument(skip(self))]
    async fn check_incoming_payment_status(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let invoice_request = fedimint_tonic_lnd::lnrpc::PaymentHash {
            r_hash: hex::decode(request_lookup_id.to_string())?,
            ..Default::default()
        };

        let invoice = self
            .client
            .lock()
            .await
            .lightning()
            .lookup_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
            .await
            .unwrap()
            .into_inner();

        if invoice.state() == InvoiceState::Settled {
            Ok(vec![WaitPaymentResponse {
                payment_identifier: request_lookup_id.clone(),
                payment_amount: Amount::from(invoice.amt_paid_msat as u64),
                unit: CurrencyUnit::Msat,
                payment_id: hex::encode(invoice.r_hash),
            }])
        } else {
            Ok(vec![])
        }
    }

    #[instrument(skip(self))]
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment_hash = match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) => hash,
            _ => return Err(payment::Error::UnknownPaymentState),
        };

        let track_request = fedimint_tonic_lnd::routerrpc::TrackPaymentRequest {
            payment_hash: payment_hash.to_vec(),
            no_inflight_updates: true,
        };

        let payment_response = self
            .client
            .lock()
            .await
            .router()
            .track_payment_v2(track_request)
            .await;

        let mut payment_stream = match payment_response {
            Ok(stream) => stream.into_inner(),
            Err(err) => {
                let err_code = err.code();
                if err_code == Code::NotFound {
                    return Ok(MakePaymentResponse {
                        payment_lookup_id: payment_identifier.clone(),
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
                            payment_lookup_id: payment_identifier.clone(),
                            payment_proof: Some(update.payment_preimage),
                            status: MeltQuoteState::Unknown,
                            total_spent: Amount::ZERO,
                            unit: self.settings.unit.clone(),
                        },
                        PaymentStatus::InFlight => {
                            // Continue waiting for the next update
                            continue;
                        }
                        PaymentStatus::Succeeded => MakePaymentResponse {
                            payment_lookup_id: payment_identifier.clone(),
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
                            payment_lookup_id: payment_identifier.clone(),
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
