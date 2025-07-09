use prometheus::{Gauge, Histogram, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Registry};
use std::sync::Arc;

/// Custom metrics for CDK applications
#[derive(Clone, Debug)]
pub struct CdkMetrics {
    registry: Arc<Registry>,
    
    // HTTP metrics
    http_requests_total: IntCounter,
    http_request_duration: Histogram,
    http_responses_by_status: IntCounter,
    
    // Authentication metrics
    auth_attempts_total: IntCounter,
    auth_successes_total: IntCounter,
    
    // Wallet metrics
    wallet_operations_total: IntCounter,
    wallet_balance: Gauge,
    wallet_proofs_count: IntGauge,
    
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
    
    // Custom business metrics
    cashu_tokens_issued: IntCounter,
    cashu_tokens_spent: IntCounter,
    mint_keysets_active: IntGauge,
}

impl CdkMetrics {
    /// Create a new instance with default metrics
    pub fn new() -> crate::Result<Self> {
        let registry = Arc::new(Registry::new());
        
        let http_requests_total = IntCounter::new(
            "cdk_http_requests_total",
            "Total number of HTTP requests"
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;
        
        let http_request_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_http_request_duration_seconds",
                "HTTP request duration in seconds"
            ).buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
        )?;
        registry.register(Box::new(http_request_duration.clone()))?;
        
        let http_responses_by_status = IntCounter::new(
            "cdk_http_responses_by_status_total",
            "HTTP responses by status code"
        )?;
        registry.register(Box::new(http_responses_by_status.clone()))?;
        
        let auth_attempts_total = IntCounter::new(
            "cdk_auth_attempts_total",
            "Total authentication attempts"
        )?;
        registry.register(Box::new(auth_attempts_total.clone()))?;
        
        let auth_successes_total = IntCounter::new(
            "cdk_auth_successes_total", 
            "Total successful authentications"
        )?;
        registry.register(Box::new(auth_successes_total.clone()))?;
        
        let wallet_operations_total = IntCounter::new(
            "cdk_wallet_operations_total",
            "Total wallet operations"
        )?;
        registry.register(Box::new(wallet_operations_total.clone()))?;
        
        let wallet_balance = Gauge::new(
            "cdk_wallet_balance_sats",
            "Current wallet balance in satoshis"
        )?;
        registry.register(Box::new(wallet_balance.clone()))?;
        
        let wallet_proofs_count = IntGauge::new(
            "cdk_wallet_proofs_count",
            "Number of proofs in wallet"
        )?;
        registry.register(Box::new(wallet_proofs_count.clone()))?;
        
        let lightning_payments_total = IntCounter::new(
            "cdk_lightning_payments_total",
            "Total Lightning payments"
        )?;
        registry.register(Box::new(lightning_payments_total.clone()))?;
        
        let lightning_payment_amount = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_lightning_payment_amount_sats",
                "Lightning payment amounts in satoshis"
            ).buckets(vec![1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0])
        )?;
        registry.register(Box::new(lightning_payment_amount.clone()))?;
        
        let lightning_payment_fees = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_lightning_payment_fees_sats",
                "Lightning payment fees in satoshis"
            ).buckets(vec![0.0, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0])
        )?;
        registry.register(Box::new(lightning_payment_fees.clone()))?;
        
        let db_operations_total = IntCounter::new(
            "cdk_db_operations_total",
            "Total database operations"
        )?;
        registry.register(Box::new(db_operations_total.clone()))?;
        
        let db_operation_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_db_operation_duration_seconds",
                "Database operation duration in seconds"
            ).buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0])
        )?;
        registry.register(Box::new(db_operation_duration.clone()))?;
        
        let db_connections_active = IntGauge::new(
            "cdk_db_connections_active",
            "Number of active database connections"
        )?;
        registry.register(Box::new(db_connections_active.clone()))?;
        
        let errors_total = IntCounter::new(
            "cdk_errors_total",
            "Total errors"
        )?;
        registry.register(Box::new(errors_total.clone()))?;
        
        let cashu_tokens_issued = IntCounter::new(
            "cdk_cashu_tokens_issued_total",
            "Total Cashu tokens issued"
        )?;
        registry.register(Box::new(cashu_tokens_issued.clone()))?;
        
        let cashu_tokens_spent = IntCounter::new(
            "cdk_cashu_tokens_spent_total",
            "Total Cashu tokens spent"
        )?;
        registry.register(Box::new(cashu_tokens_spent.clone()))?;
        
        let mint_keysets_active = IntGauge::new(
            "cdk_mint_keysets_active",
            "Number of active mint keysets"
        )?;
        registry.register(Box::new(mint_keysets_active.clone()))?;

        let mint_operations_total = IntCounterVec::new(
            prometheus::Opts::new(
                "cdk_mint_operations_total",
                "Total number of mint operations"
            ),
            &["operation", "status"]

        )?;
        registry.register(Box::new(mint_operations_total.clone()))?;

        let mint_operation_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "cdk_mint_operation_duration_seconds",
                "Duration of mint operations in seconds"
            ).buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
        )?;
        registry.register(Box::new(mint_operation_duration.clone()))?;

        let mint_in_flight_requests = IntGaugeVec::new(
            prometheus::Opts::new(
                "cdk_mint_in_flight_requests",
                "Number of in-flight mint requests"
            ),
            &["operation"]

        )?;
        registry.register(Box::new(mint_in_flight_requests.clone()))?;
        
        Ok(Self {
            registry,
            http_requests_total,
            http_request_duration,
            http_responses_by_status,
            auth_attempts_total,
            auth_successes_total,
            wallet_operations_total,
            wallet_balance,
            wallet_proofs_count,
            lightning_payments_total,
            lightning_payment_amount,
            lightning_payment_fees,
            db_operations_total,
            db_operation_duration,
            db_connections_active,
            errors_total,
            cashu_tokens_issued,
            cashu_tokens_spent,
            mint_keysets_active,
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
    pub fn record_http_request(&self) {
        self.http_requests_total.inc();
    }
    
    pub fn record_http_request_duration(&self, duration_seconds: f64) {
        self.http_request_duration.observe(duration_seconds);
    }
    
    pub fn record_http_response_status(&self) {
        self.http_responses_by_status.inc();
    }
    
    // Authentication metrics methods
    pub fn record_auth_attempt(&self) {
        self.auth_attempts_total.inc();
    }
    
    pub fn record_auth_success(&self) {
        self.auth_successes_total.inc();
    }
    
    // Wallet metrics methods
    pub fn record_wallet_operation(&self) {
        self.wallet_operations_total.inc();
    }
    
    pub fn set_wallet_balance(&self, balance: f64) {
        self.wallet_balance.set(balance);
    }
    
    pub fn set_wallet_proofs_count(&self, count: i64) {
        self.wallet_proofs_count.set(count);
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
    
    // Cashu-specific metrics methods
    pub fn record_cashu_token_issued(&self) {
        self.cashu_tokens_issued.inc();
    }
    
    pub fn record_cashu_token_spent(&self) {
        self.cashu_tokens_spent.inc();
    }
    
    pub fn set_mint_keysets_active(&self, count: i64) {
        self.mint_keysets_active.set(count);
    }

    // Mint metrics methods
    pub fn record_mint_operation(&self, operation: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        self.mint_operations_total.with_label_values(&[operation, status]).inc();
    }

    pub fn inc_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests.with_label_values(&[operation]).inc();
    }

    pub fn dec_in_flight_requests(&self, operation: &str) {
        self.mint_in_flight_requests.with_label_values(&[operation]).dec();
    }
}

impl Default for CdkMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create default CdkMetrics")
    }
}