use std::env;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

// ============================================================================
// Timing Tracking for Performance Monitoring
// ============================================================================

/// Tracks timing for regtest initialization
/// Individual component timings are logged via tracing during initialization
#[derive(Debug, Default)]
struct SimpleTimings {
    pub ldk_stop: Option<Duration>,
    pub total: Duration,
}
use cdk_ldk_node::CdkLdkNode;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::Node;
use tokio::sync::oneshot::Sender;
use tokio::sync::Notify;

use crate::ln_regtest::bitcoin_client::BitcoinClient;
use crate::ln_regtest::bitcoind::Bitcoind;
use crate::ln_regtest::cln::Clnd;
use crate::ln_regtest::ln_client::{ClnClient, LightningClient, LndClient};
use crate::ln_regtest::lnd::Lnd;
use crate::util::{poll, poll_with_timeout, ProcessManager};

pub const BITCOIND_ADDR: &str = "127.0.0.1:18443";
pub const ZMQ_RAW_BLOCK: &str = "tcp://127.0.0.1:28332";
pub const ZMQ_RAW_TX: &str = "tcp://127.0.0.1:28333";
pub const BITCOIN_RPC_USER: &str = "testuser";
pub const BITCOIN_RPC_PASS: &str = "testpass";

const BITCOIN_DIR: &str = "bitcoin";

pub const LND_ADDR: &str = "0.0.0.0:18449";
pub const LND_RPC_ADDR: &str = "localhost:10009";

pub const LND_TWO_ADDR: &str = "0.0.0.0:18410";
pub const LND_TWO_RPC_ADDR: &str = "localhost:10010";

pub const CLN_ADDR: &str = "127.0.0.1:19846";
pub const CLN_TWO_ADDR: &str = "127.0.0.1:19847";

pub fn get_temp_dir() -> PathBuf {
    let dir = env::var("CDK_ITESTS_DIR").expect("Temp dir not set");
    std::fs::create_dir_all(&dir).unwrap();
    dir.parse().expect("Valid path buf")
}

