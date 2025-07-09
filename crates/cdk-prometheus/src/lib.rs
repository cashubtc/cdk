//! # CDK Prometheus
//! 
//! A comprehensive Prometheus metrics export server for Cashu Development Kit (CDK) applications.
//! This crate provides easy-to-use abstractions for exposing Prometheus metrics via HTTP endpoint,
//! including both custom application metrics and optional system metrics.
//!
//! ## Features
//! 
//! - 🔧 **Easy Integration**: Simple builder pattern for quick setup
//! - 📊 **CDK-Specific Metrics**: Pre-built metrics for wallet, mint, and Lightning operations
//! - 🖥️ **System Metrics**: Optional CPU, memory, and disk usage metrics
//! - 🌐 **HTTP Server**: Dedicated Prometheus metrics endpoint
//! - ⚙️ **Configurable**: Flexible configuration options
//! - 📈 **Custom Metrics**: Support for application-specific metrics
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use cdk_prometheus::{CdkMetrics, PrometheusBuilder};
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create CDK metrics
//!     let metrics = CdkMetrics::new()?;
//!     
//!     // Record some metrics
//!     metrics.record_http_request();
//!     metrics.set_wallet_balance(1000.0);
//!     
//!     // Start Prometheus server
//!     let server = PrometheusBuilder::new()
//!         .bind_address("0.0.0.0:9090".parse()?)
//!         .build_with_cdk_metrics(&metrics)?;
//!     
//!     // Start in background
//!     let _handle = server.start_background();
//!     
//!     // Your application logic here
//!     
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod metrics;
pub mod server;

#[cfg(feature = "system-metrics")]
pub mod system;

// Re-exports for convenience
pub use error::{PrometheusError, Result};
pub use metrics::CdkMetrics;
pub use server::{PrometheusBuilder, PrometheusConfig, PrometheusServer};

#[cfg(feature = "system-metrics")]
pub use system::SystemMetrics;

// Re-export prometheus crate for custom metrics
pub use prometheus;

/// Convenience function to create a new CDK metrics instance
pub fn create_cdk_metrics() -> Result<CdkMetrics> {
    CdkMetrics::new()
}

/// Convenience function to start a Prometheus server with default configuration
pub async fn start_default_server(metrics: &CdkMetrics) -> Result<()> {
    let server = PrometheusBuilder::new()
        .build_with_cdk_metrics(metrics)?;
    
    server.start().await
}

/// Convenience function to start a Prometheus server in the background
pub fn start_background_server(metrics: &CdkMetrics) -> Result<tokio::task::JoinHandle<Result<()>>> {
    let server = PrometheusBuilder::new()
        .build_with_cdk_metrics(metrics)?;
    
    Ok(server.start_background())
}