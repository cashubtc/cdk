//! LND environment variables

use std::env;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::config::Lnd;

// LND environment variables
pub const ENV_LND_ADDRESS: &str = "CDK_MINTD_LND_ADDRESS";
pub const ENV_LND_CERT_FILE: &str = "CDK_MINTD_LND_CERT_FILE";
pub const ENV_LND_MACAROON_FILE: &str = "CDK_MINTD_LND_MACAROON_FILE";
pub const ENV_LND_FEE_PERCENT: &str = "CDK_MINTD_LND_FEE_PERCENT";
pub const ENV_LND_RESERVE_FEE_MIN: &str = "CDK_MINTD_LND_RESERVE_FEE_MIN";

impl Lnd {
    pub fn from_env(mut self) -> Result<Self> {
        if let Ok(address) = env::var(ENV_LND_ADDRESS) {
            self.address = address;
        }

        if let Ok(cert_path) = env::var(ENV_LND_CERT_FILE) {
            self.cert_file = PathBuf::from(cert_path);
        }

        if let Ok(macaroon_path) = env::var(ENV_LND_MACAROON_FILE) {
            self.macaroon_file = PathBuf::from(macaroon_path);
        }

        if let Ok(fee_str) = env::var(ENV_LND_FEE_PERCENT) {
            let fee = fee_str.parse::<f32>().with_context(|| {
                format!("{ENV_LND_FEE_PERCENT} must be a floating-point number")
            })?;
            if !fee.is_finite() || !(0.0..=1.0).contains(&fee) {
                bail!(
                    "{ENV_LND_FEE_PERCENT} must be finite and between 0.0 and 1.0 inclusive (0.02 means 2%)"
                );
            }
            self.fee_percent = fee;
        }

        if let Ok(reserve_fee_str) = env::var(ENV_LND_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        Ok(self)
    }
}
