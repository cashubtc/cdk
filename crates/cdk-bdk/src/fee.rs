//! Fee estimation utilities
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_esplora::esplora_client::Builder;

use crate::error::Error;
use crate::types::PaymentTier;
use crate::{CdkBdk, ChainSource};

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
        let rate_result = self.fetch_fee_rate_from_source(target_blocks).await;

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

    async fn fetch_fee_rate_from_source(&self, target_blocks: u16) -> Result<f64, Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                // Use a blocking spawn since Client is synchronous
                let rpc_config = rpc_config.clone();
                let host = rpc_config.host.clone();
                let port = rpc_config.port;

                tokio::task::spawn_blocking(move || {
                    let rpc_client = Client::new(
                        &format!("http://{}:{}", host, port),
                        Auth::UserPass(rpc_config.user, rpc_config.password),
                    )?;

                    let estimate = rpc_client.estimate_smart_fee(target_blocks, None)?;

                    if let Some(fee_rate_btc_per_kvb) = estimate.fee_rate {
                        // convert BTC/kvB to sat/vB:
                        // 1 BTC = 100,000,000 sat
                        // 1 kvB = 1,000 vB
                        // sat/vB = (BTC/kvB * 100,000,000) / 1,000 = BTC/kvB * 100_000
                        let sat_per_vb = fee_rate_btc_per_kvb.to_btc() * 100_000.0;
                        Ok(sat_per_vb)
                    } else {
                        Err(Error::FeeEstimationUnavailable)
                    }
                })
                .await
                .map_err(|e| Error::FeeEstimationFailed(e.to_string()))?
            }
            ChainSource::Esplora { url, .. } => {
                let client = Builder::new(url)
                    .build_async()
                    .map_err(|e| Error::Esplora(e.to_string()))?;

                let estimates = client
                    .get_fee_estimates()
                    .await
                    .map_err(|e| Error::Esplora(e.to_string()))?;

                // Esplora returns a map of target blocks (as u16) to fee rate (sat/vB as f64)
                if let Some(&rate) = estimates.get(&target_blocks) {
                    return Ok(rate);
                }

                // Fallback: find the closest available target block estimate that is >= our target
                let mut available_targets: Vec<u16> = estimates.keys().copied().collect();
                available_targets.sort_unstable();

                for &t in &available_targets {
                    if t >= target_blocks {
                        if let Some(&rate) = estimates.get(&t) {
                            return Ok(rate);
                        }
                    }
                }

                // If nothing >= target, take the largest available
                if let Some(&t) = available_targets.last() {
                    if let Some(&rate) = estimates.get(&t) {
                        return Ok(rate);
                    }
                }

                Err(Error::FeeEstimationUnavailable)
            }
        }
    }
}
