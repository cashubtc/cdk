use std::{
    collections::HashSet,
    fmt::format,
    path::PathBuf,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bitcoin::{hashes::Hash, BlockHash, Network};
use cdk::{
    cdk_lightning::{
        self, Amount, BalanceResponse, InvoiceInfo, MintLightning, PayInvoiceResponse,
    },
    lightning_invoice::{
        payment::payment_parameters_from_invoice, utils::create_invoice_from_channelmanager,
    },
    types::InvoiceStatus,
    util::unix_time,
    Bolt11Invoice, Sha256,
};
use futures::Stream;
use lightning::{
    chain::{chainmonitor::ChainMonitor, ChannelMonitorUpdateStatus, Filter, Listen, Watch},
    events::Event,
    ln::{
        channelmanager::{
            ChainParameters, ChannelManager, ChannelManagerReadArgs, PaymentId, Retry,
        },
        peer_handler::{IgnoringMessageHandler, MessageHandler, PeerManager},
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
    sign::{EntropySource, InMemorySigner, KeysManager},
    util::{
        config::UserConfig,
        logger::{Level, Logger, Record},
        persist::{
            read_channel_monitors, KVStore, CHANNEL_MANAGER_PERSISTENCE_KEY,
            CHANNEL_MANAGER_PERSISTENCE_PRIMARY_NAMESPACE,
            CHANNEL_MANAGER_PERSISTENCE_SECONDARY_NAMESPACE, NETWORK_GRAPH_PERSISTENCE_KEY,
            NETWORK_GRAPH_PERSISTENCE_PRIMARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_SECONDARY_NAMESPACE,
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
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{BitcoinClient, Error};

const BLOCK_TIMER: u64 = 10;

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

    inflight_payments: Arc<RwLock<HashSet<Sha256>>>,
}

impl Node {
    pub async fn start(
        data_dir: PathBuf,
        network: Network,
        rpc_client: BitcoinClient,
        seed: [u8; 32],
    ) -> Result<Self, Error> {
        // Create utils
        let bitcoin_client = Arc::new(rpc_client);
        let db = NodeDatabase::open(data_dir.join("node"))?;
        let logger = Arc::new(NodeLogger);
        let persister = Arc::new(FilesystemStore::new(data_dir.join("data")));

        // Derive keys manager
        let starting_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        let keys_manager = Arc::new(KeysManager::new(
            &seed,
            starting_time.as_secs(),
            starting_time.subsec_nanos(),
        ));

        // Setup chain monitor
        let chain_monitor = Arc::new(ChainMonitor::new(
            None,
            bitcoin_client.clone(),
            logger.clone(),
            bitcoin_client.clone(),
            persister.clone(),
        ));
        let polled_chain_tip = validate_best_block_header(bitcoin_client.clone()).await?;

        // Configure router
        let network_graph_bytes = persister.read(
            NETWORK_GRAPH_PERSISTENCE_PRIMARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_SECONDARY_NAMESPACE,
            NETWORK_GRAPH_PERSISTENCE_KEY,
        )?;
        let network_graph = Arc::new(if network_graph_bytes.is_empty() {
            NetworkGraph::new(network, logger.clone())
        } else {
            NetworkGraph::read(&mut &network_graph_bytes[..], logger.clone())?
        });

        let scorer = Arc::new(std::sync::RwLock::new(ProbabilisticScorer::new(
            ProbabilisticScoringDecayParameters::default(),
            network_graph.clone(),
            logger.clone(),
        )));
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

            inflight_payments: Arc::new(RwLock::new(HashSet::new())),
        };
        node.start_background_processor();
        Ok(node)
    }

    fn start_background_processor(&self) {
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
            Event::PaymentSent { payment_hash, .. } => {
                let payment_hash = Sha256::from_byte_array(payment_hash.0);
                if let Err(e) = self.db.update_paid_invoice(payment_hash).await {
                    tracing::warn!("Error updating invoice: {:?}", e);
                }
                self.inflight_payments.write().await.remove(&payment_hash);
            }
            _ => tracing::warn!("Unhandled event: {:?}", event),
        }
    }

    pub async fn get_events(
        &self,
        start: Option<u128>,
        end: Option<u128>,
    ) -> Result<Vec<(u128, Event)>, Error> {
        self.db.get_events(start, end).await
    }

    pub async fn stop(&self) {
        self.cancel_token.cancel();
    }
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
        let expiry = 3600;
        let unix_expiry = unix_time() + expiry;
        let invoice = create_invoice_from_channelmanager(
            &self.channel_manager,
            self.keys_manager.clone(),
            self.logger.clone(),
            self.network.into(),
            Some(amount.to_sat() * 1000),
            description.to_string(),
            expiry as u32,
            None,
        )
        .map_err(map_err)?;
        let payment_hash =
            Sha256::from_str(&invoice.payment_hash().to_string()).map_err(map_err)?;
        self.db
            .insert_invoice(payment_hash, unix_expiry)
            .await
            .map_err(map_err)?;
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
        todo!()
    }

    async fn check_invoice_status(
        &self,
        payment_hash: &cdk::Sha256,
    ) -> Result<InvoiceStatus, Self::Err> {
        if let Some((paid, expiry)) = self.db.get_invoice(*payment_hash).await.map_err(map_err)? {
            if paid {
                Ok(InvoiceStatus::Paid)
            } else {
                let time_now = unix_time();
                if expiry < time_now {
                    Ok(InvoiceStatus::Expired)
                } else {
                    let inflight_invoices = self.inflight_payments.read().await;
                    if inflight_invoices.contains(payment_hash) {
                        Ok(InvoiceStatus::InFlight)
                    } else {
                        Ok(InvoiceStatus::Unpaid)
                    }
                }
            }
        } else {
            Err(cdk_lightning::Error::Lightning(Box::new(Error::Ldk(
                "Invoice not found".to_string(),
            ))))
        }
    }

    async fn pay_invoice(
        &self,
        bolt11: Bolt11Invoice,
        partial_msat: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let mut inflight_payments = self.inflight_payments.write().await;
        let payment_hash = bolt11.payment_hash();
        inflight_payments.insert(*payment_hash);
        drop(inflight_payments);

        let (payment_hash, recipient_onion, mut route_params) =
            payment_parameters_from_invoice(&bolt11)
                .map_err(|_| map_err("Error extracting payment parameters"))?;
        route_params.final_value_msat = partial_msat
            .map(|f| f.to_msat())
            .or(bolt11.amount_milli_satoshis())
            .ok_or(map_err("No amount"))?;
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

        todo!()
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
            Some(amount.to_sat() * 1000),
            description,
            (unix_expiry - time_now) as u32,
            None,
        )
        .map_err(map_err)?;
        self.db
            .insert_invoice(*invoice.payment_hash(), unix_expiry)
            .await
            .map_err(map_err)?;
        Ok(invoice)
    }
}

