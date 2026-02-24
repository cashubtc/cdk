//! Blink environment variables

use std::env;

use cdk::nuts::CurrencyUnit;

use crate::config::Blink;

// Blink environment variables
pub const ENV_BLINK_API_KEY: &str = "CDK_MINTD_BLINK_API_KEY";
pub const ENV_BLINK_API_URL: &str = "CDK_MINTD_BLINK_API_URL";
pub const ENV_BLINK_SUPPORTED_UNITS: &str = "CDK_MINTD_BLINK_SUPPORTED_UNITS";
pub const ENV_BLINK_FEE_PERCENT: &str = "CDK_MINTD_BLINK_FEE_PERCENT";
pub const ENV_BLINK_RESERVE_FEE_MIN: &str = "CDK_MINTD_BLINK_RESERVE_FEE_MIN";

impl Blink {
    pub fn from_env(mut self) -> Self {
        if let Ok(api_key) = env::var(ENV_BLINK_API_KEY) {
            self.api_key = api_key;
        }

        if let Ok(api_url) = env::var(ENV_BLINK_API_URL) {
            self.api_url = api_url;
        }

        if let Ok(units_str) = env::var(ENV_BLINK_SUPPORTED_UNITS) {
            if let Ok(units) = units_str
                .split(',')
                .map(|s| s.trim().parse())
                .collect::<Result<Vec<CurrencyUnit>, _>>()
            {
                self.supported_units = units;
            }
        }

        if let Ok(fee_str) = env::var(ENV_BLINK_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_BLINK_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}
