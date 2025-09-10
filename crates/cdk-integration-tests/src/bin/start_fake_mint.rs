//! Binary for starting a fake mint for testing
//!
//! This binary provides a programmatic way to start a fake mint instance for testing purposes:
//! 1. Sets up a fake mint instance using the cdk-mintd library
//! 2. Configures the mint with fake wallet backend for testing Lightning Network interactions
//! 3. Waits for the mint to be ready and responsive
//! 4. Keeps it running until interrupted (Ctrl+C)
//! 5. Gracefully shuts down on receiving shutdown signal
//!
//! This approach offers better control and integration compared to external scripts,
//! making it easier to run integration tests with consistent configuration.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use cdk::nuts::CurrencyUnit;
use cdk_integration_tests::cli::CommonArgs;
use cdk_integration_tests::shared;
use clap::Parser;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "start-fake-mint")]
#[command(about = "Start a fake mint for testing", long_about = None)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// Database type (sqlite)
    database_type: String,

    /// Working directory path
    work_dir: String,

    /// Port to listen on (default: 8086)
    #[arg(default_value_t = 8086)]
    port: u16,

    /// Use external signatory
    #[arg(long, default_value_t = false)]
    external_signatory: bool,
}

/// Start a fake mint using the library
async fn start_fake_mint(
    temp_dir: &Path,
    port: u16,
    database: &str,
    shutdown: Arc<Notify>,
    external_signatory: bool,
) -> Result<tokio::task::JoinHandle<()>> {
    let signatory_config = if external_signatory {
        println!("Configuring external signatory");
        Some((
            "https://127.0.0.1:15060".to_string(),  // Default signatory URL
            temp_dir.to_string_lossy().to_string(), // Certs directory as string
        ))
    } else {
        None
    };

    let mnemonic = if external_signatory {
        None
    } else {
        Some(
            "eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
                .to_string(),
        )
    };

    let fake_wallet_config = Some(cdk_mintd::config::FakeWallet {
        supported_units: vec![CurrencyUnit::Sat, CurrencyUnit::Usd],
        fee_percent: 0.0,
        reserve_fee_min: 1.into(),
        min_delay_time: 1,
        max_delay_time: 3,
    });

    // Create settings struct for fake mint using shared function
    let settings = shared::create_fake_wallet_settings(
        port,
        database,
        mnemonic,
        signatory_config,
        fake_wallet_config,
    );

    println!("Starting fake mintd on port {port}");

    let temp_dir = temp_dir.to_path_buf();
    let shutdown_clone = shutdown.clone();

    // Run the mint in a separate task
    let handle = tokio::spawn(async move {
        // Create a future that resolves when the shutdown signal is received
        let shutdown_future = async move {
            shutdown_clone.notified().await;
            println!("Fake mint shutdown signal received");
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
            Ok(_) => println!("Fake mint exited normally"),
            Err(e) => eprintln!("Fake mint exited with error: {e}"),
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

    // Write environment variables to a .env file in the temp_dir BEFORE starting the mint
    let mint_url = format!("http://127.0.0.1:{}", args.port);
    let itests_dir = temp_dir.display().to_string();
    let env_vars: Vec<(&str, &str)> = vec![
        ("CDK_TEST_MINT_URL", &mint_url),
        ("CDK_ITESTS_DIR", &itests_dir),
    ];

    shared::write_env_file(&temp_dir, &env_vars)?;

    // Start fake mint
    let shutdown = shared::create_shutdown_handler();
    let shutdown_clone = shutdown.clone();

    let handle = start_fake_mint(
        &temp_dir,
        args.port,
        &args.database_type,
        shutdown_clone,
        args.external_signatory,
    )
    .await?;

    let cancel_token = Arc::new(CancellationToken::new());

    // Wait for fake mint to be ready
    if let Err(e) = shared::wait_for_mint_ready_with_shutdown(args.port, 100, cancel_token).await {
        eprintln!("Error waiting for fake mint: {e}");
        return Err(e);
    }

    shared::display_mint_info(args.port, &temp_dir, &args.database_type);

    println!();
    println!(
        "Environment variables written to: {}/.env",
        temp_dir.display()
    );
    println!("You can source these variables with:");
    println!("  source {}/.env", temp_dir.display());
    println!();
    println!("Environment variables set:");
    println!("  CDK_TEST_MINT_URL=http://127.0.0.1:{}", args.port);
    println!("  CDK_ITESTS_DIR={}", temp_dir.display());
    println!();
    println!("You can now run integration tests with:");
    println!("  cargo test -p cdk-integration-tests --test fake_wallet");
    println!("  cargo test -p cdk-integration-tests --test happy_path_mint_wallet");
    println!("  etc.");
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
