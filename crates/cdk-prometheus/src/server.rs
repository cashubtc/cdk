use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use prometheus::{Registry, TextEncoder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::metrics::METRICS;
#[cfg(feature = "system-metrics")]
use crate::process::SystemMetrics;

const MAX_REQUEST_BYTES: usize = 4096;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

type MetricsHandler = Arc<dyn Fn() -> String + Send + Sync + 'static>;

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

fn request_matches_path(request: &str, metrics_path: &str) -> bool {
    let Some(request_line) = request.lines().next() else {
        return false;
    };

    let mut parts = request_line.split_whitespace();
    let method = parts.next();
    let target = parts.next();
    let version = parts.next();

    if method != Some("GET") || target.is_none() || version.is_none() {
        return false;
    }

    let target_path = target
        .and_then(|target| target.split('?').next())
        .unwrap_or_default();

    target_path == metrics_path
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];

    loop {
        let bytes_read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut buffer))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out reading request"))??;

        if bytes_read == 0 {
            if request.is_empty() {
                return Ok(None);
            }
            break;
        }

        request.extend_from_slice(&buffer[..bytes_read]);

        if request.windows(2).any(|window| window == b"\r\n")
            || request.contains(&b'\n')
            || request.len() >= MAX_REQUEST_BYTES
        {
            break;
        }
    }

    Ok(Some(String::from_utf8_lossy(&request).to_string()))
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );

    tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(response.as_bytes()))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out writing response"))??;

    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    metrics_path: String,
    metrics_handler: MetricsHandler,
) -> io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };

    if request_matches_path(&request, &metrics_path) {
        let metrics = metrics_handler();
        write_response(
            &mut stream,
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            &metrics,
        )
        .await
    } else {
        write_response(&mut stream, "404 Not Found", "text/plain", "Not Found").await
    }
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
    ) -> MetricsHandler {
        Arc::new(move || {
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
        })
    }

    /// Start the Prometheus HTTP server
    ///
    /// # Errors
    /// Returns an error if the server cannot bind to the configured address
    pub async fn start(
        self,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> crate::Result<()> {
        let binding = self.config.bind_address;
        let registry_clone = Arc::<Registry>::clone(&self.registry);
        let path = self.config.metrics_path.clone();

        #[cfg(feature = "system-metrics")]
        let metrics_handler =
            Self::create_metrics_handler(registry_clone, self.system_metrics.clone());

        #[cfg(not(feature = "system-metrics"))]
        let metrics_handler = Self::create_metrics_handler(registry_clone);

        let listener = TcpListener::bind(binding).await.map_err(|source| {
            crate::error::PrometheusError::ServerBind {
                address: binding.to_string(),
                source,
            }
        })?;

        tracing::info!("Started Prometheus server on {} at path {}", binding, path);

        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                _ = &mut shutdown_signal => {
                    tracing::info!("Shutdown signal received, stopping Prometheus server");
                    break;
                }
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _peer_addr)) => {
                            let metrics_path = path.clone();
                            let metrics_handler = Arc::clone(&metrics_handler);

                            tokio::spawn(async move {
                                if let Err(e) =
                                    handle_connection(stream, metrics_path, metrics_handler).await
                                {
                                    tracing::warn!("Failed to serve Prometheus scrape: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to accept connection: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }

        tracing::info!("Prometheus server stopped");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::request_matches_path;

    #[test]
    fn request_matching_requires_exact_request_target() {
        assert!(request_matches_path(
            "GET /metrics HTTP/1.1\r\n\r\n",
            "/metrics"
        ));
        assert!(request_matches_path(
            "GET /metrics?name=value HTTP/1.1\r\n\r\n",
            "/metrics"
        ));
        assert!(!request_matches_path(
            "GET /not-metrics HTTP/1.1\r\nX-Path: /metrics\r\n\r\n",
            "/metrics"
        ));
        assert!(!request_matches_path(
            "POST /metrics HTTP/1.1\r\n\r\n",
            "/metrics"
        ));
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
