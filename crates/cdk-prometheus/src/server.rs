use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use prometheus::{Registry, TextEncoder};

use crate::metrics::METRICS;
#[cfg(feature = "system-metrics")]
use crate::process::SystemMetrics;

/// Configuration for the Prometheus server
#[derive(Debug, Clone)]
pub struct PrometheusConfig {
    /// Address to bind the server to (default: "127.0.0.1:9090")
    pub bind_address: SocketAddr,
    /// Path to serve metrics on (default: "/metrics")
    pub metrics_path: String,
    /// Whether to include system metrics (default: true if feature enabled)
    #[cfg(feature = "system-metrics")]
    pub include_system_metrics: bool,
    /// How often to update system metrics in seconds (default: 15)
    #[cfg(feature = "system-metrics")]
    pub system_metrics_interval: u64,
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:9090".parse().expect("Invalid default address"),
            metrics_path: "/metrics".to_string(),
            #[cfg(feature = "system-metrics")]
            include_system_metrics: true,
            #[cfg(feature = "system-metrics")]
            system_metrics_interval: 15,
        }
    }
}

/// Prometheus metrics server
#[derive(Debug)]
pub struct PrometheusServer {
    config: PrometheusConfig,
    registry: Arc<Registry>,
    #[cfg(feature = "system-metrics")]
    system_metrics: Option<SystemMetrics>,
}

impl PrometheusServer {
    /// Create a new Prometheus server with CDK metrics
    ///
    /// # Errors
    /// Returns an error if system metrics cannot be created (when enabled)
    pub fn new(config: PrometheusConfig) -> crate::Result<Self> {
        let registry = METRICS.registry();

        #[cfg(feature = "system-metrics")]
        let system_metrics = if config.include_system_metrics {
            let sys_metrics = SystemMetrics::new()?;
            Some(sys_metrics)
        } else {
            None
        };

        Ok(Self {
            config,
            registry,
            #[cfg(feature = "system-metrics")]
            system_metrics,
        })
    }

    /// Create a new Prometheus server with custom registry
    #[must_use]
    pub const fn with_registry(config: PrometheusConfig, registry: Arc<Registry>) -> Self {
        Self {
            config,
            registry,
            #[cfg(feature = "system-metrics")]
            system_metrics: None,
        }
    }

