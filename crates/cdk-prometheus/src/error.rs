use thiserror::Error;

/// Errors that can occur in the Prometheus crate
#[derive(Error, Debug)]
pub enum PrometheusError {
    /// Server binding error
    #[error("Failed to bind to address {address}: {source}")]
    ServerBind {
        /// Address that failed to bind
        address: String,
        #[source]
        /// Underlying I/O error
        source: std::io::Error,
    },

    /// Metrics collection error
    #[error("Failed to collect metrics: {0}")]
    MetricsCollection(String),

    /// Registry error
    #[error("Registry error: {source}")]
    Registry {
        #[from]
        /// Underlying Prometheus error
        source: prometheus::Error,
    },

    /// System metrics error
    #[cfg(feature = "system-metrics")]
    #[error("System metrics error: {0}")]
    SystemMetrics(String),
}

/// Result type for Prometheus operations
pub type Result<T> = std::result::Result<T, PrometheusError>;
