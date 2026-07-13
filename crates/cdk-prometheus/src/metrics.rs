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

    // Federation metrics
    federation_consensus_operation_count: IntGaugeVec,
    federation_sync_ready: IntGaugeVec,
    federation_sync_state: IntGaugeVec,
    federation_catch_up_certificate_quorum_operation_count: IntGaugeVec,
    federation_payment_observation_reports: IntGaugeVec,
    federation_peer_connected: IntGaugeVec,
    federation_peer_operation_count: IntGaugeVec,
    federation_pending_mempool_items: IntGaugeVec,
    federation_accepted_operations_total: IntCounterVec,
    federation_rejected_operations_total: IntCounterVec,
    federation_signing_share_latency: HistogramVec,
    federation_aggregation_failures_total: IntCounterVec,
    federation_state_digest_mismatches_total: IntCounterVec,
}

#[derive(Debug)]
struct FederationMetricCollectors {
    consensus_operation_count: IntGaugeVec,
    sync_ready: IntGaugeVec,
    sync_state: IntGaugeVec,
    catch_up_certificate_quorum_operation_count: IntGaugeVec,
    payment_observation_reports: IntGaugeVec,
    peer_connected: IntGaugeVec,
    peer_operation_count: IntGaugeVec,
    pending_mempool_items: IntGaugeVec,
    accepted_operations_total: IntCounterVec,
    rejected_operations_total: IntCounterVec,
    signing_share_latency: HistogramVec,
    aggregation_failures_total: IntCounterVec,
    state_digest_mismatches_total: IntCounterVec,
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

        // Create and register federation metrics
        let federation_metrics = Self::create_federation_metrics(&registry)?;

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
            federation_consensus_operation_count: federation_metrics.consensus_operation_count,
            federation_sync_ready: federation_metrics.sync_ready,
            federation_sync_state: federation_metrics.sync_state,
            federation_catch_up_certificate_quorum_operation_count: federation_metrics
                .catch_up_certificate_quorum_operation_count,
            federation_payment_observation_reports: federation_metrics.payment_observation_reports,
            federation_peer_connected: federation_metrics.peer_connected,
            federation_peer_operation_count: federation_metrics.peer_operation_count,
            federation_pending_mempool_items: federation_metrics.pending_mempool_items,
            federation_accepted_operations_total: federation_metrics.accepted_operations_total,
            federation_rejected_operations_total: federation_metrics.rejected_operations_total,
            federation_signing_share_latency: federation_metrics.signing_share_latency,
            federation_aggregation_failures_total: federation_metrics.aggregation_failures_total,
            federation_state_digest_mismatches_total: federation_metrics
                .state_digest_mismatches_total,
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

