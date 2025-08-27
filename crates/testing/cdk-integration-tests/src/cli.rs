//! Common CLI and logging utilities for CDK integration test binaries
//!
//! This module provides standardized CLI argument parsing and logging setup
//! for integration test binaries.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Common CLI arguments for CDK integration test binaries
#[derive(Parser, Debug)]
pub struct CommonArgs {
    /// Enable logging (default is false)
    #[arg(long, default_value_t = false)]
    pub enable_logging: bool,

    /// Logging level when enabled (default is debug)
    #[arg(long, default_value = "debug")]
    pub log_level: tracing::Level,
}

/// Initialize logging based on CLI arguments
pub fn init_logging(enable_logging: bool, log_level: tracing::Level) {
    if enable_logging {
        let default_filter = log_level.to_string();

        // Common filters to reduce noise
        let hyper_filter = "hyper=warn";
        let h2_filter = "h2=warn";
        let rustls_filter = "rustls=warn";
        let reqwest_filter = "reqwest=warn";
        let tower_filter = "tower_http=warn";

        let env_filter = EnvFilter::new(format!(
            "{default_filter},{hyper_filter},{h2_filter},{rustls_filter},{reqwest_filter},{tower_filter}"
        ));

        // Ok if successful, Err if already initialized
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init();
    }
}
