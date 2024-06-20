use std::{
    any::type_name,
    collections::HashMap,
    fmt::Debug,
    fs,
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bitcoin::{hashes::Hash, secp256k1::PublicKey, BlockHash, Network, ScriptBuf, Transaction};
use cdk::{
    cdk_lightning::{
        self, Amount, BalanceResponse, InvoiceInfo, MintLightning, PayInvoiceResponse,
    },
    lightning_invoice::{
        payment::payment_parameters_from_invoice, utils::create_invoice_from_channelmanager,
    },
    secp256k1::rand::random,
    types::InvoiceStatus,
    util::{hex, unix_time},
    Bolt11Invoice, Sha256,
};
use chrono::{DateTime, Utc};
use futures::Stream;
use lightning::{
    chain::{chainmonitor::ChainMonitor, ChannelMonitorUpdateStatus, Filter, Listen, Watch},
    events::{Event, PaymentPurpose},
    ln::{
        channelmanager::{
            ChainParameters, ChannelManager, ChannelManagerReadArgs, PaymentId, Retry,
        },
        peer_handler::{IgnoringMessageHandler, MessageHandler, PeerManager},
        ChannelId, PaymentHash, PaymentPreimage,
    },
    onion_message::messenger::OnionMessenger,
    routing::{
        gossip::{NetworkGraph, P2PGossipSync},
        router::DefaultRouter,
        scoring::{
            ProbabilisticScorer, ProbabilisticScoringDecayParameters,
            ProbabilisticScoringFeeParameters,
        },
    },
    sign::{EntropySource, InMemorySigner, KeysManager, NodeSigner, Recipient},
    util::{
        config::UserConfig,
        logger::{Level, Logger, Record},
        persist::{
            read_channel_monitors, KVStore, CHANNEL_MANAGER_PERSISTENCE_KEY,
            CHANNEL_MANAGER_PERSISTENCE_PRIMARY_NAMESPACE,
            CHANNEL_MANAGER_PERSISTENCE_SECONDARY_NAMESPACE, NETWORK_GRAPH_PERSISTENCE_KEY,
            NETWORK_GRAPH_PERSISTENCE_PRIMARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_SECONDARY_NAMESPACE, SCORER_PERSISTENCE_KEY,
            SCORER_PERSISTENCE_PRIMARY_NAMESPACE, SCORER_PERSISTENCE_SECONDARY_NAMESPACE,
        },
        ser::{MaybeReadable, ReadableArgs, Writeable},
    },
};
use lightning_background_processor::{process_events_async, GossipSync};
use lightning_block_sync::{
    gossip::GossipVerifier,
    init::{synchronize_listeners, validate_best_block_header},
    poll::ChainPoller,
    SpvClient, UnboundedCache,
};
use lightning_persister::fs_store::FilesystemStore;
use redb::{Database, ReadableTable, TableDefinition, TypeName, Value};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{BitcoinClient, Error};

const BLOCK_TIMER: u64 = 10;
const DB_FILE: &str = "db";
const NETWORK_DIR: &str = "network";

type NodeChainMonitor = ChainMonitor<
    InMemorySigner,
    Box<dyn Filter + Send + Sync>,
    Arc<BitcoinClient>,
    Arc<BitcoinClient>,
    Arc<NodeLogger>,
    Arc<FilesystemStore>,
>;

type NodeChannelManager = ChannelManager<
    Arc<NodeChainMonitor>,
    Arc<BitcoinClient>,
    Arc<KeysManager>,
    Arc<KeysManager>,
    Arc<KeysManager>,
    Arc<BitcoinClient>,
    Arc<NodeRouter>,
    Arc<NodeLogger>,
>;

type NodeGossipVerifier =
    GossipVerifier<lightning_block_sync::gossip::TokioSpawner, Arc<BitcoinClient>, Arc<NodeLogger>>;

type NodeNetworkGraph = NetworkGraph<Arc<NodeLogger>>;

type NodeOnionMessenger = OnionMessenger<
    Arc<KeysManager>,
    Arc<KeysManager>,
    Arc<NodeLogger>,
    Arc<NodeChannelManager>,
    Arc<NodeRouter>,
    IgnoringMessageHandler,
    IgnoringMessageHandler,
>;

type NodeP2PSync = P2PGossipSync<Arc<NodeNetworkGraph>, NodeGossipVerifier, Arc<NodeLogger>>;

type NodePeerManager = PeerManager<
    lightning_net_tokio::SocketDescriptor,
    Arc<NodeChannelManager>,
    Arc<NodeP2PSync>,
    Arc<NodeOnionMessenger>,
    Arc<NodeLogger>,
    IgnoringMessageHandler,
    Arc<KeysManager>,
>;

type NodeRouter = DefaultRouter<
    Arc<NodeNetworkGraph>,
    Arc<NodeLogger>,
    Arc<KeysManager>,
    Arc<std::sync::RwLock<NodeScorer>>,
    ProbabilisticScoringFeeParameters,
    ProbabilisticScorer<Arc<NodeNetworkGraph>, Arc<NodeLogger>>,
>;

type NodeScorer = ProbabilisticScorer<Arc<NodeNetworkGraph>, Arc<NodeLogger>>;

#[derive(Clone)]
pub struct Node {
    cancel_token: CancellationToken,
    chain_monitor: Arc<NodeChainMonitor>,
    channel_manager: Arc<NodeChannelManager>,
    db: NodeDatabase,
    gossip_sync: Arc<NodeP2PSync>,
    keys_manager: Arc<KeysManager>,
    logger: Arc<NodeLogger>,
    network: Network,
    peer_manager: Arc<NodePeerManager>,
    persister: Arc<FilesystemStore>,
    scorer: Arc<std::sync::RwLock<NodeScorer>>,

    inflight_payments: Arc<RwLock<HashMap<PaymentHash, oneshot::Sender<Payment>>>>,
    opened_channel_ids: Arc<Mutex<HashMap<ChannelId, oneshot::Sender<ChannelId>>>>,
    paid_invoices: broadcast::Sender<PaymentHash>,
    pending_channel_scripts: Arc<Mutex<HashMap<ChannelId, oneshot::Sender<ScriptBuf>>>>,
}

impl Node {
    pub async fn start(
        data_dir: PathBuf,
        network: Network,
        rpc_client: BitcoinClient,
        seed: [u8; 32],
        p2p_addr: Option<SocketAddr>,
    ) -> Result<Self, Error> {
        // Create utils
        let bitcoin_client = Arc::new(rpc_client);
        fs::create_dir_all(data_dir.join(NETWORK_DIR))?;
        let db = NodeDatabase::open(data_dir.join(DB_FILE))?;
        let logger = Arc::new(NodeLogger);
        let persister = Arc::new(FilesystemStore::new(data_dir.join(NETWORK_DIR)));

        // Derive keys manager
        let starting_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        let keys_manager = Arc::new(KeysManager::new(
            &seed,
            starting_time.as_secs(),
            starting_time.subsec_nanos(),
        ));
        tracing::info!(
            "Starting node {}",
            keys_manager.get_node_id(Recipient::Node).unwrap()
        );

        // Setup chain monitor
        let chain_monitor = Arc::new(ChainMonitor::new(
            None,
            bitcoin_client.clone(),
            logger.clone(),
            bitcoin_client.clone(),
            persister.clone(),
        ));
        let polled_chain_tip = validate_best_block_header(bitcoin_client.clone()).await?;
        tracing::debug!("Polled chain tip: {:?}", polled_chain_tip);

        // Configure router
        let network_graph_bytes = match persister.read(
            NETWORK_GRAPH_PERSISTENCE_PRIMARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_SECONDARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_KEY,
        ) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("Error reading network graph: {:?}", e);
                Vec::new()
            }
        };
        let network_graph = Arc::new(if network_graph_bytes.is_empty() {
            tracing::info!("Creating new network graph");
            NetworkGraph::new(network, logger.clone())
        } else {
            tracing::info!("Loading network graph");
            NetworkGraph::read(&mut &network_graph_bytes[..], logger.clone())?
        });
        let scorer_bytes = match persister.read(
            SCORER_PERSISTENCE_PRIMARY_NAMESPACE,
            SCORER_PERSISTENCE_SECONDARY_NAMESPACE,
            SCORER_PERSISTENCE_KEY,
        ) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("Error reading scorer: {:?}", e);
                Vec::new()
            }
        };
        let scorer = if scorer_bytes.is_empty() {
            tracing::info!("Creating new scorer");
            Arc::new(std::sync::RwLock::new(ProbabilisticScorer::new(
                ProbabilisticScoringDecayParameters::default(),
                network_graph.clone(),
                logger.clone(),
            )))
        } else {
            tracing::info!("Loading scorer");
            Arc::new(std::sync::RwLock::new(ProbabilisticScorer::read(
                &mut &scorer_bytes[..],
                (
                    ProbabilisticScoringDecayParameters::default(),
                    network_graph.clone(),
                    logger.clone(),
                ),
            )?))
        };
        let router = Arc::new(DefaultRouter::new(
            network_graph.clone(),
            logger.clone(),
            keys_manager.clone(),
            scorer.clone(),
            ProbabilisticScoringFeeParameters::default(),
        ));

        // Load channel manager
        let mut channel_monitors = read_channel_monitors(
            persister.clone(),
            keys_manager.clone(),
            keys_manager.clone(),
        )?;
        let mut restarting_node = true;
        let (channel_manager_blockhash, channel_manager) = {
            if let Ok(data) = persister.read(
                CHANNEL_MANAGER_PERSISTENCE_PRIMARY_NAMESPACE,
                CHANNEL_MANAGER_PERSISTENCE_SECONDARY_NAMESPACE,
                CHANNEL_MANAGER_PERSISTENCE_KEY,
            ) {
                tracing::info!("Restarting node");
                let mut channel_monitor_mut_references = Vec::new();
                for (_, channel_monitor) in channel_monitors.iter_mut() {
                    channel_monitor_mut_references.push(channel_monitor);
                }
                let read_args = ChannelManagerReadArgs::new(
                    keys_manager.clone(),
                    keys_manager.clone(),
                    keys_manager.clone(),
                    bitcoin_client.clone(),
                    chain_monitor.clone(),
                    bitcoin_client.clone(),
                    router.clone(),
                    logger.clone(),
                    UserConfig::default(),
                    channel_monitor_mut_references,
                );
                <(BlockHash, NodeChannelManager)>::read(&mut &data[..], read_args)?
            } else {
                tracing::info!("Starting fresh node");
                // We're starting a fresh node.
                restarting_node = false;

                let polled_best_block = polled_chain_tip.to_best_block();
                let polled_best_block_hash = polled_best_block.block_hash;
                let chain_params = ChainParameters {
                    network,
                    best_block: polled_best_block,
                };
                let fresh_channel_manager = ChannelManager::new(
                    bitcoin_client.clone(),
                    chain_monitor.clone(),
                    bitcoin_client.clone(),
                    router.clone(),
                    logger.clone(),
                    keys_manager.clone(),
                    keys_manager.clone(),
                    keys_manager.clone(),
                    UserConfig::default(),
                    chain_params,
                    starting_time.as_secs() as u32,
                );
                (polled_best_block_hash, fresh_channel_manager)
            }
        };
        tracing::info!("Channel manager blockhash: {}", channel_manager_blockhash);

        // Sync ChannelMonitors and ChannelManager to chain tip
        let mut chain_listener_channel_monitors = Vec::new();
        let mut cache = UnboundedCache::new();
        let chain_tip = if restarting_node {
            let mut chain_listeners = vec![(
                channel_manager_blockhash,
                &channel_manager as &(dyn Listen + Send + Sync),
            )];

            for (blockhash, channel_monitor) in channel_monitors.drain(..) {
                let outpoint = channel_monitor.get_funding_txo().0;
                chain_listener_channel_monitors.push((
                    blockhash,
                    (
                        channel_monitor,
                        bitcoin_client.clone(),
                        bitcoin_client.clone(),
                        logger.clone(),
                    ),
                    outpoint,
                ));
            }

            for monitor_listener_info in chain_listener_channel_monitors.iter_mut() {
                chain_listeners.push((
                    monitor_listener_info.0,
                    &monitor_listener_info.1 as &(dyn Listen + Send + Sync),
                ));
            }

            synchronize_listeners(bitcoin_client.clone(), network, &mut cache, chain_listeners)
                .await?
        } else {
            polled_chain_tip
        };
        tracing::debug!("Chain tip: {:?}", chain_tip);

        for item in chain_listener_channel_monitors.drain(..) {
            let channel_monitor = item.1 .0;
            let funding_outpoint = item.2;
            assert_eq!(
                chain_monitor.watch_channel(funding_outpoint, channel_monitor),
                Ok(ChannelMonitorUpdateStatus::Completed)
            );
        }

        // Connect and disconnect blocks
        let cancel_token = CancellationToken::new();
        let cancel_token_listener = cancel_token.clone();
        let channel_manager = Arc::new(channel_manager);
        let channel_manager_listener = channel_manager.clone();
        let chain_monitor_listener = chain_monitor.clone();
        let bitcoind_block_source = bitcoin_client.clone();
        tokio::spawn(async move {
            let chain_poller = ChainPoller::new(bitcoind_block_source.as_ref(), network);
            let chain_listener = (chain_monitor_listener, channel_manager_listener);
            let mut spv_client =
                SpvClient::new(chain_tip, chain_poller, &mut cache, &chain_listener);
            loop {
                tracing::trace!("Polling best tip");
                if let Err(e) = spv_client.poll_best_tip().await {
                    tracing::error!("Error polling best tip: {:?}", e);
                };
                tokio::select! {
                    _ = cancel_token_listener.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(BLOCK_TIMER)) => {}
                };
            }
        });

        // Setup peer manager
        let gossip_sync: Arc<NodeP2PSync> = Arc::new(P2PGossipSync::new(
            network_graph.clone(),
            None,
            logger.clone(),
        ));
        let peer_manager = Arc::new(PeerManager::new(
            MessageHandler {
                chan_handler: channel_manager.clone(),
                route_handler: gossip_sync.clone(),
                onion_message_handler: Arc::new(OnionMessenger::new(
                    keys_manager.clone(),
                    keys_manager.clone(),
                    logger.clone(),
                    channel_manager.clone(),
                    router.clone(),
                    IgnoringMessageHandler {},
                    IgnoringMessageHandler {},
                )),
                custom_message_handler: IgnoringMessageHandler {},
            },
            unix_time() as u32,
            &keys_manager.get_secure_random_bytes(),
            logger.clone(),
            keys_manager.clone(),
        ));

        // Listen for incoming connections
        if let Some(p2p_addr) = p2p_addr {
            tracing::info!("Listening for incoming connections on {}", p2p_addr);
            let listener = tokio::net::TcpListener::bind(p2p_addr).await?;
            let peer_manager_listener = peer_manager.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((tcp_stream, peer_addr)) => {
                            tracing::info!("Accepted connection from {}", peer_addr);
                            lightning_net_tokio::setup_inbound(
                                peer_manager_listener.clone(),
                                tcp_stream.into_std().unwrap(),
                            )
                            .await;
                        }
                        Err(e) => tracing::error!("Error accepting connection: {:?}", e),
                    }
                }
            });
        }

        let (paid_invoices, _) = broadcast::channel(100);
        let node = Self {
            cancel_token,
            chain_monitor,
            channel_manager,
            db,
            gossip_sync,
            keys_manager,
            logger,
            network,
            peer_manager,
            persister,
            scorer,

            inflight_payments: Arc::new(RwLock::new(HashMap::new())),
            opened_channel_ids: Arc::new(Mutex::new(HashMap::new())),
            paid_invoices,
            pending_channel_scripts: Arc::new(Mutex::new(HashMap::new())),
        };
        node.start_background_processor();
        Ok(node)
    }

    fn start_background_processor(&self) {
        tracing::info!("Starting background processor");
        let cancel_token = self.cancel_token.clone();
        let sleeper = move |d| {
            let cancel_token = cancel_token.clone();
            Box::pin(async move {
                tokio::select! {
                    _ = tokio::time::sleep(d) => false,
                    _ = cancel_token.cancelled() => true,
                }
            })
        };
        let persister = self.persister.clone();
        let chain_monitor = self.chain_monitor.clone();
        let channel_manager = self.channel_manager.clone();
        let gossip_sync = self.gossip_sync.clone();
        let peer_manager = self.peer_manager.clone();
        let logger = self.logger.clone();
        let scorer = self.scorer.clone();
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(e) = process_events_async(
                persister,
                |e| async { self_clone.handle_event(e).await },
                chain_monitor,
                channel_manager,
                GossipSync::p2p(gossip_sync),
                peer_manager,
                logger,
                Some(scorer),
                sleeper,
                false,
                || {
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .ok()
                },
            )
            .await
            {
                tracing::error!("Error processing events: {:?}", e);
            };
        });
    }

    async fn handle_event(&self, event: Event) {
        tracing::debug!("Handling event: {:?}", event);
        if let Err(e) = self.db.save_event(event.clone()).await {
            tracing::warn!("Error saving event: {:?}", e);
        }
        match event {
            Event::FundingGenerationReady {
                temporary_channel_id,
                output_script,
                ..
            } => {
                let mut pending_channel_scripts = self.pending_channel_scripts.lock().await;
                if let Some(tx) = pending_channel_scripts.remove(&temporary_channel_id) {
                    let _ = tx.send(output_script);
                }
            }
            Event::PaymentClaimable { purpose, .. } => {
                let payment_preimage = match purpose {
                    PaymentPurpose::Bolt11InvoicePayment {
                        payment_preimage, ..
                    } => payment_preimage,
                    PaymentPurpose::Bolt12OfferPayment {
                        payment_preimage, ..
                    } => payment_preimage,
                    PaymentPurpose::Bolt12RefundPayment {
                        payment_preimage, ..
                    } => payment_preimage,
                    PaymentPurpose::SpontaneousPayment(preimage) => Some(preimage),
                };
                if let Some(payment_preimage) = payment_preimage {
                    self.channel_manager.claim_funds(payment_preimage);
                }
            }
            Event::PaymentClaimed {
                payment_hash,
                amount_msat,
                ..
            } => {
                tracing::info!("Payment claimed: {:?} {} msat", payment_hash, amount_msat);
                if let Err(e) = self.db.update_paid_invoice(payment_hash).await {
                    tracing::warn!("Error updating invoice: {:?}", e);
                }
                let _ = self.paid_invoices.send(payment_hash);
            }
            Event::PaymentSent {
                payment_hash,
                payment_preimage,
                fee_paid_msat,
                ..
            } => {
                let payment = match self
                    .db
                    .update_payment(
                        payment_hash,
                        payment_preimage,
                        Amount::from_msat(fee_paid_msat.unwrap_or_default()),
                    )
                    .await
                {
                    Ok(payment) => payment,
                    Err(e) => {
                        tracing::error!("Error updating payment: {:?}", e);
                        None
                    }
                };
                if let Some(payment) = payment {
                    let _ = self
                        .inflight_payments
                        .write()
                        .await
                        .remove(&PaymentHash(payment_hash.0))
                        .map(|tx| {
                            let _ = tx.send(payment);
                        });
                } else {
                    tracing::warn!("Payment not found: {}", payment_hash);
                }
            }
            Event::PaymentFailed {
                payment_hash,
                reason,
                ..
            } => {
                tracing::warn!("Payment failed: {:?} {:?}", payment_hash, reason);
                match self.db.get_payment(PaymentHash(payment_hash.0)).await {
                    Ok(payment) => {
                        if let Some(payment) = payment {
                            let _ = self
                                .inflight_payments
                                .write()
                                .await
                                .remove(&PaymentHash(payment_hash.0))
                                .map(|tx| {
                                    let _ = tx.send(payment);
                                });
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Error getting payment: {:?}", e);
                    }
                };
            }
            Event::PendingHTLCsForwardable { time_forwardable } => {
                tokio::time::sleep(time_forwardable).await;
                self.channel_manager.process_pending_htlc_forwards();
            }
            _ => tracing::warn!("Unhandled event: {:?}", event),
        }
    }

    pub async fn get_info(&self) -> Result<NodeInfo, Error> {
        let balance_msat: u64 = self
            .channel_manager
            .list_channels()
            .iter()
            .map(|c| c.balance_msat)
            .sum();
        Ok(NodeInfo {
            node_id: self.keys_manager.get_node_id(Recipient::Node).unwrap(),
            balance: Amount::from_msat(balance_msat),
        })
    }

    pub async fn connect_peer(&self, node_id: PublicKey, addr: SocketAddr) -> Result<(), Error> {
        match lightning_net_tokio::connect_outbound(self.peer_manager.clone(), node_id, addr).await
        {
            Some(_) => Ok(()),
            None => Err(Error::Ldk("Failed to connect peer".to_string())),
        }
    }

    pub async fn open_channel(
        &self,
        node_id: PublicKey,
        amount: Amount,
    ) -> Result<PendingChannel, Error> {
        let mut pending_channel_scripts = self.pending_channel_scripts.lock().await;
        let (tx, rx) = oneshot::channel();
        let channel_id = self
            .channel_manager
            .create_channel(node_id, amount.to_sat(), 0, random(), None, None)
            .map_err(|e| Error::Ldk(format!("{:?}", e)))?;
        pending_channel_scripts.insert(channel_id, tx);
        drop(pending_channel_scripts);

        self.db
            .insert_temp_channel(
                channel_id,
                Channel {
                    node_id,
                    amount,
                    is_claimed: false,
                },
            )
            .await?;

        let funding_script = rx
            .await
            .map_err(|_| Error::Ldk("Channel open timed out".to_string()))?;
        Ok(PendingChannel {
            channel_id,
            funding_script,
        })
    }

    pub async fn fund_channel(
        &self,
        node_id: PublicKey,
        channel_id: ChannelId,
        funding_tx: Transaction,
    ) -> Result<ChannelId, Error> {
        let (tx, rx) = oneshot::channel();
        let mut opened_channel_ids = self.opened_channel_ids.lock().await;
        opened_channel_ids.insert(channel_id, tx);
        drop(opened_channel_ids);

        self.channel_manager
            .funding_transaction_generated(&channel_id, &node_id, funding_tx)
            .map_err(|e| Error::Ldk(format!("{:?}", e)))?;

        let new_channel_id = rx
            .await
            .map_err(|_| Error::Ldk("Channel funding timed out".to_string()))?;
        self.db
            .update_channel_id(channel_id, new_channel_id)
            .await?;
        Ok(new_channel_id)
    }

    pub async fn claim_channel(&self, channel_id: ChannelId) -> Result<(), Error> {
        if !self.is_channel_ready(channel_id) {
            return Err(Error::ChannelNotReady);
        }
        self.db.update_channel_claimed(channel_id).await?;
        Ok(())
    }

    pub fn is_channel_ready(&self, channel_id: ChannelId) -> bool {
        self.channel_manager
            .list_channels()
            .iter()
            .find(|c| c.channel_id == channel_id)
            .map_or(false, |c| c.is_channel_ready)
    }

    pub async fn is_channel_claimed(&self, channel_id: ChannelId) -> Result<bool, Error> {
        let channel = self.db.get_channel(channel_id).await?;
        Ok(channel.map_or(false, |c| c.is_claimed))
    }

    pub async fn get_events(
        &self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<(DateTime<Utc>, Event)>, Error> {
        self.db.get_events(start, end).await
    }

    pub fn stop(&self) {
        self.cancel_token.cancel();
    }
}

pub struct NodeInfo {
    pub node_id: PublicKey,
    pub balance: Amount,
}

pub struct PendingChannel {
    pub channel_id: ChannelId,
    pub funding_script: ScriptBuf,
}

#[async_trait]
impl MintLightning for Node {
    type Err = cdk_lightning::Error;

    async fn get_invoice(
        &self,
        amount: Amount,
        hash: &str,
        description: &str,
    ) -> Result<InvoiceInfo, Self::Err> {
        let invoice = create_invoice_from_channelmanager(
            &self.channel_manager,
            self.keys_manager.clone(),
            self.logger.clone(),
            self.network.into(),
            Some(amount.to_msat()),
            description.to_string(),
            3600,
            None,
        )
        .map_err(map_err)?;
        let payment_hash =
            Sha256::from_str(&invoice.payment_hash().to_string()).map_err(map_err)?;
        self.db.insert_invoice(&invoice).await.map_err(map_err)?;
        Ok(InvoiceInfo {
            payment_hash: payment_hash.to_string(),
            hash: hash.to_string(),
            invoice,
            amount,
            status: InvoiceStatus::Unpaid,
            memo: description.to_string(),
            confirmed_at: None,
        })
    }

    async fn wait_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = (Bolt11Invoice, Option<u64>)> + Send>>, Self::Err> {
        let mut rx = self.paid_invoices.subscribe();
        let db = self.db.clone();
        Ok(Box::pin(async_stream::stream! {
            while let Ok(payment_hash) = rx.recv().await {
                if let Ok(Some(invoice)) = db.get_invoice(payment_hash).await {
                    if let Ok(invoice) = Bolt11Invoice::from_str(&invoice.bolt_11) {
                        yield (invoice, None);
                    }
                }
            }
        }))
    }

    async fn check_invoice_status(
        &self,
        payment_hash: &cdk::Sha256,
    ) -> Result<InvoiceStatus, Self::Err> {
        let payment_hash = PaymentHash(payment_hash.to_byte_array());
        let inflight_payments = self.inflight_payments.read().await;
        if inflight_payments.contains_key(&payment_hash) {
            return Ok(InvoiceStatus::InFlight);
        }
        let payment = self.db.get_payment(payment_hash).await.map_err(map_err)?;
        Ok(payment.map_or(InvoiceStatus::Unpaid, |payment| {
            if payment.paid {
                InvoiceStatus::Paid
            } else {
                InvoiceStatus::Unpaid
            }
        }))
    }

    async fn pay_invoice(
        &self,
        bolt11: Bolt11Invoice,
        partial_msat: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let amount_msat = partial_msat
            .map(|f| f.to_msat())
            .or(bolt11.amount_milli_satoshis())
            .ok_or(map_err("No amount"))?;
        let (payment_hash, recipient_onion, mut route_params) =
            payment_parameters_from_invoice(&bolt11)
                .map_err(|_| map_err("Error extracting payment parameters"))?;
        self.db
            .insert_payment(&bolt11, Amount::from_msat(amount_msat))
            .await
            .map_err(map_err)?;
        let (tx, rx) = oneshot::channel();
        let mut inflight_payments = self.inflight_payments.write().await;
        inflight_payments.insert(payment_hash, tx);
        drop(inflight_payments);

        route_params.final_value_msat = amount_msat;
        route_params.max_total_routing_fee_msat = max_fee.map(|f| f.to_msat());
        self.channel_manager
            .send_payment(
                payment_hash,
                recipient_onion,
                PaymentId(payment_hash.0),
                route_params,
                Retry::Timeout(Duration::from_secs(60)),
            )
            .map_err(|e| map_err(format!("{:?}", e)))?;

        let payment = rx.await.map_err(map_err)?;
        Ok(PayInvoiceResponse {
            payment_hash: Sha256::from_byte_array(payment_hash.0),
            payment_preimage: payment.pre_image.map(|p| hex::encode(p)),
            status: if payment.pre_image.is_some() {
                InvoiceStatus::Paid
            } else {
                InvoiceStatus::Unpaid
            },
            total_spent: payment.spent_msat,
        })
    }

    async fn get_balance(&self) -> Result<BalanceResponse, Self::Err> {
        let balance_msat: u64 = self
            .channel_manager
            .list_channels()
            .iter()
            .map(|c| c.balance_msat)
            .sum();
        Ok(BalanceResponse {
            on_chain_spendable: Amount::ZERO,
            on_chain_total: Amount::ZERO,
            ln: Amount::from_sat(balance_msat / 1000),
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        description: String,
        unix_expiry: u64,
    ) -> Result<Bolt11Invoice, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let invoice = create_invoice_from_channelmanager(
            &self.channel_manager,
            self.keys_manager.clone(),
            self.logger.clone(),
            self.network.into(),
            Some(amount.to_msat()),
            description,
            (unix_expiry - time_now) as u32,
            None,
        )
        .map_err(map_err)?;
        self.db.insert_invoice(&invoice).await.map_err(map_err)?;
        Ok(invoice)
    }
}

fn map_err<E: ToString>(e: E) -> cdk_lightning::Error {
    cdk_lightning::Error::Lightning(Box::new(Error::Ldk(e.to_string())))
}

const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const CHANNELS_TABLE: TableDefinition<[u8; 32], Bincode<Channel>> =
    TableDefinition::new("channels");
const EVENTS_TABLE: TableDefinition<u128, Vec<u8>> = TableDefinition::new("events");
const INVOICES_TABLE: TableDefinition<[u8; 32], Bincode<Invoice>> =
    TableDefinition::new("invoices");
const PAYMENTS_TABLE: TableDefinition<[u8; 32], Bincode<Payment>> =
    TableDefinition::new("payments");

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Channel {
    node_id: PublicKey,
    amount: Amount,
    is_claimed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Invoice {
    bolt_11: String,
    expiry: u64,
    paid: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Payment {
    bolt_11: String,
    amount: Amount,
    paid: bool,
    spent_msat: Amount,
    pre_image: Option<[u8; 32]>,
}

const DATABASE_VERSION: u64 = 0;

#[derive(Clone)]
struct NodeDatabase {
    db: Arc<RwLock<Database>>,
}

impl NodeDatabase {
    fn open(path: PathBuf) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;
        // Check database version
        {
            let _ = write_txn.open_table(CONFIG_TABLE)?;
            let mut table = write_txn.open_table(CONFIG_TABLE)?;

            let db_version = table.get("db_version")?;
            let db_version = db_version.map(|v| v.value().to_owned());

            match db_version {
                Some(db_version) => {
                    let current_file_version = u64::from_str(&db_version)?;
                    if current_file_version.ne(&DATABASE_VERSION) {
                        // Database needs to be upgraded
                        todo!()
                    }
                }
                None => {
                    // Open all tables to init a new db
                    let _ = write_txn.open_table(CHANNELS_TABLE)?;
                    let _ = write_txn.open_table(EVENTS_TABLE)?;
                    let _ = write_txn.open_table(INVOICES_TABLE)?;
                    let _ = write_txn.open_table(PAYMENTS_TABLE)?;

                    table.insert("db_version", "0")?;
                }
            }
        }

        write_txn.commit()?;
        Ok(Self {
            db: Arc::new(RwLock::new(db)),
        })
    }

    async fn save_event(&self, event: Event) -> Result<(), Error> {
        let data = event.encode();
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(EVENTS_TABLE)?;
            if !data.is_empty() {
                table.insert(timestamp, data)?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn get_events(
        &self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<(DateTime<Utc>, Event)>, Error> {
        let start = start
            .map(|s| s.timestamp_nanos_opt().map(|s| s as u128))
            .flatten()
            .unwrap_or(0);
        let end = end
            .map(|e| e.timestamp_nanos_opt().map(|e| e as u128))
            .flatten()
            .unwrap_or(u128::MAX);
        let mut events = Vec::new();
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(EVENTS_TABLE)?;
        let entries = table.range(start..end)?;
        for entry in entries {
            let (timestamp, data) = entry?;
            if let Some(event) = Event::read(&mut &data.value()[..])? {
                events.push((
                    DateTime::from_timestamp_nanos(timestamp.value() as i64),
                    event,
                ));
            }
        }
        Ok(events)
    }

    async fn insert_temp_channel(
        &self,
        channel_id: ChannelId,
        channel: Channel,
    ) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(CHANNELS_TABLE)?;
            table.insert(channel_id.0, &channel)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn update_channel_id(
        &self,
        temp_channel_id: ChannelId,
        channel_id: ChannelId,
    ) -> Result<Option<Channel>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut channel = {
            let table = write_txn.open_table(CHANNELS_TABLE)?;
            let entry = table.get(temp_channel_id.0)?;
            entry.map(|e| e.value())
        };
        if let Some(channel) = channel.as_mut() {
            let mut table = write_txn.open_table(CHANNELS_TABLE)?;
            table.insert(channel_id.0, channel)?;
            table.remove(temp_channel_id.0)?;
        }
        write_txn.commit()?;
        Ok(channel)
    }

    async fn update_channel_claimed(
        &self,
        channel_id: ChannelId,
    ) -> Result<Option<Channel>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut channel = {
            let table = write_txn.open_table(CHANNELS_TABLE)?;
            let entry = table.get(channel_id.0)?;
            entry.map(|e| e.value())
        };
        if let Some(channel) = channel.as_mut() {
            if channel.is_claimed {
                return Err(Error::ChannelAlreadyClaimed);
            }
            let mut table = write_txn.open_table(CHANNELS_TABLE)?;
            channel.is_claimed = true;
            table.insert(channel_id.0, channel)?;
        }
        write_txn.commit()?;
        Ok(channel)
    }

    async fn get_channel(&self, channel_id: ChannelId) -> Result<Option<Channel>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(CHANNELS_TABLE)?;
        let entry = table.get(channel_id.0)?;
        Ok(entry.map(|e| e.value()))
    }

    async fn insert_invoice(&self, invoice: &Bolt11Invoice) -> Result<(), Error> {
        let payment_hash = invoice.payment_hash();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            table.insert(
                payment_hash.as_ref(),
                &Invoice {
                    bolt_11: invoice.to_string(),
                    expiry: invoice.expires_at().unwrap_or_default().as_secs(),
                    paid: false,
                },
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn update_paid_invoice(
        &self,
        payment_hash: PaymentHash,
    ) -> Result<Option<Invoice>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut invoice = {
            let table = write_txn.open_table(INVOICES_TABLE)?;
            let entry = table.get(payment_hash.0)?;
            entry.map(|e| e.value())
        };
        if let Some(invoice) = invoice.as_mut() {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            invoice.paid = true;
            table.insert(payment_hash.0, invoice)?;
        }
        write_txn.commit()?;
        Ok(invoice)
    }

    async fn get_invoice(&self, payment_hash: PaymentHash) -> Result<Option<Invoice>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(INVOICES_TABLE)?;
        let entry = table.get(payment_hash.0)?;
        Ok(entry.map(|e| e.value()))
    }

    async fn insert_payment(&self, invoice: &Bolt11Invoice, amount: Amount) -> Result<(), Error> {
        let payment_hash = invoice.payment_hash();
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAYMENTS_TABLE)?;
            table.insert(
                payment_hash.as_ref(),
                Payment {
                    bolt_11: invoice.to_string(),
                    amount,
                    paid: false,
                    spent_msat: Amount::ZERO,
                    pre_image: None,
                },
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn update_payment(
        &self,
        payment_hash: PaymentHash,
        pre_image: PaymentPreimage,
        fee_paid: Amount,
    ) -> Result<Option<Payment>, Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let mut payment = {
            let table = write_txn.open_table(PAYMENTS_TABLE)?;
            let entry = table.get(payment_hash.0)?;
            entry.map(|e| e.value())
        };
        if let Some(payment) = payment.as_mut() {
            let mut table = write_txn.open_table(PAYMENTS_TABLE)?;
            payment.paid = true;
            payment.spent_msat = Amount::from_msat(payment.amount.to_msat() + fee_paid.to_msat());
            payment.pre_image = Some(pre_image.0);
            table.insert(payment_hash.0, payment)?;
        }
        write_txn.commit()?;
        Ok(payment)
    }

    async fn get_payment(&self, payment_hash: PaymentHash) -> Result<Option<Payment>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PAYMENTS_TABLE)?;
        let entry = table.get(payment_hash.0)?;
        Ok(entry.map(|e| e.value()))
    }
}

// https://github.com/cberner/redb/blob/master/examples/bincode_keys.rs
#[derive(Debug)]
struct Bincode<T>(pub T);

impl<T> Value for Bincode<T>
where
    T: Debug + Serialize + for<'a> Deserialize<'a>,
{
    type SelfType<'a> = T
    where
        Self: 'a;

    type AsBytes<'a> = Vec<u8>
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        None
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        bincode::deserialize(data).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'a,
        Self: 'b,
    {
        bincode::serialize(value).unwrap()
    }

    fn type_name() -> TypeName {
        TypeName::new(&format!("Bincode<{}>", type_name::<T>()))
    }
}

struct NodeLogger;

impl Logger for NodeLogger {
    fn log(&self, record: Record) {
        match record.level {
            Level::Gossip => tracing::trace!("{}", record.args),
            Level::Trace => tracing::trace!("{}", record.args),
            Level::Debug => tracing::debug!("{}", record.args),
            Level::Info => tracing::info!("{}", record.args),
            Level::Warn => tracing::warn!("{}", record.args),
            Level::Error => tracing::error!("{}", record.args),
        }
    }
}
