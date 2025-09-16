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
use cdk_common::database::mint::DynMintKVStore;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::hex;
use cdk_common::Bolt11Invoice;
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

use crate::lnrpc::invoice::InvoiceState;

/// LND KV Store constants
const LND_KV_PRIMARY_NAMESPACE: &str = "cdk_lnd_lightning_backend";
const LND_KV_SECONDARY_NAMESPACE: &str = "payment_indices";
const LAST_ADD_INDEX_KV_KEY: &str = "last_add_index";
const LAST_SETTLE_INDEX_KV_KEY: &str = "last_settle_index";

/// Lnd mint backend
#[derive(Clone)]
pub struct Lnd {
    _address: String,
    _cert_file: PathBuf,
    _macaroon_file: PathBuf,
    lnd_client: client::Client,
    fee_reserve: FeeReserve,
    kv_store: DynMintKVStore,
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
        kv_store: DynMintKVStore,
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
            kv_store,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            settings: Bolt11Settings {
                mpp: true,
                unit: CurrencyUnit::Msat,
                invoice_description: true,
                amountless: true,
                bolt12: false,
            },
        })
    }

    /// Get last add and settle indices from KV store
    #[instrument(skip_all)]
    async fn get_last_indices(&self) -> Result<(Option<u64>, Option<u64>), Error> {
        let add_index = if let Some(stored_index) = self
            .kv_store
            .kv_read(
                LND_KV_PRIMARY_NAMESPACE,
                LND_KV_SECONDARY_NAMESPACE,
                LAST_ADD_INDEX_KV_KEY,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(stored_index.as_slice()) {
                index_str.parse::<u64>().ok()
            } else {
                None
            }
        } else {
            None
        };

        let settle_index = if let Some(stored_index) = self
            .kv_store
            .kv_read(
                LND_KV_PRIMARY_NAMESPACE,
                LND_KV_SECONDARY_NAMESPACE,
                LAST_SETTLE_INDEX_KV_KEY,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(stored_index.as_slice()) {
                index_str.parse::<u64>().ok()
            } else {
                None
            }
        } else {
            None
        };

        tracing::debug!(
            "LND: Retrieved last indices from KV store - add_index: {:?}, settle_index: {:?}",
            add_index,
            settle_index
        );
        Ok((add_index, settle_index))
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
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let mut lnd_client = self.lnd_client.clone();

        // Get last indices from KV store
        let (last_add_index, last_settle_index) =
            self.get_last_indices().await.unwrap_or((None, None));

        let stream_req = lnrpc::InvoiceSubscription {
            add_index: last_add_index.unwrap_or(0),
            settle_index: last_settle_index.unwrap_or(0),
        };

        tracing::debug!(
            "LND: Starting invoice subscription with add_index: {}, settle_index: {}",
            stream_req.add_index,
            stream_req.settle_index
        );

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
        let kv_store = self.kv_store.clone();

        let event_stream = futures::stream::unfold(
            (
                stream,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
                kv_store,
                last_add_index.unwrap_or(0),
                last_settle_index.unwrap_or(0),
            ),
            |(
                mut stream,
                cancel_token,
                is_active,
                kv_store,
                mut current_add_index,
                mut current_settle_index,
            )| async move {
                is_active.store(true, Ordering::SeqCst);

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Stream is cancelled
                            is_active.store(false, Ordering::SeqCst);
                            tracing::info!("Waiting for lnd invoice ending");
                            return None;
                        }
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(msg)) => {
                                    // Update indices based on the message
                                    current_add_index = current_add_index.max(msg.add_index);
                                    current_settle_index = current_settle_index.max(msg.settle_index);

                                    // Store the updated indices in KV store regardless of settlement status
                                    let add_index_str = current_add_index.to_string();
                                    let settle_index_str = current_settle_index.to_string();

                                    if let Ok(mut tx) = kv_store.begin_transaction().await {
                                        let mut has_error = false;

                                        if let Err(e) = tx.kv_write(LND_KV_PRIMARY_NAMESPACE, LND_KV_SECONDARY_NAMESPACE, LAST_ADD_INDEX_KV_KEY, add_index_str.as_bytes()).await {
                                            tracing::warn!("LND: Failed to write add_index {} to KV store: {}", current_add_index, e);
                                            has_error = true;
                                        }

                                        if let Err(e) = tx.kv_write(LND_KV_PRIMARY_NAMESPACE, LND_KV_SECONDARY_NAMESPACE, LAST_SETTLE_INDEX_KV_KEY, settle_index_str.as_bytes()).await {
                                            tracing::warn!("LND: Failed to write settle_index {} to KV store: {}", current_settle_index, e);
                                            has_error = true;
                                        }

                                        if !has_error {
                                            if let Err(e) = tx.commit().await {
                                                tracing::warn!("LND: Failed to commit indices to KV store: {}", e);
                                            } else {
                                                tracing::debug!("LND: Stored updated indices - add_index: {}, settle_index: {}", current_add_index, current_settle_index);
                                            }
                                        }
                                    } else {
                                        tracing::warn!("LND: Failed to begin KV transaction for storing indices");
                                    }

                                    // Only emit event for settled invoices
                                    if msg.state() == InvoiceState::Settled {
                                        let hash_slice: Result<[u8;32], _> = msg.r_hash.try_into();

                                        if let Ok(hash_slice) = hash_slice {
                                            let hash = hex::encode(hash_slice);

                                            tracing::info!("LND: Payment for {} with amount {} msat", hash,  msg.amt_paid_msat);

                                            let wait_response = WaitPaymentResponse {
                                                payment_identifier: PaymentIdentifier::PaymentHash(hash_slice),
                                                payment_amount: Amount::from(msg.amt_paid_msat as u64),
                                                unit: CurrencyUnit::Msat,
                                                payment_id: hash,
                                            };
                                            let event = Event::PaymentReceived(wait_response);
                                            return Some((event, (stream, cancel_token, is_active, kv_store, current_add_index, current_settle_index)));
                                        } else {
                                            // Invalid hash, skip this message but continue streaming
                                            tracing::error!("LND returned invalid payment hash");
                                            // Continue the loop without yielding
                                            continue;
                                        }
                                    } else {
                                        // Not a settled invoice, continue but don't emit event
                                        tracing::debug!("LND: Received non-settled invoice, continuing to wait for settled invoices");
                                        // Continue the loop without yielding
                                        continue;
                                    }
                                }
                                Ok(None) => {
                                    is_active.store(false, Ordering::SeqCst);
                                    tracing::info!("LND invoice stream ended.");
                                    return None;
                                }
                                Err(err) => {
                                    is_active.store(false, Ordering::SeqCst);
                                    tracing::warn!("Encountered error in LND invoice stream. Stream ending");
                                    tracing::error!("{:?}", err);
                                    return None;
                                }
                            }
                        }
                    }
                }
            },
        );

        Ok(Box::pin(event_stream))
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
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    )),
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
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

                // Detect partial payments
                match bolt11_options.melt_options {
                    Some(MeltOptions::Mpp { mpp }) => {
                        let amount_msat: u64 = bolt11
                            .amount_milli_satoshis()
                            .ok_or(Error::UnknownInvoiceAmount)?;
                        {
                            let partial_amount_msat = mpp.amount;
                            let invoice = bolt11;
                            let max_fee: Option<Amount> = bolt11_options.max_fee_amount;

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
                                    payment_lookup_id: PaymentIdentifier::PaymentHash(
                                        payment_hash.to_byte_array(),
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
                    }
                    _ => {
                        let mut lnd_client = self.lnd_client.clone();

                        let max_fee: Option<Amount> = bolt11_options.max_fee_amount;

                        let amount_msat = u64::from(
                            bolt11_options
                                .melt_options
                                .map(|a| a.amount_msat())
                                .unwrap_or_default(),
                        );

                        let pay_req = lnrpc::SendRequest {
                            payment_request: bolt11.to_string(),
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

    #[instrument(skip(self, options))]
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

                let invoice_request = lnrpc::Invoice {
                    value_msat: u64::from(amount_msat) as i64,
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

                let payment_identifier =
                    PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref());

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: payment_identifier,
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
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut lnd_client = self.lnd_client.clone();

        let invoice_request = lnrpc::PaymentHash {
            r_hash: hex::decode(payment_identifier.to_string()).unwrap(),
            ..Default::default()
        };

        let invoice = lnd_client
            .lightning()
            .lookup_invoice(tonic::Request::new(invoice_request))
            .await
            .map_err(|e| payment::Error::Anyhow(anyhow!(e)))?
            .into_inner();

        if invoice.state() == InvoiceState::Settled {
            Ok(vec![WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
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
        let mut lnd_client = self.lnd_client.clone();

        let payment_hash = &payment_identifier.to_string();

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
                        PaymentStatus::InFlight | PaymentStatus::Initiated => {
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
