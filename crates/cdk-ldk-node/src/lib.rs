//! CDK lightning backend for ldk-node

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::amount::to_unit;
use cdk_common::common::FeeReserve;
use cdk_common::payment::{self, *};
use cdk_common::util::{hex, unix_time};
use cdk_common::{Amount, CurrencyUnit, MeltOptions, MeltQuoteState};
use futures::{Stream, StreamExt};
use ldk_node::bitcoin::hashes::Hash;
use ldk_node::bitcoin::Network;
use ldk_node::lightning::ln::channelmanager::PaymentId;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::lightning_invoice::{Bolt11InvoiceDescription, Description};
use ldk_node::lightning_types::payment::PaymentHash;
use ldk_node::payment::{PaymentDirection, PaymentKind, PaymentStatus, SendingParameters};
use ldk_node::{Builder, Event, Node};
use tokio::runtime::Runtime;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::error::Error;

mod error;
mod web;

/// CDK Lightning backend using LDK Node
///
/// Provides Lightning Network functionality for CDK with support for Cashu operations.
/// Handles payment creation, processing, and event management using the Lightning Development Kit.
#[derive(Clone)]
pub struct CdkLdkNode {
    inner: Arc<Node>,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    sender: tokio::sync::broadcast::Sender<WaitPaymentResponse>,
    receiver: Arc<tokio::sync::broadcast::Receiver<WaitPaymentResponse>>,
    events_cancel_token: CancellationToken,
    runtime: Option<Arc<Runtime>>,
    web_addr: Option<SocketAddr>,
}

/// Configuration for connecting to Bitcoin RPC
///
/// Contains the necessary connection parameters for Bitcoin Core RPC interface.
#[derive(Debug, Clone)]
pub struct BitcoinRpcConfig {
    /// Bitcoin RPC server hostname or IP address
    pub host: String,
    /// Bitcoin RPC server port number
    pub port: u16,
    /// Username for Bitcoin RPC authentication
    pub user: String,
    /// Password for Bitcoin RPC authentication
    pub password: String,
}

/// Source of blockchain data for the Lightning node
///
/// Specifies how the node should connect to the Bitcoin network to retrieve
/// blockchain information and broadcast transactions.
#[derive(Debug, Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    ///
    /// Contains the URL of the Esplora server endpoint
    Esplora(String),
    /// Use Bitcoin Core RPC for blockchain data
    ///
    /// Contains the configuration for connecting to Bitcoin Core
    BitcoinRpc(BitcoinRpcConfig),
}

/// Source of Lightning network gossip data
///
/// Specifies how the node should learn about the Lightning Network topology
/// and routing information.
#[derive(Debug, Clone)]
pub enum GossipSource {
    /// Learn gossip through peer-to-peer connections
    ///
    /// The node will connect to other Lightning nodes and exchange gossip data directly
    P2P,
    /// Use Rapid Gossip Sync for efficient gossip updates
    ///
    /// Contains the URL of the RGS server for compressed gossip data
    RapidGossipSync(String),
}