pub fn get_bitcoin_dir(temp_dir: &Path) -> PathBuf {
    let dir = temp_dir.join(BITCOIN_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn get_cln_dir(work_dir: &Path, name: &str) -> PathBuf {
    let dir = work_dir.join("cln").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn get_lnd_dir(work_dir: &Path, name: &str) -> PathBuf {
    let dir = work_dir.join("lnd").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn get_lnd_cert_file_path(lnd_dir: &Path) -> PathBuf {
    lnd_dir.join("tls.cert")
}

pub fn get_lnd_macaroon_path(lnd_dir: &Path) -> PathBuf {
    lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon")
}

// ============================================================================
// New Async Initialization Functions for Phase 2 Parallelization
// ============================================================================

/// Initialize bitcoind process with ProcessManager
pub async fn init_bitcoind_async(
    process_mgr: Arc<ProcessManager>,
    temp_dir: PathBuf,
) -> Result<Arc<Bitcoind>> {
    let step_start = Instant::now();
    tracing::info!("Initializing bitcoind...");

    let mut bitcoind = Bitcoind::new(
        get_bitcoin_dir(&temp_dir),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    );

    bitcoind.start_bitcoind(&process_mgr).await?;

    let pid = bitcoind.pid();
    let bitcoind_arc = Arc::new(bitcoind);
    let strong_count = Arc::strong_count(&bitcoind_arc);

    tracing::info!(
        ?pid,
        strong_count,
        "Bitcoind initialized and wrapped in Arc in {:.2}s",
        step_start.elapsed().as_secs_f64()
    );

    Ok(bitcoind_arc)
}

/// Initialize bitcoin RPC client and prepare wallet
pub async fn init_bitcoin_client_async(bitcoind: Arc<Bitcoind>) -> Result<Arc<BitcoinClient>> {
    let step_start = Instant::now();
    let bitcoind_strong_count = Arc::strong_count(&bitcoind);
    let pid = bitcoind.pid();
    tracing::info!(
        ?pid,
        bitcoind_arc_count = bitcoind_strong_count,
        "Initializing bitcoin client (bitcoind Arc strong count at start)"
    );

    let bitcoin_client = BitcoinClient::new(
        "wallet".to_string(),
        BITCOIND_ADDR.into(),
        None,
        Some(BITCOIN_RPC_USER.to_string()),
        Some(BITCOIN_RPC_PASS.to_string()),
    )?;

    // Wait for bitcoind to be ready using poll
    // Use get_blockchain_info() which doesn't require a wallet
    use std::ops::ControlFlow;
    poll("bitcoind ready", || async {
        match bitcoin_client.get_blockchain_info() {
            Ok(_) => Ok(()),
            Err(e) => Err(ControlFlow::Continue(e)),
        }
    })
    .await?;

    // Create and load wallet
    bitcoin_client.create_wallet().ok();
    bitcoin_client.load_wallet()?;

    // Generate initial blocks
    let new_addr = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&new_addr, 200)?;

    let bitcoind_strong_count_end = Arc::strong_count(&bitcoind);
    tracing::info!(
        ?pid,
        bitcoind_arc_count_end = bitcoind_strong_count_end,
        "Bitcoin client initialized and 200 blocks generated in {:.2}s",
        step_start.elapsed().as_secs_f64()
    );
    tracing::info!(
        ?pid,
        "Bitcoin client init complete - bitcoind Arc will be dropped now (unless kept by caller)"
    );

    Ok(Arc::new(bitcoin_client))
}

/// Initialize a CLN node
pub async fn init_cln_node_async(
    name: &str,
    process_mgr: Arc<ProcessManager>,
    bitcoind: Arc<Bitcoind>,
    bitcoin_client: Arc<BitcoinClient>,
    addr: &str,
) -> Result<(Arc<Clnd>, Arc<ClnClient>)> {
    let step_start = Instant::now();
    tracing::info!("Initializing CLN node: {}", name);

    let temp_dir = bitcoind.data_dir.parent().unwrap().to_path_buf();
    let cln_dir = get_cln_dir(&temp_dir, name);

    let mut clnd = Clnd::new(
        bitcoind.data_dir.clone(),
        cln_dir.clone(),
        addr.parse().unwrap(),
        bitcoind.rpc_user.clone(),
        bitcoind.rpc_password.clone(),
    );

    let process_handle = clnd.start_clnd(&process_mgr, name).await?;

    // Poll for the lightning-rpc socket to exist before trying to connect
    // Also check if the process is still running
    let rpc_socket_path = cln_dir.join("regtest/lightning-rpc");
    let debug_log_path = cln_dir.join("debug.log");
    use std::ops::ControlFlow;
    poll("CLN socket ready", || async {
        // First check if process is still running
        if !process_handle.is_running().await {
            // Process died - try to read debug log for error details
            let error_msg = std::fs::read_to_string(&debug_log_path)
                .map(|log| {
                    // Get last 20 lines of log
                    let lines: Vec<&str> = log.lines().collect();
                    let start = lines.len().saturating_sub(20);
                    lines[start..].join("\n")
                })
                .unwrap_or_else(|_| "Could not read debug.log".to_string());

            return Err(ControlFlow::Break(anyhow::anyhow!(
                "CLN process died before socket was created. Last log entries:\n{}",
                error_msg
            )));
        }

        // Process is running, check for socket
        if rpc_socket_path.exists() {
            Ok(())
        } else {
            Err(ControlFlow::Continue(anyhow::anyhow!(
                "Socket not ready: {}",
                rpc_socket_path.display()
            )))
        }
    })
    .await?;

    let cln_client = ClnClient::new(cln_dir.clone(), None).await?;

    // Fund the node
    fund_ln(&bitcoin_client, &cln_client).await?;

    tracing::info!(
        "CLN node {} initialized and funded in {:.2}s",
        name,
        step_start.elapsed().as_secs_f64()
    );

    Ok((Arc::new(clnd), Arc::new(cln_client)))
}

/// Initialize an LND node
pub async fn init_lnd_node_async(
    name: &str,
    process_mgr: Arc<ProcessManager>,
    bitcoind: Arc<Bitcoind>,
    bitcoin_client: Arc<BitcoinClient>,
    addr: &str,
    rpc_addr: &str,
) -> Result<(Arc<Lnd>, Arc<LndClient>)> {
    let step_start = Instant::now();
    tracing::info!("Initializing LND node: {}", name);

    let temp_dir = bitcoind.data_dir.parent().unwrap().to_path_buf();
    let lnd_dir = get_lnd_dir(&temp_dir, name);

    let mut lnd = Lnd::new(
        bitcoind.data_dir.clone(),
        lnd_dir.clone(),
        addr.parse().unwrap(),
        rpc_addr.to_string(),
        bitcoind.rpc_user.clone(),
        bitcoind.rpc_password.clone(),
        bitcoind.zmq_raw_block.clone(),
        bitcoind.zmq_raw_tx.clone(),
    );

    // Wait for bitcoind's settings.json file to exist - LND needs this to start
    let settings_json_path = bitcoind.data_dir.join("regtest/settings.json");
    use std::ops::ControlFlow;
    poll("bitcoind settings.json ready", || async {
        if settings_json_path.exists() {
            Ok(())
        } else {
            Err(ControlFlow::Continue(anyhow::anyhow!(
                "settings.json not ready: {}",
                settings_json_path.display()
            )))
        }
    })
    .await?;

    let process_handle = lnd.start_lnd(&process_mgr, name).await?;

    // Poll for LND RPC readiness with exponential backoff
    let rpc_url = format!("https://{}", rpc_addr);
    let cert_path = get_lnd_cert_file_path(&lnd_dir);
    let macaroon_path = get_lnd_macaroon_path(&lnd_dir);

    let lnd_client: LndClient = poll_with_timeout(
        "LND RPC ready",
        Duration::from_secs(90), // LND can take longer to start than CLN
        || async {
            // First check if process is still running
            if !process_handle.is_running().await {
                let log_path = lnd_dir.join("logs/bitcoin/regtest/lnd.log");
                let error_msg = std::fs::read_to_string(&log_path)
                    .map(|log| {
                        let lines: Vec<&str> = log.lines().collect();
                        let start = lines.len().saturating_sub(30);
                        lines[start..].join("\n")
                    })
                    .unwrap_or_else(|_| "Could not read lnd.log".to_string());

                return Err(ControlFlow::Break(anyhow::anyhow!(
                    "LND process died before RPC was ready. Last log entries:\n{}",
                    error_msg
                )));
            }

            // Try to create the client and verify it's ready by calling wait_chain_sync
            match LndClient::new(rpc_url.clone(), cert_path.clone(), macaroon_path.clone()).await {
                Ok(client) => {
                    // Verify RPC is actually ready by attempting chain sync check
                    match client.wait_chain_sync().await {
                        Ok(_) => Ok(client),
                        Err(e) => {
                            let err_msg = e.to_string();
                            // If it's a "not yet ready" error, retry
                            if err_msg.contains("not yet ready") || err_msg.contains("starting up")
                            {
                                Err(ControlFlow::Continue(anyhow::anyhow!(
                                    "LND RPC server starting: {}",
                                    err_msg
                                )))
                            } else {
                                // Other errors should fail
                                Err(ControlFlow::Break(e))
                            }
                        }
                    }
                }
                Err(e) => Err(ControlFlow::Continue(anyhow::anyhow!(
                    "LND RPC not ready: {}",
                    e
                ))),
            }
        },
    )
    .await?;

    // Fund the node
    fund_ln(&bitcoin_client, &lnd_client).await?;

    tracing::info!(
        "LND node {} initialized and funded in {:.2}s",
        name,
        step_start.elapsed().as_secs_f64()
    );

    Ok((Arc::new(lnd), Arc::new(lnd_client)))
}

/// Initialize an LDK node
pub async fn init_ldk_node_async(
    bitcoind: Arc<Bitcoind>,
    bitcoin_client: Arc<BitcoinClient>,
    temp_dir: PathBuf,
) -> Result<Arc<Node>> {
    let step_start = Instant::now();
    tracing::info!("Initializing LDK node");

    let ldk_dir = temp_dir.join("ldk");
    std::fs::create_dir_all(&ldk_dir)?;

    let cdk_ldk = CdkLdkNode::new(
        bitcoin::Network::Regtest,
        cdk_ldk_node::ChainSource::BitcoinRpc(cdk_ldk_node::BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: 18443,
            user: bitcoind.rpc_user.clone(),
            password: bitcoind.rpc_password.clone(),
        }),
        cdk_ldk_node::GossipSource::P2P,
        ldk_dir.to_string_lossy().to_string(),
        cdk_common::common::FeeReserve {
            min_fee_reserve: cashu::Amount::ZERO,
            percent_fee_reserve: 0.0,
        },
        vec![SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: 8092,
        }],
        None,
    )?;

    let node = cdk_ldk.node();

    // Start the node
    tracing::info!("Starting LDK node");
    node.start()?;

    // Fund the node
    let addr = node.onchain_payment().new_address()?;
    bitcoin_client.send_to_address(&addr.to_string(), 5_000_000)?;

    // Mine blocks to confirm funding
    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 6)?;

    // Sync the wallet
    node.sync_wallets()?;

    tracing::info!(
        "LDK node initialized and funded in {:.2}s",
        step_start.elapsed().as_secs_f64()
    );

    Ok(node)
}

