//! CDK lightning backend for LND

// Copyright (c) 2023 Steffen (MIT)

#![doc = include_str!("../README.md")]

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use cdk_common::amount::Amount;
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::secp256k1::PublicKey;
use cdk_common::common::FeeReserve;
use cdk_common::database::DynKVStore;
use cdk_common::lightning_invoice::Currency;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::util::{hex, unix_time};
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
    node_pubkey: PublicKey,
    invoice_currency: Currency,
    fee_reserve: FeeReserve,
    allow_self_payment: bool,
    kv_store: DynKVStore,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    settings: SettingsResponse,
    unit: CurrencyUnit,
}

impl std::fmt::Debug for Lnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lnd")
            .field("fee_reserve", &self.fee_reserve)
            .field("allow_self_payment", &self.allow_self_payment)
            .finish_non_exhaustive()
    }
}

impl Lnd {
    fn bolt11_payment_quote(
        unit: &CurrencyUnit,
        bolt11_options: payment::Bolt11OutgoingPaymentOptions,
        fee_reserve: &FeeReserve,
        invoice_currency: &Currency,
    ) -> Result<PaymentQuoteResponse, payment::Error> {
        if bolt11_options.bolt11.currency() != *invoice_currency {
            return Err(Error::InvoiceNetworkMismatch.into());
        }
        let amount_msat = match bolt11_options.melt_options {
            Some(MeltOptions::Amountless { amountless }) => {
                let amount_msat = amountless.amount_msat;

                bolt11_rpc_amount_msat(&bolt11_options.bolt11, Some(u64::from(amount_msat)))?;

                amount_msat
            }
            Some(MeltOptions::Mpp { mpp }) => {
                bolt11_mpp_amounts(&bolt11_options.bolt11, u64::from(mpp.amount))?;
                mpp.amount
            }
            None => bolt11_options
                .bolt11
                .amount_milli_satoshis()
                .ok_or(Error::UnknownInvoiceAmount)?
                .into(),
        };

        // The quote must cover the entire millisatoshi principal.
        let amount = Amount::new(amount_msat.into(), CurrencyUnit::Msat).convert_to_ceil(unit)?;

        let fee = fee_reserve.for_amount(amount.clone().into());

        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                *bolt11_options.bolt11.payment_hash().as_ref(),
            )),
            amount,
            fee: Amount::new(fee.to_u64(), unit.clone()),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: None,
            fee_options: None,
        })
    }

    /// Maximum number of attempts at a partial payment
    pub const MAX_ROUTE_RETRIES: usize = 50;

    /// Create new [`Lnd`].
    ///
    /// Fetches and caches the node identity and invoice network using `GetInfo`.
    /// LND must be available and the macaroon must permit `info:read`.
    pub async fn new(
        address: String,
        cert_file: PathBuf,
        macaroon_file: PathBuf,
        fee_reserve: FeeReserve,
        kv_store: DynKVStore,
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

        let mut lnd_client = client::connect(&address, &cert_file, &macaroon_file)
            .await
            .map_err(|err| {
                tracing::error!("Connection error: {}", err.to_string());
                Error::Connection
            })?;

        let info = lnd_client
            .lightning()
            .get_info(lnrpc::GetInfoRequest {})
            .await
            .map_err(Error::GetInfo)?
            .into_inner();
        let (node_pubkey, invoice_currency) = lnd_node_identity(&info)?;

        let unit = CurrencyUnit::Msat;
        Ok(Self {
            _address: address,
            _cert_file: cert_file,
            _macaroon_file: macaroon_file,
            lnd_client,
            node_pubkey,
            invoice_currency,
            fee_reserve,
            allow_self_payment: true,
            kv_store,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            settings: SettingsResponse {
                unit: unit.to_string(),
                bolt11: Some(payment::Bolt11Settings {
                    mpp: true,
                    amountless: true,
                    invoice_description: true,
                }),
                bolt12: None,
                onchain: None,
                custom: std::collections::HashMap::new(),
            },
            unit,
        })
    }

    /// Enable or disable circular payments back to this LND node (enabled by default).
    pub fn with_allow_self_payment(mut self, allow_self_payment: bool) -> Self {
        self.allow_self_payment = allow_self_payment;
        self
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

/// Validate the immutable node identity and invoice network returned by LND.
fn lnd_node_identity(info: &lnrpc::GetInfoResponse) -> Result<(PublicKey, Currency), Error> {
    let node_pubkey = PublicKey::from_str(&info.identity_pubkey).map_err(|_| {
        Error::InvalidConfig("LND GetInfo returned an invalid identity pubkey".to_owned())
    })?;
    let chain = match info.chains.as_slice() {
        [chain] => chain,
        _ => {
            return Err(Error::InvalidConfig(
                "LND GetInfo must report exactly one network".to_owned(),
            ))
        }
    };
    let invoice_currency = match chain.network.as_str() {
        "mainnet" => Currency::Bitcoin,
        "testnet" | "testnet4" => Currency::BitcoinTestnet,
        "regtest" => Currency::Regtest,
        "simnet" => Currency::Simnet,
        "signet" => Currency::Signet,
        _ => {
            return Err(Error::InvalidConfig(format!(
                "LND GetInfo returned an unsupported network: {}",
                chain.network
            )))
        }
    };
    Ok((node_pubkey, invoice_currency))
}

/// Validate a full payment's amount and select the amount sent to LND.
/// LND requires an explicit positive amount only when the invoice omits it.
fn bolt11_rpc_amount_msat(
    bolt11: &Bolt11Invoice,
    requested_amount_msat: Option<u64>,
) -> Result<u64, payment::Error> {
    match bolt11.amount_milli_satoshis() {
        Some(invoice_amount) => {
            if requested_amount_msat.is_some_and(|amount| amount != invoice_amount) {
                return Err(payment::Error::AmountMismatch);
            }
            Ok(0)
        }
        None => requested_amount_msat
            .filter(|amount| *amount > 0)
            .ok_or_else(|| Error::UnknownInvoiceAmount.into()),
    }
}

/// Validate the shard and total amounts before querying or sending an MPP route.
fn bolt11_mpp_amounts(
    bolt11: &Bolt11Invoice,
    shard_msat: u64,
) -> Result<(i64, i64), payment::Error> {
    let total_msat = bolt11
        .amount_milli_satoshis()
        .ok_or(Error::UnknownInvoiceAmount)?;
    if shard_msat == 0 || shard_msat > total_msat {
        return Err(payment::Error::AmountMismatch);
    }
    let shard_msat = i64::try_from(shard_msat).map_err(|_| Error::AmountOverflow)?;
    let total_msat = i64::try_from(total_msat).map_err(|_| Error::AmountOverflow)?;
    Ok((shard_msat, total_msat))
}

fn mpp_fee_limit(max_fee: Option<&Amount<CurrencyUnit>>) -> Result<Option<FeeLimit>, Error> {
    max_fee
        .map(|fee| {
            let fee_msat = i64::try_from(fee.to_msat()?).map_err(|_| Error::AmountOverflow)?;
            Ok(FeeLimit {
                limit: Some(Limit::FixedMsat(fee_msat)),
            })
        })
        .transpose()
}

/// Only the first route query is guaranteed to precede any send attempt.
fn mpp_route_query_failure(
    err: tonic::Status,
    attempt: usize,
    unit: &CurrencyUnit,
    payment_lookup_id: PaymentIdentifier,
) -> Result<MakePaymentResponse, payment::Error> {
    match attempt {
        0 => Ok(outgoing_payment_failure_response(unit, payment_lookup_id)),
        _ => Err(Error::LndError(err).into()),
    }
}

fn lnrpc_payment_total_spent(payment: &lnrpc::Payment) -> Result<Amount<CurrencyUnit>, Error> {
    let total_msat = payment
        .value_msat
        .checked_add(payment.fee_msat)
        .ok_or(Error::AmountOverflow)?;
    let total_msat = u64::try_from(total_msat).map_err(|_| Error::AmountOverflow)?;

    Ok(Amount::new(total_msat, CurrencyUnit::Msat))
}

/// Build an authoritative terminal-failure response for a payment that was
/// rejected before dispatch.
///
/// The mint treats an `Ok` response with `MeltQuoteState::Failed` as
/// authoritative (it may compensate the melt), unlike an `Err`, whose dispatch
/// phase is unknown and which is therefore kept indeterminate. Pre-dispatch
/// rejections must be returned as this response so the melt can be rolled back
/// instead of parked pending.
///
/// Conversely, errors that straddle the dispatch boundary — a gRPC `Status`
/// error from `send_*` (`Error::LndError`), or a stream that drops after
/// dispatch began (`Error::AmbiguousDispatch`) — must stay `Err` so the melt
/// stays indeterminate. Do not convert those to this response.
fn outgoing_payment_failure_response(
    unit: &CurrencyUnit,
    payment_lookup_id: PaymentIdentifier,
) -> MakePaymentResponse {
    MakePaymentResponse {
        payment_lookup_id,
        payment_proof: None,
        status: MeltQuoteState::Failed,
        total_spent: Amount::new(0, unit.clone()),
    }
}

/// Preserve an existing payment before rejecting expired, wrong-network, or
/// disallowed self-payment invoices locally.
fn bolt11_pre_dispatch_response(
    unit: &CurrencyUnit,
    bolt11: &Bolt11Invoice,
    pay_state: MakePaymentResponse,
    invoice_currency: &Currency,
    disallowed_payee: Option<&PublicKey>,
) -> Result<Option<MakePaymentResponse>, payment::Error> {
    let payment_lookup_id = PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref());
    Ok(match pay_state.status {
        MeltQuoteState::Paid | MeltQuoteState::Pending => Some(MakePaymentResponse {
            payment_lookup_id,
            total_spent: pay_state.total_spent.convert_to_ceil(unit)?,
            ..pay_state
        }),
        MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => {
            // Validate locally while no dispatch has been attempted. LND does
            // not record these validation failures for later status lookups.
            let rejected = bolt11.is_expired()
                || bolt11.currency() != *invoice_currency
                || disallowed_payee.is_some_and(|pubkey| bolt11.get_payee_pub_key() == *pubkey);
            rejected.then(|| outgoing_payment_failure_response(unit, payment_lookup_id))
        }
    })
}

