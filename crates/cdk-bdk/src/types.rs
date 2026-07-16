use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default average Bitcoin block interval used for delayed batch deadlines.
pub const DEFAULT_TARGET_BLOCK_TIME_SECS: u64 = 600;

/// Default Payjoin v2 session expiry in seconds.
pub const DEFAULT_PAYJOIN_EXPIRY_SECS: u64 = 86_400;

/// Payjoin v2 directory/OHTTP configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayjoinConfig {
    /// Payjoin directory URL.
    pub directory_url: String,
    /// OHTTP relay URL.
    pub ohttp_relay_url: String,
    /// Receiver session expiry in seconds.
    pub expiry_secs: u64,
    /// DER-encoded localhost TLS certificate for regtest-only Payjoin services.
    #[cfg(feature = "payjoin-local-https")]
    pub local_tls_cert_der: Option<Vec<u8>>,
}

impl PayjoinConfig {
    /// Create and validate a Payjoin config.
    pub fn new(
        directory_url: String,
        ohttp_relay_url: String,
        expiry_secs: Option<u64>,
    ) -> Result<Self, String> {
        let expiry_secs = expiry_secs.unwrap_or(DEFAULT_PAYJOIN_EXPIRY_SECS);
        if expiry_secs == 0 {
            return Err("payjoin_expiry_secs must be greater than zero".to_string());
        }

        validate_http_url("payjoin_directory_url", &directory_url)?;
        validate_http_url("payjoin_ohttp_relay_url", &ohttp_relay_url)?;

        Ok(Self {
            directory_url,
            ohttp_relay_url,
            expiry_secs,
            #[cfg(feature = "payjoin-local-https")]
            local_tls_cert_der: None,
        })
    }

    /// Configure a DER-encoded localhost TLS certificate for regtest-only Payjoin services.
    #[cfg(feature = "payjoin-local-https")]
    pub fn with_local_tls_cert_der(mut self, cert_der: Vec<u8>) -> Self {
        self.local_tls_cert_der = Some(cert_der);
        self
    }
}

fn validate_http_url(field: &str, value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|err| format!("{field} is not a valid URL: {err}"))?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(format!("{field} must use http or https, got {scheme}")),
    }
}

/// Configuration for BDK fee estimation.
///
/// Fee rates are cached per payment tier. Melt quote fees use a conservative
/// weight estimate because the quote is created before BDK performs final coin
/// selection. These knobs expose the operator-facing tradeoffs: fallback fee
/// rate, maximum quote size, and quote safety padding. Internal constants cover
/// the lower-level wallet sampling and input weight assumptions.
#[derive(Debug, Clone)]
pub struct FeeEstimationConfig {
    /// Fee rate used when chain-source estimation fails, in sat/vB.
    pub fallback_sat_per_vb: f64,
    /// How long a per-tier fee-rate estimate is cached, in seconds.
    pub cache_ttl_secs: u64,
    /// Maximum input count reserved at quote time.
    pub quote_max_input_count: usize,
    /// Fixed safety margin added to quote-time fee estimates, in sats.
    pub quote_fixed_safety_sat: u64,
    /// Multiplicative safety margin applied after the raw quote fee estimate.
    pub quote_safety_multiplier: f64,
}

