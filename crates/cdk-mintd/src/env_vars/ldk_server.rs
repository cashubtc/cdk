//! LDK Server environment variables

use std::env;
use std::path::PathBuf;

use crate::config::LdkServer;

/// LDK Server address environment variable.
pub const LDK_SERVER_ADDRESS_ENV_VAR: &str = "CDK_MINTD_LDK_SERVER_ADDRESS";
/// LDK Server API key environment variable.
pub const LDK_SERVER_API_KEY_ENV_VAR: &str = "CDK_MINTD_LDK_SERVER_API_KEY";
/// LDK Server TLS certificate path environment variable.
pub const LDK_SERVER_CERT_PATH_ENV_VAR: &str = "CDK_MINTD_LDK_SERVER_CERT_PATH";
/// LDK Server fee percentage environment variable.
pub const LDK_SERVER_FEE_PERCENT_ENV_VAR: &str = "CDK_MINTD_LDK_SERVER_FEE_PERCENT";
/// LDK Server minimum reserve fee environment variable.
pub const LDK_SERVER_RESERVE_FEE_MIN_ENV_VAR: &str = "CDK_MINTD_LDK_SERVER_RESERVE_FEE_MIN";
/// LDK Server max payment scan pages environment variable.
pub const LDK_SERVER_MAX_PAYMENT_SCAN_PAGES_ENV_VAR: &str =
    "CDK_MINTD_LDK_SERVER_MAX_PAYMENT_SCAN_PAGES";

impl LdkServer {
    /// Load LDK Server configuration from environment variables.
    pub fn from_env(mut self) -> Self {
        if let Ok(address) = env::var(LDK_SERVER_ADDRESS_ENV_VAR) {
            self.address = address;
        }

        if let Ok(api_key) = env::var(LDK_SERVER_API_KEY_ENV_VAR) {
            self.api_key = api_key;
        }

        if let Ok(cert_path) = env::var(LDK_SERVER_CERT_PATH_ENV_VAR) {
            self.cert_path = PathBuf::from(cert_path);
        }

        if let Ok(fee_percent) = env::var(LDK_SERVER_FEE_PERCENT_ENV_VAR) {
            if let Ok(fee_percent) = fee_percent.parse::<f32>() {
                self.fee_percent = fee_percent;
            }
        }

        if let Ok(reserve_fee_min) = env::var(LDK_SERVER_RESERVE_FEE_MIN_ENV_VAR) {
            if let Ok(reserve_fee_min) = reserve_fee_min.parse::<u64>() {
                self.reserve_fee_min = reserve_fee_min.into();
            }
        }

        if let Ok(max_payment_scan_pages) = env::var(LDK_SERVER_MAX_PAYMENT_SCAN_PAGES_ENV_VAR) {
            if let Ok(max_payment_scan_pages) = max_payment_scan_pages.parse::<u16>() {
                self.max_payment_scan_pages = max_payment_scan_pages;
            }
        }

        self
    }
}
