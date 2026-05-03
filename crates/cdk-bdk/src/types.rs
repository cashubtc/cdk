use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for fee estimation
#[derive(Debug, Clone)]
pub struct FeeEstimationConfig {
    /// Used when chain-source estimation fails (sat/vB)
    pub fallback_sat_per_vb: f64,
    /// How long a per-tier estimate is cached in seconds
    pub cache_ttl_secs: u64,
}

impl Default for FeeEstimationConfig {
    fn default() -> Self {
        Self {
            fallback_sat_per_vb: 2.0,
            cache_ttl_secs: 60,
        }
    }
}

/// Configuration for the background batch processor
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// How often the batch processor wakes up to check for ready intents
    pub poll_interval: Duration,
    /// Maximum number of intents to include in a single batch
    pub max_batch_size: usize,
    /// How long standard-tier intents wait before being eligible
    pub standard_deadline: Duration,
    /// How long economy-tier intents wait before being eligible
    pub economy_deadline: Duration,
    /// Minimum number of pending intents required before creating a
    /// non-immediate batch. Immediate tier bypasses this threshold.
    /// Expired tier deadlines also override this threshold.
    pub min_batch_threshold: usize,
    /// Maximum age for a pending intent before it is expired and removed.
    /// Set to `None` to disable automatic expiry (default: 24 hours).
    pub max_intent_age: Option<Duration>,
    /// Fee estimation configuration
    pub fee_estimation: FeeEstimationConfig,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            max_batch_size: 50,
            standard_deadline: Duration::from_secs(300),
            economy_deadline: Duration::from_secs(3600),
            min_batch_threshold: 1,
            max_intent_age: Some(Duration::from_secs(24 * 60 * 60)),
            fee_estimation: FeeEstimationConfig::default(),
        }
    }
}

/// Configuration for blockchain synchronization
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Number of blocks to apply per wallet-lock acquisition (RPC path)
    pub apply_chunk_size: usize,
    /// Warn if a single lock acquisition exceeds this duration (milliseconds)
    pub lock_hold_warn_ms: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            apply_chunk_size: 16,
            lock_hold_warn_ms: 500,
        }
    }
}

/// Batching tier for on-chain send intents
///
/// Controls when a send intent is eligible for inclusion in a batch.
/// `Immediate` intents are processed right away; `Standard` and `Economy`
/// intents wait until their respective deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PaymentTier {
    /// Process immediately without waiting for other intents
    #[default]
    Immediate,
    /// Process when the standard deadline is reached or an immediate batch
    /// is available
    Standard,
    /// Process when the economy deadline is reached or an immediate batch
    /// is available
    Economy,
}

impl PaymentTier {
    /// Parse a tier from an optional string value.
    ///
    /// Returns `Immediate` when `None` is provided or the string is
    /// unrecognized.
    pub fn from_optional_str(s: Option<&str>) -> Self {
        let Some(value) = s else {
            return Self::default();
        };

        if value.eq_ignore_ascii_case("immediate") {
            Self::Immediate
        } else if value.eq_ignore_ascii_case("standard") {
            Self::Standard
        } else if value.eq_ignore_ascii_case("economy") {
            Self::Economy
        } else {
            Self::default()
        }
    }
}

/// Opaque key-value metadata attached to a send intent
///
/// Stored for future extensions. In v1 no behavior is driven by metadata
/// values. Future features like payjoin may consume this metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PaymentMetadata {
    /// Key-value pairs
    pub entries: std::collections::HashMap<String, String>,
}

impl PaymentMetadata {
    /// Create metadata from an optional JSON string.
    ///
    /// Accepts either a bare `{"key": "value"}` object (interpreted as the
    /// entries map) or the full struct form `{"entries": {"key": "value"}}`.
    pub fn from_optional_json(json: Option<&str>) -> Self {
        let Some(s) = json else {
            return Self::default();
        };
        // Try deserializing as full struct first
        if let Ok(meta) = serde_json::from_str::<PaymentMetadata>(s) {
            return meta;
        }
        // Fall back to interpreting the JSON as a bare key-value map
        if let Ok(entries) = serde_json::from_str::<std::collections::HashMap<String, String>>(s) {
            return Self { entries };
        }
        Self::default()
    }
}
