//! CDK lightning backend for ldk-node

#![doc = include_str!("../README.md")]

use std::fmt;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk_common::common::FeeReserve;
use cdk_common::database::DynKVStore;
use cdk_common::payment::{self, *};
use cdk_common::redact::url_for_logs;
use cdk_common::util::{hex, unix_time};
use cdk_common::{Amount, CurrencyUnit, MeltOptions, MeltQuoteState, QuoteId};
use futures::{Stream, StreamExt};
use ldk_node::bitcoin::hashes::Hash;
use ldk_node::bitcoin::Network;
use ldk_node::lightning::ln::channelmanager::PaymentId;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::lightning::routing::router::RouteParametersConfig;
use ldk_node::lightning_invoice::{Bolt11InvoiceDescription, Description};
use ldk_node::lightning_types::payment::PaymentHash;
use ldk_node::logger::{LogLevel, LogWriter};
use ldk_node::payment::{PaymentDetails, PaymentDirection, PaymentKind, PaymentStatus};
use ldk_node::{Builder, Event, Node};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::error::Error;
use crate::log::StdoutLogWriter;

mod error;
mod log;
mod web;

/// Primary KV namespace for the ldk-node backend's durable bookkeeping
const LDK_KV_PRIMARY_NAMESPACE: &str = "cdk_ldk_node_lightning_backend";
/// Secondary KV namespace holding the bolt12 melt quote id -> payment id
/// mapping used to resolve `PaymentIdentifier::QuoteId` lookups
const LDK_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE: &str = "bolt12_outgoing_payments";

/// Result of looking up the payment id recorded for a bolt12 melt quote
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bolt12QuotePaymentIdLookup {
    /// A payment id was recorded: the payment was dispatched and is tracked
    Found(PaymentId),
    /// The dispatch sentinel is present: `send` was started but no payment id
    /// was recorded (crash during dispatch, or dispatch errored before the
    /// sentinel could be cleaned up). The payment state is indeterminate.
    Dispatching,
    /// No record exists: the payment was never dispatched
    Missing,
    /// A record exists but cannot be parsed
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bolt12QuotePaymentIdResolution {
    PaymentId(PaymentId),
    Status(MeltQuoteState),
}

impl Bolt12QuotePaymentIdLookup {
    fn resolve(self) -> Bolt12QuotePaymentIdResolution {
        match self {
            Self::Found(payment_id) => Bolt12QuotePaymentIdResolution::PaymentId(payment_id),
            // Dispatch was attempted but no payment id was recorded. Pending
            // prevents the live melt saga from compensating an indeterminate
            // payment after a dispatch-ambiguous error.
            Self::Dispatching => Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Pending),
            // Without the pre-dispatch sentinel, the payment was never sent.
            Self::Missing => Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Unpaid),
            // Corrupt bookkeeping cannot establish any payment state.
            Self::Malformed => Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Unknown),
        }
    }
}

/// Whether an LDK BOLT12 send error can occur after dispatch was accepted.
///
/// In `ldk-node` 0.7, [`ldk_node::NodeError::PersistenceFailed`] can be returned
/// while persisting the payment record after `ChannelManager::pay_for_offer`
/// accepted the payment. All other errors returned by the BOLT12 send methods
/// occur before dispatch or after `pay_for_offer` rejected the attempt.
fn bolt12_send_error_has_ambiguous_dispatch(err: &ldk_node::NodeError) -> bool {
    matches!(err, ldk_node::NodeError::PersistenceFailed)
}

/// CDK Lightning backend using LDK Node
///
/// Provides Lightning Network functionality for CDK with support for Cashu operations.
/// Handles payment creation, processing, and event management using the Lightning Development Kit.
#[derive(Clone)]
pub struct CdkLdkNode {
    inner: Arc<Node>,
    fee_reserve: FeeReserve,
    kv_store: DynKVStore,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    sender: tokio::sync::broadcast::Sender<WaitPaymentResponse>,
    receiver: Arc<tokio::sync::broadcast::Receiver<WaitPaymentResponse>>,
    events_cancel_token: CancellationToken,
    web_addr: Option<SocketAddr>,
}

impl fmt::Debug for CdkLdkNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkLdkNode")
            .field("fee_reserve", &self.fee_reserve)
            .field("web_addr", &self.web_addr)
            .finish_non_exhaustive()
    }
}

/// Configuration for connecting to Bitcoin RPC
///
/// Contains the necessary connection parameters for Bitcoin Core RPC interface.
#[derive(Clone)]
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

impl fmt::Debug for BitcoinRpcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitcoinRpcConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// Source of blockchain data for the Lightning node
///
/// Specifies how the node should connect to the Bitcoin network to retrieve
/// blockchain information and broadcast transactions.
#[derive(Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    ///
    /// Contains the URL of the Esplora server endpoint
    Esplora(String),
    /// Use an Electrum server for blockchain data
    ///
    /// Contains the URL of the Electrum server endpoint
    Electrum(String),
    /// Use Bitcoin Core RPC for blockchain data
    ///
    /// Contains the configuration for connecting to Bitcoin Core
    BitcoinRpc(BitcoinRpcConfig),
}

