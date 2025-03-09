//! LNBits environment variables

use std::env;

use crate::config::LNbits;

// LNBits environment variables
pub const ENV_LNBITS_ADMIN_API_KEY: &str = "CDK_MINTD_LNBITS_ADMIN_API_KEY";
pub const ENV_LNBITS_INVOICE_API_KEY: &str = "CDK_MINTD_LNBITS_INVOICE_API_KEY";
pub const ENV_LNBITS_API: &str = "CDK_MINTD_LNBITS_API";
pub const ENV_LNBITS_FEE_PERCENT: &str = "CDK_MINTD_LNBITS_FEE_PERCENT";
pub const ENV_LNBITS_RESERVE_FEE_MIN: &str = "CDK_MINTD_LNBITS_RESERVE_FEE_MIN";

impl LNbits {
    pub fn from_env(mut self) -> Self {
        if let Ok(admin_key) = env::var(ENV_LNBITS_ADMIN_API_KEY) {
            self.admin_api_key = admin_key;
        }

        if let Ok(invoice_key) = env::var(ENV_LNBITS_INVOICE_API_KEY) {
            self.invoice_api_key = invoice_key;
        }

        if let Ok(api) = env::var(ENV_LNBITS_API) {
            self.lnbits_api = api;
        }

        if let Ok(fee_str) = env::var(ENV_LNBITS_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_LNBITS_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}