fn map_err<E: ToString>(e: E) -> cdk_lightning::Error {
    cdk_lightning::Error::Lightning(Box::new(Error::Ldk(e.to_string())))
}

const EVENTS_TABLE: TableDefinition<u128, Vec<u8>> = TableDefinition::new("events");
const INVOICES_TABLE: TableDefinition<[u8; 32], (bool, u64)> = TableDefinition::new("invoices");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");

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
                    let _ = write_txn.open_table(INVOICES_TABLE)?;

                    table.insert("db_version", "0")?;
                }
            }
        }

        write_txn.commit()?;
        Ok(Self {
            db: Arc::new(RwLock::new(db)),
        })
    }

    async fn insert_invoice(&self, payment_hash: Sha256, unix_expiry: u64) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            table.insert(payment_hash.as_ref(), (false, unix_expiry))?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn update_paid_invoice(&self, payment_hash: Sha256) -> Result<(), Error> {
        let db = self.db.read().await;
        let write_txn = db.begin_write()?;
        let expiry = {
            let table = write_txn.open_table(INVOICES_TABLE)?;
            let entry = table.get(payment_hash.as_ref())?;
            entry.map(|e| e.value().1)
        };
        if let Some(expiry) = expiry {
            let mut table = write_txn.open_table(INVOICES_TABLE)?;
            table.insert(payment_hash.as_ref(), (true, expiry))?;
        }
        write_txn.commit()?;
        Ok(())
    }

    async fn get_invoice(&self, payment_hash: Sha256) -> Result<Option<(bool, u64)>, Error> {
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(INVOICES_TABLE)?;
        let entry = table.get(payment_hash.as_ref())?;
        Ok(entry.map(|e| e.value()))
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
        start: Option<u128>,
        end: Option<u128>,
    ) -> Result<Vec<(u128, Event)>, Error> {
        let mut events = Vec::new();
        let db = self.db.read().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(EVENTS_TABLE)?;
        let entries = table.range(start.unwrap_or(0)..end.unwrap_or(u128::MAX))?;
        for entry in entries {
            let (timestamp, data) = entry?;
            if let Some(event) = Event::read(&mut &data.value()[..])? {
                events.push((timestamp.value(), event));
            }
        }
        Ok(events)
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
