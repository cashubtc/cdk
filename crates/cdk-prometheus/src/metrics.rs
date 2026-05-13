use std::sync::Arc;
use std::time::Instant;

use prometheus::{HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Registry};

/// Global metrics instance
pub static METRICS: std::sync::LazyLock<CdkMetrics> = std::sync::LazyLock::new(CdkMetrics::default);

/// RAII guard for recording mint operation metrics.
///
/// The guard increments the in-flight gauge when it is created, records the
/// operation count and duration when [`Self::record`] is called, and always
/// decrements the in-flight gauge when it is dropped.
#[derive(Debug)]
pub struct MintMetricGuard {
    operation: &'static str,
    start_time: Instant,
}

impl MintMetricGuard {
    /// Start tracking a mint operation.
    #[must_use]
    pub fn new(operation: &'static str) -> Self {
        METRICS.inc_in_flight_requests(operation);

        Self {
            operation,
            start_time: Instant::now(),
        }
    }

    /// Record the operation result and duration.
    pub fn record(self, success: bool) {
        METRICS.record_mint_operation(self.operation, success);
        METRICS.record_mint_operation_histogram(
            self.operation,
            success,
            self.start_time.elapsed().as_secs_f64(),
        );

        if !success {
            METRICS.record_error();
        }
    }
}

impl Drop for MintMetricGuard {
    fn drop(&mut self) {
        METRICS.dec_in_flight_requests(self.operation);
    }
}

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

    // Payment metrics
    payments_total: IntCounterVec,
    payment_amount: HistogramVec,
    payment_fees: HistogramVec,

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

        // Create and register payment metrics
        let (payments_total, payment_amount, payment_fees) =
            Self::create_payment_metrics(&registry)?;

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
            payments_total,
            payment_amount,
            payment_fees,
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

    /// Create and register payment metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_payment_metrics(
        registry: &Registry,
    ) -> crate::Result<(IntCounterVec, HistogramVec, HistogramVec)> {
        let wallet_operations_total =
            IntCounter::new("cdk_wallet_operations_total", "Total wallet operations")?;
        registry.register(Box::new(wallet_operations_total))?;

        let payments_total = IntCounterVec::new(
            prometheus::Opts::new("cdk_payments_total", "Total confirmed payments"),
            &["method"],
        )?;
        registry.register(Box::new(payments_total.clone()))?;

        let payment_amount = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_payment_amount_sats",
                "Confirmed payment amounts in satoshis",
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
            &["method"],
        )?;
        registry.register(Box::new(payment_amount.clone()))?;

        let payment_fees = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_payment_fees_sats",
                "Confirmed payment fees in satoshis",
            )
            .buckets(vec![0.0, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0]),
            &["method"],
        )?;
        registry.register(Box::new(payment_fees.clone()))?;

        Ok((payments_total, payment_amount, payment_fees))
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
    /// Record an HTTP request
    pub fn record_http_request(&self, endpoint: &str, status: &str) {
        self.http_requests_total
            .with_label_values(&[endpoint, status])
            .inc();
    }

    /// Record HTTP request duration
    pub fn record_http_request_duration(&self, duration_seconds: f64, endpoint: &str) {
        self.http_request_duration
            .with_label_values(&[endpoint])
            .observe(duration_seconds);
    }

    // Authentication metrics methods
    /// Record an authentication attempt
    pub fn record_auth_attempt(&self) {
        self.auth_attempts_total.inc();
    }

    /// Record a successful authentication
    pub fn record_auth_success(&self) {
        self.auth_successes_total.inc();
    }

    // Payment metrics methods
    /// Record a confirmed payment with known amount and fee in sats.
    pub fn record_payment(&self, method: &str, amount: f64, fee: f64) {
        self.record_payment_total(method);
        self.record_payment_amount(method, amount);
        self.record_payment_fee(method, fee);
    }

    /// Record a confirmed payment.
    pub fn record_payment_total(&self, method: &str) {
        self.payments_total.with_label_values(&[method]).inc();
    }

    /// Record a confirmed payment amount in sats.
    pub fn record_payment_amount(&self, method: &str, amount: f64) {
        self.payment_amount
            .with_label_values(&[method])
            .observe(amount);
    }

    /// Record a confirmed payment fee in sats.
    pub fn record_payment_fee(&self, method: &str, fee: f64) {
        self.payment_fees.with_label_values(&[method]).observe(fee);
    }

    // Database metrics methods
    /// Record a database operation
    pub fn record_db_operation(&self, duration_seconds: f64, op: &str) {
        self.db_operations_total.inc();
        self.db_operation_duration
            .with_label_values(&[op])
            .observe(duration_seconds);
    }

    /// Set the number of active database connections
    pub fn set_db_connections_active(&self, count: i64) {
        self.db_connections_active.set(count);
    }

    // Error metrics methods
    /// Record an error
    pub fn record_error(&self) {
        self.errors_total.inc();
    }

    // Mint metrics methods
    /// Record a mint operation
    pub fn record_mint_operation(&self, operation: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        self.mint_operations_total
            .with_label_values(&[operation, status])
            .inc();
    }

    /// Record a mint operation with duration
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

    /// Increment in-flight mint requests
    pub fn inc_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests
            .with_label_values(&[operation])
            .inc();
    }

    /// Decrement in-flight mint requests
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