    /// Create and register federation metrics
    ///
    /// # Errors
    /// Returns an error if any of the metrics cannot be created or registered
    fn create_federation_metrics(registry: &Registry) -> crate::Result<FederationMetricCollectors> {
        let federation_consensus_operation_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_consensus_operation_count",
                "Number of consensus operations accepted by a federation member",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_consensus_operation_count.clone()))?;

        let federation_sync_ready = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_sync_ready",
                "Whether a federation member reports it is ready to serve federation requests",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_sync_ready.clone()))?;

        let federation_sync_state = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_sync_state",
                "Current sync state for a federation member, exported as one-hot gauges",
            ),
            &["member", "state"],
        )?;
        registry.register(Box::new(federation_sync_state.clone()))?;

        let federation_catch_up_certificate_quorum_operation_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_catch_up_certificate_quorum_operation_count",
                "Highest federation operation count covered by quorum catch-up certificates",
            ),
            &["member"],
        )?;
        registry.register(Box::new(
            federation_catch_up_certificate_quorum_operation_count.clone(),
        ))?;

        let federation_payment_observation_reports = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_payment_observation_reports",
                "Number of redacted payment observation reports grouped by status and recommended action",
            ),
            &["member", "status", "action"],
        )?;
        registry.register(Box::new(federation_payment_observation_reports.clone()))?;

        let federation_peer_connected = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_peer_connected",
                "Whether the latest private REST sync attempt to a federation peer succeeded",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_peer_connected.clone()))?;

        let federation_peer_operation_count = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_peer_operation_count",
                "Latest consensus operation count reported by a federation peer",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_peer_operation_count.clone()))?;

        let federation_pending_mempool_items = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_federation_pending_mempool_items",
                "Number of locally queued or dispatched federation consensus items awaiting finalization",
            ),
            &["member"],
        )?;
        registry.register(Box::new(federation_pending_mempool_items.clone()))?;

        let federation_accepted_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_accepted_operations_total",
                "Total number of federation operations accepted into local consensus",
            ),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_accepted_operations_total.clone()))?;

        let federation_rejected_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_rejected_operations_total",
                "Total number of federation operations rejected before local consensus acceptance",
            ),
            &["member", "operation", "reason"],
        )?;
        registry.register(Box::new(federation_rejected_operations_total.clone()))?;

        let federation_signing_share_latency = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "cdk_federation_signing_share_latency_seconds",
                "Latency for producing local federation BLS signing shares in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
            ]),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_signing_share_latency.clone()))?;

        let federation_aggregation_failures_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_aggregation_failures_total",
                "Total number of wallet-side federation signature aggregation failures",
            ),
            &["member", "operation"],
        )?;
        registry.register(Box::new(federation_aggregation_failures_total.clone()))?;

        let federation_state_digest_mismatches_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_federation_state_digest_mismatches_total",
                "Total number of detected materialized-state digest mismatches with federation peers",
            ),
            &["member", "peer"],
        )?;
        registry.register(Box::new(federation_state_digest_mismatches_total.clone()))?;

        Ok(FederationMetricCollectors {
            consensus_operation_count: federation_consensus_operation_count,
            sync_ready: federation_sync_ready,
            sync_state: federation_sync_state,
            catch_up_certificate_quorum_operation_count:
                federation_catch_up_certificate_quorum_operation_count,
            payment_observation_reports: federation_payment_observation_reports,
            peer_connected: federation_peer_connected,
            peer_operation_count: federation_peer_operation_count,
            pending_mempool_items: federation_pending_mempool_items,
            accepted_operations_total: federation_accepted_operations_total,
            rejected_operations_total: federation_rejected_operations_total,
            signing_share_latency: federation_signing_share_latency,
            aggregation_failures_total: federation_aggregation_failures_total,
            state_digest_mismatches_total: federation_state_digest_mismatches_total,
        })
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

    /// Increment the number of active database connections.
    pub fn increment_db_connections_active(&self) {
        self.db_connections_active.inc();
    }

    /// Decrement the number of active database connections.
    pub fn decrement_db_connections_active(&self) {
        self.db_connections_active.dec();
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

    /// Set the observed federation consensus operation count for a member.
    pub fn set_federation_consensus_operation_count(&self, member: &str, count: i64) {
        self.federation_consensus_operation_count
            .with_label_values(&[member])
            .set(count);
    }

    /// Set whether a federation member reports that it is ready.
    pub fn set_federation_sync_ready(&self, member: &str, ready: bool) {
        self.federation_sync_ready
            .with_label_values(&[member])
            .set(if ready { 1 } else { 0 });
    }

    /// Set a one-hot federation sync-state gauge.
    pub fn set_federation_sync_state(&self, member: &str, state: &str, active: bool) {
        self.federation_sync_state
            .with_label_values(&[member, state])
            .set(if active { 1 } else { 0 });
    }

    /// Set the operation count covered by quorum catch-up certificates.
    pub fn set_federation_catch_up_certificate_quorum_operation_count(
        &self,
        member: &str,
        count: i64,
    ) {
        self.federation_catch_up_certificate_quorum_operation_count
            .with_label_values(&[member])
            .set(count);
    }

    /// Set the count of redacted payment observation reports for a member.
    pub fn set_federation_payment_observation_report_count(
        &self,
        member: &str,
        status: &str,
        action: &str,
        count: i64,
    ) {
        self.federation_payment_observation_reports
            .with_label_values(&[member, status, action])
            .set(count);
    }

    /// Set whether the latest private REST sync attempt to a federation peer succeeded.
    pub fn set_federation_peer_connected(&self, member: &str, peer: &str, connected: bool) {
        self.federation_peer_connected
            .with_label_values(&[member, peer])
            .set(if connected { 1 } else { 0 });
    }

    /// Set the latest operation count reported by a federation peer.
    pub fn set_federation_peer_operation_count(&self, member: &str, peer: &str, count: i64) {
        self.federation_peer_operation_count
            .with_label_values(&[member, peer])
            .set(count);
    }

    /// Set the number of federation consensus items awaiting finalization.
    pub fn set_federation_pending_mempool_items(&self, member: &str, count: i64) {
        self.federation_pending_mempool_items
            .with_label_values(&[member])
            .set(count);
    }

    /// Record a federation operation accepted into local consensus.
    pub fn record_federation_accepted_operation(&self, member: &str, operation: &str) {
        self.federation_accepted_operations_total
            .with_label_values(&[member, operation])
            .inc();
    }

    /// Record a federation operation rejected before local consensus acceptance.
    pub fn record_federation_rejected_operation(
        &self,
        member: &str,
        operation: &str,
        reason: &str,
    ) {
        self.federation_rejected_operations_total
            .with_label_values(&[member, operation, reason])
            .inc();
    }

    /// Record local federation signing-share production latency.
    pub fn record_federation_signing_share_latency(
        &self,
        member: &str,
        operation: &str,
        duration_seconds: f64,
    ) {
        self.federation_signing_share_latency
            .with_label_values(&[member, operation])
            .observe(duration_seconds);
    }

    /// Record a wallet-side federation signature aggregation failure.
    pub fn record_federation_aggregation_failure(&self, member: &str, operation: &str) {
        self.federation_aggregation_failures_total
            .with_label_values(&[member, operation])
            .inc();
    }

    /// Record a materialized-state digest mismatch with a federation peer.
    pub fn record_federation_state_digest_mismatch(&self, member: &str, peer: &str) {
        self.federation_state_digest_mismatches_total
            .with_label_values(&[member, peer])
            .inc();
    }
}

