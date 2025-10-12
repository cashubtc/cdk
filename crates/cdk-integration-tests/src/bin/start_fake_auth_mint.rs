//! Binary for starting a fake mint with authentication for testing
//!
//! This binary provides a programmatic way to start a fake mint instance with authentication for testing purposes:
//! 1. Sets up a fake mint instance with authentication using the cdk-mintd library
//! 2. Configures OpenID Connect authentication settings
//! 3. Waits for the mint to be ready and responsive
//! 4. Keeps it running until interrupted (Ctrl+C)
//! 5. Gracefully shuts down on receiving shutdown signal
//!
//! This approach offers better control and integration compared to external scripts,
//! making it easier to run authentication integration tests with consistent configuration.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cdk_integration_tests::cli::CommonArgs;
use cdk_integration_tests::shared;
use cdk_mintd::config::AuthType;
use clap::Parser;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "start-fake-auth-mint")]
#[command(about = "Start a fake mint with authentication for testing", long_about = None)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// Database type (sqlite)
    database_type: String,

    /// Working directory path
    work_dir: String,

    /// OpenID discovery URL
    openid_discovery: String,

    /// Port to listen on (default: 8087)
    #[arg(default_value_t = 8087)]
    port: u16,
}

/// Start a fake mint with authentication using the library
async fn start_fake_auth_mint(
    temp_dir: &Path,
    database: &str,
    port: u16,
    openid_discovery: String,
    shutdown: Arc<Notify>,
) -> Result<tokio::task::JoinHandle<()>> {
    println!("Starting fake auth mintd on port {port}");

    // Create settings struct for fake mint with auth using shared function
    let fake_wallet_config = cdk_mintd::config::FakeWallet {
        supported_units: vec![cdk::nuts::CurrencyUnit::Sat, cdk::nuts::CurrencyUnit::Usd],
        fee_percent: 0.0,
        reserve_fee_min: cdk::Amount::from(1),
        min_delay_time: 1,
        max_delay_time: 3,
    };

    let mut settings = shared::create_fake_wallet_settings(
        port,
        database,
        Some(Mnemonic::generate(12)?.to_string()),
        None,
        Some(fake_wallet_config),
    );

    // Enable authentication
    settings.auth = Some(cdk_mintd::config::Auth {
        auth_enabled: true,
        openid_discovery,
        openid_client_id: "cashu-client".to_string(),
        mint_max_bat: 50,
        mint: AuthType::Blind,
        get_mint_quote: AuthType::Blind,
        check_mint_quote: AuthType::Blind,
        melt: AuthType::Blind,
        get_melt_quote: AuthType::Blind,
        check_melt_quote: AuthType::Blind,
        swap: AuthType::Blind,
        restore: AuthType::Blind,
        check_proof_state: AuthType::Blind,
        websocket_auth: AuthType::Blind,
    });

    // Set description for the mint
    settings.mint_info.description = "fake test mint with auth".to_string();

    let temp_dir = temp_dir.to_path_buf();
    let shutdown_clone = shutdown.clone();

    // Run the mint in a separate task
    let handle = tokio::spawn(async move {
        // Create a future that resolves when the shutdown signal is received
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("Fake auth mint shutdown signal received");
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
            Ok(_) => println!("Fake auth mint exited normally"),
            Err(e) => eprintln!("Fake auth mint exited with error: {e}"),
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

    // Start fake auth mint
    let shutdown = shared::create_shutdown_handler();
    let shutdown_clone = shutdown.clone();

    let handle = start_fake_auth_mint(
        &temp_dir,
        &args.database_type,
        args.port,
        args.openid_discovery.clone(),
        shutdown_clone,
    )
    .await?;

    let cancel_token = Arc::new(CancellationToken::new());

    // Wait for fake auth mint to be ready
    if let Err(e) = shared::wait_for_mint_ready_with_shutdown(args.port, 100, cancel_token).await {
        eprintln!("Error waiting for fake auth mint: {e}");
        return Err(e);
    }

    println!("Fake auth mint started successfully!");
    println!("Fake auth mint: http://127.0.0.1:{}", args.port);
    println!("Temp directory: {temp_dir:?}");
    println!("Database type: {}", args.database_type);
    println!("OpenID Discovery: {}", args.openid_discovery);
    println!();
    println!("Environment variables needed for tests:");
    println!("  CDK_TEST_OIDC_USER=<username>");
    println!("  CDK_TEST_OIDC_PASSWORD=<password>");
    println!();
    println!("You can now run auth integration tests with:");
    println!("  cargo test -p cdk-integration-tests --test fake_auth");
    println!();

    println!("Press Ctrl+C to stop the mint...");

    // Wait for Ctrl+C signal
    shared::wait_for_shutdown_signal(shutdown).await;

    println!("\nReceived Ctrl+C, shutting down mint...");

    // Wait for mint to finish gracefully
    if let Err(e) = handle.await {
        eprintln!("Error waiting for mint to shut down: {e}");
    }

    println!("Mint shut down successfully");

    Ok(())
}
