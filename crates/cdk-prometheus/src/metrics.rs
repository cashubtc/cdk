use std::sync::Arc;

use prometheus::{
    Histogram, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Registry,
};

/// Global metrics instance
pub static METRICS: std::sync::LazyLock<CdkMetrics> = std::sync::LazyLock::new(CdkMetrics::default);

/// Custom metrics for CDK applications
#[derive(Clone, Debug)]
pub struct CdkMetrics {
    registry: Arc<Registry>,

    // HTTP metrics
    http_requests_total: IntCounterVec,
    http_request_duration: HistogramVec,

    // Authentication metrics
    auth_attempts_total: IntCounter,
    auth_successes_total: IntCounter,

    // Lightning metrics
    lightning_payments_total: IntCounter,
    lightning_payment_amount: Histogram,
    lightning_payment_fees: Histogram,

    // Database metrics
    db_operations_total: IntCounter,
    db_operation_duration: HistogramVec,
    db_connections_active: IntGauge,

    // Error metrics
    errors_total: IntCounter,

    // Mint metrics
    mint_operations_total: IntCounterVec,
    mint_in_flight_requests: IntGaugeVec,
    mint_operation_duration: HistogramVec,
}

impl CdkMetrics {
    /// Create a new instance with default metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());

        // Create and register HTTP metrics
        let (http_requests_total, http_request_duration) = Self::create_http_metrics(&registry)?;

        // Create and register authentication metrics
        let (auth_attempts_total, auth_successes_total) = Self::create_auth_metrics(&registry)?;

        // Create and register Lightning metrics
        let (lightning_payments_total, lightning_payment_amount, lightning_payment_fees) =
            Self::create_lightning_metrics(&registry)?;

        // Create and register database metrics
        let (db_operations_total, db_operation_duration, db_connections_active) =
            Self::create_db_metrics(&registry)?;

        // Create and register error metrics
        let errors_total = Self::create_error_metrics(&registry)?;

        // Create and register mint metrics
        let (mint_operations_total, mint_operation_duration, mint_in_flight_requests) =
            Self::create_mint_metrics(&registry)?;

        Ok(Self {
            registry,
            http_requests_total,
            http_request_duration,
            auth_attempts_total,
            auth_successes_total,
            lightning_payments_total,
            lightning_payment_amount,
            lightning_payment_fees,
            db_operations_total,
            db_operation_duration,
            db_connections_active,
            errors_total,
            mint_operations_total,
            mint_in_flight_requests,
            mint_operation_duration,
        })
    }

    /// Create and register HTTP metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_http_metrics(registry: &Registry) -> crate::Result<(IntCounterVec, HistogramVec)> {
        let http_requests_total = IntCounterVec::new(
            prometheus::Opts::new("cdk_http_requests_total", "Total number of HTTP requests"),
            &["endpoint", "status"],
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;

        let http_request_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["endpoint"],
        )?;
        registry.register(Box::new(http_request_duration.clone()))?;

        Ok((http_requests_total, http_request_duration))
    }

    /// Create and register authentication metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_auth_metrics(registry: &Registry) -> crate::Result<(IntCounter, IntCounter)> {
        let auth_attempts_total =
            IntCounter::new("cdk_auth_attempts_total", "Total authentication attempts")?;
        registry.register(Box::new(auth_attempts_total.clone()))?;

        let auth_successes_total = IntCounter::new(
            "cdk_auth_successes_total",
            "Total successful authentications",
        )?;
        registry.register(Box::new(auth_successes_total.clone()))?;

        Ok((auth_attempts_total, auth_successes_total))
    }

    /// Create and register Lightning metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_lightning_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounter, Histogram, Histogram)> {
        let wallet_operations_total =
            IntCounter::new("cdk_wallet_operations_total", "Total wallet operations")?;
        registry.register(Box::new(wallet_operations_total))?;

        let lightning_payments_total =
            IntCounter::new("cdk_lightning_payments_total", "Total Lightning payments")?;
        registry.register(Box::new(lightning_payments_total.clone()))?;

        let lightning_payment_amount = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_lightning_payment_amount_sats",
                "Lightning payment amounts in satoshis",
            )
            .buckets(vec![
                1.0,
                10.0,
                100.0,
                1000.0,
                10_000.0,
                100_000.0,
                1_000_000.0,
            ]),
        )?;
        registry.register(Box::new(lightning_payment_amount.clone()))?;

        let lightning_payment_fees = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_lightning_payment_fees_sats",
                "Lightning payment fees in satoshis",
            )
            .buckets(vec![0.0, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0]),
        )?;
        registry.register(Box::new(lightning_payment_fees.clone()))?;

        Ok((
            lightning_payments_total,
            lightning_payment_amount,
            lightning_payment_fees,
        ))
    }

    /// Create and register database metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_db_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounter, HistogramVec, IntGauge)> {
        let db_operations_total =
            IntCounter::new("cdk_db_operations_total", "Total database operations")?;
        registry.register(Box::new(db_operations_total.clone()))?;
        let db_operation_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_db_operation_duration_seconds",
                "Database operation duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
            &["operation"],
        )?;
        registry.register(Box::new(db_operation_duration.clone()))?;

        let db_connections_active = IntGauge::new(
            "cdk_db_connections_active",
            "Number of active database connections",
        )?;
        registry.register(Box::new(db_connections_active.clone()))?;

        Ok((
            db_operations_total,
            db_operation_duration,
            db_connections_active,
        ))
    }

    /// Create and register error metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_error_metrics(registry: &Registry) -> crate::Result<IntCounter> {
        let errors_total = IntCounter::new("cdk_errors_total", "Total errors")?;
        registry.register(Box::new(errors_total.clone()))?;

        Ok(errors_total)
    }

    /// Create and register mint metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_mint_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounterVec, HistogramVec, IntGaugeVec)> {
        let mint_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_mint_operations_total",
                "Total number of mint operations",
            ),
            &["operation", "status"],
        )?;
        registry.register(Box::new(mint_operations_total.clone()))?;

        let mint_operation_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_mint_operation_duration_seconds",
                "Duration of mint operations in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["operation", "status"],
        )?;
        registry.register(Box::new(mint_operation_duration.clone()))?;

        let mint_in_flight_requests = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_mint_in_flight_requests",
                "Number of in-flight mint requests",
            ),
            &["operation"],
        )?;
        registry.register(Box::new(mint_in_flight_requests.clone()))?;

        Ok((
            mint_operations_total,
            mint_operation_duration,
            mint_in_flight_requests,
        ))
    }

    /// Get the metrics registry
    #[must_use]
    pub fn registry(&self) -> Arc<Registry> {
        Arc::<Registry>::clone(&self.registry)
    }

    // HTTP metrics methods
    pub fn record_http_request(&self, endpoint: &str, status: &str) {
        self.http_requests_total
            .with_label_values(&[endpoint, status])
            .inc();
    }

    pub fn record_http_request_duration(&self, duration_seconds: f64, endpoint: &str) {
        self.http_request_duration
            .with_label_values(&[endpoint])
            .observe(duration_seconds);
    }

    // Authentication metrics methods
    pub fn record_auth_attempt(&self) {
        self.auth_attempts_total.inc();
    }

    pub fn record_auth_success(&self) {
        self.auth_successes_total.inc();
    }

    // Lightning metrics methods
    pub fn record_lightning_payment(&self, amount: f64, fee: f64) {
        self.lightning_payments_total.inc();
        self.lightning_payment_amount.observe(amount);
        self.lightning_payment_fees.observe(fee);
    }

    // Database metrics methods
    pub fn record_db_operation(&self, duration_seconds: f64, op: &str) {
        self.db_operations_total.inc();
        self.db_operation_duration
            .with_label_values(&[op])
            .observe(duration_seconds);
    }

    pub fn set_db_connections_active(&self, count: i64) {
        self.db_connections_active.set(count);
    }

    // Error metrics methods
    pub fn record_error(&self) {
        self.errors_total.inc();
    }

    // Mint metrics methods
    pub fn record_mint_operation(&self, operation: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        self.mint_operations_total
            .with_label_values(&[operation, status])
            .inc();
    }
    pub fn record_mint_operation_histogram(
        &self,
        operation: &str,
        success: bool,
        duration_seconds: f64,
    ) {
        let status = if success { "success" } else { "error" };
        self.mint_operation_duration
            .with_label_values(&[operation, status])
            .observe(duration_seconds);
    }
    pub fn inc_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests
            .with_label_values(&[operation])
            .inc();
    }

    pub fn dec_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests
            .with_label_values(&[operation])
            .dec();
    }
}