impl fmt::Debug for ChainSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Esplora(url) => f.debug_tuple("Esplora").field(&url_for_logs(url)).finish(),
            Self::Electrum(url) => f.debug_tuple("Electrum").field(&url_for_logs(url)).finish(),
            Self::BitcoinRpc(config) => f.debug_tuple("BitcoinRpc").field(config).finish(),
        }
    }
}

/// Source of Lightning network gossip data
///
/// Specifies how the node should learn about the Lightning Network topology
/// and routing information.
#[derive(Clone)]
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

impl fmt::Debug for GossipSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::P2P => f.write_str("P2P"),
            Self::RapidGossipSync(url) => f
                .debug_tuple("RapidGossipSync")
                .field(&url_for_logs(url))
                .finish(),
        }
    }
}
/// A builder for an [`CdkLdkNode`] instance.
pub struct CdkLdkNodeBuilder {
    network: Network,
    chain_source: ChainSource,
    gossip_source: GossipSource,
    log_dir_path: Option<String>,
    storage_dir_path: String,
    fee_reserve: FeeReserve,
    kv_store: DynKVStore,
    listening_addresses: Vec<SocketAddress>,
    seed: Option<Mnemonic>,
    announcement_addresses: Option<Vec<SocketAddress>>,
}

impl std::fmt::Debug for CdkLdkNodeBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdkLdkNodeBuilder")
            .field("network", &self.network)
            .field("chain_source", &self.chain_source)
            .field("gossip_source", &self.gossip_source)
            .field("log_dir_path", &self.log_dir_path)
            .field("storage_dir_path", &self.storage_dir_path)
            .field("fee_reserve", &self.fee_reserve)
            .field("listening_addresses", &self.listening_addresses)
            .field("announcement_addresses", &self.announcement_addresses)
            .finish_non_exhaustive()
    }
}

impl CdkLdkNodeBuilder {
    /// Creates a new builder instance.
    pub fn new(
        network: Network,
        chain_source: ChainSource,
        gossip_source: GossipSource,
        storage_dir_path: String,
        fee_reserve: FeeReserve,
        listening_addresses: Vec<SocketAddress>,
        kv_store: DynKVStore,
    ) -> Self {
        Self {
            network,
            chain_source,
            gossip_source,
            storage_dir_path,
            fee_reserve,
            kv_store,
            listening_addresses,
            seed: None,
            announcement_addresses: None,
            log_dir_path: None,
        }
    }

    /// Configures the [`CdkLdkNode`] to use the Mnemonic for entropy source configuration
    pub fn with_seed(mut self, seed: Mnemonic) -> Self {
        self.seed = Some(seed);
        self
    }
    /// Configures the [`CdkLdkNode`] to use announce this address to the lightning network
    pub fn with_announcement_address(mut self, announcement_addresses: Vec<SocketAddress>) -> Self {
        self.announcement_addresses = Some(announcement_addresses);
        self
    }
    /// Configures the [`CdkLdkNode`] to use announce this address to the lightning network
    pub fn with_log_dir_path(mut self, log_dir_path: String) -> Self {
        self.log_dir_path = Some(log_dir_path);
        self
    }

    /// Builds the [`CdkLdkNode`] instance
    ///
    /// # Errors
    /// Returns an error if the LDK node builder fails to create the node
    pub fn build(self) -> Result<CdkLdkNode, Error> {
        let mut ldk = Builder::new();
        ldk.set_network(self.network);
        tracing::info!("Storage dir of node is {}", self.storage_dir_path);
        ldk.set_storage_dir_path(self.storage_dir_path);

        match self.chain_source {
            ChainSource::Esplora(esplora_url) => {
                ldk.set_chain_source_esplora(esplora_url, None);
            }
            ChainSource::Electrum(electrum_url) => {
                ldk.set_chain_source_electrum(electrum_url, None);
            }
            ChainSource::BitcoinRpc(BitcoinRpcConfig {
                host,
                port,
                user,
                password,
            }) => {
                ldk.set_chain_source_bitcoind_rpc(host, port, user, password);
            }
        }

        match self.gossip_source {
            GossipSource::P2P => {
                ldk.set_gossip_source_p2p();
            }
            GossipSource::RapidGossipSync(rgs_url) => {
                ldk.set_gossip_source_rgs(rgs_url);
            }
        }

        ldk.set_listening_addresses(self.listening_addresses)?;
        if self.log_dir_path.is_some() {
            ldk.set_filesystem_logger(self.log_dir_path, Some(LogLevel::Info));
        } else {
            ldk.set_custom_logger(Arc::new(StdoutLogWriter));
        }

        ldk.set_node_alias("cdk-ldk-node".to_string())?;
        // set the seed as bip39 entropy mnemonic
        if let Some(seed) = self.seed {
            ldk.set_entropy_bip39_mnemonic(seed, None);
        }
        // set the announcement addresses
        if let Some(announcement_addresses) = self.announcement_addresses {
            ldk.set_announcement_addresses(announcement_addresses)?;
        }

        let node = ldk.build()?;

        tracing::info!("Creating tokio channel for payment notifications");
        let (sender, receiver) = tokio::sync::broadcast::channel(8);

        let id = node.node_id();

        let adr = node.announcement_addresses();

        tracing::info!(
            "Created node {} with address {:?} on network {}",
            id,
            adr,
            self.network
        );

        Ok(CdkLdkNode {
            inner: node.into(),
            fee_reserve: self.fee_reserve,
            kv_store: self.kv_store,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            sender,
            receiver: Arc::new(receiver),
            events_cancel_token: CancellationToken::new(),
            web_addr: None,
        })
    }
}

