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

use std::cell::Cell;
use std::str::FromStr;

use bdk_wallet::bitcoin::{
    absolute, transaction, Amount as BitcoinAmount, FeeRate, Script, Transaction, TxIn, TxOut,
    Weight,
};
use bdk_wallet::coin_selection::{
    decide_change, BranchAndBoundCoinSelection, CoinSelectionAlgorithm, CoinSelectionResult,
    Excess, InsufficientFunds,
};
use bdk_wallet::{KeychainKind, Utxo, WeightedUtxo};

use crate::error::Error;
use crate::types::{FeeEstimationConfig, PaymentTier};
use crate::CdkBdk;

const P2WPKH_CHANGE_OUTPUT_VBYTES: u64 = 31;

pub(crate) fn fee_rate_from_sat_per_vb(sat_per_vb: f64) -> Result<FeeRate, Error> {
    if !sat_per_vb.is_finite() || sat_per_vb <= 0.0 {
        return Err(Error::FeeEstimationFailed(format!(
            "invalid fee rate {sat_per_vb} sat/vB"
        )));
    }

    let rounded_sat_per_vb = sat_per_vb.ceil();
    if rounded_sat_per_vb > f64::from(u32::MAX) {
        return Err(Error::FeeEstimationFailed(format!(
            "fee rate {sat_per_vb} sat/vB exceeds supported range"
        )));
    }

    Ok(FeeRate::from_sat_per_vb_u32(rounded_sat_per_vb as u32))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuoteSelectionPath {
    Bnb,
    PessimisticFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuoteFeeEstimate {
    pub(crate) raw_fee_sat: u64,
    pub(crate) padded_fee_sat: u64,
    pub(crate) fee_reserve_sat: u64,
    pub(crate) selected_input_count: usize,
    pub(crate) sampled_utxo_count: usize,
    pub(crate) path: QuoteSelectionPath,
}

#[derive(Debug, Clone, Copy)]
struct PessimisticFallback;

impl CoinSelectionAlgorithm for PessimisticFallback {
    fn coin_select<R: bdk_wallet::bitcoin::key::rand::RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: BitcoinAmount,
        drain_script: &Script,
        _rand: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        optional_utxos.sort_unstable_by_key(|weighted_utxo| weighted_utxo.utxo.txout().value);

        let mut selected = Vec::new();
        let mut selected_amount = BitcoinAmount::ZERO;
        let mut fee_amount = BitcoinAmount::ZERO;

        for weighted_utxo in required_utxos.into_iter().chain(optional_utxos) {
            let input_fee = input_fee(fee_rate, weighted_utxo.satisfaction_weight);
            let effective_value = weighted_utxo
                .utxo
                .txout()
                .value
                .checked_sub(input_fee)
                .unwrap_or(BitcoinAmount::ZERO);

            if selected_amount < target_amount + fee_amount || effective_value > BitcoinAmount::ZERO
            {
                fee_amount += input_fee;
                selected_amount += weighted_utxo.utxo.txout().value;
                selected.push(weighted_utxo.utxo);
            }

            if selected_amount >= target_amount + fee_amount {
                break;
            }
        }

        let amount_needed_with_fees = target_amount + fee_amount;
        if selected_amount < amount_needed_with_fees {
            return Err(InsufficientFunds {
                needed: amount_needed_with_fees,
                available: selected_amount,
            });
        }

        let remaining_amount = selected_amount - amount_needed_with_fees;
        let excess = decide_change(remaining_amount, fee_rate, drain_script);

        Ok(CoinSelectionResult {
            selected,
            fee_amount,
            excess,
        })
    }
}

#[derive(Debug)]
struct TrackingFallback<'a> {
    fallback: PessimisticFallback,
    used: &'a Cell<bool>,
}

impl CoinSelectionAlgorithm for TrackingFallback<'_> {
    fn coin_select<R: bdk_wallet::bitcoin::key::rand::RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: BitcoinAmount,
        drain_script: &Script,
        rand: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        self.used.set(true);
        self.fallback.coin_select(
            required_utxos,
            optional_utxos,
            fee_rate,
            target_amount,
            drain_script,
            rand,
        )
    }
}

fn input_fee(fee_rate: FeeRate, satisfaction_weight: Weight) -> BitcoinAmount {
    fee_rate
        * TxIn::default()
            .segwit_weight()
            .checked_add(satisfaction_weight)
            .expect("input weight should not overflow")
}

fn base_transaction_fee(
    fee_rate: FeeRate,
    recipient_script: &Script,
    amount_sat: u64,
) -> BitcoinAmount {
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: Vec::new(),
        output: vec![TxOut {
            value: BitcoinAmount::from_sat(amount_sat),
            script_pubkey: recipient_script.to_owned(),
        }],
    };

    fee_rate * tx.weight()
}