impl Default for FeeEstimationConfig {
    fn default() -> Self {
        Self {
            fallback_sat_per_vb: 2.0,
            cache_ttl_secs: 60,
            quote_max_input_count: 24,
            quote_fixed_safety_sat: 500,
            quote_safety_multiplier: 1.25,
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
    /// Average block interval used to derive default delayed tier deadlines.
    pub target_block_time: Duration,
    /// How long standard-tier intents wait before being eligible
    pub standard_deadline: Duration,
    /// How long economy-tier intents wait before being eligible
    pub economy_deadline: Duration,
    /// Maximum age for a pending intent before it is expired and removed.
    /// Set to `None` to disable automatic expiry.
    pub max_intent_age: Option<Duration>,
    /// Fee tiers exposed in melt quotes. The configured order defines the
    /// backend-owned `fee_index` values.
    pub fee_options: Vec<PaymentTier>,
    /// Fee estimation configuration
    pub fee_estimation: FeeEstimationConfig,
}

impl Default for BatchConfig {
    fn default() -> Self {
        let poll_interval = Duration::from_secs(30);
        let target_block_time = Duration::from_secs(DEFAULT_TARGET_BLOCK_TIME_SECS);
        let standard_deadline =
            Self::deadline_for_target_blocks(PaymentTier::Standard, target_block_time);
        let economy_deadline =
            Self::deadline_for_target_blocks(PaymentTier::Economy, target_block_time);

        Self {
            poll_interval,
            max_batch_size: 50,
            target_block_time,
            standard_deadline,
            economy_deadline,
            max_intent_age: Some(economy_deadline.saturating_add(poll_interval)),
            fee_options: vec![PaymentTier::Immediate],
            fee_estimation: FeeEstimationConfig::default(),
        }
    }
}

impl BatchConfig {
    /// Derive a delayed tier deadline from its advertised confirmation target.
    pub fn deadline_for_target_blocks(tier: PaymentTier, target_block_time: Duration) -> Duration {
        Duration::from_secs(
            target_block_time
                .as_secs()
                .saturating_mul(u64::from(tier.estimated_blocks())),
        )
    }

    /// Validate operator-selected fee tiers.
    pub fn validate(&self) -> Result<(), String> {
        if self.target_block_time.is_zero() {
            return Err("BDK batch_config.target_block_time must be greater than zero".to_string());
        }

        if !self.fee_estimation.fallback_sat_per_vb.is_finite()
            || self.fee_estimation.fallback_sat_per_vb <= 0.0
            || self.fee_estimation.fallback_sat_per_vb.ceil() > f64::from(u32::MAX)
        {
            return Err(
                "BDK batch_config.fee_estimation.fallback_sat_per_vb must be finite, greater than zero, and fit in u32 after rounding"
                    .to_string(),
            );
        }

        validate_fee_options(&self.fee_options)
    }

    /// Resolve a wallet-selected fee index to the configured tier.
    pub fn tier_for_fee_index(&self, fee_index: Option<u32>) -> Result<PaymentTier, u32> {
        let Some(fee_index) = fee_index else {
            return Ok(PaymentTier::Immediate);
        };

        self.fee_options
            .get(fee_index as usize)
            .copied()
            .ok_or(fee_index)
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
    /// Parse a tier from a configuration name.
    pub fn from_config_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "immediate" => Some(Self::Immediate),
            "standard" => Some(Self::Standard),
            "economy" => Some(Self::Economy),
            _ => None,
        }
    }

    /// Stable configuration name for this tier.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Standard => "standard",
            Self::Economy => "economy",
        }
    }

    /// Target confirmation blocks advertised for this tier.
    pub fn estimated_blocks(self) -> u32 {
        match self {
            Self::Immediate => 1,
            Self::Standard => 6,
            Self::Economy => 144,
        }
    }

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

/// Validate an ordered list of exposed fee tiers.
pub fn validate_fee_options(fee_options: &[PaymentTier]) -> Result<(), String> {
    if fee_options.is_empty() {
        return Err("BDK batch_config.fee_options must not be empty".to_string());
    }

    if fee_options.len() > 3 {
        return Err("BDK batch_config.fee_options must contain at most 3 entries".to_string());
    }

    for (idx, tier) in fee_options.iter().enumerate() {
        if fee_options[..idx].contains(tier) {
            return Err(format!(
                "BDK batch_config.fee_options contains duplicate tier '{}'",
                tier.config_name()
            ));
        }
    }

    Ok(())
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

#[cfg(test)]
mod payjoin_tests {
    use super::*;

    #[test]
    fn payjoin_config_requires_positive_expiry() {
        let err = PayjoinConfig::new(
            "https://directory.example".to_string(),
            "https://relay.example".to_string(),
            Some(0),
        )
        .expect_err("zero expiry should fail");

        assert!(err.contains("payjoin_expiry_secs"));
    }

    #[test]
    fn payjoin_config_rejects_non_http_urls() {
        let err = PayjoinConfig::new(
            "ftp://directory.example".to_string(),
            "https://relay.example".to_string(),
            Some(DEFAULT_PAYJOIN_EXPIRY_SECS),
        )
        .expect_err("non-http directory URL should fail");

        assert!(err.contains("payjoin_directory_url"));
    }

    #[test]
    fn payjoin_config_defaults_expiry() {
        let config = PayjoinConfig::new(
            "https://directory.example".to_string(),
            "https://relay.example".to_string(),
            None,
        )
        .expect("valid config");

        assert_eq!(config.expiry_secs, DEFAULT_PAYJOIN_EXPIRY_SECS);
    }
}