impl CdkLdkNode {
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

    /// Best-effort removal of the pre-dispatch sentinel after a send that did
    /// not dispatch. If removal fails the sentinel remains and the payment
    /// resolves as `Pending`, keeping the melt proofs reserved.
    async fn cleanup_bolt12_dispatch_sentinel(&self, quote_id: &QuoteId) {
        if let Err(err) = delete_bolt12_quote_payment_id(&self.kv_store, quote_id).await {
            tracing::warn!(
                "Could not remove BOLT12 dispatch sentinel for quote {quote_id}: {err}. \
                 The payment will remain Pending."
            );
        }
    }

    fn make_payment_response_from_details(
        unit: &CurrencyUnit,
        payment_lookup_id: PaymentIdentifier,
        payment_details: &PaymentDetails,
    ) -> Result<MakePaymentResponse, payment::Error> {
        let status = match payment_details.status {
            PaymentStatus::Pending => MeltQuoteState::Pending,
            PaymentStatus::Succeeded => MeltQuoteState::Paid,
            PaymentStatus::Failed => MeltQuoteState::Failed,
        };

        let payment_proof = match &payment_details.kind {
            PaymentKind::Bolt11 { preimage, .. } => preimage.map(|p| p.to_string()),
            PaymentKind::Bolt12Offer { preimage, .. } => preimage.map(|p| p.to_string()),
            _ => return Err(Error::UnexpectedPaymentKind.into()),
        };

        let total_spent = if status == MeltQuoteState::Paid {
            let total_spent = payment_details
                .amount_msat
                .ok_or(Error::CouldNotGetAmountSpent)?
                + payment_details.fee_paid_msat.unwrap_or_default();
            Amount::new(total_spent, CurrencyUnit::Msat).convert_to(unit)?
        } else {
            Amount::new(0, unit.clone())
        };

        Ok(MakePaymentResponse {
            payment_lookup_id,
            payment_proof,
            status,
            total_spent,
        })
    }