#[async_trait]
impl MintPayment for Lnd {
    type Err = payment::Error;

    #[instrument(skip_all)]
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(self.settings.clone())
    }

    #[instrument(skip_all)]
    fn is_payment_event_stream_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    #[instrument(skip_all)]
    fn cancel_payment_event_stream(&self) {
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
                                                payment_amount: Amount::new(msg.amt_paid_msat as u64, CurrencyUnit::Msat),
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
            OutgoingPaymentOptions::Bolt11(bolt11_options) => Self::bolt11_payment_quote(
                unit,
                *bolt11_options,
                &self.fee_reserve,
                &self.invoice_currency,
            ),
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
            OutgoingPaymentOptions::Custom(_) | OutgoingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
            }
        }
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;
                let payment_lookup_id =
                    PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref());

                // A prior lookup is authoritative evidence, not an error:
                // report the already-recorded outcome so the mint reconciles
                // against durable state instead of treating the duplicate melt
                // as an ambiguous dispatch failure.
                let pay_state = self.check_outgoing_payment(&payment_lookup_id).await?;

                if let Some(response) = bolt11_pre_dispatch_response(
                    unit,
                    &bolt11,
                    pay_state,
                    &self.invoice_currency,
                    (!self.allow_self_payment).then_some(&self.node_pubkey),
                )? {
                    return Ok(response);
                }

                // Detect partial payments
                match bolt11_options.melt_options {
                    Some(MeltOptions::Mpp { mpp }) => {
                        let (partial_amount_msat, amount_msat) =
                            match bolt11_mpp_amounts(&bolt11, u64::from(mpp.amount)) {
                                Ok(amounts) => amounts,
                                Err(err) => {
                                    tracing::warn!(
                                        payment_lookup_id = %payment_lookup_id,
                                        error = %err,
                                        "LND MPP amount rejected before dispatch",
                                    );
                                    return Ok(outgoing_payment_failure_response(
                                        unit,
                                        payment_lookup_id,
                                    ));
                                }
                            };
                        {
                            let invoice = bolt11;
                            let fee_limit =
                                match mpp_fee_limit(bolt11_options.max_fee_amount.as_ref()) {
                                    Ok(fee_limit) => fee_limit,
                                    Err(err) => {
                                        tracing::warn!(
                                            payment_lookup_id = %payment_lookup_id,
                                            error = %err,
                                            "LND MPP fee limit rejected before dispatch",
                                        );
                                        return Ok(outgoing_payment_failure_response(
                                            unit,
                                            payment_lookup_id,
                                        ));
                                    }
                                };

                            // Extract information from invoice
                            let pub_key = invoice.get_payee_pub_key();
                            let payer_addr = invoice.payment_secret().0.to_vec();
                            let payment_hash = invoice.payment_hash();

                            let mut lnd_client = self.lnd_client.clone();

                            for attempt in 0..Self::MAX_ROUTE_RETRIES {
                                // Create a request for the routes
                                let route_req = lnrpc::QueryRoutesRequest {
                                    pub_key: hex::encode(pub_key.serialize()),
                                    amt_msat: partial_amount_msat,
                                    fee_limit,
                                    use_mission_control: true,
                                    ..Default::default()
                                };

                                // Query the routes
                                let mut routes_response =
                                    match lnd_client.lightning().query_routes(route_req).await {
                                        Ok(response) => response.into_inner(),
                                        Err(err) => {
                                            tracing::warn!(
                                                payment_lookup_id = %payment_lookup_id,
                                                attempt = attempt + 1,
                                                rpc_code = %err.code(),
                                                error = %err.message(),
                                                "LND MPP route query failed",
                                            );
                                            return mpp_route_query_failure(
                                                err,
                                                attempt,
                                                unit,
                                                payment_lookup_id,
                                            );
                                        }
                                    };

                                // Get first route and update its MPP record. An
                                // empty route set means LND found no path; the
                                // payment was never dispatched.
                                let route = match routes_response.routes.first_mut() {
                                    Some(route) => route,
                                    None => {
                                        tracing::warn!(
                                            payment_lookup_id = %payment_lookup_id,
                                            attempt = attempt + 1,
                                            "LND MPP route query returned no routes",
                                        );
                                        return Ok(outgoing_payment_failure_response(
                                            unit,
                                            payment_lookup_id,
                                        ));
                                    }
                                };

                                // attempt it and check the result
                                let last_hop: &mut Hop = match route.hops.last_mut() {
                                    Some(last_hop) => last_hop,
                                    None => {
                                        tracing::warn!(
                                            payment_lookup_id = %payment_lookup_id,
                                            attempt = attempt + 1,
                                            "LND MPP route has no hops",
                                        );
                                        return Ok(outgoing_payment_failure_response(
                                            unit,
                                            payment_lookup_id,
                                        ));
                                    }
                                };
                                let mpp_record = MppRecord {
                                    payment_addr: payer_addr.clone(),
                                    total_amt_msat: amount_msat,
                                };
                                last_hop.mpp_record = Some(mpp_record);

                                let payment_response = lnd_client
                                    .router()
                                    .send_to_route_v2(routerrpc::SendToRouteRequest {
                                        payment_hash: payment_hash.to_byte_array().to_vec(),
                                        route: Some(route.clone()),
                                        ..Default::default()
                                    })
                                    .await
                                    .inspect_err(|err| {
                                        tracing::warn!(
                                            payment_lookup_id = %payment_lookup_id,
                                            attempt = attempt + 1,
                                            rpc_code = %err.code(),
                                            error = %err.message(),
                                            "LND MPP dispatch RPC failed; payment outcome requires verification",
                                        );
                                    })
                                    .map_err(Error::LndError)?
                                    .into_inner();

                                if let Some(failure) = payment_response.failure {
                                    if failure.code == 15 {
                                        tracing::debug!(
                                            payment_lookup_id = %payment_lookup_id,
                                            attempt = attempt + 1,
                                            failure_code = failure.code,
                                            failure_reason = failure.code().as_str_name(),
                                            failure_source_index = failure.failure_source_index,
                                            "LND MPP route failed; querying another route",
                                        );
                                        continue;
                                    }
                                    tracing::warn!(
                                        payment_lookup_id = %payment_lookup_id,
                                        attempt = attempt + 1,
                                        failure_code = failure.code,
                                        failure_reason = failure.code().as_str_name(),
                                        failure_source_index = failure.failure_source_index,
                                        "LND MPP attempt returned a failure",
                                    );
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

                                // Get the actual amount paid in msats
                                let total_amt_msat: u64 = match payment_response.route {
                                    Some(route) => u64::try_from(route.total_amt_msat)
                                        .map_err(|_| Error::AmountOverflow)?,
                                    None => 0,
                                };

                                return Ok(MakePaymentResponse {
                                    payment_lookup_id: PaymentIdentifier::PaymentHash(
                                        payment_hash.to_byte_array(),
                                    ),
                                    payment_proof: payment_preimage,
                                    status,
                                    total_spent: Amount::new(total_amt_msat, CurrencyUnit::Msat)
                                        .convert_to_ceil(unit)?,
                                });
                            }

                            // "We have exhausted all tactical options" -- STEM, Upgrade (2018)
                            // All route attempts returned retryable failures.
                            tracing::warn!(
                                payment_lookup_id = %payment_lookup_id,
                                attempts = Self::MAX_ROUTE_RETRIES,
                                "LND MPP payment exhausted route retries",
                            );
                            Ok(outgoing_payment_failure_response(unit, payment_lookup_id))
                        }
                    }
                    _ => {
                        let mut lnd_client = self.lnd_client.clone();

                        let max_fee: Option<Amount<CurrencyUnit>> = bolt11_options.max_fee_amount;

                        let requested_amount_msat = match bolt11_options.melt_options {
                            Some(MeltOptions::Amountless { amountless }) => {
                                Some(u64::from(amountless.amount_msat))
                            }
                            _ => None,
                        };
                        let amount_msat =
                            match bolt11_rpc_amount_msat(&bolt11, requested_amount_msat) {
                                Ok(amount_msat) => amount_msat,
                                Err(err) => {
                                    tracing::warn!(
                                        payment_lookup_id = %payment_lookup_id,
                                        error = %err,
                                        "LND payment amount rejected before dispatch",
                                    );
                                    return Ok(outgoing_payment_failure_response(
                                        unit,
                                        payment_lookup_id,
                                    ));
                                }
                            };

                        let fee_limit_msat = match max_fee {
                            Some(fee) => fee.convert_to(&CurrencyUnit::Msat)?.value() as i64,
                            None => 0,
                        };

                        let pay_req = routerrpc::SendPaymentRequest {
                            payment_request: bolt11.to_string(),
                            fee_limit_msat,
                            amt_msat: amount_msat as i64,
                            allow_self_payment: self.allow_self_payment,
                            ..Default::default()
                        };

                        let mut payment_stream = lnd_client
                            .router()
                            .send_payment_v2(pay_req)
                            .await
                            .map_err(|err| {
                                tracing::warn!(
                                    payment_lookup_id = %payment_lookup_id,
                                    rpc_code = %err.code(),
                                    error = %err.message(),
                                    "LND payment dispatch RPC failed; payment outcome requires verification",
                                );
                                // A gRPC error here may arrive after LND accepted
                                // the payment; the dispatch outcome is unknown.
                                Error::AmbiguousDispatch
                            })?
                            .into_inner();

                        while let Some(update) = payment_stream.message().await.map_err(|err| {
                            tracing::warn!(
                                payment_lookup_id = %payment_lookup_id,
                                rpc_code = %err.code(),
                                error = %err.message(),
                                "LND payment stream failed after dispatch; payment may still settle",
                            );
                            // The stream dropped after dispatch began; the payment
                            // may still settle.
                            Error::AmbiguousDispatch
                        })? {
                            let status = update.status();

                            let response_status = match status {
                                PaymentStatus::InFlight | PaymentStatus::Initiated => {
                                    continue;
                                }
                                PaymentStatus::Succeeded => MeltQuoteState::Paid,
                                PaymentStatus::Failed => {
                                    tracing::warn!(
                                        payment_lookup_id = %payment_lookup_id,
                                        failure_code = update.failure_reason,
                                        failure_reason = update.failure_reason().as_str_name(),
                                        "LND outgoing payment failed",
                                    );
                                    MeltQuoteState::Failed
                                }
                                #[allow(deprecated)]
                                PaymentStatus::Unknown => MeltQuoteState::Unknown,
                            };

                            let total_msat = update
                                .value_msat
                                .checked_add(update.fee_msat)
                                .and_then(|total| u64::try_from(total).ok())
                                .ok_or(Error::AmountOverflow)?;

                            let payment_preimage = if update.payment_preimage.is_empty() {
                                None
                            } else {
                                Some(update.payment_preimage)
                            };

                            let payment_identifier =
                                PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref());

                            return Ok(MakePaymentResponse {
                                payment_lookup_id: payment_identifier,
                                payment_proof: payment_preimage,
                                status: response_status,
                                total_spent: Amount::new(total_msat, CurrencyUnit::Msat)
                                    .convert_to_ceil(unit)?,
                            });
                        }

                        tracing::warn!(
                            payment_lookup_id = %payment_lookup_id,
                            "LND payment stream ended without a terminal result; payment outcome remains unknown",
                        );
                        Err(Error::UnknownPaymentStatus.into())
                    }
                }
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
            OutgoingPaymentOptions::Custom(_) | OutgoingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
            }
        }
    }

    #[instrument(skip(self, options))]
    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let description = bolt11_options.description.unwrap_or_default();
                let amount = bolt11_options.amount;
                let unix_expiry = bolt11_options.unix_expiry;

                let amount_msat: Amount = amount.convert_to(&CurrencyUnit::Msat)?.into();

                let invoice_request = lnrpc::Invoice {
                    value_msat: u64::from(amount_msat) as i64,
                    memo: description,
                    expiry: unix_expiry
                        .map(|t| {
                            t.checked_sub(unix_time())
                                .ok_or(payment::Error::InvalidExpiry)
                        })
                        .transpose()?
                        .unwrap_or_default() as i64,
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

                let expiry = bolt11.expires_at().map(|t| t.as_secs());

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: payment_identifier,
                    request: bolt11.to_string(),
                    expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by LND")))
            }
            IncomingPaymentOptions::Custom(_) | IncomingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
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
            r_hash: hex::decode(payment_identifier.to_string())?,
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
                payment_amount: Amount::new(invoice.amt_paid_msat as u64, CurrencyUnit::Msat),
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
                    tracing::debug!(
                        payment_lookup_id = %payment_identifier,
                        "LND does not know this outgoing payment; reporting Unknown because absence is not authoritative proof of permanent failure",
                    );
                    return Ok(MakePaymentResponse {
                        payment_lookup_id: payment_identifier.clone(),
                        payment_proof: None,
                        status: MeltQuoteState::Unknown,
                        total_spent: Amount::new(0, self.unit.clone()),
                    });
                } else {
                    tracing::warn!(
                        payment_lookup_id = %payment_identifier,
                        rpc_code = %err_code,
                        error = %err.message(),
                        "LND outgoing payment status RPC failed; payment outcome remains unknown",
                    );
                    return Err(payment::Error::UnknownPaymentState);
                }
            }
        };

        while let Some(update_result) = payment_stream.next().await {
            match update_result {
                Ok(update) => {
                    let status = update.status();

                    let response = match status {
                        #[allow(deprecated)]
                        PaymentStatus::Unknown => MakePaymentResponse {
                            payment_lookup_id: payment_identifier.clone(),
                            payment_proof: Some(update.payment_preimage),
                            status: MeltQuoteState::Unknown,
                            total_spent: Amount::new(0, self.unit.clone()),
                        },
                        PaymentStatus::InFlight | PaymentStatus::Initiated => {
                            // Continue waiting for the next update
                            continue;
                        }
                        PaymentStatus::Succeeded => {
                            let total_spent = lnrpc_payment_total_spent(&update)?;

                            MakePaymentResponse {
                                payment_lookup_id: payment_identifier.clone(),
                                payment_proof: Some(update.payment_preimage),
                                status: MeltQuoteState::Paid,
                                total_spent,
                            }
                        }
                        PaymentStatus::Failed => {
                            // Status checks also run before dispatch and may
                            // repeatedly observe the same recorded failure.
                            tracing::debug!(
                                payment_lookup_id = %payment_identifier,
                                failure_code = update.failure_reason,
                                failure_reason = update.failure_reason().as_str_name(),
                                "LND outgoing payment status is failed",
                            );
                            MakePaymentResponse {
                                payment_lookup_id: payment_identifier.clone(),
                                payment_proof: Some(update.payment_preimage),
                                status: MeltQuoteState::Failed,
                                total_spent: Amount::new(0, self.unit.clone()),
                            }
                        }
                    };

                    return Ok(response);
                }
                Err(err) => {
                    // Handle the case where the update itself is an error (e.g., stream failure)
                    tracing::warn!(
                        payment_lookup_id = %payment_identifier,
                        rpc_code = %err.code(),
                        error = %err.message(),
                        "LND outgoing payment status stream failed; payment outcome remains unknown",
                    );
                    return Err(Error::UnknownPaymentStatus.into());
                }
            }
        }

        // If the stream is exhausted without a final status
        tracing::warn!(
            payment_lookup_id = %payment_identifier,
            "LND outgoing payment status stream ended without a terminal result; payment outcome remains unknown",
        );
        Err(Error::UnknownPaymentStatus.into())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk_common::bitcoin::hashes::sha256;
    use cdk_common::bitcoin::secp256k1::{Secp256k1, SecretKey};
    use cdk_common::lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

    use super::*;

    fn invoice_with_timestamp(timestamp: Duration) -> Bolt11Invoice {
        invoice_with_amount(timestamp, None)
    }

    fn invoice_with_amount(timestamp: Duration, amount_msat: Option<u64>) -> Bolt11Invoice {
        let key = SecretKey::from_slice(&[1; 32]).unwrap();
        let builder = InvoiceBuilder::new(Currency::Regtest)
            .description("expiry test".to_owned())
            .payment_hash(sha256::Hash::from_byte_array([42; 32]))
            .payment_secret(PaymentSecret([43; 32]))
            .duration_since_epoch(timestamp)
            .expiry_time(Duration::from_secs(3600))
            .min_final_cltv_expiry_delta(144);
        let builder = match amount_msat {
            Some(amount) => builder.amount_milli_satoshis(amount),
            None => builder,
        };
        builder
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &key))
            .unwrap()
    }

    #[test]
    fn payment_quotes_round_up_sat_principals() {
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        };
        for (msat, sat) in [(1, 1), (999, 1), (1_000, 1), (1_999, 2), (2_000, 2)] {
            for (unit, expected) in [(CurrencyUnit::Sat, sat), (CurrencyUnit::Msat, msat)] {
                for melt_options in [
                    None,
                    Some(MeltOptions::new_mpp(msat)),
                    Some(MeltOptions::new_amountless(msat)),
                ] {
                    let invoice_amount = match melt_options {
                        Some(MeltOptions::Amountless { .. }) => None,
                        _ => Some(msat),
                    };
                    let quote = Lnd::bolt11_payment_quote(
                        &unit,
                        payment::Bolt11OutgoingPaymentOptions {
                            bolt11: invoice_with_amount(
                                Duration::from_secs(unix_time()),
                                invoice_amount,
                            ),
                            max_fee_amount: None,
                            timeout_secs: None,
                            melt_options,
                            quote_id: cdk_common::QuoteId::new(),
                        },
                        &fee_reserve,
                        &Currency::Regtest,
                    )
                    .unwrap();
                    assert_eq!(quote.amount, Amount::new(expected, unit.clone()));
                }
            }
        }
    }

    #[test]
    fn amountless_invoice_requires_a_positive_rpc_amount() {
        let invoice = invoice_with_amount(Duration::from_secs(unix_time()), None);
        for amount in [None, Some(0)] {
            assert!(bolt11_rpc_amount_msat(&invoice, amount).is_err());
        }
        assert_eq!(bolt11_rpc_amount_msat(&invoice, Some(1)).unwrap(), 1);
        assert_eq!(
            bolt11_rpc_amount_msat(&invoice, Some(10_000)).unwrap(),
            10_000
        );
    }

    #[test]
    fn invoice_amount_is_not_repeated_in_rpc_request() {
        let invoice = invoice_with_amount(Duration::from_secs(unix_time()), Some(10_000));
        for amount in [None, Some(10_000)] {
            assert_eq!(bolt11_rpc_amount_msat(&invoice, amount).unwrap(), 0);
        }
        for amount in [0, 9_999, 10_001] {
            assert!(matches!(
                bolt11_rpc_amount_msat(&invoice, Some(amount)),
                Err(payment::Error::AmountMismatch),
            ));
        }
    }

    #[test]
    fn amountless_quotes_reject_zero_and_mismatched_amounts() {
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        };
        for (invoice_amount, requested_amount) in
            [(None, 0), (Some(10_000), 0), (Some(10_000), 9_999)]
        {
            let result = Lnd::bolt11_payment_quote(
                &CurrencyUnit::Sat,
                payment::Bolt11OutgoingPaymentOptions {
                    bolt11: invoice_with_amount(Duration::from_secs(unix_time()), invoice_amount),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options: Some(MeltOptions::new_amountless(requested_amount)),
                    quote_id: cdk_common::QuoteId::new(),
                },
                &fee_reserve,
                &Currency::Regtest,
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn matching_amountless_option_preserves_quote_principal() {
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        };
        for invoice_amount in [None, Some(10_000)] {
            let quote = Lnd::bolt11_payment_quote(
                &CurrencyUnit::Sat,
                payment::Bolt11OutgoingPaymentOptions {
                    bolt11: invoice_with_amount(Duration::from_secs(unix_time()), invoice_amount),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options: Some(MeltOptions::new_amountless(10_000)),
                    quote_id: cdk_common::QuoteId::new(),
                },
                &fee_reserve,
                &Currency::Regtest,
            )
            .unwrap();
            assert_eq!(quote.amount, Amount::new(10, CurrencyUnit::Sat));
        }
    }

    #[test]
    fn mpp_quotes_require_an_invoice_total_and_valid_shard() {
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        };
        for (invoice_amount, shard, valid) in [
            (None, 1, false),
            (Some(10_000), 0, false),
            (Some(10_000), 10_001, false),
            (Some(10_000), u64::MAX, false),
            (Some(10_000), 1, true),
            (Some(10_000), 5_000, true),
            (Some(10_000), 10_000, true),
        ] {
            let invoice = invoice_with_amount(Duration::from_secs(unix_time()), invoice_amount);
            let result = Lnd::bolt11_payment_quote(
                &CurrencyUnit::Msat,
                payment::Bolt11OutgoingPaymentOptions {
                    bolt11: invoice.clone(),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options: Some(MeltOptions::new_mpp(shard)),
                    quote_id: cdk_common::QuoteId::new(),
                },
                &fee_reserve,
                &Currency::Regtest,
            );
            match valid {
                true => {
                    assert_eq!(
                        result.unwrap().amount,
                        Amount::new(shard, CurrencyUnit::Msat)
                    );
                    assert_eq!(
                        bolt11_mpp_amounts(&invoice, shard).unwrap(),
                        (shard as i64, 10_000)
                    );
                }
                false => {
                    assert!(result.is_err());
                    assert!(bolt11_mpp_amounts(&invoice, shard).is_err());
                }
            }
        }
    }

    #[test]
    fn mpp_fee_limits_use_checked_rpc_amounts() {
        assert!(mpp_fee_limit(None).unwrap().is_none());
        for fee_msat in [0, 1, i64::MAX as u64] {
            let fee = Amount::new(fee_msat, CurrencyUnit::Msat);
            assert_eq!(
                mpp_fee_limit(Some(&fee)).unwrap().unwrap().limit,
                Some(Limit::FixedMsat(fee_msat as i64)),
            );
        }
        for fee in [
            Amount::new(i64::MAX as u64 + 1, CurrencyUnit::Msat),
            Amount::new(i64::MAX as u64 / 1_000 + 1, CurrencyUnit::Sat),
        ] {
            assert!(matches!(
                mpp_fee_limit(Some(&fee)),
                Err(Error::AmountOverflow)
            ));
        }
    }

    #[test]
    fn first_mpp_route_query_failure_is_authoritative() {
        let lookup_id = PaymentIdentifier::PaymentHash([42; 32]);
        for code in [
            tonic::Code::Unknown,
            tonic::Code::Unavailable,
            tonic::Code::InvalidArgument,
        ] {
            let response = mpp_route_query_failure(
                tonic::Status::new(code, "route query failed"),
                0,
                &CurrencyUnit::Sat,
                lookup_id.clone(),
            )
            .unwrap();
            assert_eq!(response.status, MeltQuoteState::Failed);
            assert_eq!(response.payment_lookup_id, lookup_id);
            assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
            assert!(response.payment_proof.is_none());
        }
    }

    #[test]
    fn mpp_route_query_failure_after_a_send_remains_ambiguous() {
        for attempt in [1, Lnd::MAX_ROUTE_RETRIES - 1] {
            let response = mpp_route_query_failure(
                tonic::Status::unavailable("route query failed"),
                attempt,
                &CurrencyUnit::Sat,
                PaymentIdentifier::PaymentHash([42; 32]),
            );
            assert!(matches!(response, Err(payment::Error::Backend(_))));
        }
    }

    #[test]
    fn node_info_maps_supported_invoice_networks() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let pubkey = invoice.get_payee_pub_key();
        for (network, currency) in [
            ("mainnet", Currency::Bitcoin),
            ("testnet", Currency::BitcoinTestnet),
            ("testnet4", Currency::BitcoinTestnet),
            ("regtest", Currency::Regtest),
            ("simnet", Currency::Simnet),
            ("signet", Currency::Signet),
        ] {
            let info = lnrpc::GetInfoResponse {
                identity_pubkey: pubkey.to_string(),
                chains: vec![lnrpc::Chain {
                    network: network.to_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            assert_eq!(lnd_node_identity(&info).unwrap(), (pubkey, currency));
        }
    }

    #[test]
    fn node_info_rejects_invalid_identity_and_networks() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let chain = lnrpc::Chain {
            network: "regtest".to_owned(),
            ..Default::default()
        };
        let info = lnrpc::GetInfoResponse {
            identity_pubkey: invoice.get_payee_pub_key().to_string(),
            chains: vec![chain.clone()],
            ..Default::default()
        };
        for invalid in [
            lnrpc::GetInfoResponse {
                identity_pubkey: String::new(),
                ..info.clone()
            },
            lnrpc::GetInfoResponse {
                identity_pubkey: "invalid".to_owned(),
                ..info.clone()
            },
            lnrpc::GetInfoResponse {
                chains: vec![],
                ..info.clone()
            },
            lnrpc::GetInfoResponse {
                chains: vec![chain.clone(), chain],
                ..info.clone()
            },
            lnrpc::GetInfoResponse {
                chains: vec![lnrpc::Chain {
                    network: "unknown".to_owned(),
                    ..Default::default()
                }],
                ..info
            },
        ] {
            assert!(matches!(
                lnd_node_identity(&invalid),
                Err(Error::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn wrong_network_quotes_are_rejected_for_all_bolt11_options() {
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::ZERO,
            percent_fee_reserve: 0.0,
        };
        for melt_options in [
            None,
            Some(MeltOptions::new_amountless(10_000)),
            Some(MeltOptions::new_mpp(1_000)),
        ] {
            let result = Lnd::bolt11_payment_quote(
                &CurrencyUnit::Sat,
                payment::Bolt11OutgoingPaymentOptions {
                    bolt11: invoice_with_amount(Duration::from_secs(unix_time()), Some(10_000)),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options,
                    quote_id: cdk_common::QuoteId::new(),
                },
                &fee_reserve,
                &Currency::Bitcoin,
            );
            assert!(matches!(result, Err(payment::Error::Backend(_))));
        }
    }

    #[test]
    fn local_invoice_rejections_return_authoritative_failure() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let pubkey = invoice.get_payee_pub_key();
        let lookup_id = PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref());
        for (network, disallowed_payee) in [
            (Currency::Bitcoin, None),
            (Currency::Regtest, Some(&pubkey)),
        ] {
            for status in [
                MeltQuoteState::Unknown,
                MeltQuoteState::Unpaid,
                MeltQuoteState::Failed,
            ] {
                let pay_state = MakePaymentResponse {
                    status,
                    ..outgoing_payment_failure_response(&CurrencyUnit::Msat, lookup_id.clone())
                };
                let response = bolt11_pre_dispatch_response(
                    &CurrencyUnit::Sat,
                    &invoice,
                    pay_state,
                    &network,
                    disallowed_payee,
                )
                .unwrap()
                .unwrap();
                assert_eq!(response.status, MeltQuoteState::Failed);
                assert_eq!(response.payment_lookup_id, lookup_id);
                assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
                assert!(response.payment_proof.is_none());
            }
        }
    }

    #[test]
    fn existing_payments_take_precedence_over_local_invoice_rejections() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let pubkey = invoice.get_payee_pub_key();
        for status in [MeltQuoteState::Paid, MeltQuoteState::Pending] {
            let pay_state = MakePaymentResponse {
                payment_lookup_id: PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref()),
                payment_proof: Some("existing preimage".to_owned()),
                status,
                total_spent: Amount::new(1_234, CurrencyUnit::Msat),
            };
            let response = bolt11_pre_dispatch_response(
                &CurrencyUnit::Sat,
                &invoice,
                pay_state,
                &Currency::Bitcoin,
                Some(&pubkey),
            )
            .unwrap()
            .unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.total_spent, Amount::new(2, CurrencyUnit::Sat));
            assert_eq!(response.payment_proof.as_deref(), Some("existing preimage"));
        }
    }

    #[test]
    fn self_payment_check_allows_another_payee() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let key = SecretKey::from_slice(&[2; 32]).unwrap();
        let other_pubkey = PublicKey::from_secret_key(&Secp256k1::new(), &key);
        let pay_state = MakePaymentResponse {
            status: MeltQuoteState::Unknown,
            ..outgoing_payment_failure_response(
                &CurrencyUnit::Msat,
                PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref()),
            )
        };
        assert!(bolt11_pre_dispatch_response(
            &CurrencyUnit::Sat,
            &invoice,
            pay_state,
            &Currency::Regtest,
            Some(&other_pubkey),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn expired_invoice_without_active_payment_fails_before_dispatch() {
        let invoice = invoice_with_timestamp(Duration::from_secs(1));
        let payment_lookup_id = PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref());

        for status in [
            MeltQuoteState::Unknown,
            MeltQuoteState::Unpaid,
            MeltQuoteState::Failed,
        ] {
            let pay_state = MakePaymentResponse {
                status,
                ..outgoing_payment_failure_response(&CurrencyUnit::Msat, payment_lookup_id.clone())
            };
            let response = bolt11_pre_dispatch_response(
                &CurrencyUnit::Sat,
                &invoice,
                pay_state,
                &Currency::Regtest,
                None,
            )
            .unwrap()
            .unwrap();

            assert_eq!(response.status, MeltQuoteState::Failed);
            assert_eq!(response.payment_lookup_id, payment_lookup_id);
            assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
            assert!(response.payment_proof.is_none());
        }
    }

    #[test]
    fn existing_payment_response_uses_requested_unit() {
        for timestamp in [Duration::from_secs(1), Duration::from_secs(unix_time())] {
            let invoice = invoice_with_timestamp(timestamp);
            let payment_lookup_id =
                PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref());

            for status in [MeltQuoteState::Paid, MeltQuoteState::Pending] {
                for (total_msat, total_sat) in [(0, 0), (1_234, 2), (2_000, 2)] {
                    for (unit, expected) in [
                        (CurrencyUnit::Sat, total_sat),
                        (CurrencyUnit::Msat, total_msat),
                    ] {
                        let pay_state = MakePaymentResponse {
                            payment_lookup_id: payment_lookup_id.clone(),
                            payment_proof: Some("existing preimage".to_owned()),
                            status,
                            total_spent: Amount::new(total_msat, CurrencyUnit::Msat),
                        };
                        let response = bolt11_pre_dispatch_response(
                            &unit,
                            &invoice,
                            pay_state,
                            &Currency::Regtest,
                            None,
                        )
                        .unwrap()
                        .unwrap();

                        assert_eq!(response.status, status);
                        assert_eq!(response.payment_lookup_id, payment_lookup_id);
                        assert_eq!(response.total_spent, Amount::new(expected, unit));
                        assert_eq!(response.payment_proof.as_deref(), Some("existing preimage"));
                    }
                }
            }
        }
    }

    #[test]
    fn unexpired_invoice_without_active_payment_can_dispatch() {
        let invoice = invoice_with_timestamp(Duration::from_secs(unix_time()));
        let payment_lookup_id = PaymentIdentifier::PaymentHash(*invoice.payment_hash().as_ref());

        for status in [
            MeltQuoteState::Unknown,
            MeltQuoteState::Unpaid,
            MeltQuoteState::Failed,
        ] {
            let pay_state = MakePaymentResponse {
                status,
                ..outgoing_payment_failure_response(&CurrencyUnit::Msat, payment_lookup_id.clone())
            };
            assert!(bolt11_pre_dispatch_response(
                &CurrencyUnit::Sat,
                &invoice,
                pay_state,
                &Currency::Regtest,
                None
            )
            .unwrap()
            .is_none());
        }
    }

    #[test]
    fn lnrpc_payment_total_spent_uses_msat_fields() {
        let payment = lnrpc::Payment {
            value_msat: 1500,
            fee_msat: 500,
            value_sat: 1,
            fee_sat: 0,
            ..Default::default()
        };

        let total_spent = lnrpc_payment_total_spent(&payment)
            .expect("sub-sat payment total should be calculated");

        assert_eq!(
            total_spent
                .convert_to(&CurrencyUnit::Msat)
                .expect("msat amount should convert to msat")
                .value(),
            2000
        );
    }

    #[test]
    fn lnrpc_payment_total_spent_rejects_overflow() {
        let payment = lnrpc::Payment {
            value_msat: i64::MAX,
            fee_msat: 1,
            ..Default::default()
        };

        let err = lnrpc_payment_total_spent(&payment)
            .expect_err("overflowing payment total should be rejected");

        assert!(matches!(err, Error::AmountOverflow));
    }

    #[test]
    fn authoritative_outgoing_failure_response_is_terminal_and_spends_nothing() {
        let payment_lookup_id = PaymentIdentifier::PaymentHash([42; 32]);
        let response =
            outgoing_payment_failure_response(&CurrencyUnit::Sat, payment_lookup_id.clone());

        assert_eq!(response.payment_lookup_id, payment_lookup_id);
        assert_eq!(response.status, MeltQuoteState::Failed);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
        assert!(response.payment_proof.is_none());
    }

    /// The dispatch-boundary variants must remain distinct from the
    /// pre-dispatch `PaymentFailed`, so only the former stay `Err` (ambiguous)
    /// and the latter can be converted to an authoritative `Failed` response.
    /// This guards against a future change re-collapsing the two.
    #[test]
    fn dispatch_boundary_errors_are_distinct_from_pre_dispatch_failure() {
        // `AmbiguousDispatch` is returned by send_* / stream failures (may have
        // been accepted by LND) and must never be treated as a terminal
        // pre-dispatch failure. It is a separate variant from `PaymentFailed`.
        assert_ne!(
            Error::AmbiguousDispatch.to_string(),
            Error::PaymentFailed.to_string()
        );
        assert_ne!(
            Error::UnknownPaymentStatus.to_string(),
            Error::PaymentFailed.to_string()
        );
    }
}
