//! FakeWallet environment variables

use std::env;

use cdk::nuts::CurrencyUnit;

use crate::config::{FakeWallet, FakeWalletCustomPaymentMethod, FakeWalletKeysetRotation};

// Fake Wallet environment variables
pub const ENV_FAKE_WALLET_SUPPORTED_UNITS: &str = "CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS";
pub const ENV_FAKE_WALLET_FEE_PERCENT: &str = "CDK_MINTD_FAKE_WALLET_FEE_PERCENT";
pub const ENV_FAKE_WALLET_RESERVE_FEE_MIN: &str = "CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN";
pub const ENV_FAKE_WALLET_CUSTOM_PAYMENT_METHODS: &str =
    "CDK_MINTD_FAKE_WALLET_CUSTOM_PAYMENT_METHODS";
pub const ENV_FAKE_WALLET_MIN_DELAY: &str = "CDK_MINTD_FAKE_WALLET_MIN_DELAY";
pub const ENV_FAKE_WALLET_MAX_DELAY: &str = "CDK_MINTD_FAKE_WALLET_MAX_DELAY";
/// JSON array of keyset rotations, e.g.:
/// `[{"unit":"sat","version":"v3","input_fee_ppk":0,"expired":true}]`
pub const ENV_FAKE_WALLET_KEYSET_ROTATIONS: &str = "CDK_MINTD_FAKE_WALLET_KEYSET_ROTATIONS";

impl FakeWallet {
    pub fn from_env(mut self) -> Self {
        // Supported Units - expects comma-separated list
        if let Ok(units_str) = env::var(ENV_FAKE_WALLET_SUPPORTED_UNITS) {
            if let Ok(units) = units_str
                .split(',')
                .map(|s| s.trim().parse())
                .collect::<Result<Vec<CurrencyUnit>, _>>()
            {
                self.supported_units = units;
            }
        }

        if let Ok(fee_str) = env::var(ENV_FAKE_WALLET_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_FAKE_WALLET_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        if let Ok(methods_str) = env::var(ENV_FAKE_WALLET_CUSTOM_PAYMENT_METHODS) {
            self.custom_payment_methods = methods_str
                .split(',')
                .filter_map(parse_custom_payment_method_env)
                .collect();
        }

        if let Ok(min_delay_str) = env::var(ENV_FAKE_WALLET_MIN_DELAY) {
            if let Ok(min_delay) = min_delay_str.parse() {
                self.min_delay_time = min_delay;
            }
        }

        if let Ok(max_delay_str) = env::var(ENV_FAKE_WALLET_MAX_DELAY) {
            if let Ok(max_delay) = max_delay_str.parse() {
                self.max_delay_time = max_delay;
            }
        }

        if let Ok(rotations_str) = env::var(ENV_FAKE_WALLET_KEYSET_ROTATIONS) {
            if let Ok(rotations) =
                serde_json::from_str::<Vec<FakeWalletKeysetRotation>>(&rotations_str)
            {
                self.keyset_rotations = rotations;
            }
        }

        self
    }
}

fn parse_custom_payment_method_env(value: &str) -> Option<FakeWalletCustomPaymentMethod> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let Some((method, unit)) = value.split_once(':') else {
        return Some(FakeWalletCustomPaymentMethod::Method(value.to_string()));
    };

    let method = method.trim();
    let unit = unit.trim();
    if method.is_empty() || unit.is_empty() {
        return None;
    }

    unit.parse()
        .ok()
        .map(|unit| FakeWalletCustomPaymentMethod::MethodForUnit {
            method: method.to_string(),
            unit,
        })
}