/// Compatibility helpers for recording metrics using the global instance.
///
/// New code should call methods on [`METRICS`] directly or use a guard such as
/// [`MintMetricGuard`]. These helpers remain to preserve the existing public
/// API for callers using `cdk_prometheus::global`.
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

    /// Record confirmed payment using the global metrics instance
    pub fn record_payment(method: &str, amount: f64, fee: f64) {
        METRICS.record_payment(method, amount, fee);
    }

    /// Record confirmed payment count using the global metrics instance
    pub fn record_payment_total(method: &str) {
        METRICS.record_payment_total(method);
    }

    /// Record confirmed payment amount using the global metrics instance
    pub fn record_payment_amount(method: &str, amount: f64) {
        METRICS.record_payment_amount(method, amount);
    }

    /// Record confirmed payment fee using the global metrics instance
    pub fn record_payment_fee(method: &str, fee: f64) {
        METRICS.record_payment_fee(method, fee);
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

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use super::{MintMetricGuard, METRICS};

    static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn metrics_lock() -> MutexGuard<'static, ()> {
        METRICS_TEST_LOCK
            .lock()
            .expect("metrics test lock should not be poisoned")
    }

    #[test]
    fn mint_metric_guard_records_success_and_balances_in_flight() {
        let _lock = metrics_lock();
        let operation = "test_guard_success";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let success_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "success"]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "success"]);

        let in_flight_before = in_flight.get();
        let success_count_before = success_count.get();
        let error_count_before = error_count.get();
        let duration_count_before = duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        let guard = MintMetricGuard::new(operation);
        assert_eq!(in_flight.get(), in_flight_before + 1);

        std::thread::sleep(Duration::from_millis(1));
        guard.record(true);

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(success_count.get(), success_count_before + 1);
        assert_eq!(error_count.get(), error_count_before);
        assert_eq!(duration.get_sample_count(), duration_count_before + 1);
        assert_eq!(METRICS.errors_total.get(), errors_before);
    }

    #[test]
    fn mint_metric_guard_records_error_and_global_error_count() {
        let _lock = metrics_lock();
        let operation = "test_guard_error";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "error"]);

        let in_flight_before = in_flight.get();
        let error_count_before = error_count.get();
        let duration_count_before = duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        let guard = MintMetricGuard::new(operation);
        assert_eq!(in_flight.get(), in_flight_before + 1);

        guard.record(false);

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(error_count.get(), error_count_before + 1);
        assert_eq!(duration.get_sample_count(), duration_count_before + 1);
        assert_eq!(METRICS.errors_total.get(), errors_before + 1);
    }

    #[test]
    fn mint_metric_guard_drop_without_record_only_balances_in_flight() {
        let _lock = metrics_lock();
        let operation = "test_guard_drop_without_record";
        let in_flight = METRICS
            .mint_in_flight_requests
            .with_label_values(&[operation]);
        let success_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "success"]);
        let error_count = METRICS
            .mint_operations_total
            .with_label_values(&[operation, "error"]);
        let success_duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "success"]);
        let error_duration = METRICS
            .mint_operation_duration
            .with_label_values(&[operation, "error"]);

        let in_flight_before = in_flight.get();
        let success_count_before = success_count.get();
        let error_count_before = error_count.get();
        let success_duration_before = success_duration.get_sample_count();
        let error_duration_before = error_duration.get_sample_count();
        let errors_before = METRICS.errors_total.get();

        {
            let _guard = MintMetricGuard::new(operation);
            assert_eq!(in_flight.get(), in_flight_before + 1);
        }

        assert_eq!(in_flight.get(), in_flight_before);
        assert_eq!(success_count.get(), success_count_before);
        assert_eq!(error_count.get(), error_count_before);
        assert_eq!(success_duration.get_sample_count(), success_duration_before);
        assert_eq!(error_duration.get_sample_count(), error_duration_before);
        assert_eq!(METRICS.errors_total.get(), errors_before);
    }

    #[test]
    fn payment_metrics_are_labeled_by_method() {
        let _lock = metrics_lock();
        let method = "test_payment_method";
        let payments = METRICS.payments_total.with_label_values(&[method]);
        let amount = METRICS.payment_amount.with_label_values(&[method]);
        let fee = METRICS.payment_fees.with_label_values(&[method]);

        let payments_before = payments.get();
        let amount_count_before = amount.get_sample_count();
        let fee_count_before = fee.get_sample_count();

        METRICS.record_payment(method, 21.0, 1.0);

        assert_eq!(payments.get(), payments_before + 1);
        assert_eq!(amount.get_sample_count(), amount_count_before + 1);
        assert_eq!(fee.get_sample_count(), fee_count_before + 1);
    }
}
