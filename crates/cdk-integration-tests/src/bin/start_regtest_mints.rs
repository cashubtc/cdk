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
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk_integration_tests::cli::CommonArgs;
use cdk_integration_tests::shared;
use cdk_mintd::config::LoggingConfig;
use clap::Parser;
use tokio::runtime::Runtime;
use tokio::signal;
use tokio::signal::unix::SignalKind;
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

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
}

/// Start regtest CLN mint using the library
async fn start_cln_mint(
    temp_dir: &Path,
    port: u16,
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
        fee_percent: 0.0,
        reserve_fee_min: 0.into(),
    };

    // Create settings struct for CLN mint using shared function
    let settings = shared::create_cln_settings(
        port,
        temp_dir
            .join("cln")
            .join("one")
            .join("regtest")
            .join("lightning-rpc"),
        "eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal".to_string(),
        cln_config,
    );

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
            None,
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
    let settings = shared::create_lnd_settings(
        port,
        lnd_config,
        "cattle gold bind busy sound reduce tone addict baby spend february strategy".to_string(),
    );

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
            None,
        )
        .await
        {
            Ok(_) => println!("LND mint exited normally"),
            Err(e) => eprintln!("LND mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

/// Start regtest LDK mint using the library
async fn start_ldk_mint(
    temp_dir: &Path,
    port: u16,
    shutdown: Arc<Notify>,
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    ldk_node: Option<std::sync::Arc<ldk_node::Node>>,
) -> Result<tokio::task::JoinHandle<()>> {
    // IMPORTANT: Use the same directory as the regtest LDK node so we use the same node instance
    // with all the pre-established channels
    let ldk_work_dir = temp_dir.join("ldk");

    // Create work directory for LDK mint (may already exist from regtest setup)
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
        storage_dir_path: Some(ldk_work_dir.to_string_lossy().to_string()),
        ldk_node_host: Some("127.0.0.1".to_string()),
        ldk_node_port: Some(8092), // Use the same port as the regtest LDK node (not port + 10!)
        gossip_source_type: None,
        rgs_url: None,
        webserver_host: Some("127.0.0.1".to_string()),
        webserver_port: Some(port + 1), // Use next port for web interface
    };

    // Create settings struct for LDK mint using a new shared function
    let settings = create_ldk_settings(port, ldk_config, Mnemonic::generate(12)?.to_string());

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
            ldk_node, // existing_ldk_node
        )
        .await
        {
            Ok(_) => println!("LDK mint exited normally"),
            Err(e) => eprintln!("LDK mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

/// Create settings for an LDK mint
fn create_ldk_settings(
    port: u16,
    ldk_config: cdk_mintd::config::LdkNode,
    mnemonic: String,
) -> cdk_mintd::config::Settings {
    cdk_mintd::config::Settings {
        info: cdk_mintd::config::Info {
            quote_ttl: None,
            url: format!("http://127.0.0.1:{port}"),
            listen_host: "127.0.0.1".to_string(),
            listen_port: port,
            seed: None,
            mnemonic: Some(mnemonic),
            signatory_url: None,
            signatory_certs: None,
            input_fee_ppk: None,
            http_cache: cdk_axum::cache::Config::default(),
            enable_swagger_ui: None,
            logging: LoggingConfig::default(),
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        ln: cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::LdkNode,
            invoice_description: None,
            min_mint: 1.into(),
            max_mint: 500_000.into(),
            min_melt: 1.into(),
            max_melt: 500_000.into(),
        },
        cln: None,
        lnbits: None,
        lnd: None,
        ldk_node: Some(ldk_config),
        fake_wallet: None,
        grpc_processor: None,
        database: cdk_mintd::config::Database::default(),
        auth_database: None,
        mint_management_rpc: None,
        prometheus: None,
        auth: None,
    }
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
        let shutdown_clone_one = Arc::clone(&shutdown_clone);

        // Use start_regtest_with_access to get the regtest instance
        let regtest = match timeout(
            Duration::from_secs(300),
            cdk_integration_tests::init_regtest::start_regtest_with_access(&temp_dir),
        )
        .await
        {
            Ok(Ok(regtest)) => {
                tracing::info!("Regtest set up");
                regtest
            }
            Ok(Err(e)) => {
                tracing::error!("Error starting regtest: {:?}", e);
                anyhow::bail!("Error starting regtest: {:?}", e);
            }
            Err(_) => {
                tracing::error!("regtest setup timed out after 5 minutes");
                anyhow::bail!("Could not set up regtest");
            }
        };

        // Get the LDK node from regtest to reuse it in the mint
        // This ensures the mint uses the same node instance with established channels
        tracing::info!("Getting regtest LDK node to inject into mint...");
        let ldk_node = regtest.ldk_node().await?;

        println!("lnd port: {}", args.ldk_port);

        // Start LND mint
        let lnd_handle = start_lnd_mint(&temp_dir, args.lnd_port, shutdown_clone.clone()).await?;

        // Start LDK mint with the injected node
        let ldk_handle = start_ldk_mint(
            &temp_dir,
            args.ldk_port,
            shutdown_clone.clone(),
            Some(rt_clone),
            Some(ldk_node),
        )
        .await?;

        // Start CLN mint
        let cln_handle = start_cln_mint(&temp_dir, args.cln_port, shutdown_clone.clone()).await?;

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
            Ok(_) => println!("All mints are ready!"),
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

        // Cleanup regtest environment
        tracing::info!("Cleaning up regtest environment...");
        regtest.fast_terminate().await;

        println!("All services shut down successfully");

        Ok(())
    })
}
