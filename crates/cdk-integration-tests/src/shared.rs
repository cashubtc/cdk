//! Shared utilities for mint integration tests
//!
//! This module provides common functionality used across different
//! integration test binaries to reduce code duplication.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use cdk_axum::cache;
use cdk_mintd::config::{Database, DatabaseEngine};
use tokio::signal;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::cli::{init_logging, CommonArgs};

/// Default minimum mint amount for test mints
const DEFAULT_MIN_MINT: u64 = 1;
/// Default maximum mint amount for test mints
const DEFAULT_MAX_MINT: u64 = 500_000;
/// Default minimum melt amount for test mints
const DEFAULT_MIN_MELT: u64 = 1;
/// Default maximum melt amount for test mints
const DEFAULT_MAX_MELT: u64 = 500_000;

/// Wait for mint to be ready by checking its info endpoint, with optional shutdown signal
pub async fn wait_for_mint_ready_with_shutdown(
    port: u16,
    timeout_secs: u64,
    shutdown_notify: Arc<CancellationToken>,
) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/v1/info");
    let start_time = std::time::Instant::now();

    println!("Waiting for mint on port {port} to be ready...");

    loop {
        // Check if timeout has been reached
        if start_time.elapsed().as_secs() > timeout_secs {
            return Err(anyhow::anyhow!("Timeout waiting for mint on port {}", port));
        }

        if shutdown_notify.is_cancelled() {
            return Err(anyhow::anyhow!("Canceled waiting for {}", port));
        }

        tokio::select! {
            // Try to make a request to the mint info endpoint
            result = reqwest::get(&url) => {
                match result {
                    Ok(response) => {
                        if response.status().is_success() {
                            println!("Mint on port {port} is ready");
                            return Ok(());
                        } else {
                            println!(
                                "Mint on port {} returned status: {}",
                                port,
                                response.status()
                            );
                        }
                    }
                    Err(e) => {
                        println!("Error connecting to mint on port {port}: {e}");
                    }
                }
            }

            // Check for shutdown signal
            _ = shutdown_notify.cancelled() => {
                return Err(anyhow::anyhow!(
                    "Shutdown requested while waiting for mint on port {}",
                    port
                ));
            }



        }
    }
}

/// Initialize working directory
pub fn init_working_directory(work_dir: &str) -> Result<PathBuf> {
    let temp_dir = PathBuf::from_str(work_dir)?;

    // Create the temp directory if it doesn't exist
    fs::create_dir_all(&temp_dir)?;

    Ok(temp_dir)
}

/// Write environment variables to .env file
pub fn write_env_file(temp_dir: &Path, env_vars: &[(&str, &str)]) -> Result<()> {
    let mut env_content = String::new();
    for (key, value) in env_vars {
        env_content.push_str(&format!("{key}={value}\n"));
    }

    let env_file_path = temp_dir.join(".env");

    fs::write(&env_file_path, &env_content)
        .map(|_| {
            println!(
                "Environment variables written to: {}",
                env_file_path.display()
            );
        })
        .map_err(|e| anyhow::anyhow!("Could not write .env file: {}", e))
}

