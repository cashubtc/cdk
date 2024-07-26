use std::{
    any::type_name,
    cmp::Ordering,
    collections::{HashMap, HashSet},
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
use bitcoin::{
    absolute::LockTime, key::Secp256k1, secp256k1::PublicKey, BlockHash, Network, OutPoint,
    ScriptBuf, Transaction, Txid,
};
use cdk::{
    amount::Amount,
    cdk_lightning::{
        self, CreateInvoiceResponse, MintLightning, MintMeltSettings, PayInvoiceResponse,
        PaymentQuoteResponse, Settings,
    },
    lightning_invoice::{
        payment::payment_parameters_from_invoice, utils::create_invoice_from_channelmanager,
    },
    mint::MeltQuote,
    nuts::{
        nut04::Settings as MintSettings, nut05::Settings as MeltSettings, CurrencyUnit,
        MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteState, MintMethodSettings,
        MintQuoteState, PaymentMethod,
    },
    secp256k1::rand::random,
    util::{hex, unix_time},
    Bolt11Invoice,
};
use chrono::{DateTime, Utc};
use futures::Stream;
use lightning::{
    chain::{
        chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator},
        chainmonitor::ChainMonitor,
        ChannelMonitorUpdateStatus, Filter, Listen, Watch,
    },
    events::{Event, PaymentPurpose},
    ln::{
        channelmanager::{
            ChainParameters, ChannelManager, ChannelManagerReadArgs, PaymentId, Retry,
        },
        msgs::SocketAddress,
        peer_handler::{IgnoringMessageHandler, MessageHandler, PeerManager},
        script::ShutdownScript,
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
    sign::{
        EntropySource, InMemorySigner, KeysManager, NodeSigner, OutputSpender, Recipient,
        SpendableOutputDescriptor,
    },
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
use lightning_rapid_gossip_sync::RapidGossipSync;
use redb::{Database, Key, ReadableTable, TableDefinition, TypeName, Value};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{BitcoinClient, Error};

const BLOCK_TIMER: u64 = 1;
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
    bitcoin_client: Arc<BitcoinClient>,
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
        config: UserConfig,
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

        // Sync network graph
        if network == Network::Bitcoin {
            tracing::info!("Syncing network graph");
            match reqwest::get("https://rapidsync.lightningdevkit.org/snapshot/0").await {
                Ok(res) => match res.bytes().await {
                    Ok(data) => {
                        if let Err(e) = RapidGossipSync::new(network_graph.clone(), logger.clone())
                            .update_network_graph(data.as_ref())
                        {
                            tracing::warn!("Error updating network graph: {:?}", e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Error fetching network snapshot: {}", e);
                    }
                },
                Err(e) => tracing::warn!("Error fetching network snapshot: {}", e),
            }
        }
        tracing::info!(
            "Network graph stats: {} nodes, {} channels",
            network_graph.read_only().nodes().len(),
            network_graph.read_only().channels().len()
        );

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
                    config,
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
                    config,
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
            bitcoin_client,
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
        node.start_peer_reconnect();
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

    fn start_peer_reconnect(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let cancel_token = self_clone.cancel_token.clone();

            loop {
                let counterparty_node_ids = self_clone
                    .channel_manager
                    .list_channels()
                    .iter()
                    .map(|c| c.counterparty.node_id)
                    .collect::<HashSet<_>>();
                for node_id in counterparty_node_ids {
                    if self_clone.peer_manager.peer_by_node_id(&node_id).is_none() {
                        if let Ok(Some(addr)) = self_clone.db.get_peer_address(node_id).await {
                            tracing::info!("Reconnecting to peer: {}@{}", node_id, addr);
                            if let Err(e) = self_clone.connect_peer(node_id, addr).await {
                                tracing::warn!("Error reconnecting to peer: {:?}", e);
                            }
                        }
                    }
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {},
                    _ = cancel_token.cancelled() => break,
                }
            }
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
            Event::ChannelPending {
                channel_id,
                former_temporary_channel_id,
                ..
            } => {
                let mut opened_channel_ids = self.opened_channel_ids.lock().await;
                let temp_channel_id = former_temporary_channel_id.unwrap();
                if let Err(e) = self.db.update_channel_id(temp_channel_id, channel_id).await {
                    tracing::warn!("Error updating channel id: {:?}", e);
                }
                if let Some(tx) = opened_channel_ids.remove(&former_temporary_channel_id.unwrap()) {
                    let _ = tx.send(channel_id);
                }
            }
            Event::ChannelReady {
                channel_id,
                counterparty_node_id,
                ..
            } => {
                tracing::info!(
                    "Channel ready with {}: {}",
                    counterparty_node_id,
                    channel_id
                );
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
            Event::PaymentPathSuccessful {
                payment_hash, path, ..
            } => {
                tracing::debug!(
                    "Payment path successful ({}): {:?}",
                    payment_hash.unwrap(),
                    path
                );
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
                        Amount::from(fee_paid_msat.unwrap_or_default()),
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
            Event::SpendableOutputs {
                outputs,
                channel_id,
            } => {
                let channel_id = channel_id.unwrap();
                tracing::info!("Spendable outputs found for {}", channel_id);
                if let Err(e) = self
                    .db
                    .insert_spendable_outputs(channel_id, outputs.clone())
                    .await
                {
                    tracing::error!("Error saving spendable outputs: {:?}", e);
                    tracing::warn!("Manual spending of outputs required: {:?}", outputs);
                }
            }
            Event::PendingHTLCsForwardable { time_forwardable } => {
                tokio::time::sleep(time_forwardable).await;
                self.channel_manager.process_pending_htlc_forwards();
            }
            _ => tracing::warn!("Unhandled event: {:?}", event),
        }
    }

    pub fn announce_node(
        &self,
        alias: &str,
        color: [u8; 3],
        addrs: Vec<SocketAddr>,
    ) -> Result<(), Error> {
        let alias = {
            if alias.len() > 32 {
                return Err(Error::Ldk("Alias too long".to_string()));
            }
            let mut bytes = [0; 32];
            bytes[..alias.len()].copy_from_slice(alias.as_bytes());
            bytes
        };
        self.peer_manager.broadcast_node_announcement(
            color,
            alias,
            addrs.into_iter().map(|a| a.into()).collect(),
        );
        Ok(())
    }

    pub async fn get_info(&self) -> Result<NodeInfo, Error> {
        let node_id = self.keys_manager.get_node_id(Recipient::Node).unwrap();
        let channels = self.channel_manager.list_channels();
        let channel_balances = channels
            .iter()
            .map(|c| (c.channel_id, Amount::from(c.balance_msat / 1000)))
            .collect();
        let peers = self
            .peer_manager
            .list_peers()
            .iter()
            .filter_map(|p| {
                Some((
                    p.counterparty_node_id,
                    match p.socket_address {
                        Some(SocketAddress::TcpIpV4 { addr, port }) => {
                            Some(SocketAddr::new(addr.into(), port))
                        }
                        Some(SocketAddress::TcpIpV6 { addr, port }) => {
                            Some(SocketAddr::new(addr.into(), port))
                        }
                        _ => None,
                    }?,
                ))
            })
            .collect();
        let spendable_balance = self.get_spendable_output_balance().await?;
        let inbound_liquidity = self.get_inbound_liquidity()?;

        let read_only_graph = self.gossip_sync.network_graph().read_only();
        let network_nodes = read_only_graph.nodes().len();
        let network_channels = read_only_graph.channels().len();

        Ok(NodeInfo {
            node_id,
            channel_balances,
            peers,
            spendable_balance,
            inbound_liquidity,
            network_nodes,
            network_channels,
        })
    }

    pub async fn connect_peer(&self, node_id: PublicKey, addr: SocketAddr) -> Result<(), Error> {
        match lightning_net_tokio::connect_outbound(self.peer_manager.clone(), node_id, addr).await
        {
            Some(_) => {
                if let Err(e) = self.db.insert_peer_address(node_id, addr).await {
                    tracing::warn!("Error saving peer address: {:?}", e);
                }
                Ok(())
            }
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
            .create_channel(node_id, amount.into(), 0, random(), None, None)
            .map_err(|e| Error::Ldk(format!("{:?}", e)))?;
        pending_channel_scripts.insert(channel_id, tx);
        drop(pending_channel_scripts);

        self.db
            .insert_temp_channel(channel_id, Channel { node_id, amount })
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
        channel_id: ChannelId,
        funding_tx: Transaction,
    ) -> Result<ChannelId, Error> {
        let node_id = self
            .db
            .get_channel(channel_id)
            .await?
            .ok_or(Error::ChannelNotFound)?
            .node_id;

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
        Ok(new_channel_id)
    }

    pub async fn close_channel(
        &self,
        channel_id: ChannelId,
        script: ScriptBuf,
    ) -> Result<(), Error> {
        let shutdown_script =
            ShutdownScript::try_from(script).map_err(|e| Error::Ldk(format!("{:?}", e)))?;
        let channel = self
            .db
            .get_channel(channel_id)
            .await?
            .ok_or(Error::ChannelNotFound)?;
        let fee_rate = self
            .bitcoin_client
            .get_est_sat_per_1000_weight(ConfirmationTarget::ChannelCloseMinimum);
        self.channel_manager
            .close_channel_with_feerate_and_script(
                &channel_id,
                &channel.node_id,
                Some(fee_rate),
                Some(shutdown_script),
            )
            .map_err(|e| Error::Ldk(format!("{:?}", e)))
    }

    pub fn get_channel_info(&self, channel_id: ChannelId) -> Result<ChannelInfo, Error> {
        let channels = self.channel_manager.list_channels();
        let channel = channels
            .iter()
            .find(|c| c.channel_id == channel_id)
            .ok_or(Error::ChannelNotFound)?;
        Ok(ChannelInfo {
            channel_id,
            counterparty_node_id: channel.counterparty.node_id,
            funding_txo: channel.funding_txo.map(|o| o.into_bitcoin_outpoint()),
            balance: Amount::from(channel.balance_msat / 1000),
            inbound_capacity: Amount::from(channel.inbound_capacity_msat / 1000),
            is_ready: channel.is_channel_ready,
            is_usable: channel.is_usable,
            is_outbound: channel.is_outbound,
            short_channel_id: channel.short_channel_id,
            outbound_scid: channel.outbound_scid_alias,
            inbound_scid: channel.inbound_scid_alias,
        })
    }

    pub fn is_channel_ready(&self, channel_id: ChannelId) -> bool {
        self.channel_manager
            .list_channels()
            .iter()
            .find(|c| c.channel_id == channel_id)
            .map_or(false, |c| c.is_channel_ready)
    }

    pub async fn get_open_channel_value(&self, channel_id: ChannelId) -> Result<Amount, Error> {
        let channel = self
            .db
            .get_channel(channel_id)
            .await?
            .ok_or(Error::ChannelNotFound)?;
        Ok(channel.amount)
    }

    pub fn get_inbound_liquidity(&self) -> Result<Amount, Error> {
        let channels = self.channel_manager.list_channels();
        let inbound_liquidity = channels
            .iter()
            .filter(|c| c.is_usable)
            .map(|c| c.inbound_capacity_msat / 1000)
            .sum::<u64>();
        Ok(Amount::from(inbound_liquidity))
    }

    pub fn get_mint_settings(&self) -> MintSettings {
        let settings = self.get_settings();
        MintSettings {
            methods: vec![MintMethodSettings {
                method: PaymentMethod::Bolt11,
                unit: CurrencyUnit::Sat,
                min_amount: Some(settings.mint_settings.min_amount),
                max_amount: Some(settings.mint_settings.max_amount),
            }],
            disabled: false,
        }
    }

    pub fn get_melt_settings(&self) -> MeltSettings {
        let settings = self.get_settings();
        MeltSettings {
            methods: vec![MeltMethodSettings {
                method: PaymentMethod::Bolt11,
                unit: CurrencyUnit::Sat,
                min_amount: Some(settings.melt_settings.min_amount),
                max_amount: Some(settings.melt_settings.max_amount),
            }],
            disabled: false,
        }
    }

    pub async fn get_spendable_output_balance(&self) -> Result<Amount, Error> {
        let spendable_balance =
            self.db
                .get_all_spendable_outputs()
                .await
                .map_or(Amount::ZERO, |outputs| {
                    Amount::from(
                        outputs
                            .values()
                            .flat_map(|a| a)
                            .map(|o| match o {
                                SpendableOutputDescriptor::StaticOutput { output, .. } => {
                                    output.value
                                }
                                SpendableOutputDescriptor::DelayedPaymentOutput(o) => {
                                    o.output.value
                                }
                                SpendableOutputDescriptor::StaticPaymentOutput(o) => o.output.value,
                            })
                            .sum::<u64>(),
                    )
                });
        Ok(spendable_balance)
    }

    pub async fn sweep_spendable_outputs(&self, script: ScriptBuf) -> Result<Txid, Error> {
        let secp = Secp256k1::new();
        let outputs = self.db.get_all_spendable_outputs().await?;
        if outputs.is_empty() {
            return Err(Error::NoSpendableOutputs);
        }

        let fee_rate = self
            .bitcoin_client
            .get_est_sat_per_1000_weight(ConfirmationTarget::OnChainSweep);
        let cur_height = self.channel_manager.current_best_block().height;
        let locktime = LockTime::from_height(cur_height).map_or(LockTime::ZERO, |l| l.into());
        let tx = self
            .keys_manager
            .spend_spendable_outputs(
                &outputs.values().flat_map(|a| a).collect::<Vec<_>>(),
                Vec::new(),
                script,
                fee_rate,
                Some(locktime),
                &secp,
            )
            .map_err(|_| Error::Ldk("Error spending outputs".to_string()))?;
        let txid = tx.txid();
        tracing::info!("Sweeping outputs in txid {}", txid);
        self.bitcoin_client.broadcast_transactions(&[&tx]);

        self.db
            .clear_spendable_outputs(outputs.keys().map(|k| *k).collect())
            .await?;

        Ok(txid)
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

pub struct ChannelInfo {
    pub channel_id: ChannelId,
    pub counterparty_node_id: PublicKey,
    pub funding_txo: Option<OutPoint>,
    pub balance: Amount,
    pub inbound_capacity: Amount,
    pub is_ready: bool,
    pub is_usable: bool,
    pub is_outbound: bool,
    pub short_channel_id: Option<u64>,
    pub outbound_scid: Option<u64>,
    pub inbound_scid: Option<u64>,
}

pub struct NodeInfo {
    pub node_id: PublicKey,
    pub channel_balances: HashMap<ChannelId, Amount>,
    pub peers: HashMap<PublicKey, SocketAddr>,
    pub spendable_balance: Amount,
    pub inbound_liquidity: Amount,
    pub network_nodes: usize,
    pub network_channels: usize,
}

pub struct PendingChannel {
    pub channel_id: ChannelId,
    pub funding_script: ScriptBuf,
}

#[async_trait]
impl MintLightning for Node {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: false,
            mint_settings: MintMeltSettings {
                min_amount: Amount::from(1),
                max_amount: Amount::from(10_000_000),
                enabled: true,
            },
            melt_settings: MintMeltSettings {
                min_amount: Amount::from(1),
                max_amount: Amount::from(10_000_000),
                enabled: true,
            },
            unit: CurrencyUnit::Sat,
        }
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        tracing::info!("Creating invoice: {} {}", amount, description);
        let expiry = unix_expiry - unix_time();
        let invoice = create_invoice_from_channelmanager(
            &self.channel_manager,
            self.keys_manager.clone(),
            self.logger.clone(),
            self.network.into(),
            Some(Into::<u64>::into(amount) * 1000),
            description.to_string(),
            expiry as u32,
            None,
        )
        .map_err(map_err)?;
        self.db.insert_invoice(&invoice).await.map_err(map_err)?;
        Ok(CreateInvoiceResponse {
            request_lookup_id: invoice.payment_hash().to_string(),
            request: invoice,
        })
    }

    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        Ok(PaymentQuoteResponse {
            request_lookup_id: melt_quote_request.request.payment_hash().to_string(),
            amount: Amount::from(
                melt_quote_request
                    .request
                    .amount_milli_satoshis()
                    .unwrap_or_default()
                    / 1000,
            ),
            fee: Amount::from(
                melt_quote_request
                    .request
                    .amount_milli_satoshis()
                    .unwrap_or_default()
                    / 1000
                    / 100,
            ), // TODO: estimate fee
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: MeltQuote,
        partial_amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        tracing::info!("Paying invoice: {}", melt_quote.request);
        let bolt11 = Bolt11Invoice::from_str(&melt_quote.request)?;
        let amount_msats = partial_amount
            .map(|a| Into::<u64>::into(a) * 1000)
            .or(bolt11.amount_milli_satoshis())
            .ok_or(map_err("No amount"))?;
        let (payment_hash, recipient_onion, mut route_params) =
            payment_parameters_from_invoice(&bolt11)
                .map_err(|_| map_err("Error extracting payment parameters"))?;
        self.db
            .insert_payment(&bolt11, Amount::from(amount_msats / 1000))
            .await
            .map_err(map_err)?;
        let (tx, rx) = oneshot::channel();
        let mut inflight_payments = self.inflight_payments.write().await;
        inflight_payments.insert(payment_hash, tx);
        drop(inflight_payments);

        route_params.final_value_msat = amount_msats;
        route_params.max_total_routing_fee_msat = max_fee.map(|f| Into::<u64>::into(f) * 1000);
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
            payment_hash: payment_hash.to_string(),
            payment_preimage: payment.pre_image.map(|p| hex::encode(p)),
            status: if payment.pre_image.is_some() {
                MeltQuoteState::Paid
            } else {
                MeltQuoteState::Unpaid
            },
            total_spent: payment.spent,
        })
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let mut rx = self.paid_invoices.subscribe();
        Ok(Box::pin(async_stream::stream! {
            while let Ok(payment_hash) = rx.recv().await {
                tracing::info!("Invoice paid: {}", payment_hash);
                yield payment_hash.to_string();
            }
        }))
    }

    async fn check_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        tracing::debug!("Checking invoice status: {}", request_lookup_id);
        let payment_hash = PaymentHash(
            hex::decode(request_lookup_id)
                .map_err(map_err)?
                .try_into()
                .map_err(|_| map_err("Invalid request_lookup_id"))?,
        );
        let invoice = self
            .db
            .get_invoice(payment_hash)
            .await
            .map_err(map_err)?
            .ok_or(map_err("Invoice not found"))?;
        Ok(if invoice.paid {
            MintQuoteState::Paid
        } else {
            MintQuoteState::Unpaid
        })
    }
}

fn map_err<E: ToString>(e: E) -> cdk_lightning::Error {
    cdk_lightning::Error::Lightning(Box::new(Error::Ldk(e.to_string())))
}

// property key -> value
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
// channeld id -> channel
const CHANNELS_TABLE: TableDefinition<[u8; 32], Bincode<Channel>> =
    TableDefinition::new("channels");
// timestamp -> event
const EVENTS_TABLE: TableDefinition<u128, Vec<u8>> = TableDefinition::new("events");
// payment hash -> invoice
const INVOICES_TABLE: TableDefinition<[u8; 32], Bincode<Invoice>> =
    TableDefinition::new("invoices");
// payment hash -> payment
const PAYMENTS_TABLE: TableDefinition<[u8; 32], Bincode<Payment>> =
    TableDefinition::new("payments");
// node id -> socket address
const PEERS_TABLE: TableDefinition<Bincode<PublicKey>, Bincode<SocketAddr>> =
    TableDefinition::new("peers");
// channel id -> spendable outputs
const SPENDABLE_OUTPUTS_TABLE: TableDefinition<[u8; 32], Bincode<Vec<Vec<u8>>>> =
    TableDefinition::new("spendable_outputs");

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Channel {
    node_id: PublicKey,
    amount: Amount,
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
    spent: Amount,
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
                    let _ = write_txn.open_table(PEERS_TABLE)?;

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
                    spent: Amount::ZERO,
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
            payment.spent = payment.amount + fee_paid;
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

    async fn insert_peer_address(&self, node_id: PublicKey, addr: SocketAddr) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(PEERS_TABLE)?;
            table.insert(node_id, addr)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn get_peer_address(&self, node_id: PublicKey) -> Result<Option<SocketAddr>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(PEERS_TABLE)?;
        let entry = table.get(node_id)?;
        Ok(entry.map(|e| e.value()))
    }

    async fn insert_spendable_outputs(
        &self,
        channel_id: ChannelId,
        outputs: Vec<SpendableOutputDescriptor>,
    ) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
            table.insert(
                channel_id.0,
                &outputs.into_iter().map(|o| o.encode()).collect(),
            )?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn get_all_spendable_outputs(
        &self,
    ) -> Result<HashMap<ChannelId, Vec<SpendableOutputDescriptor>>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
        Ok(table
            .iter()?
            .filter_map(|e| {
                let (channel_id, outputs) = e.ok()?;
                let outputs = outputs.value();
                let outputs = outputs
                    .into_iter()
                    .filter_map(|o| SpendableOutputDescriptor::read(&mut &o[..]).ok().flatten())
                    .collect();
                Some((ChannelId(channel_id.value()), outputs))
            })
            .collect())
    }

    async fn clear_spendable_outputs(&self, channel_ids: Vec<ChannelId>) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SPENDABLE_OUTPUTS_TABLE)?;
            for channel_id in channel_ids {
                table.remove(channel_id.0)?;
            }
        }
        write_txn.commit()?;
        Ok(())
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

impl<T> Key for Bincode<T>
where
    T: Debug + Serialize + DeserializeOwned + Ord,
{
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        Self::from_bytes(data1).cmp(&Self::from_bytes(data2))
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