impl CdkLdkNode {
    /// Create a new CDK LDK Node instance
    ///
    /// # Arguments
    /// * `network` - Bitcoin network (mainnet, testnet, regtest, signet)
    /// * `chain_source` - Source of blockchain data (Esplora or Bitcoin RPC)
    /// * `gossip_source` - Source of Lightning network gossip data
    /// * `storage_dir_path` - Directory path for node data storage
    /// * `fee_reserve` - Fee reserve configuration for payments
    /// * `listening_address` - Socket addresses for peer connections
    /// * `runtime` - Optional Tokio runtime to use for starting the node
    ///
    /// # Returns
    /// A new `CdkLdkNode` instance ready to be started
    ///
    /// # Errors
    /// Returns an error if the LDK node builder fails to create the node
    pub fn new(
        network: Network,
        chain_source: ChainSource,
        gossip_source: GossipSource,
        storage_dir_path: String,
        fee_reserve: FeeReserve,
        listening_address: Vec<SocketAddress>,
        runtime: Option<Arc<Runtime>>,
    ) -> Result<Self, Error> {
        let mut builder = Builder::new();
        builder.set_network(network);
        tracing::info!("Storage dir of node is {}", storage_dir_path);
        builder.set_storage_dir_path(storage_dir_path);

        match chain_source {
            ChainSource::Esplora(esplora_url) => {
                builder.set_chain_source_esplora(esplora_url, None);
            }
            ChainSource::BitcoinRpc(BitcoinRpcConfig {
                host,
                port,
                user,
                password,
            }) => {
                builder.set_chain_source_bitcoind_rpc(host, port, user, password);
            }
        }

        match gossip_source {
            GossipSource::P2P => {
                builder.set_gossip_source_p2p();
            }
            GossipSource::RapidGossipSync(rgs_url) => {
                builder.set_gossip_source_rgs(rgs_url);
            }
        }

        builder.set_listening_addresses(listening_address)?;

        builder.set_node_alias("cdk-ldk-node".to_string())?;

        let node = builder.build()?;

        tracing::info!("Creating tokio channel for payment notifications");
        let (sender, receiver) = tokio::sync::broadcast::channel(8);

        let id = node.node_id();

        let adr = node.announcement_addresses();

        tracing::info!(
            "Created node {} with address {:?} on network {}",
            id,
            adr,
            network
        );

        Ok(Self {
            inner: node.into(),
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            sender,
            receiver: Arc::new(receiver),
            events_cancel_token: CancellationToken::new(),
            runtime,
            web_addr: None,
        })
    }

    /// Set the web server address for the LDK node management interface
    ///
    /// # Arguments
    /// * `addr` - Socket address for the web server. If None, no web server will be started.
    pub fn set_web_addr(&mut self, addr: Option<SocketAddr>) {
        self.web_addr = addr;
    }