fn raw_fee_from_selection(base_fee: BitcoinAmount, result: &CoinSelectionResult) -> BitcoinAmount {
    let excess_fee = match result.excess {
        Excess::Change { fee, .. } => fee,
        Excess::NoChange {
            remaining_amount, ..
        } => remaining_amount,
    };

    base_fee + result.fee_amount + excess_fee
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

fn target_blocks_for_tier(tier: PaymentTier) -> u16 {
    match tier {
        PaymentTier::Immediate => 1,
        PaymentTier::Standard => 6,
        PaymentTier::Economy => 144,
    }
}

impl CdkBdk {
    pub(crate) async fn estimate_onchain_fee_reserve(
        &self,
        address: &str,
        amount_sat: u64,
        tier: PaymentTier,
    ) -> Result<QuoteFeeEstimate, Error> {
        let sat_per_vb = self
            .estimate_fee_rate_sat_per_vb(tier)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tier = ?tier,
                    error = %e,
                    "Fee-rate estimation failed, using configured fallback"
                );
                self.batch_config.fee_estimation.fallback_sat_per_vb
            });

        let fee_rate = fee_rate_from_sat_per_vb(sat_per_vb)?;
        let recipient_script = bdk_wallet::bitcoin::Address::from_str(address)
            .map_err(|e| Error::Wallet(e.to_string()))?
            .require_network(self.network)
            .map_err(|e| Error::Wallet(e.to_string()))?
            .script_pubkey();

        let (weighted_utxos, change_script) = {
            let wallet_with_db = self.wallet_with_db.lock().await;
            let max_inputs = self
                .batch_config
                .fee_estimation
                .quote_max_input_count
                .max(1);
            let weighted_utxos = wallet_with_db
                .wallet
                .list_unspent()
                .take(max_inputs)
                .map(|utxo| {
                    Ok(WeightedUtxo {
                        satisfaction_weight: wallet_with_db
                            .wallet
                            .public_descriptor(utxo.keychain)
                            .max_weight_to_satisfy()
                            .map_err(|e| Error::Wallet(e.to_string()))?,
                        utxo: Utxo::Local(utxo),
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let change_script = wallet_with_db
                .wallet
                .peek_address(KeychainKind::Internal, 0)
                .address
                .script_pubkey();
            (weighted_utxos, change_script)
        };

        if weighted_utxos.is_empty() {
            return Err(Error::NoSpendableUtxos);
        }
        let sampled_utxo_count = weighted_utxos.len();

        let base_fee = base_transaction_fee(fee_rate, recipient_script.as_script(), amount_sat);
        let target_amount = BitcoinAmount::from_sat(amount_sat) + base_fee;
        let fallback_used = Cell::new(false);
        let fallback = TrackingFallback {
            fallback: PessimisticFallback,
            used: &fallback_used,
        };
        let selector = BranchAndBoundCoinSelection::new(P2WPKH_CHANGE_OUTPUT_VBYTES, fallback);
        let mut rng = bdk_wallet::bitcoin::key::rand::thread_rng();
        let result = selector
            .coin_select(
                Vec::new(),
                weighted_utxos,
                fee_rate,
                target_amount,
                change_script.as_script(),
                &mut rng,
            )
            .map_err(|e| Error::FeeEstimationFailed(e.to_string()))?;

        let raw_fee_sat = raw_fee_from_selection(base_fee, &result).to_sat();
        let padded_fee_sat = apply_quote_fee_safety(raw_fee_sat, &self.batch_config.fee_estimation);
        let fee_reserve_sat = self.fee_reserve_for_estimate(padded_fee_sat);
        let path = if fallback_used.get() {
            QuoteSelectionPath::PessimisticFallback
        } else {
            QuoteSelectionPath::Bnb
        };

        tracing::debug!(
            tier = ?tier,
            sampled_utxo_count,
            selected_input_count = result.selected.len(),
            raw_fee_sat,
            padded_fee_sat,
            fee_reserve_sat,
            selection_path = ?path,
            "Estimated onchain fee reserve"
        );

        Ok(QuoteFeeEstimate {
            raw_fee_sat,
            padded_fee_sat,
            fee_reserve_sat,
            selected_input_count: result.selected.len(),
            sampled_utxo_count,
            path,
        })
    }

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

        fee_rate_from_sat_per_vb(rate)?;

        {
            let mut cache = self.fee_rate_cache.lock().await;
            cache.insert(tier, (rate, now));
        }

        Ok(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_rate_from_sat_per_vb_rejects_invalid_values() {
        for sat_per_vb in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                fee_rate_from_sat_per_vb(sat_per_vb).is_err(),
                "fee rate {sat_per_vb} should be rejected"
            );
        }
    }

    #[test]
    fn fee_rate_from_sat_per_vb_rejects_overlarge_values() {
        assert!(fee_rate_from_sat_per_vb(f64::from(u32::MAX) + 1.0).is_err());
    }

    #[test]
    fn fee_rate_from_sat_per_vb_rounds_up_valid_values() {
        let fee_rate = fee_rate_from_sat_per_vb(1.25).expect("valid fee rate");

        assert_eq!(fee_rate, FeeRate::from_sat_per_vb_u32(2));
    }
}
