//! Compact state filter environment variables

use std::env;

use crate::config::StateFilters;

pub const ENV_STATE_FILTERS_ENABLED: &str = "CDK_MINTD_STATE_FILTERS_ENABLED";
pub const ENV_STATE_FILTERS_EPOCH: &str = "CDK_MINTD_STATE_FILTERS_EPOCH";
pub const ENV_STATE_FILTERS_P: &str = "CDK_MINTD_STATE_FILTERS_P";
pub const ENV_STATE_FILTERS_PAGE_SIZE: &str = "CDK_MINTD_STATE_FILTERS_PAGE_SIZE";
pub const ENV_STATE_FILTERS_PENDING: &str = "CDK_MINTD_STATE_FILTERS_PENDING";
pub const ENV_STATE_FILTERS_KINDS: &str = "CDK_MINTD_STATE_FILTERS_KINDS";

impl StateFilters {
    /// Override state filter settings with environment variables if set
    pub fn from_env(&self) -> Self {
        let mut filters = self.clone();

        if let Ok(enabled) = env::var(ENV_STATE_FILTERS_ENABLED) {
            if let Ok(enabled) = enabled.parse() {
                filters.enabled = enabled;
            }
        }

        if let Ok(epoch) = env::var(ENV_STATE_FILTERS_EPOCH) {
            if let Ok(epoch) = epoch.parse() {
                filters.epoch = epoch;
            }
        }

        if let Ok(p) = env::var(ENV_STATE_FILTERS_P) {
            if let Ok(p) = p.parse() {
                filters.p = p;
            }
        }

        if let Ok(page_size) = env::var(ENV_STATE_FILTERS_PAGE_SIZE) {
            if let Ok(page_size) = page_size.parse() {
                filters.page_size = page_size;
            }
        }

        if let Ok(pending) = env::var(ENV_STATE_FILTERS_PENDING) {
            if let Ok(pending) = pending.parse() {
                filters.pending = pending;
            }
        }

        if let Ok(kinds) = env::var(ENV_STATE_FILTERS_KINDS) {
            filters.kinds = Some(
                kinds
                    .split(',')
                    .map(|kind| kind.trim().to_string())
                    .filter(|kind| !kind.is_empty())
                    .collect(),
            );
        }

        filters
    }
}