    fn select_bolt11_payment_details(
        payment_details: impl IntoIterator<Item = PaymentDetails>,
    ) -> Option<PaymentDetails> {
        payment_details.into_iter().min_by_key(|details| {
            let status_order = match details.status {
                PaymentStatus::Succeeded => 0_u8,
                PaymentStatus::Pending => 1,
                PaymentStatus::Failed => 2,
            };

            (
                status_order,
                std::cmp::Reverse(details.latest_update_timestamp),
            )
        })
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
        tracing::info!("Starting cdk-ldk node");
        self.inner.start()?;
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

        // Cancel any payment event streams
        if self.is_payment_event_stream_active() {
            tracing::info!("Cancelling payment event stream");
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
            payment_amount: Amount::new(amount_msat, CurrencyUnit::Msat),
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
                            Event::PaymentFailed {
                                payment_id,
                                payment_hash,
                                reason,
                            } => {
                                tracing::error!(
                                    payment_id = ?payment_id,
                                    payment_hash = ?payment_hash,
                                    reason = ?reason,
                                    "LDK node payment failed"
                                );
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
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        let settings = SettingsResponse {
            unit: CurrencyUnit::Msat.to_string(),
            bolt11: Some(payment::Bolt11Settings {
                mpp: false,
                amountless: true,
                invoice_description: true,
            }),
            bolt12: Some(payment::Bolt12Settings { amountless: true }),
            onchain: None,
            custom: std::collections::HashMap::new(),
        };
        Ok(settings)
    }

    /// Create a new invoice
    #[instrument(skip(self))]
    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat: Amount = bolt11_options
                    .amount
                    .convert_to(&CurrencyUnit::Msat)?
                    .into();
                let description = bolt11_options.description.unwrap_or_default();
                let time = match bolt11_options.unix_expiry {
                    Some(t) => t
                        .checked_sub(unix_time())
                        .ok_or(payment::Error::InvalidExpiry)?,
                    None => 36000,
                };

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
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Bolt12(bolt12_options) => {
                let Bolt12IncomingPaymentOptions {
                    description,
                    amount,
                    unix_expiry,
                } = *bolt12_options;

                let time = unix_expiry
                    .map(|t| {
                        t.checked_sub(unix_time())
                            .ok_or(payment::Error::InvalidExpiry)
                            .map(|t| t as u32)
                    })
                    .transpose()?;

                let offer = match amount {
                    Some(amount) => {
                        let amount_msat: Amount = amount.convert_to(&CurrencyUnit::Msat)?.into();

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
                    expiry: unix_expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Custom(_) | IncomingPaymentOptions::Onchain(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
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
            cdk_common::payment::OutgoingPaymentOptions::Custom(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
            }
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;

                let amount_msat = match bolt11_options.melt_options {
                    Some(MeltOptions::Amountless { amountless }) => {
                        let amount_msat = amountless.amount_msat;

                        if let Some(invoice_amount) = bolt11.amount_milli_satoshis() {
                            if invoice_amount != u64::from(amount_msat) {
                                return Err(payment::Error::AmountMismatch);
                            }
                        }

                        amount_msat
                    }
                    Some(MeltOptions::Mpp { mpp }) => mpp.amount,
                    None => bolt11
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .into(),
                };

                let amount =
                    Amount::new(amount_msat.into(), CurrencyUnit::Msat).convert_to(unit)?;

                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * amount.value() as f32) as u64;

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
                    fee: Amount::new(fee, unit.clone()),
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
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
                let amount =
                    Amount::new(amount_msat.into(), CurrencyUnit::Msat).convert_to(unit)?;

                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * amount.value() as f32) as u64;

                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

                let fee = match relative_fee_reserve > absolute_fee_reserve {
                    true => relative_fee_reserve,
                    false => absolute_fee_reserve,
                };

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::QuoteId(
                        bolt12_options.quote_id.clone(),
                    )),
                    amount,
                    fee: Amount::new(fee, unit.clone()),
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            OutgoingPaymentOptions::Onchain(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
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
            cdk_common::payment::OutgoingPaymentOptions::Custom(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
            }
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;

                let send_params = match bolt11_options
                    .max_fee_amount
                    .map(|f| {
                        f.convert_to(&CurrencyUnit::Msat)
                            .map(|amount_msat| RouteParametersConfig {
                                max_total_routing_fee_msat: Some(amount_msat.value()),
                                ..Default::default()
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
                    Some(MeltOptions::Amountless { amountless }) => {
                        if let Some(invoice_amount) = bolt11.amount_milli_satoshis() {
                            if invoice_amount != u64::from(amountless.amount_msat) {
                                return Err(payment::Error::AmountMismatch);
                            }
                        }

                        self.inner
                            .bolt11_payment()
                            .send_using_amount(&bolt11, amountless.amount_msat.into(), send_params)
                            .map_err(|err| {
                                tracing::error!("Could not send send amountless bolt11: {}", err);
                                Error::CouldNotSendBolt11WithoutAmount
                            })?
                    }
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

                let payment_details = loop {
                    let details = self
                        .inner
                        .payment(&payment_id)
                        .ok_or(Error::PaymentNotFound)?;

                    match details.status {
                        PaymentStatus::Succeeded => break details,
                        PaymentStatus::Failed => {
                            tracing::error!("Failed to pay bolt11 payment.");
                            break details;
                        }
                        PaymentStatus::Pending => {
                            if start.elapsed() > timeout {
                                tracing::warn!(
                                    "Paying bolt11 exceeded timeout 10 seconds no longer waitning."
                                );
                                break details;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            continue;
                        }
                    }
                };

                Self::make_payment_response_from_details(
                    unit,
                    PaymentIdentifier::PaymentHash(bolt11.payment_hash().to_byte_array()),
                    &payment_details,
                )
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = bolt12_options.offer;
                let quote_id = bolt12_options.quote_id.clone();
                let quote_payment_identifier = PaymentIdentifier::QuoteId(quote_id.clone());

                let send_params = match bolt12_options
                    .max_fee_amount
                    .map(|f| {
                        f.convert_to(&CurrencyUnit::Msat)
                            .map(|amount_msat| RouteParametersConfig {
                                max_total_routing_fee_msat: Some(amount_msat.value()),
                                ..Default::default()
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

                // Write the pre-dispatch sentinel before attempting the send so
                // a crash during dispatch is distinguishable from "never
                // dispatched" (mirrors cdk-cln's pre-dispatch bolt12 quote
                // mapping). The payment must not be attempted unless this
                // marker is durable.
                write_bolt12_quote_payment_id(&self.kv_store, &quote_id, None).await?;

                let payment_id = match bolt12_options.melt_options {
                    Some(MeltOptions::Amountless { amountless }) => {
                        self.inner.bolt12_payment().send_using_amount(
                            &offer,
                            amountless.amount_msat.into(),
                            None,
                            None,
                            send_params,
                        )
                    }
                    None => self
                        .inner
                        .bolt12_payment()
                        .send(&offer, None, None, send_params),
                    _ => {
                        self.cleanup_bolt12_dispatch_sentinel(&quote_id).await;
                        return Err(payment::Error::UnsupportedPaymentOption);
                    }
                };

                let payment_id = match payment_id {
                    Ok(payment_id) => payment_id,
                    Err(err) => {
                        match bolt12_send_error_has_ambiguous_dispatch(&err) {
                            true => {
                                tracing::warn!(
                                    quote_id = %quote_id,
                                    "LDK payment persistence failed after BOLT12 send; retaining \
                                     the dispatch sentinel because the payment may have been dispatched"
                                );
                            }
                            false => {
                                self.cleanup_bolt12_dispatch_sentinel(&quote_id).await;
                            }
                        }
                        return Err(Error::LdkNode(err).into());
                    }
                };

                // Record the payment id so QuoteId lookups resolve to the
                // dispatched payment. Best-effort: if this write fails the
                // sentinel remains and the payment resolves as Pending, keeping
                // the melt proofs reserved.
                if let Err(err) =
                    write_bolt12_quote_payment_id(&self.kv_store, &quote_id, Some(&payment_id))
                        .await
                {
                    tracing::error!(
                        "Could not record BOLT12 payment id for quote {quote_id}: {err}. \
                         The payment will remain Pending until manual intervention."
                    );
                }

                // Check payment status for up to 10 seconds
                let start = std::time::Instant::now();
                let timeout = std::time::Duration::from_secs(10);

                let payment_details = loop {
                    let details = self
                        .inner
                        .payment(&payment_id)
                        .ok_or(Error::PaymentNotFound)?;

                    match details.status {
                        PaymentStatus::Succeeded => break details,
                        PaymentStatus::Failed => {
                            tracing::error!(
                                payment_id = %payment_id,
                                amount_msat = ?details.amount_msat,
                                fee_paid_msat = ?details.fee_paid_msat,
                                payment_kind = ?details.kind,
                                "Bolt12 payment failed"
                            );
                            break details;
                        }
                        PaymentStatus::Pending => {
                            if start.elapsed() > timeout {
                                tracing::warn!(
                                    "Payment has been being for 10 seconds. No longer waiting"
                                );
                                break details;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            continue;
                        }
                    }
                };

                Self::make_payment_response_from_details(
                    unit,
                    quote_payment_identifier,
                    &payment_details,
                )
            }
            OutgoingPaymentOptions::Onchain(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
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

    /// Is payment event stream active
    fn is_payment_event_stream_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel payment event stream
    fn cancel_payment_event_stream(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    /// Check the status of an incoming payment
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        // Bolt12 offers are identified by offer id and can be paid more than
        // once, so collect every settled inbound payment for the offer.
        if let PaymentIdentifier::OfferId(offer_id) = payment_identifier {
            let payments = self.inner.list_payments_with_filter(|p| {
                p.direction == PaymentDirection::Inbound
                    && p.status == PaymentStatus::Succeeded
                    && matches!(
                        &p.kind,
                        PaymentKind::Bolt12Offer { offer_id: oid, .. } if oid.to_string() == *offer_id
                    )
            });

            return Ok(payments
                .into_iter()
                .filter_map(|p| {
                    let payment_id = match &p.kind {
                        PaymentKind::Bolt12Offer {
                            hash: Some(hash), ..
                        } => hash.to_string(),
                        _ => {
                            tracing::warn!("Bolt12 payment for offer {} missing hash", offer_id);
                            return None;
                        }
                    };

                    Some(WaitPaymentResponse {
                        payment_identifier: payment_identifier.clone(),
                        payment_amount: Amount::new(p.amount_msat?, CurrencyUnit::Msat),
                        payment_id,
                    })
                })
                .collect());
        }

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
            payment_amount: Amount::new(amount, CurrencyUnit::Msat),
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
            PaymentIdentifier::PaymentHash(id_hash) => {
                Self::select_bolt11_payment_details(self.inner.list_payments_with_filter(|p| {
                    p.direction == PaymentDirection::Outbound
                        && matches!(&p.kind, PaymentKind::Bolt11 { hash, .. } if &hash.0 == id_hash)
                }))
            }
            PaymentIdentifier::PaymentId(id) => self.inner.payment(&PaymentId(*id)),
            PaymentIdentifier::QuoteId(quote_id) => {
                match read_bolt12_quote_payment_id(&self.kv_store, quote_id)
                    .await?
                    .resolve()
                {
                    Bolt12QuotePaymentIdResolution::PaymentId(payment_id) => {
                        self.inner.payment(&payment_id)
                    }
                    Bolt12QuotePaymentIdResolution::Status(status) => {
                        return Ok(MakePaymentResponse {
                            payment_lookup_id: request_lookup_id.clone(),
                            payment_proof: None,
                            status,
                            total_spent: Amount::new(0, CurrencyUnit::Msat),
                        });
                    }
                }
            }
            _ => {
                return Ok(MakePaymentResponse {
                    payment_lookup_id: request_lookup_id.clone(),
                    payment_proof: None,
                    status: MeltQuoteState::Unknown,
                    total_spent: Amount::new(0, CurrencyUnit::Msat),
                });
            }
        }
        .ok_or(Error::PaymentNotFound)?;

        if payment_details.direction != PaymentDirection::Outbound {
            return Err(Error::InvalidPaymentDirection.into());
        }

        Self::make_payment_response_from_details(
            &CurrencyUnit::Msat,
            request_lookup_id.clone(),
            &payment_details,
        )
    }
}

impl Drop for CdkLdkNode {
    fn drop(&mut self) {
        tracing::info!("Drop called on CdkLdkNode");
        self.wait_invoice_cancel_token.cancel();
        tracing::debug!("Cancelled wait_invoice token in drop");
    }
}

/// KV key for the bolt12 melt quote id -> payment id mapping
fn bolt12_quote_payment_id_key(quote_id: &QuoteId) -> Result<String, Error> {
    match quote_id {
        QuoteId::UUID(uuid) => Ok(uuid.to_string()),
        QuoteId::BASE64(_) => Err(Error::InvalidQuoteId),
    }
}

/// Records the bolt12 melt quote id -> payment id mapping.
///
/// `payment_id` of `None` writes the pre-dispatch sentinel: it marks that
/// `send` is about to be attempted so a crash during dispatch is
/// distinguishable from "never dispatched" (mirrors cdk-cln's pre-dispatch
/// bolt12 quote mapping).
async fn write_bolt12_quote_payment_id(
    kv_store: &DynKVStore,
    quote_id: &QuoteId,
    payment_id: Option<&PaymentId>,
) -> Result<(), Error> {
    let key = bolt12_quote_payment_id_key(quote_id)?;
    let value = payment_id.map(|id| hex::encode(id.0)).unwrap_or_default();
    let mut tx = kv_store
        .begin_transaction()
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

    tx.kv_write(
        LDK_KV_PRIMARY_NAMESPACE,
        LDK_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
        &key,
        value.as_bytes(),
    )
    .await
    .map_err(|e| Error::Database(e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

    Ok(())
}

/// Reads the bolt12 melt quote id -> payment id mapping
async fn read_bolt12_quote_payment_id(
    kv_store: &DynKVStore,
    quote_id: &QuoteId,
) -> Result<Bolt12QuotePaymentIdLookup, Error> {
    let key = bolt12_quote_payment_id_key(quote_id)?;
    let Some(stored) = kv_store
        .kv_read(
            LDK_KV_PRIMARY_NAMESPACE,
            LDK_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
            &key,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?
    else {
        return Ok(Bolt12QuotePaymentIdLookup::Missing);
    };

    if stored.is_empty() {
        return Ok(Bolt12QuotePaymentIdLookup::Dispatching);
    }

    let payment_id_hex = match String::from_utf8(stored) {
        Ok(payment_id_hex) => payment_id_hex,
        Err(err) => {
            tracing::warn!(
                "LDK: invalid UTF-8 in BOLT12 payment id mapping for quote {quote_id}: {err}"
            );
            return Ok(Bolt12QuotePaymentIdLookup::Malformed);
        }
    };

    let payment_id_bytes = match hex::decode(&payment_id_hex) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(
                "LDK: invalid hex in BOLT12 payment id mapping for quote {quote_id}: {err}"
            );
            return Ok(Bolt12QuotePaymentIdLookup::Malformed);
        }
    };

    let payment_id: [u8; 32] = match payment_id_bytes.try_into() {
        Ok(payment_id) => payment_id,
        Err(_) => {
            tracing::warn!("LDK: invalid payment id length in BOLT12 mapping for quote {quote_id}");
            return Ok(Bolt12QuotePaymentIdLookup::Malformed);
        }
    };

    Ok(Bolt12QuotePaymentIdLookup::Found(PaymentId(payment_id)))
}

/// Removes the bolt12 melt quote id -> payment id mapping
async fn delete_bolt12_quote_payment_id(
    kv_store: &DynKVStore,
    quote_id: &QuoteId,
) -> Result<(), Error> {
    let key = bolt12_quote_payment_id_key(quote_id)?;
    let mut tx = kv_store
        .begin_transaction()
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

    tx.kv_remove(
        LDK_KV_PRIMARY_NAMESPACE,
        LDK_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
        &key,
    )
    .await
    .map_err(|e| Error::Database(e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitcoin_rpc_debug_redacts_password() {
        let source = ChainSource::BitcoinRpc(BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: 8332,
            user: "rpc-user".to_string(),
            password: "rpc-password-secret".to_string(),
        });

        let debug = format!("{source:?}");

        assert!(debug.contains("127.0.0.1"));
        assert!(debug.contains("rpc-user"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("rpc-password-secret"));
    }

    #[test]
    fn chain_source_debug_redacts_url_credentials() {
        for source in [
            ChainSource::Esplora("https://esplora-user:esplora-secret@example.com/api".to_string()),
            ChainSource::Electrum(
                "ssl://electrum-user:electrum-secret@example.com:50002".to_string(),
            ),
        ] {
            let debug = format!("{source:?}");

            assert!(debug.contains("example.com"));
            assert!(!debug.contains("-user"));
            assert!(!debug.contains("-secret"));
        }
    }

    #[test]
    fn gossip_source_debug_redacts_url_credentials() {
        let source = GossipSource::RapidGossipSync(
            "https://rgs-user:rgs-secret@example.com/snapshot".to_string(),
        );

        let debug = format!("{source:?}");

        assert!(debug.contains("https://example.com/snapshot"));
        assert!(!debug.contains("rgs-user"));
        assert!(!debug.contains("rgs-secret"));
    }

    fn test_payment_details(status: PaymentStatus, amount_msat: Option<u64>) -> PaymentDetails {
        PaymentDetails {
            id: PaymentId([2; 32]),
            kind: PaymentKind::Bolt11 {
                hash: PaymentHash([1; 32]),
                preimage: None,
                secret: None,
            },
            amount_msat,
            fee_paid_msat: None,
            direction: PaymentDirection::Outbound,
            status,
            latest_update_timestamp: 0,
        }
    }

    fn test_payment_details_with_id(
        id: [u8; 32],
        status: PaymentStatus,
        latest_update_timestamp: u64,
    ) -> PaymentDetails {
        PaymentDetails {
            id: PaymentId(id),
            latest_update_timestamp,
            ..test_payment_details(status, None)
        }
    }

    #[test]
    fn failed_payment_response_does_not_require_amount() {
        let details = test_payment_details(PaymentStatus::Failed, None);

        let response = CdkLdkNode::make_payment_response_from_details(
            &CurrencyUnit::Msat,
            PaymentIdentifier::PaymentId([2; 32]),
            &details,
        )
        .expect("failed payment details should map without amount");

        assert_eq!(response.status, MeltQuoteState::Failed);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Msat));
    }

    #[test]
    fn pending_payment_response_does_not_require_amount() {
        let details = test_payment_details(PaymentStatus::Pending, None);

        let response = CdkLdkNode::make_payment_response_from_details(
            &CurrencyUnit::Msat,
            PaymentIdentifier::PaymentId([2; 32]),
            &details,
        )
        .expect("pending payment details should map without amount");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Msat));
    }

    #[test]
    fn paid_payment_response_requires_amount() {
        let details = test_payment_details(PaymentStatus::Succeeded, None);

        let err = CdkLdkNode::make_payment_response_from_details(
            &CurrencyUnit::Msat,
            PaymentIdentifier::PaymentId([2; 32]),
            &details,
        )
        .expect_err("paid payment details without amount should fail");

        assert!(matches!(err, payment::Error::Lightning(_)));
    }

    #[test]
    fn bolt11_payment_selection_prefers_pending_over_failed() {
        let failed = test_payment_details_with_id([1; 32], PaymentStatus::Failed, 2);
        let pending = test_payment_details_with_id([2; 32], PaymentStatus::Pending, 1);

        let selected = CdkLdkNode::select_bolt11_payment_details([failed, pending])
            .expect("payment details should be selected");

        assert_eq!(selected.id, PaymentId([2; 32]));
        assert_eq!(selected.status, PaymentStatus::Pending);
    }

    #[test]
    fn bolt11_payment_selection_prefers_succeeded_over_pending() {
        let pending = test_payment_details_with_id([1; 32], PaymentStatus::Pending, 2);
        let succeeded = PaymentDetails {
            amount_msat: Some(1000),
            ..test_payment_details_with_id([2; 32], PaymentStatus::Succeeded, 1)
        };

        let selected = CdkLdkNode::select_bolt11_payment_details([pending, succeeded])
            .expect("payment details should be selected");

        assert_eq!(selected.id, PaymentId([2; 32]));
        assert_eq!(selected.status, PaymentStatus::Succeeded);
    }

    #[test]
    fn bolt11_payment_selection_uses_latest_failed_when_all_failed() {
        let older_failed = test_payment_details_with_id([1; 32], PaymentStatus::Failed, 1);
        let newer_failed = test_payment_details_with_id([2; 32], PaymentStatus::Failed, 2);

        let selected = CdkLdkNode::select_bolt11_payment_details([older_failed, newer_failed])
            .expect("payment details should be selected");

        assert_eq!(selected.id, PaymentId([2; 32]));
        assert_eq!(selected.status, PaymentStatus::Failed);
    }

    #[test]
    fn bolt12_persistence_failure_has_ambiguous_dispatch() {
        assert!(bolt12_send_error_has_ambiguous_dispatch(
            &ldk_node::NodeError::PersistenceFailed
        ));

        for not_dispatched in [
            ldk_node::NodeError::NotRunning,
            ldk_node::NodeError::UnsupportedCurrency,
            ldk_node::NodeError::InvalidOffer,
            ldk_node::NodeError::InvalidAmount,
            ldk_node::NodeError::DuplicatePayment,
            ldk_node::NodeError::InvoiceRequestCreationFailed,
            ldk_node::NodeError::PaymentSendingFailed,
        ] {
            assert!(
                !bolt12_send_error_has_ambiguous_dispatch(&not_dispatched),
                "{not_dispatched} must be treated as not dispatched"
            );
        }
    }

    #[test]
    fn bolt12_quote_payment_id_lookup_resolution_is_safe() {
        assert_eq!(
            Bolt12QuotePaymentIdLookup::Dispatching.resolve(),
            Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Pending),
            "an indeterminate dispatch must keep melt proofs reserved"
        );
        assert_eq!(
            Bolt12QuotePaymentIdLookup::Missing.resolve(),
            Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Unpaid),
            "a missing sentinel means the payment was never dispatched"
        );
        assert_eq!(
            Bolt12QuotePaymentIdLookup::Malformed.resolve(),
            Bolt12QuotePaymentIdResolution::Status(MeltQuoteState::Unknown),
            "corrupt bookkeeping must remain indeterminate"
        );
    }

    async fn test_kv_store() -> DynKVStore {
        std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap())
    }

    /// The mapping must resolve Missing before any dispatch, Found after the
    /// payment id is recorded, and Dispatching (indeterminate) while only the
    /// pre-dispatch sentinel exists.
    #[tokio::test]
    async fn bolt12_quote_payment_id_mapping_lifecycle() {
        let kv_store = test_kv_store().await;
        let quote_id = QuoteId::new();

        assert_eq!(
            read_bolt12_quote_payment_id(&kv_store, &quote_id)
                .await
                .unwrap(),
            Bolt12QuotePaymentIdLookup::Missing,
            "no record must resolve as never dispatched"
        );

        // Pre-dispatch sentinel
        write_bolt12_quote_payment_id(&kv_store, &quote_id, None)
            .await
            .unwrap();
        assert_eq!(
            read_bolt12_quote_payment_id(&kv_store, &quote_id)
                .await
                .unwrap(),
            Bolt12QuotePaymentIdLookup::Dispatching,
            "sentinel must resolve as indeterminate, never terminal"
        );

        // Record the payment id
        let payment_id = PaymentId([7; 32]);
        write_bolt12_quote_payment_id(&kv_store, &quote_id, Some(&payment_id))
            .await
            .unwrap();
        assert_eq!(
            read_bolt12_quote_payment_id(&kv_store, &quote_id)
                .await
                .unwrap(),
            Bolt12QuotePaymentIdLookup::Found(payment_id)
        );

        // Removal returns to Missing (failed dispatch cleanup)
        delete_bolt12_quote_payment_id(&kv_store, &quote_id)
            .await
            .unwrap();
        assert_eq!(
            read_bolt12_quote_payment_id(&kv_store, &quote_id)
                .await
                .unwrap(),
            Bolt12QuotePaymentIdLookup::Missing
        );
    }

    /// A corrupted mapping must resolve as indeterminate (`Malformed`), never
    /// as a terminal state that could trigger compensation.
    #[tokio::test]
    async fn bolt12_quote_payment_id_mapping_malformed_is_indeterminate() {
        let kv_store = test_kv_store().await;
        let quote_id = QuoteId::new();
        let key = bolt12_quote_payment_id_key(&quote_id).unwrap();

        for corrupt in ["not-hex", "0102", "zz"] {
            let mut tx = kv_store.begin_transaction().await.unwrap();
            tx.kv_write(
                LDK_KV_PRIMARY_NAMESPACE,
                LDK_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
                &key,
                corrupt.as_bytes(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            assert_eq!(
                read_bolt12_quote_payment_id(&kv_store, &quote_id)
                    .await
                    .unwrap(),
                Bolt12QuotePaymentIdLookup::Malformed,
                "corrupt value {corrupt} must be indeterminate"
            );
        }
    }
}
