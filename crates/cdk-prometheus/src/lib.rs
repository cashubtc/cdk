//! # CDK Prometheus
//! 
//! A comprehensive Prometheus metrics export server for Cashu Development Kit (CDK) applications.
//! This crate provides easy-to-use abstractions for exposing Prometheus metrics via HTTP endpoint,
//! including both custom application metrics and optional system metrics.
pub mod error;
pub mod metrics;
pub mod server;

#[cfg(feature = "system-metrics")]
pub mod process;

// Re-exports for convenience
pub use error::{PrometheusError, Result};
pub use metrics::CdkMetrics;
pub use server::{PrometheusBuilder, PrometheusConfig, PrometheusServer};

#[cfg(feature = "system-metrics")]
pub use process::SystemMetrics;

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