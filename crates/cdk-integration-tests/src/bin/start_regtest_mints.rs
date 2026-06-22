//! Binary for starting regtest mints
//!
//! This binary provides a programmatic way to start regtest mints for testing purposes:
//! 1. Sets up a regtest environment with CLN and LND nodes
//! 2. Starts CLN and LND mint instances using the cdk-mintd library
//! 3. Configures the mints to connect to the respective Lightning Network backends
//! 4. Waits for both mints to be ready and responsive
//! 5. Keeps them running until interrupted (Ctrl+C)
//! 6. Gracefully shuts down all services on receiving shutdown signal
//!
//! This approach offers better control and integration compared to external scripts,
//! making it easier to run integration tests with consistent configuration.

use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::{Amount, CurrencyUnit, PaymentMethod};
use cdk::wallet::Wallet;
use cdk_integration_tests::cli::CommonArgs;
use cdk_integration_tests::init_regtest::{get_cln_dir, start_regtest_end};
use cdk_integration_tests::ln_regtest::ln_client::{ClnClient, LightningClient};
use cdk_integration_tests::shared;
use cdk_ldk_node::CdkLdkNodeBuilder;
use cdk_mintd::config::LoggingConfig;
use cdk_sqlite::wallet::memory;
use clap::Parser;
use ldk_node::lightning::ln::msgs::SocketAddress;
use tokio::runtime::Runtime;
use tokio::signal;
use tokio::signal::unix::SignalKind;
use tokio::sync::{oneshot, Notify};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

const LDK_NODE_P2P_PORT_OFFSET: u16 = 10;

fn derive_ldk_node_p2p_port(ldk_port: u16) -> Result<u16> {
    match ldk_port.checked_add(LDK_NODE_P2P_PORT_OFFSET) {
        Some(port) => Ok(port),
        None => bail!("LDK mint port {ldk_port} is too high to derive a P2P port"),
    }
}

#[derive(Parser)]
#[command(name = "start-regtest-mints")]
#[command(about = "Start regtest mints", long_about = None)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// Database type (sqlite)
    database_type: String,

    /// Working directory path
    work_dir: String,

    /// Mint address (default: 127.0.0.1)
    #[arg(default_value = "127.0.0.1")]
    mint_addr: String,

    /// CLN port (default: 8085)
    #[arg(default_value_t = 8085)]
    cln_port: u16,

    /// LND port (default: 8087)
    #[arg(default_value_t = 8087)]
    lnd_port: u16,

    /// LDK port (default: 8089)
    #[arg(default_value_t = 8089)]
    ldk_port: u16,

    /// Skip Lightning setup
    #[arg(long, default_value_t = false)]
    skip_ln: bool,
}

