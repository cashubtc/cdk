//! Fee estimation utilities for BDK-backed onchain melts.
//!
//! Melt quotes are created before the final BDK transaction is built, so quote
//! time cannot know the exact coin selection or final transaction weight. The
//! helpers in this module therefore estimate a conservative fee reserve from:
//! the requested payment amount, the current fee rate, a bounded sample of
//! wallet UTXO values, and configurable safety margins.
//!
//! The estimate is intentionally allowed to be high. Underquoting can make a
//! melt fail later when BDK builds and signs the real transaction; overquoting
//! only reserves extra e-cash that is returned as change by the melt flow. The
//! final transaction fee is still determined by BDK at payment time.

use crate::error::Error;
use crate::types::{FeeEstimationConfig, PaymentTier};
use crate::CdkBdk;

// Conservative P2WPKH-style defaults used when the quote cannot safely run BDK
// coin selection without mutating wallet state.
const HEURISTIC_VBYTES_PER_INPUT: u64 = 68;
const HEURISTIC_VBYTES_PER_OUTPUT: u64 = 31;
const HEURISTIC_VBYTES_OVERHEAD: u64 = 11;
const P2WPKH_CHANGE_OUTPUT_VBYTES: u64 = 31;
const DEFAULT_QUOTE_INPUT_COUNT: usize = 4;
const QUOTE_INPUT_VBYTES: u64 = 75;
pub(crate) const QUOTE_UTXO_SCAN_LIMIT: usize = 100;

/// Estimate the recipient output's serialized size from the actual script.
pub(crate) fn recipient_output_vbytes(script_pubkey: &bdk_wallet::bitcoin::Script) -> u64 {
    let txout = bdk_wallet::bitcoin::TxOut {
        value: bdk_wallet::bitcoin::Amount::ZERO,
        script_pubkey: script_pubkey.to_owned(),
    };

    bdk_wallet::bitcoin::consensus::serialize(&txout).len() as u64
}

/// Estimate a conservative quote-time input count from a bounded UTXO sample.
///
/// The sample is sorted largest-first to approximate a low-input coin
/// selection. When the sampled value appears sufficient, one extra input is
/// reserved as padding because the eventual BDK selection can differ. If the
/// sample is empty, an internal fallback is used. If the sample cannot cover
/// the amount plus rough fee, the configured maximum is used.
pub(crate) fn estimate_quote_input_count(
    amount_sat: u64,
    sat_per_vb: f64,
    utxo_values_sat: &[u64],
    config: &FeeEstimationConfig,
) -> usize {
    let max_inputs = config.quote_max_input_count.max(1);
    let default_inputs = DEFAULT_QUOTE_INPUT_COUNT.clamp(1, max_inputs);

    if utxo_values_sat.is_empty() {
        return default_inputs;
    }

    let mut values = utxo_values_sat.to_vec();
    values.sort_unstable_by(|a, b| b.cmp(a));

    let mut selected_value = 0u64;
    for (idx, value) in values.iter().take(max_inputs).enumerate() {
        let input_count = idx + 1;
        selected_value = selected_value.saturating_add(*value);
        let rough_fee = estimate_quote_fee_without_safety(
            sat_per_vb,
            quote_vbytes_for_input_count(input_count, HEURISTIC_VBYTES_PER_OUTPUT),
        );

        if selected_value >= amount_sat.saturating_add(rough_fee) {
            return input_count.saturating_add(1).min(max_inputs);
        }
    }

    max_inputs
}

/// Estimate quote transaction vbytes using bounded, conservative assumptions.
///
/// This combines the estimated input count, the actual recipient output script
/// size, and a fixed change output. It does not stage or persist any BDK wallet
/// changes.
pub(crate) fn estimate_quote_vbytes(
    amount_sat: u64,
    sat_per_vb: f64,
    recipient_script: &bdk_wallet::bitcoin::Script,
    utxo_values_sat: &[u64],
    config: &FeeEstimationConfig,
) -> u64 {
    let recipient_output_vbytes = recipient_output_vbytes(recipient_script);
    let input_count = estimate_quote_input_count(amount_sat, sat_per_vb, utxo_values_sat, config);

    quote_vbytes_for_input_count(input_count, recipient_output_vbytes)
}

/// Apply quote-time safety padding to a raw fee estimate.
///
/// The multiplier handles proportional error from fee-rate movement or input
/// count mismatch. The fixed margin handles small absolute misses and avoids
/// tiny quotes being too tight.
pub(crate) fn apply_quote_fee_safety(estimated_fee_sat: u64, config: &FeeEstimationConfig) -> u64 {
    let multiplier = config.quote_safety_multiplier.max(1.0);
    let multiplied = (estimated_fee_sat as f64 * multiplier).ceil() as u64;

    multiplied.saturating_add(config.quote_fixed_safety_sat)
}

pub(crate) fn estimate_quote_fee_without_safety(sat_per_vb: f64, vbytes: u64) -> u64 {
    (sat_per_vb * vbytes as f64).ceil() as u64
}

fn quote_vbytes_for_input_count(input_count: usize, recipient_output_vbytes: u64) -> u64 {
    HEURISTIC_VBYTES_OVERHEAD
        + (input_count as u64 * QUOTE_INPUT_VBYTES.max(HEURISTIC_VBYTES_PER_INPUT))
        + recipient_output_vbytes
        + P2WPKH_CHANGE_OUTPUT_VBYTES
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
