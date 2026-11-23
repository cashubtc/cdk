use std::time::Instant;

/// Logs how long one wallet verification stage took.
///
/// `stage` is a fixed label naming the stage in logs.
pub(crate) fn record_wallet_performance_stage(
    stage: &'static str,
    started_at: Instant,
    work_items: usize,
    success: bool,
) {
    tracing::debug!(
        target: "cdk::wallet::performance",
        stage,
        work_items,
        duration_seconds = started_at.elapsed().as_secs_f64(),
        success,
        "Wallet performance stage completed"
    );
}