/// Start regtest CLN mint using the library
async fn start_cln_mint(
    temp_dir: &Path,
    port: u16,
    database_type: &str,
    shutdown: Arc<Notify>,
) -> Result<tokio::task::JoinHandle<()>> {
    let cln_rpc_path = temp_dir
        .join("cln")
        .join("one")
        .join("regtest")
        .join("lightning-rpc");

    let cln_config = cdk_mintd::config::Cln {
        rpc_path: cln_rpc_path,
        bolt12: false,
        expose_private_channels: false,
        fee_percent: 0.0,
        reserve_fee_min: 0.into(),
    };

    // Create settings struct for CLN mint using shared function
    let mut settings = shared::create_cln_settings(
        port,
        temp_dir
            .join("cln")
            .join("one")
            .join("regtest")
            .join("lightning-rpc"),
        "eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal".to_string(),
        cln_config,
    );

    apply_onchain_settings(&mut settings);
    apply_database_settings(&mut settings, database_type, port)?;

    println!("Starting CLN mintd on port {port}");

    let temp_dir = temp_dir.to_path_buf();
    let shutdown_clone = shutdown.clone();

    // Run the mint in a separate task
    let handle = tokio::spawn(async move {
        // Create a future that resolves when the shutdown signal is received
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("CLN mint shutdown signal received");
        };

        match cdk_mintd::run_mintd_with_shutdown(
            &temp_dir,
            &settings,
            shutdown_future,
            None,
            None,
            vec![],
        )
        .await
        {
            Ok(_) => println!("CLN mint exited normally"),
            Err(e) => eprintln!("CLN mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

/// Start regtest LND mint using the library
async fn start_lnd_mint(
    temp_dir: &Path,
    port: u16,
    database_type: &str,
    shutdown: Arc<Notify>,
) -> Result<tokio::task::JoinHandle<()>> {
    let lnd_cert_file = temp_dir.join("lnd").join("two").join("tls.cert");
    let lnd_macaroon_file = temp_dir
        .join("lnd")
        .join("two")
        .join("data")
        .join("chain")
        .join("bitcoin")
        .join("regtest")
        .join("admin.macaroon");
    let lnd_work_dir = temp_dir.join("lnd_mint");

    // Create work directory for LND mint
    fs::create_dir_all(&lnd_work_dir)?;

    let lnd_config = cdk_mintd::config::Lnd {
        address: "https://localhost:10010".to_string(),
        cert_file: lnd_cert_file,
        macaroon_file: lnd_macaroon_file,
        fee_percent: 0.0,
        reserve_fee_min: 0.into(),
    };

    // Create settings struct for LND mint using shared function
    let mut settings = shared::create_lnd_settings(
        port,
        lnd_config,
        "cattle gold bind busy sound reduce tone addict baby spend february strategy".to_string(),
    );

    apply_onchain_settings(&mut settings);
    apply_database_settings(&mut settings, database_type, port)?;

    println!("Starting LND mintd on port {port}");

    let lnd_work_dir = lnd_work_dir.clone();
    let shutdown_clone = shutdown.clone();

    // Run the mint in a separate task
    let handle = tokio::spawn(async move {
        // Create a future that resolves when the shutdown signal is received
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("LND mint shutdown signal received");
        };

        match cdk_mintd::run_mintd_with_shutdown(
            &lnd_work_dir,
            &settings,
            shutdown_future,
            None,
            None,
            vec![],
        )
        .await
        {
            Ok(_) => println!("LND mint exited normally"),
            Err(e) => eprintln!("LND mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

/// Start regtest LDK mint using the library.
async fn start_ldk_mint(
    temp_dir: &Path,
    port: u16,
    database_type: &str,
    shutdown: Arc<Notify>,
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
) -> Result<tokio::task::JoinHandle<()>> {
    let ldk_work_dir = temp_dir.join("ldk_mint");
    let ldk_node_p2p_port = derive_ldk_node_p2p_port(port)?;

    // Create work directory for LDK mint
    fs::create_dir_all(&ldk_work_dir)?;

    // Configure LDK node for regtest
    let ldk_config = cdk_mintd::config::LdkNode {
        fee_percent: 0.0,
        reserve_fee_min: 0.into(),
        bitcoin_network: Some("regtest".to_string()),
        // Use bitcoind RPC for regtest
        chain_source_type: Some("bitcoinrpc".to_string()),
        bitcoind_rpc_host: Some("127.0.0.1".to_string()),
        bitcoind_rpc_port: Some(18443),
        bitcoind_rpc_user: Some("testuser".to_string()),
        bitcoind_rpc_password: Some("testpass".to_string()),
        esplora_url: None,
        log_dir_path: None,
        ldk_node_announce_addresses: None,
        storage_dir_path: Some(ldk_work_dir.to_string_lossy().to_string()),
        ldk_node_host: Some("127.0.0.1".to_string()),
        ldk_node_port: Some(ldk_node_p2p_port),
        gossip_source_type: None,
        rgs_url: None,
        webserver_host: Some("127.0.0.1".to_string()),
        webserver_port: Some(port + 1), // Use next port for web interface
        ldk_node_mnemonic: Some(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                .to_string(),
        ),
    };

    // Create settings struct for LDK mint using a new shared function
    let mut settings = create_ldk_settings(port, ldk_config);

    apply_onchain_settings(&mut settings);
    apply_database_settings(&mut settings, database_type, port)?;

    println!("Starting LDK mintd on port {port}");

    let ldk_work_dir = ldk_work_dir.clone();
    let shutdown_clone = shutdown.clone();

    // Run the mint in a separate task
    let handle = tokio::spawn(async move {
        // Create a future that resolves when the shutdown signal is received
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("LDK mint shutdown signal received");
        };

        match cdk_mintd::run_mintd_with_shutdown(
            &ldk_work_dir,
            &settings,
            shutdown_future,
            None,
            runtime,
            vec![],
        )
        .await
        {
            Ok(_) => println!("LDK mint exited normally"),
            Err(e) => eprintln!("LDK mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

async fn wait_for_ldk_bolt12_ready(
    temp_dir: &Path,
    ldk_port: u16,
    ldk_node_p2p_port: u16,
    ldk_node_id: &str,
    shutdown_notify: Arc<CancellationToken>,
) -> Result<()> {
    let mint_url = format!("http://127.0.0.1:{ldk_port}");
    let readiness_amount = Amount::from(1);
    let start_time = std::time::Instant::now();
    let max_wait = Duration::from_secs(120);
    let mut attempt = 1;
    let mut last_error = None;

    println!("Waiting for LDK mint BOLT12 readiness on port {ldk_port}...");

    let wallet = Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let cln_client = ClnClient::new(get_cln_dir(temp_dir, "one"), None).await?;

    loop {
        if shutdown_notify.is_cancelled() {
            bail!("Canceled waiting for LDK mint BOLT12 readiness");
        }

        if start_time.elapsed() > max_wait {
            let last_error = last_error
                .as_deref()
                .unwrap_or("no readiness attempt completed before timeout");
            bail!("Timeout waiting for LDK mint BOLT12 readiness: {last_error}");
        }

        match cln_client
            .connect_peer(
                ldk_node_id.to_string(),
                "127.0.0.1".to_string(),
                ldk_node_p2p_port,
            )
            .await
        {
            Ok(_) => tracing::info!("CLN reconnected to LDK mint node for readiness check"),
            Err(err) => {
                let err = err.to_string();
                if !err.to_lowercase().contains("already connected") {
                    last_error = Some(format!("peer reconnect failed: {err}"));
                    tracing::warn!(
                        "LDK BOLT12 readiness attempt {attempt}: CLN reconnect to LDK mint node failed: {err}"
                    );
                    sleep(Duration::from_secs(2)).await;
                    attempt += 1;
                    continue;
                }
            }
        }

        let mint_quote = match wallet
            .mint_quote(PaymentMethod::BOLT12, Some(readiness_amount), None, None)
            .await
        {
            Ok(quote) => quote,
            Err(err) => {
                last_error = Some(format!("quote creation failed: {err}"));
                tracing::warn!(
                    "LDK BOLT12 readiness attempt {attempt}: quote creation failed: {err}"
                );
                sleep(Duration::from_secs(2)).await;
                attempt += 1;
                continue;
            }
        };

        match cln_client
            .pay_bolt12_offer(None, mint_quote.request.clone())
            .await
        {
            Ok(_) => (),
            Err(err) => {
                last_error = Some(format!("payment failed: {err}"));
                tracing::warn!("LDK BOLT12 readiness attempt {attempt}: payment failed: {err}");
                sleep(Duration::from_secs(2)).await;
                attempt += 1;
                continue;
            }
        }

        let quote_poll_start = std::time::Instant::now();
        let mut last_status_error = None;
        while quote_poll_start.elapsed() <= Duration::from_secs(20) {
            if shutdown_notify.is_cancelled() {
                bail!("Canceled waiting for LDK mint BOLT12 readiness");
            }

            match wallet.check_mint_quote_status(&mint_quote.id).await {
                Ok(quote_state) => {
                    if quote_state.amount_paid >= readiness_amount {
                        println!("LDK mint BOLT12 readiness confirmed on port {ldk_port}");
                        return Ok(());
                    }
                }
                Err(err) => {
                    last_status_error = Some(err.to_string());
                    tracing::warn!(
                        "LDK BOLT12 readiness attempt {attempt}: quote status check failed: {err}"
                    );
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        tracing::warn!(
            "LDK BOLT12 readiness attempt {attempt}: payment was sent but quote {} was not observed as paid",
            mint_quote.id
        );
        last_error = Some(match last_status_error {
            Some(err) => format!(
                "payment was sent but quote {} was not observed as paid; last status check failed: {err}",
                mint_quote.id
            ),
            None => format!(
                "payment was sent but quote {} was not observed as paid",
                mint_quote.id
            ),
        });
        attempt += 1;
    }
}

/// Create settings for an LDK mint
fn create_ldk_settings(
    port: u16,
    ldk_config: cdk_mintd::config::LdkNode,
) -> cdk_mintd::config::Settings {
    cdk_mintd::config::Settings {
        info: cdk_mintd::config::Info {
            quote_ttl: None,
            url: format!("http://127.0.0.1:{port}"),
            listen_host: "127.0.0.1".to_string(),
            listen_port: port,
            seed: None,
            mnemonic: Some(
                "eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
                    .to_string(),
            ),
            input_fee_ppk: None,
            use_keyset_v2: None,
            http_cache: cdk_axum::cache::Config::default(),
            enable_info_page: None,
            logging: LoggingConfig::default(),
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        limits: cdk_mintd::config::Limits::default(),
        ln: vec![cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::LdkNode,
            unit: cdk::nuts::CurrencyUnit::Sat,
            invoice_description: None,
            min_mint: 1.into(),
            max_mint: 500_000.into(),
            min_melt: 1.into(),
            max_melt: 500_000.into(),
        }],
        cln: None,
        lnbits: None,
        lnd: None,
        ldk_node: Some(ldk_config),
        fake_wallet: None,
        onchain: None,
        ..Default::default()
    }
}

fn apply_onchain_settings(settings: &mut cdk_mintd::config::Settings) {
    settings.onchain = Some(cdk_mintd::config::Onchain {
        onchain_backend: cdk_mintd::config::OnchainBackend::Bdk,
        min_mint: 1.into(),
        max_mint: 500_000.into(),
        min_melt: 1.into(),
        max_melt: 500_000.into(),
    });
    settings.bdk = Some(cdk_mintd::config::Bdk {
        mnemonic: Some(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                .to_string(),
        ),
        network: Some("regtest".to_string()),
        bitcoind_rpc_host: Some("127.0.0.1".to_string()),
        bitcoind_rpc_port: Some(18443),
        bitcoind_rpc_user: Some("testuser".to_string()),
        bitcoind_rpc_password: Some("testpass".to_string()),
        chain_source_type: Some("bitcoinrpc".to_string()),
        batch_config: cdk_mintd::config::BatchConfig {
            poll_interval_secs: 1,
            standard_deadline_secs: Some(1),
            economy_deadline_secs: Some(2),
            fee_options: vec![
                "immediate".to_string(),
                "standard".to_string(),
                "economy".to_string(),
            ],
            ..Default::default()
        },
        num_confs: 1,
        sync_interval_secs: 1,
        ..Default::default()
    });
}

fn apply_database_settings(
    settings: &mut cdk_mintd::config::Settings,
    database_type: &str,
    port: u16,
) -> Result<()> {
    let engine =
        cdk_mintd::config::DatabaseEngine::from_str(database_type).map_err(anyhow::Error::msg)?;

    let postgres_config = if engine == cdk_mintd::config::DatabaseEngine::Postgres {
        let url = std::env::var("CDK_MINTD_DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://cdk_user:cdk_password@localhost:5432/cdk_mint".to_string()
        });
        let url = format!("{url} schema=cdk_mint_{port}");
        Some(cdk_mintd::config::PostgresConfig {
            url,
            ..Default::default()
        })
    } else {
        None
    };

    settings.database.engine = engine;
    settings.database.postgres = postgres_config;

    Ok(())
}

fn signal_mints_ready(temp_dir: &Path) -> Result<()> {
    fs::write(temp_dir.join(".ready"), "ready\n")?;
    Ok(())
}

/// Create settings for an onchain-only mint
fn create_onchain_settings(port: u16) -> cdk_mintd::config::Settings {
    cdk_mintd::config::Settings {
        info: cdk_mintd::config::Info {
            quote_ttl: None,
            url: format!("http://127.0.0.1:{port}"),
            listen_host: "127.0.0.1".to_string(),
            listen_port: port,
            seed: None,
            mnemonic: Some(
                "eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
                    .to_string(),
            ),
            input_fee_ppk: None,
            use_keyset_v2: None,
            http_cache: cdk_axum::cache::Config::default(),
            enable_info_page: None,
            logging: LoggingConfig::default(),
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        limits: cdk_mintd::config::Limits::default(),
        ln: vec![cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::None,
            ..Default::default()
        }],
        onchain: None, // Will be set by apply_onchain_settings
        ..Default::default()
    }
}

/// Start a mint configured with only onchain backend
async fn start_onchain_mint(
    work_dir: &Path,
    port: u16,
    database_type: &str,
    shutdown: Arc<Notify>,
) -> Result<tokio::task::JoinHandle<()>> {
    let mut settings = create_onchain_settings(port);
    apply_onchain_settings(&mut settings);
    apply_database_settings(&mut settings, database_type, port)?;

    println!("Starting onchain-only mintd on port {port}");

    let work_dir = work_dir.to_path_buf();
    fs::create_dir_all(&work_dir)?;
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("Onchain mint shutdown signal received");
        };

        match cdk_mintd::run_mintd_with_shutdown(
            &work_dir,
            &settings,
            shutdown_future,
            None,
            None,
            vec![],
        )
        .await
        {
            Ok(_) => println!("Onchain mint exited normally"),
            Err(e) => eprintln!("Onchain mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = Args::parse();

        // Initialize logging based on CLI arguments
        shared::setup_logging(&args.common);

        let temp_dir = shared::init_working_directory(&args.work_dir)?;

        // Write environment variables to a .env file in the temp_dir
        let mint_url_1 = format!("http://{}:{}", args.mint_addr, args.cln_port);
        let mint_url_2 = format!("http://{}:{}", args.mint_addr, args.lnd_port);
        let mint_url_3 = format!("http://{}:{}", args.mint_addr, args.ldk_port);
        let env_vars: Vec<(&str, &str)> = vec![
            ("CDK_TEST_MINT_URL", &mint_url_1),
            ("CDK_TEST_MINT_URL_2", &mint_url_2),
            ("CDK_TEST_MINT_URL_3", &mint_url_3),
        ];

        shared::write_env_file(&temp_dir, &env_vars)?;

        // Start regtest
        println!("Starting regtest...");

        let shutdown_regtest = shared::create_shutdown_handler();
        let shutdown_clone = shutdown_regtest.clone();
        let (tx, rx) = oneshot::channel();

        let shutdown_clone_one = Arc::clone(&shutdown_clone);

        if args.skip_ln {
            let temp_dir_clone = temp_dir.clone();
            let shutdown_clone_two = Arc::clone(&shutdown_clone);
            tokio::spawn(async move {
                start_regtest_end(
                    &temp_dir_clone,
                    tx,
                    shutdown_clone_two,
                    None,
                    args.skip_ln,
                )
                .await
                .expect("Error starting regtest");
            });

            match timeout(Duration::from_secs(300), rx).await {
                Ok(k) => {
                    k?;
                    tracing::info!("Regtest (onchain only) set up");
                }
                Err(_) => {
                    tracing::error!("regtest setup timed out after 5 minutes");
                    anyhow::bail!("Could not set up regtest");
                }
            }

            let onchain_handle = start_onchain_mint(
                &temp_dir.join("onchain_mint"),
                args.cln_port,
                &args.database_type,
                shutdown_clone.clone(),
            )
            .await?;

            let cancel_token = Arc::new(CancellationToken::new());
            let ctrl_c_token = Arc::clone(&cancel_token);
            let s_u = shutdown_clone.clone();

            tokio::spawn(async move {
                signal::ctrl_c()
                    .await
                    .expect("failed to install CTRL+C handler");
                tracing::info!("Shutdown signal received during onchain mint setup");
                println!("\nReceived Ctrl+C, shutting down...");
                ctrl_c_token.cancel();
                s_u.notify_waiters();
            });

            shared::wait_for_mint_ready_with_shutdown(
                args.cln_port,
                100,
                Arc::clone(&cancel_token),
            )
            .await?;

            println!("Onchain-only mint is ready on port {}!", args.cln_port);
            signal_mints_ready(&temp_dir)?;

            // Wait for shutdown
            shutdown_clone_one.notified().await;
            let _ = onchain_handle.await;
            return Ok(());
        }

        let ldk_work_dir = temp_dir.join("ldk_mint");
        fs::create_dir_all(ldk_work_dir.join("logs"))?;
        let ldk_node_p2p_port = derive_ldk_node_p2p_port(args.ldk_port)?;
        let test_mnemonic: Mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
            .parse()
            .expect("Failed to parse test mnemonic");
        let node_builder = CdkLdkNodeBuilder::new(
            bitcoin::Network::Regtest,
            cdk_ldk_node::ChainSource::BitcoinRpc(cdk_ldk_node::BitcoinRpcConfig {
                host: "127.0.0.1".to_string(),
                port: 18443,
                user: "testuser".to_string(),
                password: "testpass".to_string(),
            }),
            cdk_ldk_node::GossipSource::P2P,
            ldk_work_dir.to_string_lossy().to_string(),
            cdk_common::common::FeeReserve {
                min_fee_reserve: Amount::ZERO,
                percent_fee_reserve: 0.0,
            },
            vec![SocketAddress::TcpIpV4 {
                addr: [127, 0, 0, 1],
                port: ldk_node_p2p_port,
            }],
        )
        .with_seed(test_mnemonic.clone());
        let cdk_ldk = match node_builder.build() {
            Ok(node) => node,
            Err(e) => {
                tracing::warn!(
                    "Failed to start LDK node: {}. Attempting to wipe data and restart...",
                    e
                );
                // Clean up the storage directory
                if ldk_work_dir.exists() {
                    fs::remove_dir_all(&ldk_work_dir)?;
                    fs::create_dir_all(ldk_work_dir.join("logs"))?;
                }
                // Recreate builder since it was consumed
                let node_builder = CdkLdkNodeBuilder::new(
                    bitcoin::Network::Regtest,
                    cdk_ldk_node::ChainSource::BitcoinRpc(cdk_ldk_node::BitcoinRpcConfig {
                        host: "127.0.0.1".to_string(),
                        port: 18443,
                        user: "testuser".to_string(),
                        password: "testpass".to_string(),
                    }),
                    cdk_ldk_node::GossipSource::P2P,
                    ldk_work_dir.to_string_lossy().to_string(),
                    cdk_common::common::FeeReserve {
                        min_fee_reserve: Amount::ZERO,
                        percent_fee_reserve: 0.0,
                    },
                    vec![SocketAddress::TcpIpV4 {
                        addr: [127, 0, 0, 1],
                        port: ldk_node_p2p_port,
                    }],
                )
                .with_seed(test_mnemonic);

                node_builder.build()?
            }
        };

        let inner_node = cdk_ldk.node();
        let ldk_node_id = inner_node.node_id().to_string();
        // The setup node is stopped by `start_regtest_end` after channels are
        // opened. `ldk-node` 0.7 can panic when dropping a stopped node, so keep
        // this setup wrapper alive for the short-lived regtest process and let
        // the mint restart from the same storage and seed.
        std::mem::forget(cdk_ldk);

        let temp_dir_clone = temp_dir.clone();
        let shutdown_clone_two = Arc::clone(&shutdown_clone);
        tokio::spawn(async move {
            start_regtest_end(
                &temp_dir_clone,
                tx,
                shutdown_clone_two,
                Some(inner_node),
                args.skip_ln,
            )
            .await
            .expect("Error starting regtest");
        });

        match timeout(Duration::from_secs(300), rx).await {
            Ok(k) => {
                k?;
                tracing::info!("Regtest set up");
            }
            Err(_) => {
                tracing::error!("regtest setup timed out after 5 minutes");
                anyhow::bail!("Could not set up regtest");
            }
        }

        // Start LND mint
        let lnd_handle =
            start_lnd_mint(&temp_dir, args.lnd_port, &args.database_type, shutdown_clone.clone())
                .await?;

        // Start LDK mint from the node storage that was set up with channels.
        let ldk_handle = start_ldk_mint(
            &temp_dir,
            args.ldk_port,
            &args.database_type,
            shutdown_clone.clone(),
            Some(rt_clone),
        )
        .await?;

        // Start CLN mint
        let cln_handle =
            start_cln_mint(&temp_dir, args.cln_port, &args.database_type, shutdown_clone.clone())
                .await?;

        let cancel_token = Arc::new(CancellationToken::new());

        // Set up Ctrl+C handler before waiting for mints to be ready
        let ctrl_c_token = Arc::clone(&cancel_token);

        let s_u = shutdown_clone.clone();
        tokio::spawn(async move {
            signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C handler");
            tracing::info!("Shutdown signal received during mint setup");
            println!("\nReceived Ctrl+C, shutting down...");
            ctrl_c_token.cancel();
            s_u.notify_waiters();
        });

        match tokio::try_join!(
            shared::wait_for_mint_ready_with_shutdown(
                args.lnd_port,
                100,
                Arc::clone(&cancel_token)
            ),
            shared::wait_for_mint_ready_with_shutdown(
                args.ldk_port,
                100,
                Arc::clone(&cancel_token)
            ),
            shared::wait_for_mint_ready_with_shutdown(
                args.cln_port,
                100,
                Arc::clone(&cancel_token)
            ),
        ) {
            Ok(_) => println!("All mint HTTP endpoints are ready!"),
            Err(e) => {
                if cancel_token.is_cancelled() {
                    bail!("Startup canceled by user");
                }
                eprintln!("Error waiting for mints to be ready: {e}");
                return Err(e);
            }
        }

        if cancel_token.is_cancelled() {
            bail!("Token canceled");
        }

        wait_for_ldk_bolt12_ready(
            &temp_dir,
            args.ldk_port,
            ldk_node_p2p_port,
            &ldk_node_id,
            Arc::clone(&cancel_token),
        )
        .await?;
        signal_mints_ready(&temp_dir)?;

        println!("All regtest mints started successfully!");
        println!("CLN mint: http://{}:{}", args.mint_addr, args.cln_port);
        println!("LND mint: http://{}:{}", args.mint_addr, args.lnd_port);
        println!("LDK mint: http://{}:{}", args.mint_addr, args.ldk_port);
        shared::display_mint_info(args.cln_port, &temp_dir, &args.database_type); // Using CLN port for display
        println!();
        println!("Environment variables set:");
        println!(
            "  CDK_TEST_MINT_URL=http://{}:{}",
            args.mint_addr, args.cln_port
        );
        println!(
            "  CDK_TEST_MINT_URL_2=http://{}:{}",
            args.mint_addr, args.lnd_port
        );
        println!(
            "  CDK_TEST_MINT_URL_3=http://{}:{}",
            args.mint_addr, args.ldk_port
        );
        println!("  CDK_ITESTS_DIR={}", temp_dir.display());
        println!();
        println!("You can now run integration tests with:");
        println!("  cargo test -p cdk-integration-tests --test regtest");
        println!("  cargo test -p cdk-integration-tests --test happy_path_mint_wallet");
        println!("  etc.");
        println!();

        println!("Press Ctrl+C to stop the mints...");

        // Create a future to wait for either Ctrl+C signal or unexpected mint termination
        let shutdown_future = async {
            // Wait for either SIGINT (Ctrl+C) or SIGTERM
            let mut sigterm = signal::unix::signal(SignalKind::terminate())
                .expect("Failed to create SIGTERM signal handler");
            tokio::select! {
                _ = signal::ctrl_c() => {
                    tracing::info!("Received SIGINT (Ctrl+C), shutting down mints...");
                }
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, shutting down mints...");
                }
            }
            println!("\nShutdown signal received, shutting down mints...");
            shutdown_clone.notify_waiters();
        };

        // Monitor mint handles for unexpected termination
        let monitor_mints = async {
            loop {
                if cln_handle.is_finished() {
                    println!("CLN mint finished unexpectedly");
                    return;
                }
                if lnd_handle.is_finished() {
                    println!("LND mint finished unexpectedly");
                    return;
                }
                if ldk_handle.is_finished() {
                    println!("LDK mint finished unexpectedly");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };

        // Wait for either shutdown signal or mint termination
        tokio::select! {
            _ = shutdown_clone_one.notified() => {
                println!("Shutdown signal received, waiting for mints to stop...");
            }
            _ = monitor_mints => {
                println!("One or more mints terminated unexpectedly");
            }
            _ = shutdown_future => ()
        }

        // Wait for mints to finish gracefully
        if let Err(e) = tokio::try_join!(ldk_handle, cln_handle, lnd_handle) {
            eprintln!("Error waiting for mints to shut down: {e}");
        }

        println!("All services shut down successfully");

        Ok(())
    })
}
