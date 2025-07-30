//! CDK FFI Bindings
//!
//! UniFFI bindings for the CDK Wallet and related types.

pub mod error;
pub mod types;
pub mod wallet;

pub use error::*;
pub use types::*;
pub use wallet::*;

// Re-export the main types from CDK for convenience
pub use cdk::nuts::CurrencyUnit as CdkCurrencyUnit;
pub use cdk::{Amount as CdkAmount, Error as CdkError};

uniffi::setup_scaffolding!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amount_conversion() {
        let amount = Amount::new(1000);
        assert_eq!(amount.value, 1000);
        assert!(!amount.is_zero());

        let zero = Amount::zero();
        assert!(zero.is_zero());
    }

    #[test]
    fn test_currency_unit_conversion() {
        let unit = CurrencyUnit::Sat;
        let cdk_unit: CdkCurrencyUnit = unit.into();
        let back: CurrencyUnit = cdk_unit.into();
        assert_eq!(back, CurrencyUnit::Sat);
    }

    #[test]
    fn test_mint_url_creation() {
        let url = MintUrl::new("https://mint.example.com".to_string());
        assert!(url.is_ok());

        let invalid_url = MintUrl::new("not-a-url".to_string());
        assert!(invalid_url.is_err());
    }

    #[test]
    fn test_send_options_default() {
        let options = SendOptions::default();
        assert!(!options.offline);
    }

    #[test]
    fn test_receive_options_default() {
        let options = ReceiveOptions::default();
        assert!(options.check_spendable);
    }

    #[test]
    fn test_generate_seed() {
        let seed = generate_seed();
        assert_eq!(seed.len(), 32);

        // Generate another seed and ensure they're different
        let seed2 = generate_seed();
        assert_ne!(seed, seed2);
    }

    #[test]
    fn test_wallet_creation() {
        let seed = generate_seed();

        // This will likely fail without a proper runtime context, but we can test
        // that the function doesn't panic and returns a proper error
        let result = std::panic::catch_unwind(|| {
            Wallet::new(
                "https://mint.example.com".to_string(),
                CurrencyUnit::Sat,
                seed,
                Some(3),
            )
        });

        // We expect this to either succeed or fail gracefully (not panic)
        assert!(result.is_ok(), "Wallet constructor should not panic");
    }
}
