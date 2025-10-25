use std::env;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use cdk::types::FeeReserve;
use cdk_cln::Cln as CdkCln;
use cdk_common::database::mint::DynMintKVStore;
use cdk_lnd::Lnd as CdkLnd;
use cdk_sqlite::mint::memory;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::Node;
use tokio::sync::oneshot::Sender;
use tokio::sync::Notify;

use crate::ln_regtest::bitcoin_client::BitcoinClient;
use crate::ln_regtest::bitcoind::Bitcoind;
use crate::ln_regtest::cln::Clnd;
use crate::ln_regtest::ln_client::{ClnClient, LightningClient, LndClient};
use crate::ln_regtest::lnd::Lnd;
use crate::util::{poll, ProcessManager};

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

/// Configuration for regtest environment
pub struct RegtestConfig {
    pub mint_addr: String,
    pub cln_port: u16,
    pub lnd_port: u16,
    pub temp_dir: PathBuf,
}

impl Default for RegtestConfig {
    fn default() -> Self {
        Self {
            mint_addr: "127.0.0.1".to_string(),
            cln_port: 8085,
            lnd_port: 8087,
            temp_dir: std::env::temp_dir().join("cdk-itests-default"),
        }
    }
}

pub fn get_mint_url_with_config(config: &RegtestConfig, which: &str) -> String {
    let port = match which {
        "0" => config.cln_port,
        "1" => config.lnd_port,
        _ => panic!("Unknown mint identifier: {which}"),
    };
    format!("http://{}:{}", config.mint_addr, port)
}

pub fn get_mint_ws_url_with_config(config: &RegtestConfig, which: &str) -> String {
    let port = match which {
        "0" => config.cln_port,
        "1" => config.lnd_port,
        _ => panic!("Unknown mint identifier: {which}"),
    };
    format!("ws://{}:{}/v1/ws", config.mint_addr, port)
}

pub fn get_temp_dir() -> PathBuf {
    let dir = env::var("CDK_ITESTS_DIR").expect("Temp dir not set");
    std::fs::create_dir_all(&dir).unwrap();
    dir.parse().expect("Valid path buf")
}

pub fn get_temp_dir_with_config(config: &RegtestConfig) -> &PathBuf {
    &config.temp_dir
}