    /// Get a default web server address using an unused port
    ///
    /// Returns a SocketAddr with localhost and port 0, which will cause
    /// the system to automatically assign an available port
    pub fn default_web_addr() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 8091))
    }

    /// Start the CDK LDK Node
    ///
    /// Starts the underlying LDK node and begins event processing.
    /// Sets up event handlers to listen for Lightning events like payment received.
    ///
    /// # Returns
    /// Returns `Ok(())` on successful start, error otherwise
    ///
    /// # Errors
    /// Returns an error if the LDK node fails to start or event handling setup fails
    pub fn start_ldk_node(&self) -> Result<(), Error> {
        match &self.runtime {
            Some(runtime) => {
                tracing::info!("Starting cdk-ldk node with existing runtime");
                self.inner.start_with_runtime(Arc::clone(runtime))?
            }
            None => {
                tracing::info!("Starting cdk-ldk-node with new runtime");
                self.inner.start()?
            }
        };
        let node_config = self.inner.config();

        tracing::info!("Starting node with network {}", node_config.network);

        tracing::info!("Node status: {:?}", self.inner.status());

        self.handle_events()?;

        Ok(())
    }

    /// Start the web server for the LDK node management interface
    ///
    /// Starts a web server that provides a user interface for managing the LDK node.
    /// The web interface allows users to view balances, manage channels, create invoices,
    /// and send payments.
    ///
    /// # Arguments
    /// * `web_addr` - The socket address to bind the web server to
    ///
    /// # Returns
    /// Returns `Ok(())` on successful start, error otherwise
    ///
    /// # Errors
    /// Returns an error if the web server fails to start
    pub fn start_web_server(&self, web_addr: SocketAddr) -> Result<(), Error> {
        let web_server = crate::web::WebServer::new(Arc::new(self.clone()));

        tokio::spawn(async move {
            if let Err(e) = web_server.serve(web_addr).await {
                tracing::error!("Web server error: {}", e);
            }
        });

        Ok(())
    }

    /// Stop the CDK LDK Node
    ///
    /// Gracefully stops the node by cancelling all active tasks and event handlers.
    /// This includes:
    /// - Cancelling the event handler task
    /// - Cancelling any active wait_invoice streams
    /// - Stopping the underlying LDK node
    ///
    /// # Returns
    /// Returns `Ok(())` on successful shutdown, error otherwise
    ///
    /// # Errors
    /// Returns an error if the underlying LDK node fails to stop
    pub fn stop_ldk_node(&self) -> Result<(), Error> {
        tracing::info!("Stopping CdkLdkNode");
        // Cancel all tokio tasks
        tracing::info!("Cancelling event handler");
        self.events_cancel_token.cancel();

        // Cancel any wait_invoice streams
        if self.is_wait_invoice_active() {
            tracing::info!("Cancelling wait_invoice stream");
            self.wait_invoice_cancel_token.cancel();
        }

        // Stop the LDK node
        tracing::info!("Stopping LDK node");
        self.inner.stop()?;
        tracing::info!("CdkLdkNode stopped successfully");
        Ok(())
    }

    /// Handle payment received event
    async fn handle_payment_received(
        node: &Arc<Node>,
        sender: &tokio::sync::broadcast::Sender<WaitPaymentResponse>,
        payment_id: Option<PaymentId>,
        payment_hash: PaymentHash,
        amount_msat: u64,
    ) {
        tracing::info!(
            "Received payment for hash={} of amount={} msat",
            payment_hash,
            amount_msat
        );

        let payment_id = match payment_id {
            Some(id) => id,
            None => {
                tracing::warn!("Received payment without payment_id");
                return;
            }
        };

        let payment_id_hex = hex::encode(payment_id.0);

        if amount_msat == 0 {
            tracing::warn!("Payment of no amount");
            return;
        }

        tracing::info!(
            "Processing payment notification: id={}, amount={} msats",
            payment_id_hex,
            amount_msat
        );

        let payment_details = match node.payment(&payment_id) {
            Some(details) => details,
            None => {
                tracing::error!("Could not find payment details for id={}", payment_id_hex);
                return;
            }
        };

        let (payment_identifier, payment_id) = match payment_details.kind {
            PaymentKind::Bolt11 { hash, .. } => {
                (PaymentIdentifier::PaymentHash(hash.0), hash.to_string())
            }
            PaymentKind::Bolt12Offer { hash, offer_id, .. } => match hash {
                Some(h) => (
                    PaymentIdentifier::OfferId(offer_id.to_string()),
                    h.to_string(),
                ),
                None => {
                    tracing::error!("Bolt12 payment missing hash");
                    return;
                }
            },
            k => {
                tracing::warn!("Received payment of kind {:?} which is not supported", k);
                return;
            }
        };

        let wait_payment_response = WaitPaymentResponse {
            payment_identifier,
            payment_amount: amount_msat.into(),
            unit: CurrencyUnit::Msat,
            payment_id,
        };

        match sender.send(wait_payment_response) {
            Ok(_) => tracing::info!("Successfully sent payment notification to stream"),
            Err(err) => tracing::error!(
                "Could not send payment received notification on channel: {}",
                err
            ),
        }
    }

    /// Set up event handling for the node
    pub fn handle_events(&self) -> Result<(), Error> {
        let node = self.inner.clone();
        let sender = self.sender.clone();
        let cancel_token = self.events_cancel_token.clone();

        tracing::info!("Starting event handler task");

        tokio::spawn(async move {
            tracing::info!("Event handler loop started");
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Event handler cancelled");
                        break;
                    }
                    event = node.next_event_async() => {
                        match event {
                            Event::PaymentReceived {
                                payment_id,
                                payment_hash,
                                amount_msat,
                                custom_records: _
                            } => {
                                Self::handle_payment_received(
                                    &node,
                                    &sender,
                                    payment_id,
                                    payment_hash,
                                    amount_msat
                                ).await;
                            }
                            event => {
                                tracing::debug!("Received other ldk node event: {:?}", event);
                            }
                        }

                        if let Err(err) = node.event_handled() {
                            tracing::error!("Error handling node event: {}", err);
                        } else {
                            tracing::debug!("Successfully handled node event");
                        }
                    }
                }
            }
            tracing::info!("Event handler loop terminated");
        });

        tracing::info!("Event handler task spawned");
        Ok(())
    }

    /// Get Node used
    pub fn node(&self) -> Arc<Node> {
        Arc::clone(&self.inner)
    }
}

