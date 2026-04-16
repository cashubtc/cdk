//! Fee estimation utilities

use crate::error::Error;
use crate::types::PaymentTier;
use crate::CdkBdk;

// Constants for P2WPKH heuristic fee estimation
const HEURISTIC_VBYTES_PER_INPUT: u64 = 68;
const HEURISTIC_VBYTES_PER_OUTPUT: u64 = 31;
const HEURISTIC_VBYTES_OVERHEAD: u64 = 11;

/// Estimate the virtual bytes of a batch transaction heuristically
///
/// This assumes P2WPKH inputs and outputs. Without knowing the actual
/// coin selection, we conservatively estimate 2 inputs per output.
pub(crate) fn estimate_batch_vbytes_heuristic(num_recipients: usize) -> u64 {
    let estimated_inputs = std::cmp::max(1, num_recipients) as u64 * 2;
    // Recipients + 1 change output
    let estimated_outputs = num_recipients as u64 + 1;

    HEURISTIC_VBYTES_OVERHEAD
        + (estimated_inputs * HEURISTIC_VBYTES_PER_INPUT)
        + (estimated_outputs * HEURISTIC_VBYTES_PER_OUTPUT)
}

fn target_blocks_for_tier(tier: PaymentTier) -> u16 {
    match tier {
        PaymentTier::Immediate => 1,
        PaymentTier::Standard => 6,
        PaymentTier::Economy => 144,
    }
}

impl CdkBdk {
    /// Estimate the fee rate in satoshis per virtual byte for a given tier.
    ///
    /// Checks the cache first, then falls back to the configured chain source.
    pub(crate) async fn estimate_fee_rate_sat_per_vb(
        &self,
        tier: PaymentTier,
    ) -> Result<f64, Error> {
        let now = crate::util::unix_now();

        {
            let cache = self.fee_rate_cache.lock().await;
            if let Some(&(rate, ts)) = cache.get(&tier) {
                if now.saturating_sub(ts) <= self.batch_config.fee_estimation.cache_ttl_secs {
                    return Ok(rate);
                }
            }
        }

        let target_blocks = target_blocks_for_tier(tier);
        let rate_result = self.chain_source.fetch_fee_rate(target_blocks).await;

        let rate = match rate_result {
            Ok(rate) => rate,
            Err(e) => {
                tracing::warn!(
                    tier = ?tier,
                    error = %e,
                    "Failed to fetch fee rate from source"
                );
                return Err(e);
            }
        };

        {
            let mut cache = self.fee_rate_cache.lock().await;
            cache.insert(tier, (rate, now));
        }

        Ok(rate)
    }
}