pub fn get_bitcoin_dir(temp_dir: &Path) -> PathBuf {
    let dir = temp_dir.join(BITCOIN_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn init_bitcoind(work_dir: &Path) -> Bitcoind {
    Bitcoind::new(
        get_bitcoin_dir(work_dir),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
}

pub fn init_bitcoin_client() -> Result<BitcoinClient> {
    BitcoinClient::new(
        "wallet".to_string(),
        BITCOIND_ADDR.into(),
        None,
        Some(BITCOIN_RPC_USER.to_string()),
        Some(BITCOIN_RPC_PASS.to_string()),
    )
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

pub async fn init_lnd(
    work_dir: &Path,
    lnd_dir: PathBuf,
    lnd_addr: &str,
    lnd_rpc_addr: &str,
) -> Lnd {
    Lnd::new(
        get_bitcoin_dir(work_dir),
        lnd_dir,
        lnd_addr.parse().unwrap(),
        lnd_rpc_addr.to_string(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
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

    // Wait for chain sync
    cln_client.wait_chain_sync().await?;

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

    lnd.start_lnd(&process_mgr, name).await?;

    let lnd_client = LndClient::new(
        format!("https://{}", rpc_addr),
        get_lnd_cert_file_path(&lnd_dir),
        get_lnd_macaroon_path(&lnd_dir),
    )
    .await?;

    // Wait for chain sync
    lnd_client.wait_chain_sync().await?;

    // Fund the node
    fund_ln(&bitcoin_client, &lnd_client).await?;

    tracing::info!(
        "LND node {} initialized and funded in {:.2}s",
        name,
        step_start.elapsed().as_secs_f64()
    );

    Ok((Arc::new(lnd), Arc::new(lnd_client)))
}

/// Open channels between all lightning nodes
pub async fn open_channels_async(
    bitcoin_client: Arc<BitcoinClient>,
    cln_one: Arc<ClnClient>,
    cln_two: Arc<ClnClient>,
    lnd_one: Arc<LndClient>,
    lnd_two: Arc<LndClient>,
) -> Result<Arc<()>> {
    let step_start = Instant::now();
    tracing::info!("Opening channels between lightning nodes...");

    // Open channel between CLN one and LND one
    open_channel(&*cln_one, &*lnd_one).await?;
    tracing::info!("Opened channel between CLN one and LND one");
    generate_block(&bitcoin_client)?;

    // Open channel between LND one and LND two
    open_channel(&*lnd_one, &*lnd_two).await?;
    tracing::info!("Opened channel between LND one and LND two");
    generate_block(&bitcoin_client)?;

    // Open channel between CLN two and LND one
    open_channel(&*cln_two, &*lnd_one).await?;
    tracing::info!("Opened channel between CLN two and LND one");
    generate_block(&bitcoin_client)?;

    // Open channel between CLN one and LND two
    open_channel(&*cln_one, &*lnd_two).await?;
    tracing::info!("Opened channel between CLN one and LND two");
    generate_block(&bitcoin_client)?;

    // Wait for all channels to become active
    tokio::try_join!(
        cln_one.wait_channels_active(),
        cln_two.wait_channels_active(),
        lnd_one.wait_channels_active(),
        lnd_two.wait_channels_active(),
    )?;

    tracing::info!(
        "All channels opened and active in {:.2}s",
        step_start.elapsed().as_secs_f64()
    );

    Ok(Arc::new(()))
}

pub fn generate_block(bitcoin_client: &BitcoinClient) -> Result<()> {
    let mine_to_address = bitcoin_client.get_new_address()?;
    let blocks = 10;
    tracing::info!("Mining {blocks} blocks to {mine_to_address}");

    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    Ok(())
}

pub async fn create_cln_backend(cln_client: &ClnClient) -> Result<CdkCln> {
    let rpc_path = cln_client.rpc_path.clone();

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let kv_store: DynMintKVStore = Arc::new(memory::empty().await?);
    Ok(CdkCln::new(rpc_path, fee_reserve, kv_store).await?)
}

pub async fn create_lnd_backend(lnd_client: &LndClient) -> Result<CdkLnd> {
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let kv_store: DynMintKVStore = Arc::new(memory::empty().await?);

    Ok(CdkLnd::new(
        lnd_client.address.clone(),
        lnd_client.cert_file.clone(),
        lnd_client.macaroon_file.clone(),
        fee_reserve,
        kv_store,
    )
    .await?)
}

pub async fn fund_ln<C>(bitcoin_client: &BitcoinClient, ln_client: &C) -> Result<()>
where
    C: LightningClient,
{
    let ln_address = ln_client.get_new_onchain_address().await?;

    bitcoin_client.send_to_address(&ln_address, 5_000_000)?;

    ln_client.wait_chain_sync().await?;

    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

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

    cln_client.wait_chain_sync().await?;
    lnd_client.wait_chain_sync().await?;

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
    ldk_node: Option<Arc<Node>>,
) -> anyhow::Result<()> {
    let total_start = Instant::now();
    tracing::info!("Starting regtest environment setup using RegtestJit");

    // Create JIT-initialized regtest environment
    let regtest = RegtestJit::new(work_dir.to_path_buf())?;

    // If LDK node is provided, handle it separately (not part of standard RegtestJit)
    if let Some(node) = ldk_node.as_ref() {
        // Get bitcoin client first for funding
        let bitcoin_client = regtest.bitcoin_client().await?;

        tracing::info!("Starting ldk node");
        node.start()?;
        let addr = node.onchain_payment().new_address().unwrap();
        bitcoin_client.send_to_address(&addr.to_string(), 5_000_000)?;
    }

    // Initialize all standard components (bitcoind, LN nodes, channels)
    regtest.finalize().await?;

    // Handle LDK node channel opening if provided
    if let Some(node) = ldk_node.as_ref() {
        let bitcoin_client = regtest.bitcoin_client().await?;
        let cln_one = regtest.cln_one().await?;
        let cln_two = regtest.cln_two().await?;
        let lnd_one = regtest.lnd_one().await?;

        let pubkey = node.node_id();
        let listen_addr = node.listening_addresses();
        let listen_addr = listen_addr.as_ref().unwrap().first().unwrap();

        let (listen_addr, port) = match listen_addr {
            SocketAddress::TcpIpV4 { addr, port } => (Ipv4Addr::from(*addr).to_string(), port),
            _ => panic!("Unexpected socket address type"),
        };

        tracing::info!("Opening channel from cln to ldk");
        cln_one
            .connect_peer(pubkey.to_string(), listen_addr.clone(), *port)
            .await?;

        cln_one
            .open_channel(1_500_000, &pubkey.to_string(), Some(750_000))
            .await?;

        generate_block(&bitcoin_client)?;

        let cln_two_info = cln_two.get_connect_info().await?;
        cln_one
            .connect_peer(cln_two_info.pubkey, listen_addr.clone(), cln_two_info.port)
            .await?;

        tracing::info!("Opening channel from lnd to ldk");
        let cln_info = cln_one.get_connect_info().await?;

        node.connect(
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
        node.connect(
            lnd_info.pubkey.parse()?,
            SocketAddress::TcpIpV4 {
                addr: [127, 0, 0, 1],
                port: lnd_info.port,
            },
            true,
        )?;

        generate_block(&bitcoin_client)?;
        lnd_one.wait_chain_sync().await?;

        node.open_announced_channel(
            lnd_info.pubkey.parse()?,
            SocketAddress::TcpIpV4 {
                addr: [127, 0, 0, 1],
                port: lnd_info.port,
            },
            1_000_000,
            Some(500_000_000),
            None,
        )?;

        generate_block(&bitcoin_client)?;

        tracing::info!("Ldk channels opened");
        node.sync_wallets()?;
        tracing::info!("Ldk wallet synced");

        cln_one.wait_channels_active().await?;
        lnd_one.wait_channels_active().await?;

        node.stop()?;
    }

    let total_elapsed = total_start.elapsed();
    tracing::info!(
        "Total regtest setup completed in {:.2}s (using parallel JIT initialization)",
        total_elapsed.as_secs_f64()
    );

    // Send notification that regtest set up is complete
    sender.send(()).expect("Could not send oneshot");

    // Wait until we are told to shutdown
    // When this returns, regtest will be dropped and all processes terminated
    notify.notified().await;

    // Explicitly fast terminate for clean shutdown
    regtest.fast_terminate().await;

    Ok(())
}

// ============================================================================
// Phase 2: JIT Lazy Initialization Pattern
// ============================================================================

use crate::jit::JitCell;

/// Helper function to drop a value in a separate async task.
///
/// This allows multiple components to be dropped in parallel using tokio::join!,
/// which is significantly faster than sequential drops. Uses spawn instead of
/// spawn_blocking because the Drop implementations use block_in_place which
/// requires being on an async worker thread.
///
/// During shutdown, tasks may be cancelled by the runtime - this is handled
/// gracefully and logged at debug level.
async fn spawn_drop<T: Send + 'static>(t: T) {
    // Use spawn (not spawn_blocking) because ProcessHandleInner::drop uses
    // block_in_place() which requires being on a Tokio async worker thread.
    // spawn_blocking runs on a separate thread pool where block_in_place() fails.
    match tokio::spawn(async move {
        drop(t);
    })
    .await
    {
        Ok(()) => {
            // Drop completed successfully
        }
        Err(e) if e.is_cancelled() => {
            // Task was cancelled during runtime shutdown - this is expected and safe
            tracing::debug!("Drop task was cancelled during shutdown");
        }
        Err(e) => {
            // Drop implementation panicked - log but don't panic ourselves during shutdown
            tracing::warn!("Drop task panicked: {}", e);
        }
    }
}

/// Just-In-Time initialized regtest environment.
///
/// This structure uses lazy initialization to parallelize the startup of independent
/// components (bitcoind, lightning nodes) while respecting dependency chains.
/// Components are only initialized when first accessed via their getter methods.
///
/// # Example
///
/// ```no_run
/// use cdk_integration_tests::init_regtest::RegtestJit;
/// use std::path::PathBuf;
///
/// # async fn example() -> anyhow::Result<()> {
/// let temp_dir = PathBuf::from("/tmp/regtest");
/// let regtest = RegtestJit::new(temp_dir)?;
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
    process_mgr: Arc<ProcessManager>,

    /// Temporary directory for all test data
    temp_dir: PathBuf,

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

    // Channel setup completion marker
    /// Marker indicating all channels have been opened
    channels_opened: JitCell<Arc<()>>,
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
    ///
    /// # Returns
    ///
    /// * `Ok(RegtestJit)` - A new regtest environment ready for lazy initialization
    /// * `Err(anyhow::Error)` - If setup fails (e.g., cannot create directories)
    pub fn new(temp_dir: PathBuf) -> Result<Self> {
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

        Ok(Self {
            process_mgr,
            temp_dir,
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
            channels_opened,
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
        self.channels_opened.get().await?;
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
    /// let regtest = RegtestJit::new(PathBuf::from("/tmp/regtest"))?;
    /// // ... use regtest ...
    /// regtest.fast_terminate().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fast_terminate(self) {
        use tokio::join;

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
            channels_opened,
            ..
        } = self;

        tracing::info!("Starting fast parallel shutdown");
        let shutdown_start = Instant::now();

        // Drop all components in parallel
        // Channels, clients, then daemons, then bitcoin components
        join!(
            spawn_drop(channels_opened),
            spawn_drop(lnd_one),
            spawn_drop(lnd_two),
            spawn_drop(cln_one),
            spawn_drop(cln_two),
            spawn_drop(lnd_daemon_one),
            spawn_drop(lnd_daemon_two),
            spawn_drop(clnd_one),
            spawn_drop(clnd_two),
            spawn_drop(bitcoin_client),
            spawn_drop(bitcoind),
        );

        tracing::info!(
            "Fast parallel shutdown completed in {:.2}s",
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
            .field("channels_opened", &self.is_channels_opened())
            .finish()
    }
}