/// Mint payment trait
#[async_trait]
impl MintPayment for CdkLdkNode {
    type Err = payment::Error;

    /// Start the payment processor
    /// Starts the LDK node and begins event processing
    async fn start(&self) -> Result<(), Self::Err> {
        self.start_ldk_node().map_err(|e| {
            tracing::error!("Failed to start CdkLdkNode: {}", e);
            e
        })?;

        tracing::info!("CdkLdkNode payment processor started successfully");

        // Start web server if configured
        if let Some(web_addr) = self.web_addr {
            tracing::info!("Starting LDK Node web interface on {}", web_addr);
            self.start_web_server(web_addr).map_err(|e| {
                tracing::error!("Failed to start web server: {}", e);
                e
            })?;
        } else {
            tracing::info!("No web server address configured, skipping web interface");
        }

        Ok(())
    }

    /// Stop the payment processor
    /// Gracefully stops the LDK node and cancels all background tasks
    async fn stop(&self) -> Result<(), Self::Err> {
        self.stop_ldk_node().map_err(|e| {
            tracing::error!("Failed to stop CdkLdkNode: {}", e);
            e.into()
        })
    }

    /// Base Settings
    async fn get_settings(&self) -> Result<serde_json::Value, Self::Err> {
        let settings = Bolt11Settings {
            mpp: false,
            unit: CurrencyUnit::Msat,
            invoice_description: true,
            amountless: true,
            bolt12: true,
        };
        Ok(serde_json::to_value(settings)?)
    }

    /// Create a new invoice
    #[instrument(skip(self))]
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = to_unit(bolt11_options.amount, unit, &CurrencyUnit::Msat)?;
                let description = bolt11_options.description.unwrap_or_default();
                let time = bolt11_options
                    .unix_expiry
                    .map(|t| t - unix_time())
                    .unwrap_or(36000);

                let description = Bolt11InvoiceDescription::Direct(
                    Description::new(description).map_err(|_| Error::InvalidDescription)?,
                );

                let payment = self
                    .inner
                    .bolt11_payment()
                    .receive(amount_msat.into(), &description, time as u32)
                    .map_err(Error::LdkNode)?;

