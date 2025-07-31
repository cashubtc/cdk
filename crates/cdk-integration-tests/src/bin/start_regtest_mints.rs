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

use anyhow::Result;
use cdk_integration_tests::cli::CommonArgs;
use cdk_integration_tests::init_regtest::start_regtest_end;
use cdk_integration_tests::shared;
use clap::Parser;
use tokio::signal::unix::SignalKind;
use tokio::signal::{self};
use tokio::sync::{oneshot, Notify};
use tokio::time::timeout;

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

        match cdk_mintd::run_mintd_with_shutdown(&temp_dir, &settings, shutdown_future, None).await
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

        match cdk_mintd::run_mintd_with_shutdown(&lnd_work_dir, &settings, shutdown_future, None)
            .await
        {
            Ok(_) => println!("LND mint exited normally"),
            Err(e) => eprintln!("LND mint exited with error: {e}"),
        }
    });

    Ok(handle)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on CLI arguments
    shared::setup_logging(&args.common);

    let temp_dir = shared::init_working_directory(&args.work_dir)?;

    // Write environment variables to a .env file in the temp_dir
    let mint_url_1 = format!("http://{}:{}", args.mint_addr, args.cln_port);
    let mint_url_2 = format!("http://{}:{}", args.mint_addr, args.lnd_port);
    let env_vars: Vec<(&str, &str)> = vec![
        ("CDK_TEST_MINT_URL", &mint_url_1),
        ("CDK_TEST_MINT_URL_2", &mint_url_2),
    ];

    shared::write_env_file(&temp_dir, &env_vars)?;

    // Start regtest
    println!("Starting regtest...");

    let shutdown_regtest = shared::create_shutdown_handler();
    let shutdown_clone = shutdown_regtest.clone();
    let (tx, rx) = oneshot::channel();

    let shutdown_clone_one = Arc::clone(&shutdown_clone);

    let temp_dir_clone = temp_dir.clone();
    tokio::spawn(async move {
        start_regtest_end(&temp_dir_clone, tx, shutdown_clone_one)
            .await
            .expect("Error starting regtest");
    });

    match timeout(Duration::from_secs(300), rx).await {
        Ok(_) => {
            tracing::info!("Regtest set up");
        }
        Err(_) => {
            tracing::error!("regtest setup timed out after 5 minutes");
            anyhow::bail!("Could not set up regtest");
        }
    }

    // Start CLN mint
    let cln_handle = start_cln_mint(&temp_dir, args.cln_port, shutdown_clone.clone()).await?;

    // Wait for CLN mint to be ready
    if let Err(e) = shared::wait_for_mint_ready(args.cln_port, 100).await {
        eprintln!("Error waiting for CLN mint: {e}");
        return Err(e);
    }

    // Start LND mint
    let lnd_handle = start_lnd_mint(&temp_dir, args.lnd_port, shutdown_clone.clone()).await?;

    // Wait for LND mint to be ready
    if let Err(e) = shared::wait_for_mint_ready(args.lnd_port, 100).await {
        eprintln!("Error waiting for LND mint: {e}");
        return Err(e);
    }

    println!("All regtest mints started successfully!");
    println!("CLN mint: http://{}:{}", args.mint_addr, args.cln_port);
    println!("LND mint: http://{}:{}", args.mint_addr, args.lnd_port);
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
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    // Wait for either shutdown signal or mint termination
    tokio::select! {
        _ = shutdown_future => {
            println!("Shutdown signal received, waiting for mints to stop...");
        }
        _ = monitor_mints => {
            println!("One or more mints terminated unexpectedly");
        }
    }

    // Wait for mints to finish gracefully
    if let Err(e) = tokio::try_join!(cln_handle, lnd_handle) {
        eprintln!("Error waiting for mints to shut down: {e}");
    }

    println!("All services shut down successfully");

    Ok(())
}
