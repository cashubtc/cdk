//! Resource limit environment variables

use std::env;

use crate::config::Limits;

pub const ENV_MAX_INPUTS: &str = "CDK_MINTD_MAX_INPUTS";
pub const ENV_MAX_OUTPUTS: &str = "CDK_MINTD_MAX_OUTPUTS";
pub const ENV_WS_MAX_CONNECTIONS: &str = "CDK_MINTD_WS_MAX_CONNECTIONS";
pub const ENV_WS_MAX_CONNECTIONS_PER_IP: &str = "CDK_MINTD_WS_MAX_CONNECTIONS_PER_IP";
pub const ENV_WS_MAX_SUBSCRIPTIONS_PER_CONNECTION: &str =
    "CDK_MINTD_WS_MAX_SUBSCRIPTIONS_PER_CONNECTION";
pub const ENV_WS_MAX_FILTERS_PER_SUBSCRIPTION: &str = "CDK_MINTD_WS_MAX_FILTERS_PER_SUBSCRIPTION";
pub const ENV_WS_MAX_TOPICS_PER_CONNECTION: &str = "CDK_MINTD_WS_MAX_TOPICS_PER_CONNECTION";
pub const ENV_WS_MAX_REQUEST_UNITS_PER_SECOND: &str = "CDK_MINTD_WS_MAX_REQUEST_UNITS_PER_SECOND";
pub const ENV_WS_MAX_REQUEST_BURST_UNITS: &str = "CDK_MINTD_WS_MAX_REQUEST_BURST_UNITS";
pub const ENV_WS_MAX_THROTTLED_REQUESTS: &str = "CDK_MINTD_WS_MAX_THROTTLED_REQUESTS";
pub const ENV_WS_MAX_MESSAGE_BYTES: &str = "CDK_MINTD_WS_MAX_MESSAGE_BYTES";
pub const ENV_WS_IDLE_TIMEOUT_SECS: &str = "CDK_MINTD_WS_IDLE_TIMEOUT_SECS";
pub const ENV_WS_PING_INTERVAL_SECS: &str = "CDK_MINTD_WS_PING_INTERVAL_SECS";
pub const ENV_PUBSUB_MAX_TOPICS: &str = "CDK_MINTD_PUBSUB_MAX_TOPICS";
pub const ENV_PUBSUB_MAX_CONCURRENT_BACKFILLS: &str = "CDK_MINTD_PUBSUB_MAX_CONCURRENT_BACKFILLS";
pub const ENV_PUBSUB_MAX_QUOTE_CHECKS_PER_BACKFILL: &str =
    "CDK_MINTD_PUBSUB_MAX_QUOTE_CHECKS_PER_BACKFILL";

fn override_from_env<T: std::str::FromStr>(key: &str, target: &mut T) {
    if let Ok(raw) = env::var(key) {
        match raw.parse::<T>() {
            Ok(value) => *target = value,
            Err(_) => tracing::warn!("Ignoring {key}: {raw:?} is not a valid value"),
        }
    }
}

impl Limits {
    /// Override limits with environment variables if set
    pub fn from_env(&self) -> Self {
        let mut limits = self.clone();

        override_from_env(ENV_MAX_INPUTS, &mut limits.max_inputs);
        override_from_env(ENV_MAX_OUTPUTS, &mut limits.max_outputs);
        override_from_env(ENV_WS_MAX_CONNECTIONS, &mut limits.ws_max_connections);
        override_from_env(
            ENV_WS_MAX_CONNECTIONS_PER_IP,
            &mut limits.ws_max_connections_per_ip,
        );
        override_from_env(
            ENV_WS_MAX_SUBSCRIPTIONS_PER_CONNECTION,
            &mut limits.ws_max_subscriptions_per_connection,
        );
        override_from_env(
            ENV_WS_MAX_FILTERS_PER_SUBSCRIPTION,
            &mut limits.ws_max_filters_per_subscription,
        );
        override_from_env(
            ENV_WS_MAX_TOPICS_PER_CONNECTION,
            &mut limits.ws_max_topics_per_connection,
        );
        override_from_env(
            ENV_WS_MAX_REQUEST_UNITS_PER_SECOND,
            &mut limits.ws_max_request_units_per_second,
        );
        override_from_env(
            ENV_WS_MAX_REQUEST_BURST_UNITS,
            &mut limits.ws_max_request_burst_units,
        );
        override_from_env(
            ENV_WS_MAX_THROTTLED_REQUESTS,
            &mut limits.ws_max_throttled_requests,
        );
        override_from_env(ENV_WS_MAX_MESSAGE_BYTES, &mut limits.ws_max_message_bytes);
        override_from_env(ENV_WS_IDLE_TIMEOUT_SECS, &mut limits.ws_idle_timeout_secs);
        override_from_env(ENV_WS_PING_INTERVAL_SECS, &mut limits.ws_ping_interval_secs);
        override_from_env(ENV_PUBSUB_MAX_TOPICS, &mut limits.pubsub_max_topics);
        override_from_env(
            ENV_PUBSUB_MAX_CONCURRENT_BACKFILLS,
            &mut limits.pubsub_max_concurrent_backfills,
        );
        override_from_env(
            ENV_PUBSUB_MAX_QUOTE_CHECKS_PER_BACKFILL,
            &mut limits.pubsub_max_quote_checks_per_backfill,
        );

        limits
    }
}