/// Wait for .env file to be created
pub async fn wait_for_env_file(temp_dir: &Path, timeout_secs: u64) -> Result<()> {
    let env_file_path = temp_dir.join(".env");
    let start_time = std::time::Instant::now();

    println!(
        "Waiting for .env file to be created at: {}",
        env_file_path.display()
    );

    loop {
        // Check if timeout has been reached
        if start_time.elapsed().as_secs() > timeout_secs {
            return Err(anyhow::anyhow!(
                "Timeout waiting for .env file at {}",
                env_file_path.display()
            ));
        }

        // Check if the file exists
        if env_file_path.exists() {
            println!(".env file found at: {}", env_file_path.display());
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Setup common logging based on CLI arguments
pub fn setup_logging(common_args: &CommonArgs) {
    init_logging(common_args.enable_logging, common_args.log_level);
}

/// Create shutdown handler for graceful termination
pub fn create_shutdown_handler() -> Arc<Notify> {
    Arc::new(Notify::new())
}

/// Wait for Ctrl+C signal
pub async fn wait_for_shutdown_signal(shutdown: Arc<Notify>) {
    signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");

    println!("\nReceived Ctrl+C, shutting down...");
    shutdown.notify_waiters();
}

/// Common mint information display
pub fn display_mint_info(port: u16, temp_dir: &Path, database_type: &str) {
    println!("Mint started successfully!");
    println!("Mint URL: http://127.0.0.1:{port}");
    println!("Temp directory: {temp_dir:?}");
    println!("Database type: {database_type}");
}

/// Create settings for a fake wallet mint
pub fn create_fake_wallet_settings(
    port: u16,
    database: &str,
    mnemonic: Option<String>,
    signatory_config: Option<(String, String)>, // (url, certs_dir)
    fake_wallet_config: Option<cdk_mintd::config::FakeWallet>,
) -> cdk_mintd::config::Settings {
    cdk_mintd::config::Settings {
        info: cdk_mintd::config::Info {
            url: format!("http://127.0.0.1:{port}"),
            quote_ttl: None,

            listen_host: "127.0.0.1".to_string(),
            listen_port: port,
            seed: None,
            mnemonic,
            signatory_url: signatory_config.as_ref().map(|(url, _)| url.clone()),
            signatory_certs: signatory_config
                .as_ref()
                .map(|(_, certs_dir)| certs_dir.clone()),
            input_fee_ppk: None,
            http_cache: cache::Config::default(),
            logging: cdk_mintd::config::LoggingConfig {
                output: cdk_mintd::config::LoggingOutput::Both,
                console_level: Some("debug".to_string()),
                file_level: Some("debug".to_string()),
            },
            enable_swagger_ui: None,
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        ln: cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::FakeWallet,
            invoice_description: None,
            min_mint: DEFAULT_MIN_MINT.into(),
            max_mint: DEFAULT_MAX_MINT.into(),
            min_melt: DEFAULT_MIN_MELT.into(),
            max_melt: DEFAULT_MAX_MELT.into(),
        },
        cln: None,
        lnbits: None,
        lnd: None,
        ldk_node: None,
        fake_wallet: fake_wallet_config,
        grpc_processor: None,
        database: Database {
            engine: DatabaseEngine::from_str(database).expect("valid database"),
            postgres: None,
        },
        auth_database: None,
        mint_management_rpc: None,
        auth: None,
        prometheus: Some(Default::default()),
    }
}

/// Create settings for a CLN mint
pub fn create_cln_settings(
    port: u16,
    _cln_rpc_path: PathBuf,
    mnemonic: String,
    cln_config: cdk_mintd::config::Cln,
) -> cdk_mintd::config::Settings {
    cdk_mintd::config::Settings {
        info: cdk_mintd::config::Info {
            url: format!("http://127.0.0.1:{port}"),
            quote_ttl: None,

            listen_host: "127.0.0.1".to_string(),
            listen_port: port,
            seed: None,
            mnemonic: Some(mnemonic),
            signatory_url: None,
            signatory_certs: None,
            input_fee_ppk: None,
            http_cache: cache::Config::default(),
            logging: cdk_mintd::config::LoggingConfig {
                output: cdk_mintd::config::LoggingOutput::Both,
                console_level: Some("debug".to_string()),
                file_level: Some("debug".to_string()),
            },
            enable_swagger_ui: None,
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        ln: cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::Cln,
            invoice_description: None,
            min_mint: DEFAULT_MIN_MINT.into(),
            max_mint: DEFAULT_MAX_MINT.into(),
            min_melt: DEFAULT_MIN_MELT.into(),
            max_melt: DEFAULT_MAX_MELT.into(),
        },
        cln: Some(cln_config),
        lnbits: None,
        lnd: None,
        ldk_node: None,
        fake_wallet: None,
        grpc_processor: None,
        database: cdk_mintd::config::Database::default(),
        auth_database: None,
        mint_management_rpc: None,
        auth: None,
        prometheus: Some(Default::default()),
    }
}

/// Create settings for an LND mint
pub fn create_lnd_settings(
    port: u16,
    lnd_config: cdk_mintd::config::Lnd,
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
            http_cache: cache::Config::default(),
            logging: cdk_mintd::config::LoggingConfig {
                output: cdk_mintd::config::LoggingOutput::Both,
                console_level: Some("debug".to_string()),
                file_level: Some("debug".to_string()),
            },
            enable_swagger_ui: None,
        },
        mint_info: cdk_mintd::config::MintInfo::default(),
        ln: cdk_mintd::config::Ln {
            ln_backend: cdk_mintd::config::LnBackend::Lnd,
            invoice_description: None,
            min_mint: DEFAULT_MIN_MINT.into(),
            max_mint: DEFAULT_MAX_MINT.into(),
            min_melt: DEFAULT_MIN_MELT.into(),
            max_melt: DEFAULT_MAX_MELT.into(),
        },
        cln: None,
        lnbits: None,
        ldk_node: None,
        lnd: Some(lnd_config),
        fake_wallet: None,
        grpc_processor: None,
        database: cdk_mintd::config::Database::default(),
        auth_database: None,
        mint_management_rpc: None,
        auth: None,
        prometheus: Some(Default::default()),
    }
}