impl Default for CdkMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create default CdkMetrics")
    }
}

/// Helper functions for recording metrics using the global instance
pub mod global {
    use super::METRICS;

    /// Record an HTTP request using the global metrics instance
    pub fn record_http_request(endpoint: &str, status: &str) {
        METRICS.record_http_request(endpoint, status);
    }

    /// Record HTTP request duration using the global metrics instance
    pub fn record_http_request_duration(duration_seconds: f64, endpoint: &str) {
        METRICS.record_http_request_duration(duration_seconds, endpoint);
    }

    /// Record authentication attempt using the global metrics instance
    pub fn record_auth_attempt() {
        METRICS.record_auth_attempt();
    }

    /// Record authentication success using the global metrics instance
    pub fn record_auth_success() {
        METRICS.record_auth_success();
    }

    /// Record Lightning payment using the global metrics instance
    pub fn record_lightning_payment(amount: f64, fee: f64) {
        METRICS.record_lightning_payment(amount, fee);
    }

    /// Record database operation using the global metrics instance
    pub fn record_db_operation(duration_seconds: f64, op: &str) {
        METRICS.record_db_operation(duration_seconds, op);
    }

    /// Set database connections active using the global metrics instance
    pub fn set_db_connections_active(count: i64) {
        METRICS.set_db_connections_active(count);
    }

    /// Record error using the global metrics instance
    pub fn record_error() {
        METRICS.record_error();
    }

    /// Record mint operation using the global metrics instance
    pub fn record_mint_operation(operation: &str, success: bool) {
        METRICS.record_mint_operation(operation, success);
    }

    /// Record mint operation with histogram using the global metrics instance
    pub fn record_mint_operation_histogram(operation: &str, success: bool, duration_seconds: f64) {
        METRICS.record_mint_operation_histogram(operation, success, duration_seconds);
    }

    /// Increment in-flight requests using the global metrics instance
    pub fn inc_in_flight_requests(operation: &str) {
        METRICS.inc_in_flight_requests(operation);
    }

    /// Decrement in-flight requests using the global metrics instance
    pub fn dec_in_flight_requests(operation: &str) {
        METRICS.dec_in_flight_requests(operation);
    }

    /// Get the metrics registry from the global instance
    pub fn registry() -> std::sync::Arc<prometheus::Registry> {
        METRICS.registry()
    }
}
