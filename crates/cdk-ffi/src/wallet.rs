//! Portable wallet object construction and configuration.

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::{RateLimitConfig, Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};

use crate::error::FfiError;
use crate::types::CurrencyUnit;

/// Portable application wallet for one mint and currency unit.
///
/// Open it with [`Wallet::open`] and use the request, session, and plan objects
/// from [`crate::portable`]. Protocol-level operations are available directly
/// from [`cdk::Wallet`].
#[derive(uniffi::Object)]
pub struct Wallet {
    inner: Arc<CdkWallet>,
}

impl Wallet {
    /// Wrap an existing engine wallet for internal facade use.
    pub(crate) fn from_inner(inner: Arc<CdkWallet>) -> Self {
        Self { inner }
    }

    /// Access the protocol engine behind this facade.
    pub(crate) fn inner(&self) -> &Arc<CdkWallet> {
        &self.inner
    }

    /// Construct a facade wallet without contacting its mint.
    pub(crate) fn new_advanced(
        mint_url: String,
        unit: CurrencyUnit,
        mnemonic: String,
        store: crate::database::WalletStore,
        config: WalletConfig,
    ) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(store)?;
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let mnemonic = Mnemonic::parse(&mnemonic)
            .map_err(|error| FfiError::invalid_input(format!("Invalid mnemonic: {error}")))?;
        let seed = mnemonic.to_seed_normalized("");

        let requested = config
            .rate_limit
            .as_ref()
            .map(RateLimit::to_config)
            .transpose()?;
        let pace_with = requested.map(|rate_limit| rate_limit.unwrap_or_default());
        let start_disabled = matches!(requested, Some(None));

        let mut builder = CdkWalletBuilder::new()
            .mint_url(mint_url.parse().map_err(|error: cdk::mint_url::Error| {
                FfiError::invalid_input(format!("Invalid URL: {error}"))
            })?)
            .unit(unit.into())
            .localstore(localstore)
            .seed(seed)
            .target_proof_count(config.target_proof_count.unwrap_or(3) as usize);

        if let Some(rate_limit) = pace_with {
            builder = builder.with_rate_limiting_config(rate_limit);
        }

        let wallet = builder.build().map_err(FfiError::from)?;
        if start_disabled {
            wallet.disable_rate_limiting();
        }

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }
}

/// Client-side request pacing selected when a wallet is opened.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum RateLimit {
    /// Built-in default pacing (capacity 20, refill 20/min).
    Default,
    /// No client-side pacing.
    Disabled,
    /// Custom burst capacity and per-minute refill. Both must be non-zero.
    Custom {
        /// Maximum requests allowed back-to-back before pacing starts.
        capacity: u32,
        /// Requests replenished per minute.
        refill_per_minute: u32,
    },
}

impl RateLimit {
    /// Translate into the engine config, where `None` disables pacing.
    pub(crate) fn to_config(&self) -> Result<Option<RateLimitConfig>, FfiError> {
        match self {
            Self::Default => Ok(Some(RateLimitConfig::default())),
            Self::Disabled => Ok(None),
            Self::Custom {
                capacity,
                refill_per_minute,
            } => RateLimitConfig::try_new(*capacity, *refill_per_minute)
                .map(Some)
                .ok_or_else(|| {
                    FfiError::invalid_input(
                        "rate limit capacity and refill_per_minute must be non-zero",
                    )
                }),
        }
    }
}

/// Optional operational tuning applied when a wallet is opened.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct WalletConfig {
    /// Preferred number of proofs retained by the wallet.
    #[uniffi(default = None)]
    pub target_proof_count: Option<u32>,
    /// Request pacing. Omit to use the built-in default.
    #[uniffi(default = None)]
    pub rate_limit: Option<RateLimit>,
}

/// Generate a new random BIP-39 mnemonic phrase.
#[uniffi::export]
pub fn generate_mnemonic() -> Result<String, FfiError> {
    let mnemonic = Mnemonic::generate(12)
        .map_err(|error| FfiError::internal(format!("Failed to generate mnemonic: {error}")))?;
    Ok(mnemonic.to_string())
}

/// Convert a BIP-39 mnemonic phrase to its entropy bytes.
#[uniffi::export]
pub fn mnemonic_to_entropy(mnemonic: String) -> Result<Vec<u8>, FfiError> {
    let mnemonic = Mnemonic::parse(&mnemonic)
        .map_err(|error| FfiError::invalid_input(format!("Invalid mnemonic: {error}")))?;
    Ok(mnemonic.to_entropy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_conversion_validates_every_variant() {
        assert_eq!(
            RateLimit::Default.to_config().expect("default is valid"),
            Some(RateLimitConfig::default())
        );
        assert_eq!(
            RateLimit::Disabled.to_config().expect("disabled is valid"),
            None
        );
        assert_eq!(
            RateLimit::Custom {
                capacity: 5,
                refill_per_minute: 30,
            }
            .to_config()
            .expect("non-zero custom values are valid"),
            RateLimitConfig::try_new(5, 30)
        );
        assert!(RateLimit::Custom {
            capacity: 0,
            refill_per_minute: 30,
        }
        .to_config()
        .is_err());
        assert!(RateLimit::Custom {
            capacity: 5,
            refill_per_minute: 0,
        }
        .to_config()
        .is_err());
    }
}
