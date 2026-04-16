use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::CdkBdk;

/// Threshold at which prolonged consecutive failures are escalated from
/// `warn!` to `error!` so operators notice sustained outages.
pub(crate) const SUSTAINED_FAILURE_THRESHOLD: u32 = 10;

/// Log a per-iteration failure at an appropriate severity based on how
/// many consecutive failures have occurred.
pub(crate) fn log_sync_failure(context: &str, err: &Error, consecutive: u32) {
    if consecutive >= SUSTAINED_FAILURE_THRESHOLD {
        tracing::error!(
            consecutive_failures = consecutive,
            transient = err.is_transient(),
            "{context}: {err}"
        );
    } else {
        tracing::warn!(
            consecutive_failures = consecutive,
            transient = err.is_transient(),
            "{context}: {err}"
        );
    }
}

impl CdkBdk {
    /// Run the reconciliation helpers that inspect the wallet after blocks
    /// have been applied. Each helper's failure is logged but does not
    /// tear down the sync task; a subsequent tick will retry naturally.
    pub(crate) async fn run_reconciliation(&self) {
        if let Err(e) = self.scan_for_new_payments().await {
            tracing::warn!(
                transient = e.is_transient(),
                "scan_for_new_payments failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.check_receive_saga_confirmations().await {
            tracing::warn!(
                transient = e.is_transient(),
                "check_receive_saga_confirmations failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.check_send_saga_confirmations().await {
            tracing::warn!(
                transient = e.is_transient(),
                "check_send_saga_confirmations failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.rebroadcast_stuck_batches().await {
            tracing::warn!(
                transient = e.is_transient(),
                "rebroadcast_stuck_batches failed during reconciliation: {e}"
            );
        }
    }

    pub(crate) async fn sync_wallet(&self, cancel_token: CancellationToken) -> Result<(), Error> {
        self.chain_source.sync_wallet(self, cancel_token).await
    }
}
