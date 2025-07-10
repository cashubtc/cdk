//! # CDK Prometheus

pub mod error;
pub mod metrics;
pub mod server;

#[cfg(feature = "system-metrics")]
pub mod process;

// Re-exports for convenience
pub use error::{PrometheusError, Result};
pub use metrics::CdkMetrics;
#[cfg(feature = "system-metrics")]
pub use process::SystemMetrics;
// Re-export prometheus crate for custom metrics
pub use prometheus;
pub use server::{PrometheusBuilder, PrometheusConfig, PrometheusServer};

/// Convenience function to create a new CDK metrics instance
///
/// # Errors
/// Returns an error if any of the metrics cannot be created or registered
pub fn create_cdk_metrics() -> Result<CdkMetrics> {
    CdkMetrics::new()
}

/// Convenience function to start a Prometheus server with default configuration
///
/// # Errors
/// Returns an error if the server cannot be created or started
pub async fn start_default_server(metrics: &CdkMetrics) -> Result<()> {
    let server = PrometheusBuilder::new().build_with_cdk_metrics(metrics)?;

    server.start().await
}

/// Convenience function to start a Prometheus server in the background
///
/// # Errors
/// Returns an error if the server cannot be created
pub fn start_background_server(
    metrics: &CdkMetrics,
) -> Result<tokio::task::JoinHandle<Result<()>>> {
    let server = PrometheusBuilder::new().build_with_cdk_metrics(metrics)?;

    Ok(server.start_background())
}