impl Default for CdkMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create default CdkMetrics")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use super::{CdkMetrics, MintMetricGuard, METRICS};

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

    #[test]
    fn federation_member_status_metrics_are_labeled_by_member() {
        let metrics = CdkMetrics::new().expect("metrics should be created");
        let member = "2";
        let peer = "3";

        metrics.set_federation_consensus_operation_count(member, 7);
        metrics.set_federation_sync_ready(member, false);
        metrics.set_federation_sync_state(member, "healthy", false);
        metrics.set_federation_sync_state(member, "lagging", true);
        metrics.set_federation_catch_up_certificate_quorum_operation_count(member, 5);
        metrics.set_federation_peer_connected(member, peer, true);
        metrics.set_federation_peer_operation_count(member, peer, 9);
        metrics.set_federation_pending_mempool_items(member, 2);
        metrics.record_federation_state_digest_mismatch(member, peer);

        assert_eq!(
            metrics
                .federation_consensus_operation_count
                .with_label_values(&[member])
                .get(),
            7
        );
        assert_eq!(
            metrics
                .federation_sync_ready
                .with_label_values(&[member])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .federation_sync_state
                .with_label_values(&[member, "healthy"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .federation_sync_state
                .with_label_values(&[member, "lagging"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_catch_up_certificate_quorum_operation_count
                .with_label_values(&[member])
                .get(),
            5
        );
        assert_eq!(
            metrics
                .federation_peer_connected
                .with_label_values(&[member, peer])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_peer_operation_count
                .with_label_values(&[member, peer])
                .get(),
            9
        );
        assert_eq!(
            metrics
                .federation_pending_mempool_items
                .with_label_values(&[member])
                .get(),
            2
        );
        assert_eq!(
            metrics
                .federation_state_digest_mismatches_total
                .with_label_values(&[member, peer])
                .get(),
            1
        );
    }

    #[test]
    fn federation_payment_report_metrics_are_labeled_by_status_and_action() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        metrics.set_federation_payment_observation_report_count("2", "conflicting", "inspect", 3);

        assert_eq!(
            metrics
                .federation_payment_observation_reports
                .with_label_values(&["2", "conflicting", "inspect"])
                .get(),
            3
        );
    }

    #[test]
    fn federation_operation_metrics_are_low_cardinality() {
        let metrics = CdkMetrics::new().expect("metrics should be created");

        metrics.record_federation_accepted_operation("2", "mint");
        metrics.record_federation_rejected_operation("2", "swap", "conflict");
        metrics.record_federation_signing_share_latency("2", "mint", 0.01);
        metrics.record_federation_aggregation_failure("wallet", "restore");

        assert_eq!(
            metrics
                .federation_accepted_operations_total
                .with_label_values(&["2", "mint"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_rejected_operations_total
                .with_label_values(&["2", "swap", "conflict"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .federation_signing_share_latency
                .with_label_values(&["2", "mint"])
                .get_sample_count(),
            1
        );
        assert_eq!(
            metrics
                .federation_aggregation_failures_total
                .with_label_values(&["wallet", "restore"])
                .get(),
            1
        );

        let scrape = prometheus::TextEncoder::new()
            .encode_to_string(&metrics.registry().gather())
            .expect("encode metrics");
        assert!(!scrape.contains("quote="));
        assert!(!scrape.contains("invoice="));
        assert!(!scrape.contains("proof="));
        assert!(!scrape.contains("secret="));
        assert!(!scrape.contains("token="));
        assert!(!scrape.contains("ip="));
    }
}