/// Open channels between LDK node and other lightning nodes
pub async fn open_ldk_channels_async(
    bitcoin_client: Arc<BitcoinClient>,
    ldk_node: Arc<Node>,
    cln_one: Arc<ClnClient>,
    cln_two: Arc<ClnClient>,
    lnd_one: Arc<LndClient>,
    lnd_two: Arc<LndClient>,
) -> Result<Arc<()>> {
    let step_start = Instant::now();
    tracing::info!("Opening channels between LDK and lightning nodes...");

    // Fund CLN one and LND one with fresh UTXOs for opening LDK channels
    // This ensures they have available funds regardless of previous channel operations
    tracing::info!("Funding nodes for LDK channel operations...");
    let cln_addr = cln_one.get_new_onchain_address().await?;
    bitcoin_client.send_to_address(&cln_addr, 3_000_000)?;

    let lnd_addr = lnd_one.get_new_onchain_address().await?;
    bitcoin_client.send_to_address(&lnd_addr, 3_000_000)?;

    // Mine blocks to confirm the new funding
    tracing::info!("Mining blocks to confirm LDK channel funding...");
    generate_block(&bitcoin_client)?;

    // Wait for nodes to sync and see the new funds
    tracing::info!("Syncing nodes to see new funding...");
    tokio::try_join!(cln_one.wait_chain_sync(), lnd_one.wait_chain_sync(),)?;

    // Explicitly wait for CLN wallet to show available funds
    // Chain sync doesn't guarantee wallet has rescanned UTXOs yet
    tracing::info!("Waiting for CLN wallet to show available funds...");
    cln_one.wait_for_funds(2_000_000).await?;

    // Sync LDK wallet to see the newly mined blocks
    tracing::info!("Syncing LDK wallet after new blocks...");
    ldk_node.sync_wallets()?;

    // Connect LDK to CLN and LND
    tracing::info!("Connecting LDK to peers...");
    let cln_info = cln_one.get_connect_info().await?;
    ldk_node.connect(
        cln_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: cln_info
                .address
                .split('.')
                .map(|part| part.parse())
                .collect::<Result<Vec<u8>, _>>()?
                .try_into()
                .unwrap(),
            port: cln_info.port,
        },
        true,
    )?;

    let lnd_info = lnd_one.get_connect_info().await?;
    ldk_node.connect(
        lnd_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: lnd_info.port,
        },
        true,
    )?;

    // Get LDK node info
    let pubkey = ldk_node.node_id();
    let listen_addr = ldk_node.listening_addresses();
    let listen_addr = listen_addr.as_ref().unwrap().first().unwrap();

    let (listen_addr, port) = match listen_addr {
        SocketAddress::TcpIpV4 { addr, port } => (Ipv4Addr::from(*addr).to_string(), port),
        _ => anyhow::bail!("Unexpected socket address type"),
    };

    // Open channels (creates funding transactions in mempool)
    tracing::info!("Opening channel from CLN to LDK");
    cln_one
        .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
        .await?;

    cln_one
        .open_channel(1_500_000, &pubkey.to_string(), Some(750_000))
        .await?;
    tracing::info!("Created funding tx: CLN one <-> LDK");

    tracing::info!("Opening channel from LDK to LND");
    ldk_node.open_announced_channel(
        lnd_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: lnd_info.port,
        },
        1_000_000,
        Some(500_000_000),
        None,
    )?;
    tracing::info!("Created funding tx: LDK <-> LND one");

    // Mine blocks once to confirm all LDK channel funding transactions
    tracing::info!("Mining blocks to confirm all LDK channel funding transactions...");
    generate_block(&bitcoin_client)?;

    tracing::info!("Syncing LDK wallet...");
    ldk_node.sync_wallets()?;

    // Wait for CLN and LND to sync with the chain before checking channel status
    tracing::info!("Waiting for nodes to sync with new blocks...");
    tokio::try_join!(cln_one.wait_chain_sync(), lnd_one.wait_chain_sync(),)?;

    // Wait for channels to become active
    tokio::try_join!(
        cln_one.wait_channels_active(),
        lnd_one.wait_channels_active(),
    )?;

    // Ensure persistent peer connections after channels are active
    tracing::info!("Ensuring LDK maintains peer connections...");

    // LDK connects to peers (already connected from channel opening, but reconnecting to be sure)
    // LDK's connect method is idempotent and doesn't error on already connected
    ldk_node.connect(
        cln_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: cln_info
                .address
                .split('.')
                .map(|part| part.parse())
                .collect::<Result<Vec<u8>, _>>()?
                .try_into()
                .unwrap(),
            port: cln_info.port,
        },
        true,
    )?;

    ldk_node.connect(
        lnd_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: lnd_info.port,
        },
        true,
    )?;

    // Peers also connect back to LDK (including nodes without direct channels)
    tracing::info!("Connecting all nodes to LDK as peers...");

    let cln_two_info = cln_two.get_connect_info().await?;
    let lnd_two_info = lnd_two.get_connect_info().await?;

    // Helper to connect peers, ignoring "already connected" errors
    let connect_if_not_connected = |result: Result<()>| -> Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("already connected") || err_str.contains("Already connected") {
                    tracing::debug!("Peer already connected (ignoring): {}", err_str);
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    };

    // Connect LDK to all nodes (LDK doesn't error on already connected, but we'll be consistent)
    ldk_node.connect(
        cln_two_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: cln_two_info
                .address
                .split('.')
                .map(|part| part.parse())
                .collect::<Result<Vec<u8>, _>>()?
                .try_into()
                .unwrap(),
            port: cln_two_info.port,
        },
        true,
    )?;

    ldk_node.connect(
        lnd_two_info.pubkey.parse()?,
        SocketAddress::TcpIpV4 {
            addr: [127, 0, 0, 1],
            port: lnd_two_info.port,
        },
        true,
    )?;

    // All nodes connect to LDK (ignoring "already connected" errors)
    connect_if_not_connected(
        cln_one
            .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
            .await,
    )?;
    connect_if_not_connected(
        cln_two
            .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
            .await,
    )?;
    connect_if_not_connected(
        lnd_one
            .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
            .await,
    )?;
    connect_if_not_connected(
        lnd_two
            .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
            .await,
    )?;

    // Sync LDK wallet after all connections to ensure it's aware of all channels and peers
    tracing::info!("Final LDK wallet sync...");
    ldk_node.sync_wallets()?;

    // Give a moment for channel state to stabilize
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    tracing::info!(
        "All LDK channels opened and active in {:.2}s",
        step_start.elapsed().as_secs_f64()
    );

    Ok(Arc::new(()))
}

