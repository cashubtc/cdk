use prometheus::{
    Histogram, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Registry,
};
use std::sync::Arc;

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
    db_operation_duration: Histogram,
    db_connections_active: IntGauge,

    // Error metrics
    errors_total: IntCounter,

    // Mint metrics
    mint_operations_total: IntCounterVec,
    mint_operation_duration: Histogram,
    mint_in_flight_requests: IntGaugeVec,
}

impl CdkMetrics {
    /// Create a new instance with default metrics
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());

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

        let auth_attempts_total =
            IntCounter::new("cdk_auth_attempts_total", "Total authentication attempts")?;
        registry.register(Box::new(auth_attempts_total.clone()))?;

        let auth_successes_total = IntCounter::new(
            "cdk_auth_successes_total",
            "Total successful authentications",
        )?;
        registry.register(Box::new(auth_successes_total.clone()))?;

        let wallet_operations_total =
            IntCounter::new("cdk_wallet_operations_total", "Total wallet operations")?;
        registry.register(Box::new(wallet_operations_total.clone()))?;

        let lightning_payments_total =
            IntCounter::new("cdk_lightning_payments_total", "Total Lightning payments")?;
        registry.register(Box::new(lightning_payments_total.clone()))?;

        let lightning_payment_amount = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_lightning_payment_amount_sats",
                "Lightning payment amounts in satoshis",
            )
            .buckets(vec![1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0]),
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

        let db_operations_total =
            IntCounter::new("cdk_db_operations_total", "Total database operations")?;
        registry.register(Box::new(db_operations_total.clone()))?;

        let db_operation_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_db_operation_duration_seconds",
                "Database operation duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        )?;
        registry.register(Box::new(db_operation_duration.clone()))?;

        let db_connections_active = IntGauge::new(
            "cdk_db_connections_active",
            "Number of active database connections",
        )?;
        registry.register(Box::new(db_connections_active.clone()))?;

        let errors_total = IntCounter::new("cdk_errors_total", "Total errors")?;
        registry.register(Box::new(errors_total.clone()))?;

        let mint_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_mint_operations_total",
                "Total number of mint operations",
            ),
            &["operation", "status"],
        )?;
        registry.register(Box::new(mint_operations_total.clone()))?;

        let mint_operation_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_mint_operation_duration_seconds",
                "Duration of mint operations in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
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
            mint_operation_duration,
            mint_in_flight_requests,
        })
    }

    /// Get the metrics registry
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.clone()
    }

    // HTTP metrics methods
    pub fn record_http_request(&self, endpoint: &str, status: &str) {
        self.http_requests_total.with_label_values(&[endpoint, status]).inc();       
    }

    pub fn record_http_request_duration(&self, duration_seconds: f64, endpoint: &str) {
        self.http_request_duration.with_label_values(&[endpoint]).observe(duration_seconds);
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
    pub fn record_db_operation(&self, duration_seconds: f64) {
        self.db_operations_total.inc();
        self.db_operation_duration.observe(duration_seconds);
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