    /// Create a metrics handler function that gathers and encodes metrics
    fn create_metrics_handler(
        registry: Arc<Registry>,
        #[cfg(feature = "system-metrics")] system_metrics: Option<SystemMetrics>,
    ) -> impl Fn() -> String {
        move || {
            let encoder = TextEncoder::new();

            // Collect metrics from our registry
            #[cfg(feature = "system-metrics")]
            let mut metric_families = registry.gather();
            #[cfg(not(feature = "system-metrics"))]
            let metric_families = registry.gather();

            // Add system metrics if available
            #[cfg(feature = "system-metrics")]
            if let Some(ref sys_metrics) = system_metrics {
                // Update system metrics before collection
                if let Err(e) = sys_metrics.update_metrics() {
                    tracing::warn!("Failed to update system metrics: {e}");
                }

                let sys_registry = sys_metrics.registry();
                let mut sys_families = sys_registry.gather();
                metric_families.append(&mut sys_families);
            }

            // Encode metrics to string
            encoder
                .encode_to_string(&metric_families)
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to encode metrics: {e}");
                    format!("Failed to encode metrics: {e}")
                })
        }
    }

    /// Start the Prometheus HTTP server
    ///
    /// # Errors
    /// This function always returns Ok as errors are handled internally
    pub async fn start(
        self,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> crate::Result<()> {
        // Create and start the exporter
        let binding = self.config.bind_address;
        let registry_clone = Arc::<Registry>::clone(&self.registry);

        // Create a handler that exposes our registry
        #[cfg(feature = "system-metrics")]
        let metrics_handler =
            Self::create_metrics_handler(registry_clone, self.system_metrics.clone());

        #[cfg(not(feature = "system-metrics"))]
        let metrics_handler = Self::create_metrics_handler(registry_clone);

        // Start the exporter in a background task
        let path = self.config.metrics_path.clone();

        // Create a channel for signaling the server task to shutdown
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn the server task
        let server_handle = tokio::spawn(async move {
            // We're using a simple HTTP server to expose our metrics
            use std::io::{Read, Write};
            use std::net::TcpListener;

            // Create a TCP listener
            let listener = match TcpListener::bind(binding) {
                Ok(listener) => {
                    // Set non-blocking mode to allow for shutdown checking
                    if let Err(e) = listener.set_nonblocking(true) {
                        tracing::error!("Failed to set non-blocking mode: {e}");
                        return;
                    }
                    listener
                }
                Err(e) => {
                    tracing::error!("Failed to bind TCP listener: {e}");
                    return;
                }
            };
            tracing::info!("Started Prometheus server on {} at path {}", binding, path);

            // Accept connections with shutdown signal handling
            loop {
                // Check for shutdown signal
                if shutdown_rx.try_recv().is_ok() {
                    tracing::info!("Shutdown signal received, stopping Prometheus server");
                    break;
                }

                // Try to accept a connection (non-blocking)
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Handle the connection
                        let mut buffer = [0; 1024];
                        match stream.read(&mut buffer) {
                            Ok(0) => {}
                            Ok(bytes_read) => {
                                // Convert the buffer to a string
                                let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                                // Check if the request is for our metrics path
                                if request.contains(&format!("GET {path} HTTP")) {
                                    // Get the metrics
                                    let metrics = metrics_handler();

                                    // Write the response
                                    let response = format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                                        metrics.len(),
                                        metrics
                                    );

                                    if let Err(e) = stream.write_all(response.as_bytes()) {
                                        tracing::error!("Failed to write response: {e}");
                                    }
                                } else {
                                    // Write a 404 response
                                    let response = "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nNot Found";
                                    if let Err(e) = stream.write_all(response.as_bytes()) {
                                        tracing::error!("Failed to write response: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to read from stream: {e}");
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No connection available, continue the loop
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept connection: {e}");
                        // Add a small delay to prevent busy looping on persistent errors
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }

            tracing::info!("Prometheus server stopped");
        });

        // Wait for the shutdown signal
        shutdown_signal.await;

        // Signal the server to shutdown
        let _ = shutdown_tx.send(());

        // Wait for the server task to complete (with a timeout)
        match tokio::time::timeout(Duration::from_secs(5), server_handle).await {
            Ok(result) => {
                if let Err(e) = result {
                    tracing::error!("Server task failed: {e}");
                }
            }
            Err(_) => {
                tracing::warn!("Server shutdown timed out after 5 seconds");
            }
        }

        Ok(())
    }
}

/// Builder for easy Prometheus server setup
#[derive(Debug)]
pub struct PrometheusBuilder {
    config: PrometheusConfig,
}

impl PrometheusBuilder {
    /// Create a new builder with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: PrometheusConfig::default(),
        }
    }

    /// Set the bind address
    #[must_use]
    pub const fn bind_address(mut self, addr: SocketAddr) -> Self {
        self.config.bind_address = addr;
        self
    }

    /// Set the metrics path
    #[must_use]
    pub fn metrics_path<S: Into<String>>(mut self, path: S) -> Self {
        self.config.metrics_path = path.into();
        self
    }

    /// Enable or disable system metrics
    #[cfg(feature = "system-metrics")]
    #[must_use]
    pub const fn system_metrics(mut self, enabled: bool) -> Self {
        self.config.include_system_metrics = enabled;
        self
    }

    /// Set system metrics update interval
    #[cfg(feature = "system-metrics")]
    #[must_use]
    pub const fn system_metrics_interval(mut self, seconds: u64) -> Self {
        self.config.system_metrics_interval = seconds;
        self
    }

    /// Build the server with specific CDK metrics instance
    ///
    /// # Errors
    /// Returns an error if system metrics cannot be created (when enabled)
    pub fn build_with_cdk_metrics(self) -> crate::Result<PrometheusServer> {
        PrometheusServer::new(self.config)
    }

    /// Build the server with custom registry
    #[must_use]
    pub fn build_with_registry(self, registry: Arc<Registry>) -> PrometheusServer {
        PrometheusServer::with_registry(self.config, registry)
    }
}

impl Default for PrometheusBuilder {
    fn default() -> Self {
        Self::new()
    }
}