/// Open channels between all lightning nodes
/// Phase 2 Optimization: Opens all channels in parallel for faster startup
pub async fn open_channels_async(
    bitcoin_client: Arc<BitcoinClient>,
    cln_one: Arc<ClnClient>,
    cln_two: Arc<ClnClient>,
    lnd_one: Arc<LndClient>,
    lnd_two: Arc<LndClient>,
) -> Result<Arc<()>> {
    let step_start = Instant::now();
    tracing::info!("Opening channels between lightning nodes (batched mining)...");

    // Ensure all nodes see their confirmed funding before opening channels
    tracing::info!("Ensuring all nodes see their confirmed funding...");
    tokio::try_join!(
        cln_one.wait_chain_sync(),
        cln_two.wait_chain_sync(),
        lnd_one.wait_chain_sync(),
        lnd_two.wait_chain_sync(),
    )?;

    // Phase 2 Optimization: Open channels sequentially but mine blocks only once at the end
    // This avoids UTXO conflicts while still batching block mining for speed
    let open_start = Instant::now();

    // Open all 4 channels (creates funding transactions in mempool)
    open_channel(&*cln_one, &*lnd_one).await?;
    tracing::info!("Created funding tx: CLN one <-> LND one");

    open_channel(&*lnd_one, &*lnd_two).await?;
    tracing::info!("Created funding tx: LND one <-> LND two");

    open_channel(&*cln_two, &*lnd_one).await?;
    tracing::info!("Created funding tx: CLN two <-> LND one");

    open_channel(&*cln_one, &*lnd_two).await?;
    tracing::info!("Created funding tx: CLN one <-> LND two");

    tracing::info!(
        "All 4 funding transactions created in {:.2}s",
        open_start.elapsed().as_secs_f64()
    );

    // Mine blocks once to confirm all funding transactions
    tracing::info!("Mining blocks to confirm all channel funding transactions...");
    generate_block(&bitcoin_client)?;

    // Wait for all nodes to sync with the new blocks before checking channel status
    tracing::info!("Waiting for nodes to sync with new blocks...");
    tokio::try_join!(
        cln_one.wait_chain_sync(),
        cln_two.wait_chain_sync(),
        lnd_one.wait_chain_sync(),
        lnd_two.wait_chain_sync(),
    )?;

    // Wait for all channels to become active in parallel
    let activate_start = Instant::now();
    tokio::try_join!(
        cln_one.wait_channels_active(),
        cln_two.wait_channels_active(),
        lnd_one.wait_channels_active(),
        lnd_two.wait_channels_active(),
    )?;

    tracing::info!(
        "All channels activated in {:.2}s",
        activate_start.elapsed().as_secs_f64()
    );

    // Ensure all nodes maintain peer connections with their channel partners
    tracing::info!("Ensuring persistent peer connections...");

    // Get connection info for all nodes
    let cln_one_info = cln_one.get_connect_info().await?;
    let cln_two_info = cln_two.get_connect_info().await?;
    let lnd_one_info = lnd_one.get_connect_info().await?;
    let lnd_two_info = lnd_two.get_connect_info().await?;

    // Helper to connect peers, ignoring "already connected" errors
    let connect_if_not_connected = |result: Result<()>| -> Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("already connected") || err_str.contains("Already connected") {
                    tracing::debug!("Peer already connected (ignoring): {}", err_str);
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    };

    // Reconnect all channel peers (ignoring "already connected" errors)
    connect_if_not_connected(
        cln_one
            .connect_peer(
                lnd_one_info.pubkey.clone(),
                lnd_one_info.address.clone(),
                lnd_one_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        cln_one
            .connect_peer(
                lnd_two_info.pubkey.clone(),
                lnd_two_info.address.clone(),
                lnd_two_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        cln_two
            .connect_peer(
                lnd_one_info.pubkey.clone(),
                lnd_one_info.address.clone(),
                lnd_one_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        lnd_one
            .connect_peer(
                cln_one_info.pubkey.clone(),
                cln_one_info.address.clone(),
                cln_one_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        lnd_one
            .connect_peer(
                cln_two_info.pubkey.clone(),
                cln_two_info.address.clone(),
                cln_two_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        lnd_one
            .connect_peer(
                lnd_two_info.pubkey.clone(),
                lnd_two_info.address.clone(),
                lnd_two_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        lnd_two
            .connect_peer(
                cln_one_info.pubkey.clone(),
                cln_one_info.address.clone(),
                cln_one_info.port,
            )
            .await,
    )?;
    connect_if_not_connected(
        lnd_two
            .connect_peer(
                lnd_one_info.pubkey.clone(),
                lnd_one_info.address.clone(),
                lnd_one_info.port,
            )
            .await,
    )?;

    // Give time for channel announcements to propagate through the network
    tracing::info!("Waiting for channel gossip propagation...");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    tracing::info!(
        "All channels opened and active in {:.2}s (batched mining optimization)",
        step_start.elapsed().as_secs_f64()
    );

    Ok(Arc::new(()))
}

pub fn generate_block(bitcoin_client: &BitcoinClient) -> Result<()> {
    let mine_to_address = bitcoin_client.get_new_address()?;
    let blocks = 6;
    tracing::info!("Mining {blocks} blocks to {mine_to_address}");

    bitcoin_client.generate_blocks(&mine_to_address, 6)?;

    Ok(())
}

pub async fn fund_ln<C>(bitcoin_client: &BitcoinClient, ln_client: &C) -> Result<()>
where
    C: LightningClient,
{
    // Create multiple UTXOs for each node to enable opening multiple channels
    // Each channel needs ~1.5M sats, so we create 3 UTXOs of 2M sats each
    // This allows nodes to open up to 3 channels without UTXO conflicts

    // Send 3 separate transactions to create 3 UTXOs
    for i in 1..=3 {
        let ln_address = ln_client.get_new_onchain_address().await?;
        bitcoin_client.send_to_address(&ln_address, 2_000_000)?;
        tracing::debug!("Sent funding transaction {} of 2M sats", i);
    }

    // Mine blocks to confirm all funding transactions
    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 6)?;

    // Wait for node to see the confirmed UTXOs
    ln_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn open_channel<C1, C2>(cln_client: &C1, lnd_client: &C2) -> Result<()>
where
    C1: LightningClient,
    C2: LightningClient,
{
    let cln_info = cln_client.get_connect_info().await?;

    let cln_pubkey = cln_info.pubkey;
    let cln_address = cln_info.address;
    let cln_port = cln_info.port;

    lnd_client
        .connect_peer(cln_pubkey.to_string(), cln_address.to_string(), cln_port)
        .await
        .unwrap();

    lnd_client
        .open_channel(1_500_000, &cln_pubkey.to_string(), Some(750_000))
        .await
        .unwrap();

    Ok(())
}

/// Legacy function for starting regtest environment with background operation.
///
/// This function is used by bin files that run a persistent regtest environment.
/// It uses RegtestJit internally but maintains the same interface for backward compatibility.
///
/// For new code, prefer using `RegtestJit` directly for better control and parallelization.
pub async fn start_regtest_end(
    work_dir: &Path,
    sender: Sender<()>,
    notify: Arc<Notify>,
    skip_ldk: bool,
) -> anyhow::Result<()> {
    let total_start = Instant::now();
    tracing::info!("Starting regtest environment setup using RegtestJit");

    // Track phase timings for summary
    let mut timings = SimpleTimings::default();

    // Create JIT-initialized regtest environment
    let regtest = RegtestJit::new(work_dir.to_path_buf(), skip_ldk)?;

    // Initialize all components (bitcoind, LN nodes, and conditionally LDK node + channels)
    // Note: Individual phase timings are tracked via tracing logs during initialization
    regtest.finalize().await?;

    // Phase 8: Stop LDK if needed
    let ldk_stop_start = Instant::now();
    if !skip_ldk {
        tracing::info!("Stopping LDK node to free it for mint reuse...");
        regtest.stop_ldk_node().await?;
        tracing::info!("LDK node stopped, channels preserved in storage directory");
        timings.ldk_stop = Some(ldk_stop_start.elapsed());
    }

    // Calculate total time
    timings.total = total_start.elapsed();

    // Log the traditional message
    tracing::info!(
        "Total regtest setup completed in {:.2}s (using parallel JIT initialization)",
        timings.total.as_secs_f64()
    );

    // Print basic timing summary
    // Note: For detailed per-component timings, check the tracing logs above
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           REGTEST ENVIRONMENT SETUP - TIMING SUMMARY                ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Total Setup Time:        {:8.2}s                                  ║",
        timings.total.as_secs_f64()
    );
    if let Some(ldk_stop) = timings.ldk_stop {
        println!(
            "║ LDK Stop Time:           {:8.2}s                                  ║",
            ldk_stop.as_secs_f64()
        );
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!("\n💡 For detailed component timings, see the tracing logs above.\n");

    // Send notification that regtest set up is complete
    sender.send(()).expect("Could not send oneshot");

    // Wait until we are told to shutdown
    // When this returns, regtest will be dropped and all processes terminated
    notify.notified().await;

    // Explicitly fast terminate for clean shutdown
    regtest.fast_terminate().await;

    Ok(())
}

/// Start regtest environment and return the RegtestJit instance for further customization.
///
/// This variant allows the caller to access the regtest instance after initialization,
/// useful for scenarios like stopping the LDK node before starting a mint server.
pub async fn start_regtest_with_access(work_dir: &Path) -> anyhow::Result<RegtestJit> {
    let total_start = Instant::now();
    tracing::info!("Starting regtest environment setup using RegtestJit");

    // Create JIT-initialized regtest environment (with LDK enabled by default)
    let regtest = RegtestJit::new(work_dir.to_path_buf(), false)?;

    // Initialize all components (bitcoind, LN nodes, LDK node, and all channels)
    regtest.finalize().await?;

    let total_elapsed = total_start.elapsed();
    tracing::info!(
        "Total regtest setup completed in {:.2}s (using parallel JIT initialization)",
        total_elapsed.as_secs_f64()
    );

    Ok(regtest)
}

// ============================================================================
// Phase 2: JIT Lazy Initialization Pattern
// ============================================================================

use crate::jit::JitCell;

/// Just-In-Time initialized regtest environment.
///
/// This structure uses lazy initialization to parallelize the startup of independent
/// components (bitcoind, lightning nodes) while respecting dependency chains.
/// Components are only initialized when first accessed via their getter methods.
///
/// # Example
///
/// ```no_run
/// use std::path::PathBuf;
///
/// use cdk_integration_tests::init_regtest::RegtestJit;
///
/// # async fn example() -> anyhow::Result<()> {
/// let temp_dir = PathBuf::from("/tmp/regtest");
/// let regtest = RegtestJit::new(temp_dir, false)?; // false = don't skip LDK
///
/// // Only starts bitcoind and bitcoin_client (not LN nodes)
/// let bitcoin_client = regtest.bitcoin_client().await?;
///
/// // Starts all components in parallel (respecting dependencies)
/// regtest.finalize().await?;
/// # Ok(())
/// # }
/// ```
pub struct RegtestJit {
    /// Process manager for spawning and managing daemon processes
    _process_mgr: Arc<ProcessManager>,

    /// Temporary directory for all test data
    temp_dir: PathBuf,

    /// Whether to skip LDK node initialization
    skip_ldk: bool,

    // Core Bitcoin components
    /// Bitcoind daemon (Bitcoin Core regtest node)
    bitcoind: JitCell<Arc<Bitcoind>>,

    /// Bitcoin RPC client for blockchain operations
    bitcoin_client: JitCell<Arc<BitcoinClient>>,

    // Lightning daemon processes (must be kept alive)
    /// First Core Lightning daemon
    clnd_one: JitCell<Arc<Clnd>>,

    /// Second Core Lightning daemon
    clnd_two: JitCell<Arc<Clnd>>,

    /// First LND daemon
    lnd_daemon_one: JitCell<Arc<Lnd>>,

    /// Second LND daemon
    lnd_daemon_two: JitCell<Arc<Lnd>>,

    // Lightning clients
    /// First Core Lightning node client
    cln_one: JitCell<Arc<ClnClient>>,

    /// Second Core Lightning node client
    cln_two: JitCell<Arc<ClnClient>>,

    /// First LND node client
    lnd_one: JitCell<Arc<LndClient>>,

    /// Second LND node client
    lnd_two: JitCell<Arc<LndClient>>,

    // LDK node (optional - may be disabled in interactive mode)
    /// LDK node instance (None if skip_ldk is true)
    ldk_node: Option<JitCell<Arc<Node>>>,

    // Channel setup completion markers
    /// Marker indicating all standard channels have been opened
    channels_opened: JitCell<Arc<()>>,

    /// Marker indicating LDK channels have been opened (None if skip_ldk is true)
    ldk_channels_opened: Option<JitCell<Arc<()>>>,
}

impl RegtestJit {
    /// Create a new JIT-initialized regtest environment.
    ///
    /// This creates all the lazy initialization cells but does not start any
    /// processes. Components will be initialized on first access.
    ///
    /// # Arguments
    ///
    /// * `temp_dir` - Directory for storing all regtest data (bitcoind, LN nodes, etc.)
    /// * `skip_ldk` - Whether to skip LDK node initialization (useful for interactive mode)
    ///
    /// # Returns
    ///
    /// * `Ok(RegtestJit)` - A new regtest environment ready for lazy initialization
    /// * `Err(anyhow::Error)` - If setup fails (e.g., cannot create directories)
    pub fn new(temp_dir: PathBuf, skip_ldk: bool) -> Result<Self> {
        // Ensure temp directory exists
        std::fs::create_dir_all(&temp_dir)?;

        // Create logs directory for process output
        let logs_dir = temp_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;

        let process_mgr = Arc::new(ProcessManager::new(logs_dir));

        // Initialize bitcoind
        let bitcoind = JitCell::new_async({
            let pm = process_mgr.clone();
            let temp_dir = temp_dir.clone();
            move || init_bitcoind_async(pm, temp_dir)
        });

        // Initialize bitcoin client (depends on bitcoind)
        let bitcoin_client = JitCell::new_async({
            let bitcoind = bitcoind.clone();
            move || async move {
                let bd = bitcoind.get().await?;
                init_bitcoin_client_async(bd).await
            }
        });

        // Initialize CLN daemon 1 and client 1 (depends on bitcoind and bitcoin_client)
        let cln_init_one = JitCell::new_async({
            let pm = process_mgr.clone();
            let bitcoind = bitcoind.clone();
            let bitcoin_client = bitcoin_client.clone();
            move || async move {
                let bd = bitcoind.get().await?;
                let bc = bitcoin_client.get().await?;
                init_cln_node_async("one", pm, bd, bc, CLN_ADDR).await
            }
        });

        let clnd_one = JitCell::new_async({
            let cln_init_one = cln_init_one.clone();
            move || async move {
                let (daemon, _client) = cln_init_one.get().await?;
                Ok(daemon)
            }
        });

        let cln_one = JitCell::new_async({
            let cln_init_one = cln_init_one.clone();
            move || async move {
                let (_daemon, client) = cln_init_one.get().await?;
                Ok(client)
            }
        });

        // Initialize CLN daemon 2 and client 2 (depends on bitcoind and bitcoin_client)
        let cln_init_two = JitCell::new_async({
            let pm = process_mgr.clone();
            let bitcoind = bitcoind.clone();
            let bitcoin_client = bitcoin_client.clone();
            move || async move {
                let bd = bitcoind.get().await?;
                let bc = bitcoin_client.get().await?;
                init_cln_node_async("two", pm, bd, bc, CLN_TWO_ADDR).await
            }
        });

        let clnd_two = JitCell::new_async({
            let cln_init_two = cln_init_two.clone();
            move || async move {
                let (daemon, _client) = cln_init_two.get().await?;
                Ok(daemon)
            }
        });

        let cln_two = JitCell::new_async({
            let cln_init_two = cln_init_two.clone();
            move || async move {
                let (_daemon, client) = cln_init_two.get().await?;
                Ok(client)
            }
        });

        // Initialize LND daemon 1 and client 1 (depends on bitcoind and bitcoin_client)
        let lnd_init_one = JitCell::new_async({
            let pm = process_mgr.clone();
            let bitcoind = bitcoind.clone();
            let bitcoin_client = bitcoin_client.clone();
            move || async move {
                let bd = bitcoind.get().await?;
                let bc = bitcoin_client.get().await?;
                init_lnd_node_async("one", pm, bd, bc, LND_ADDR, LND_RPC_ADDR).await
            }
        });

        let lnd_daemon_one = JitCell::new_async({
            let lnd_init_one = lnd_init_one.clone();
            move || async move {
                let (daemon, _client) = lnd_init_one.get().await?;
                Ok(daemon)
            }
        });

        let lnd_one = JitCell::new_async({
            let lnd_init_one = lnd_init_one.clone();
            move || async move {
                let (_daemon, client) = lnd_init_one.get().await?;
                Ok(client)
            }
        });

        // Initialize LND daemon 2 and client 2 (depends on bitcoind and bitcoin_client)
        let lnd_init_two = JitCell::new_async({
            let pm = process_mgr.clone();
            let bitcoind = bitcoind.clone();
            let bitcoin_client = bitcoin_client.clone();
            move || async move {
                let bd = bitcoind.get().await?;
                let bc = bitcoin_client.get().await?;
                init_lnd_node_async("two", pm, bd, bc, LND_TWO_ADDR, LND_TWO_RPC_ADDR).await
            }
        });

        let lnd_daemon_two = JitCell::new_async({
            let lnd_init_two = lnd_init_two.clone();
            move || async move {
                let (daemon, _client) = lnd_init_two.get().await?;
                Ok(daemon)
            }
        });

        let lnd_two = JitCell::new_async({
            let lnd_init_two = lnd_init_two.clone();
            move || async move {
                let (_daemon, client) = lnd_init_two.get().await?;
                Ok(client)
            }
        });

        // Initialize LDK node (depends on bitcoind and bitcoin_client) - unless skipped
        let ldk_node = if skip_ldk {
            tracing::info!("Skipping LDK node initialization (--skip-ldk flag set)");
            None
        } else {
            Some(JitCell::new_async({
                let bitcoind = bitcoind.clone();
                let bitcoin_client = bitcoin_client.clone();
                let temp_dir = temp_dir.clone();
                move || async move {
                    let bd = bitcoind.get().await?;
                    let bc = bitcoin_client.get().await?;
                    init_ldk_node_async(bd, bc, temp_dir).await
                }
            }))
        };

        // Initialize channels (depends on all LN nodes)
        let channels_opened = JitCell::new_async({
            let bitcoin_client = bitcoin_client.clone();
            let cln_one = cln_one.clone();
            let cln_two = cln_two.clone();
            let lnd_one = lnd_one.clone();
            let lnd_two = lnd_two.clone();
            move || async move {
                let bc = bitcoin_client.get().await?;
                let cln1 = cln_one.get().await?;
                let cln2 = cln_two.get().await?;
                let lnd1 = lnd_one.get().await?;
                let lnd2 = lnd_two.get().await?;
                open_channels_async(bc, cln1, cln2, lnd1, lnd2).await
            }
        });

        // Initialize LDK channels (depends on LDK node and CLN/LND clients, but not on channels_opened)
        // This allows LDK channels to open in parallel with standard channels - unless LDK is skipped
        let ldk_channels_opened = ldk_node.as_ref().map(|ldk_node_cell| {
            JitCell::new_async({
                let bitcoin_client = bitcoin_client.clone();
                let ldk_node = ldk_node_cell.clone();
                let cln_one = cln_one.clone();
                let cln_two = cln_two.clone();
                let lnd_one = lnd_one.clone();
                let lnd_two = lnd_two.clone();
                move || async move {
                    let bc = bitcoin_client.get().await?;
                    let ldk = ldk_node.get().await?;
                    let cln1 = cln_one.get().await?;
                    let cln2 = cln_two.get().await?;
                    let lnd1 = lnd_one.get().await?;
                    let lnd2 = lnd_two.get().await?;
                    open_ldk_channels_async(bc, ldk, cln1, cln2, lnd1, lnd2).await
                }
            })
        });

        Ok(Self {
            _process_mgr: process_mgr,
            temp_dir,
            skip_ldk,
            bitcoind,
            bitcoin_client,
            clnd_one,
            clnd_two,
            lnd_daemon_one,
            lnd_daemon_two,
            cln_one,
            cln_two,
            lnd_one,
            lnd_two,
            ldk_node,
            channels_opened,
            ldk_channels_opened,
        })
    }

    /// Get the bitcoind instance, starting it if not already initialized.
    pub async fn bitcoind(&self) -> Result<Arc<Bitcoind>> {
        self.bitcoind.get().await
    }

    /// Get the bitcoin client, starting bitcoind if needed.
    pub async fn bitcoin_client(&self) -> Result<Arc<BitcoinClient>> {
        self.bitcoin_client.get().await
    }

    /// Get the first CLN client, starting dependencies if needed.
    pub async fn cln_one(&self) -> Result<Arc<ClnClient>> {
        self.cln_one.get().await
    }

    /// Get the second CLN client, starting dependencies if needed.
    pub async fn cln_two(&self) -> Result<Arc<ClnClient>> {
        self.cln_two.get().await
    }

    /// Get the first LND client, starting dependencies if needed.
    pub async fn lnd_one(&self) -> Result<Arc<LndClient>> {
        self.lnd_one.get().await
    }

    /// Get the second LND client, starting dependencies if needed.
    pub async fn lnd_two(&self) -> Result<Arc<LndClient>> {
        self.lnd_two.get().await
    }

    /// Get the LDK node, starting dependencies if needed.
    /// Returns an error if LDK was skipped during initialization.
    pub async fn ldk_node(&self) -> Result<Arc<Node>> {
        match &self.ldk_node {
            Some(node) => node.get().await,
            None => anyhow::bail!("LDK node was skipped during initialization (--skip-ldk flag)"),
        }
    }

    /// Stop the LDK node if it's running.
    /// This is useful when you want to start a mint that will use the same LDK data directory.
    pub async fn stop_ldk_node(&self) -> Result<()> {
        if let Some(ldk_node) = &self.ldk_node {
            if ldk_node.is_initialized() {
                let node = ldk_node.get().await?;
                node.stop()?;
                tracing::info!("LDK node stopped");
            }
        }
        Ok(())
    }

    /// Force all components to initialize and all channels to open.
    ///
    /// This method is useful when you want to ensure the entire regtest
    /// environment is fully set up before proceeding with tests.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - All components initialized and channels opened successfully
    /// * `Err(anyhow::Error)` - If any component fails to initialize
    pub async fn finalize(&self) -> Result<()> {
        // Wait for standard channels to be opened
        self.channels_opened.get().await?;

        // Wait for LDK channels if LDK is enabled
        if let Some(ldk_channels) = &self.ldk_channels_opened {
            ldk_channels.get().await?;
        }

        Ok(())
    }

    /// Check if specific components have been initialized.
    pub fn is_bitcoind_initialized(&self) -> bool {
        self.bitcoind.is_initialized()
    }

    pub fn is_bitcoin_client_initialized(&self) -> bool {
        self.bitcoin_client.is_initialized()
    }

    pub fn is_channels_opened(&self) -> bool {
        self.channels_opened.is_initialized()
    }

    /// Fast parallel shutdown of all components.
    ///
    /// This method drops all components in parallel, which is significantly
    /// faster than dropping them sequentially. Components are dropped in
    /// reverse dependency order (LN nodes before bitcoind).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cdk_integration_tests::init_regtest::RegtestJit;
    /// # use std::path::PathBuf;
    /// # async fn example() -> anyhow::Result<()> {
    /// let regtest = RegtestJit::new(PathBuf::from("/tmp/regtest"), false)?;
    /// // ... use regtest ...
    /// regtest.fast_terminate().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fast_terminate(self) {
        let Self {
            bitcoind,
            bitcoin_client,
            clnd_one,
            clnd_two,
            lnd_daemon_one,
            lnd_daemon_two,
            cln_one,
            cln_two,
            lnd_one,
            lnd_two,
            ldk_node,
            channels_opened,
            ldk_channels_opened,
            ..
        } = self;

        tracing::info!("Starting shutdown");
        let shutdown_start = Instant::now();

        // Drop components in reverse dependency order (sequentially to avoid RefCell conflicts)
        // Channels first
        drop(ldk_channels_opened);
        drop(channels_opened);

        // Then clients
        drop(lnd_one);
        drop(lnd_two);
        drop(cln_one);
        drop(cln_two);
        drop(ldk_node);

        // Then daemons
        drop(lnd_daemon_one);
        drop(lnd_daemon_two);
        drop(clnd_one);
        drop(clnd_two);

        // Finally bitcoin components
        drop(bitcoin_client);
        drop(bitcoind);

        tracing::info!(
            "Shutdown completed in {:.2}s",
            shutdown_start.elapsed().as_secs_f64()
        );
    }
}

impl std::fmt::Debug for RegtestJit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegtestJit")
            .field("temp_dir", &self.temp_dir)
            .field("bitcoind_initialized", &self.is_bitcoind_initialized())
            .field(
                "bitcoin_client_initialized",
                &self.is_bitcoin_client_initialized(),
            )
            .field("clnd_one_initialized", &self.clnd_one.is_initialized())
            .field("clnd_two_initialized", &self.clnd_two.is_initialized())
            .field(
                "lnd_daemon_one_initialized",
                &self.lnd_daemon_one.is_initialized(),
            )
            .field(
                "lnd_daemon_two_initialized",
                &self.lnd_daemon_two.is_initialized(),
            )
            .field("cln_one_initialized", &self.cln_one.is_initialized())
            .field("cln_two_initialized", &self.cln_two.is_initialized())
            .field("lnd_one_initialized", &self.lnd_one.is_initialized())
            .field("lnd_two_initialized", &self.lnd_two.is_initialized())
            .field("skip_ldk", &self.skip_ldk)
            .field(
                "ldk_node_initialized",
                &self.ldk_node.as_ref().map(|n| n.is_initialized()),
            )
            .field("channels_opened", &self.is_channels_opened())
            .field(
                "ldk_channels_opened",
                &self
                    .ldk_channels_opened
                    .as_ref()
                    .map(|c| c.is_initialized()),
            )
            .finish()
    }
}
