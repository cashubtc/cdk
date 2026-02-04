//! Transaction limits environment variables

use std::env;

use crate::config::Limits;

pub const ENV_MAX_INPUTS: &str = "CDK_MINTD_MAX_INPUTS";
pub const ENV_MAX_OUTPUTS: &str = "CDK_MINTD_MAX_OUTPUTS";

impl Limits {
    /// Override limits with environment variables if set
    pub fn from_env(&self) -> Self {
        let mut limits = self.clone();

        if let Ok(max_inputs_str) = env::var(ENV_MAX_INPUTS) {
            if let Ok(max_inputs) = max_inputs_str.parse::<usize>() {
                limits.max_inputs = max_inputs;
            }
        }

        if let Ok(max_outputs_str) = env::var(ENV_MAX_OUTPUTS) {
            if let Ok(max_outputs) = max_outputs_str.parse::<usize>() {
                limits.max_outputs = max_outputs;
            }
        }

        limits
    }
}