                let payment_hash = payment.payment_hash().to_string();
                let payment_identifier = PaymentIdentifier::PaymentHash(
                    hex::decode(&payment_hash)?
                        .try_into()
                        .map_err(|_| Error::InvalidPaymentHashLength)?,
                );

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: payment_identifier,
                    request: payment.to_string(),
                    expiry: Some(unix_time() + time),
                })
            }
            IncomingPaymentOptions::Bolt12(bolt12_options) => {
                let Bolt12IncomingPaymentOptions {
                    description,
                    amount,
                    unix_expiry,
                } = *bolt12_options;

                let time = unix_expiry.map(|t| (t - unix_time()) as u32);

                let offer = match amount {
                    Some(amount) => {
                        let amount_msat = to_unit(amount, unit, &CurrencyUnit::Msat)?;

                        self.inner
                            .bolt12_payment()
                            .receive(
                                amount_msat.into(),
                                &description.unwrap_or("".to_string()),
                                time,
                                None,
                            )
                            .map_err(Error::LdkNode)?
                    }
                    None => self
                        .inner
                        .bolt12_payment()
                        .receive_variable_amount(&description.unwrap_or("".to_string()), time)
                        .map_err(Error::LdkNode)?,
                };
                let payment_identifier = PaymentIdentifier::OfferId(offer.id().to_string());

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: payment_identifier,
                    request: offer.to_string(),
                    expiry: time.map(|a| a as u64),
                })
            }
        }
    }

    /// Get payment quote
    /// Used to get fee and amount required for a payment request
    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;

                let amount_msat = match bolt11_options.melt_options {
                    Some(melt_options) => melt_options.amount_msat(),
                    None => bolt11
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .into(),
                };

                let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

                let fee = match relative_fee_reserve > absolute_fee_reserve {
                    true => relative_fee_reserve,
                    false => absolute_fee_reserve,
                };

                let payment_hash = bolt11.payment_hash().to_string();
                let payment_hash_bytes = hex::decode(&payment_hash)?
                    .try_into()
                    .map_err(|_| Error::InvalidPaymentHashLength)?;

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(payment_hash_bytes)),
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = bolt12_options.offer;

                let amount_msat = match bolt12_options.melt_options {
                    Some(melt_options) => melt_options.amount_msat(),
                    None => {
                        let amount = offer.amount().ok_or(payment::Error::AmountMismatch)?;

                        match amount {
                            ldk_node::lightning::offers::offer::Amount::Bitcoin {
                                amount_msats,
                            } => amount_msats.into(),
                            _ => return Err(payment::Error::AmountMismatch),
                        }
                    }
                };
                let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

                let fee = match relative_fee_reserve > absolute_fee_reserve {
                    true => relative_fee_reserve,
                    false => absolute_fee_reserve,
                };

                Ok(PaymentQuoteResponse {
                    request_lookup_id: None,
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
        }
    }

    /// Pay request
    #[instrument(skip(self, options))]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;

                let send_params = match bolt11_options
                    .max_fee_amount
                    .map(|f| {
                        to_unit(f, unit, &CurrencyUnit::Msat).map(|amount_msat| SendingParameters {
                            max_total_routing_fee_msat: Some(Some(amount_msat.into())),
                            max_channel_saturation_power_of_half: None,
                            max_total_cltv_expiry_delta: None,
                            max_path_count: None,
                        })
                    })
                    .transpose()
                {
                    Ok(params) => params,
                    Err(err) => {
                        tracing::error!("Failed to convert fee amount: {}", err);
                        return Err(payment::Error::Custom(format!("Invalid fee amount: {err}")));
                    }
                };

                let payment_id = match bolt11_options.melt_options {
                    Some(MeltOptions::Amountless { amountless }) => self
                        .inner
                        .bolt11_payment()
                        .send_using_amount(&bolt11, amountless.amount_msat.into(), send_params)
                        .map_err(|err| {
                            tracing::error!("Could not send send amountless bolt11: {}", err);
                            Error::CouldNotSendBolt11WithoutAmount
                        })?,
                    None => self
                        .inner
                        .bolt11_payment()
                        .send(&bolt11, send_params)
                        .map_err(|err| {
                            tracing::error!("Could not send bolt11 {}", err);
                            Error::CouldNotSendBolt11
                        })?,
                    _ => return Err(payment::Error::UnsupportedPaymentOption),
                };

                // Check payment status for up to 10 seconds
                let start = std::time::Instant::now();
                let timeout = std::time::Duration::from_secs(10);

                let (status, payment_details) = loop {
                    let details = self
                        .inner
                        .payment(&payment_id)
                        .ok_or(Error::PaymentNotFound)?;

                    match details.status {
                        PaymentStatus::Succeeded => break (MeltQuoteState::Paid, details),
                        PaymentStatus::Failed => {
                            tracing::error!("Failed to pay bolt11 payment.");
                            break (MeltQuoteState::Failed, details);
                        }
                        PaymentStatus::Pending => {
                            if start.elapsed() > timeout {
                                tracing::warn!(
                                    "Paying bolt11 exceeded timeout 10 seconds no longer waitning."
                                );
                                break (MeltQuoteState::Pending, details);
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            continue;
                        }
                    }
                };

                let payment_proof = match payment_details.kind {
                    PaymentKind::Bolt11 {
                        hash: _,
                        preimage,
                        secret: _,
                    } => preimage.map(|p| p.to_string()),
                    _ => return Err(Error::UnexpectedPaymentKind.into()),
                };

                let total_spent = payment_details
                    .amount_msat
                    .ok_or(Error::CouldNotGetAmountSpent)?;

                let total_spent = to_unit(total_spent, &CurrencyUnit::Msat, unit)?;

                Ok(MakePaymentResponse {
                    payment_lookup_id: PaymentIdentifier::PaymentHash(
                        bolt11.payment_hash().to_byte_array(),
                    ),
                    payment_proof,
                    status,
                    total_spent,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = bolt12_options.offer;

                let payment_id = match bolt12_options.melt_options {
                    Some(MeltOptions::Amountless { amountless }) => self
                        .inner
                        .bolt12_payment()
                        .send_using_amount(&offer, amountless.amount_msat.into(), None, None)
                        .map_err(Error::LdkNode)?,
                    None => self
                        .inner
                        .bolt12_payment()
                        .send(&offer, None, None)
                        .map_err(Error::LdkNode)?,
                    _ => return Err(payment::Error::UnsupportedPaymentOption),
                };

                // Check payment status for up to 10 seconds
                let start = std::time::Instant::now();
                let timeout = std::time::Duration::from_secs(10);

                let (status, payment_details) = loop {
                    let details = self
                        .inner
                        .payment(&payment_id)
                        .ok_or(Error::PaymentNotFound)?;

                    match details.status {
                        PaymentStatus::Succeeded => break (MeltQuoteState::Paid, details),
                        PaymentStatus::Failed => {
                            tracing::error!("Payment with id {} failed.", payment_id);
                            break (MeltQuoteState::Failed, details);
                        }
                        PaymentStatus::Pending => {
                            if start.elapsed() > timeout {
                                tracing::warn!(
                                    "Payment has been being for 10 seconds. No longer waiting"
                                );
                                break (MeltQuoteState::Pending, details);
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            continue;
                        }
                    }
                };

                let payment_proof = match payment_details.kind {
                    PaymentKind::Bolt12Offer {
                        hash: _,
                        preimage,
                        secret: _,
                        offer_id: _,
                        payer_note: _,
                        quantity: _,
                    } => preimage.map(|p| p.to_string()),
                    _ => return Err(Error::UnexpectedPaymentKind.into()),
                };

                let total_spent = payment_details
                    .amount_msat
                    .ok_or(Error::CouldNotGetAmountSpent)?;

                let total_spent = to_unit(total_spent, &CurrencyUnit::Msat, unit)?;

                Ok(MakePaymentResponse {
                    payment_lookup_id: PaymentIdentifier::PaymentId(payment_id.0),
                    payment_proof,
                    status,
                    total_spent,
                    unit: unit.clone(),
                })
            }
        }
    }

    /// Listen for invoices to be paid to the mint
    /// Returns a stream of request_lookup_id once invoices are paid
    #[instrument(skip(self))]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>, Self::Err> {
        tracing::info!("Starting stream for invoices - wait_any_incoming_payment called");

        // Set active flag to indicate stream is active
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);
        tracing::debug!("wait_invoice_is_active set to true");

        let receiver = self.receiver.clone();

        tracing::info!("Receiver obtained successfully, creating response stream");

        // Transform the String stream into a WaitPaymentResponse stream
        let response_stream = BroadcastStream::new(receiver.resubscribe());

        // Map the stream to handle BroadcastStreamRecvError and wrap in Event
        let response_stream = response_stream.filter_map(|result| async move {
            match result {
                Ok(payment) => Some(cdk_common::payment::Event::PaymentReceived(payment)),
                Err(err) => {
                    tracing::warn!("Error in broadcast stream: {}", err);
                    None
                }
            }
        });

        // Create a combined stream that also handles cancellation
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let is_active = self.wait_invoice_is_active.clone();

        let stream = Box::pin(response_stream);

        // Set up a task to clean up when the stream is dropped
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            tracing::info!("wait_invoice stream cancelled");
            is_active.store(false, Ordering::SeqCst);
        });

        tracing::info!("wait_any_incoming_payment returning stream");
        Ok(stream)
    }

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    /// Check the status of an incoming payment
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let payment_id_str = match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) => hex::encode(hash),
            PaymentIdentifier::CustomId(id) => id.clone(),
            _ => return Err(Error::UnsupportedPaymentIdentifierType.into()),
        };

        let payment_id = PaymentId(
            hex::decode(&payment_id_str)?
                .try_into()
                .map_err(|_| Error::InvalidPaymentIdLength)?,
        );

        let payment_details = self
            .inner
            .payment(&payment_id)
            .ok_or(Error::PaymentNotFound)?;

        if payment_details.direction == PaymentDirection::Outbound {
            return Err(Error::InvalidPaymentDirection.into());
        }

        let amount = if payment_details.status == PaymentStatus::Succeeded {
            payment_details
                .amount_msat
                .ok_or(Error::CouldNotGetPaymentAmount)?
        } else {
            return Ok(vec![]);
        };

        let response = WaitPaymentResponse {
            payment_identifier: payment_identifier.clone(),
            payment_amount: amount.into(),
            unit: CurrencyUnit::Msat,
            payment_id: payment_id_str,
        };

        Ok(vec![response])
    }

    /// Check the status of an outgoing payment
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment_details = match request_lookup_id {
            PaymentIdentifier::PaymentHash(id_hash) => self
                .inner
                .list_payments_with_filter(
                    |p| matches!(&p.kind, PaymentKind::Bolt11 { hash, .. } if &hash.0 == id_hash),
                )
                .first()
                .cloned(),
            PaymentIdentifier::PaymentId(id) => self.inner.payment(&PaymentId(
                hex::decode(id)?
                    .try_into()
                    .map_err(|_| payment::Error::Custom("Invalid hex".to_string()))?,
            )),
            _ => {
                return Ok(MakePaymentResponse {
                    payment_lookup_id: request_lookup_id.clone(),
                    status: MeltQuoteState::Unknown,
                    payment_proof: None,
                    total_spent: Amount::ZERO,
                    unit: CurrencyUnit::Msat,
                });
            }
        }
        .ok_or(Error::PaymentNotFound)?;

        // This check seems reversed in the original code, so I'm fixing it here
        if payment_details.direction != PaymentDirection::Outbound {
            return Err(Error::InvalidPaymentDirection.into());
        }

        let status = match payment_details.status {
            PaymentStatus::Pending => MeltQuoteState::Pending,
            PaymentStatus::Succeeded => MeltQuoteState::Paid,
            PaymentStatus::Failed => MeltQuoteState::Failed,
        };

        let payment_proof = match payment_details.kind {
            PaymentKind::Bolt11 {
                hash: _,
                preimage,
                secret: _,
            } => preimage.map(|p| p.to_string()),
            _ => return Err(Error::UnexpectedPaymentKind.into()),
        };

        let total_spent = payment_details
            .amount_msat
            .ok_or(Error::CouldNotGetAmountSpent)?;

        Ok(MakePaymentResponse {
            payment_lookup_id: request_lookup_id.clone(),
            payment_proof,
            status,
            total_spent: total_spent.into(),
            unit: CurrencyUnit::Msat,
        })
    }
}

impl Drop for CdkLdkNode {
    fn drop(&mut self) {
        tracing::info!("Drop called on CdkLdkNode");
        self.wait_invoice_cancel_token.cancel();
        tracing::debug!("Cancelled wait_invoice token in drop");
    }
}
