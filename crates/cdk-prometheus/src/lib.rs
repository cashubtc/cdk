//! # CDK Prometheus

pub mod error;
pub mod metrics;
pub mod server;

#[cfg(feature = "system-metrics")]
pub mod process;

// Re-exports for convenience
pub use error::{PrometheusError, Result};
pub use metrics::{global, CdkMetrics, METRICS};
#[cfg(feature = "system-metrics")]
pub use process::SystemMetrics;
// Re-export prometheus crate for custom metrics
pub use prometheus;
pub use server::{PrometheusBuilder, PrometheusConfig, PrometheusServer};

/// Macro for recording metrics with optional fallback to global instance
///
/// Usage:
/// ```rust
/// use cdk_prometheus::record_metrics;
///
/// // With optional metrics instance
/// record_metrics!(metrics_option => {
///     dec_in_flight_requests("operation");
///     record_mint_operation("operation", true);
/// });
///
/// // Direct global calls
/// record_metrics!({
///     dec_in_flight_requests("operation");
///     record_mint_operation("operation", true);
/// });
/// ```
#[macro_export]
macro_rules! record_metrics {
    // Pattern for using optional metrics with fallback to global
    ($metrics_opt:expr => { $($method:ident($($arg:expr),*));* $(;)? }) => {
        #[cfg(feature = "prometheus")]
        {
            if let Some(metrics) = $metrics_opt.as_ref() {
                $(
                    metrics.$method($($arg),*);
                )*
            } else {
                $(
                    $crate::global::$method($($arg),*);
                )*
            }
        }
    };

    // Pattern for using global metrics directly
    ({ $($method:ident($($arg:expr),*));* $(;)? }) => {
        #[cfg(feature = "prometheus")]
        {
            $(
                $crate::global::$method($($arg),*);
            )*
        }
    };
}

/// Convenience function to create a new CDK metrics instance
///
/// # Errors
/// Returns an error if any of the metrics cannot be created or registered
pub fn create_cdk_metrics() -> Result<CdkMetrics> {
    CdkMetrics::new()
}

/// Convenience function to start a Prometheus server with specific metrics
///
/// # Errors
/// Returns an error if the server cannot be created or started
pub async fn start_default_server_with_metrics(
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let server = PrometheusBuilder::new().build_with_cdk_metrics()?;

    server.start(shutdown_signal).await
}
