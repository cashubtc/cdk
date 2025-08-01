//! CDK FFI Bindings
//!
//! UniFFI bindings for the CDK Wallet and related types.

pub mod error;
pub mod types;
pub mod wallet;

pub use error::*;
pub use types::*;
pub use wallet::*;

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
        assert!(options.memo.is_none());
        assert!(options.conditions.is_none());
        assert!(matches!(options.amount_split_target, SplitTarget::None));
        assert!(matches!(options.send_kind, SendKind::OnlineExact));
        assert!(!options.include_fee);
        assert!(options.max_proofs.is_none());
        assert!(options.metadata.is_empty());
    }

    #[test]
    fn test_receive_options_default() {
        let options = ReceiveOptions::default();
        assert!(matches!(options.amount_split_target, SplitTarget::None));
        assert!(options.p2pk_signing_keys.is_empty());
        assert!(options.preimages.is_empty());
        assert!(options.metadata.is_empty());
    }

    #[test]
    fn test_send_memo() {
        let memo_text = "Test memo".to_string();
        let memo = SendMemo {
            memo: memo_text.clone(),
            include_memo: true,
        };

        assert_eq!(memo.memo, memo_text);
        assert!(memo.include_memo);
    }

    #[test]
    fn test_split_target_variants() {
        let split_none = SplitTarget::None;
        assert!(matches!(split_none, SplitTarget::None));

        let amount = Amount::new(1000);
        let split_value = SplitTarget::Value { amount };
        assert!(matches!(split_value, SplitTarget::Value { .. }));

        let amounts = vec![Amount::new(100), Amount::new(200)];
        let split_values = SplitTarget::Values { amounts };
        assert!(matches!(split_values, SplitTarget::Values { .. }));
    }

    #[test]
    fn test_send_kind_variants() {
        let online_exact = SendKind::OnlineExact;
        assert!(matches!(online_exact, SendKind::OnlineExact));

        let tolerance = Amount::new(50);
        let online_tolerance = SendKind::OnlineTolerance { tolerance };
        assert!(matches!(online_tolerance, SendKind::OnlineTolerance { .. }));

        let offline_exact = SendKind::OfflineExact;
        assert!(matches!(offline_exact, SendKind::OfflineExact));

        let offline_tolerance = SendKind::OfflineTolerance { tolerance };
        assert!(matches!(
            offline_tolerance,
            SendKind::OfflineTolerance { .. }
        ));
    }

    #[test]
    fn test_secret_key_from_hex() {
        // Test valid hex string (64 characters)
        let valid_hex = "a".repeat(64);
        let secret_key = SecretKey::from_hex(valid_hex.clone());
        assert!(secret_key.is_ok());
        assert_eq!(secret_key.unwrap().hex, valid_hex);

        // Test invalid length
        let invalid_length = "a".repeat(32); // 32 chars instead of 64
        let secret_key = SecretKey::from_hex(invalid_length);
        assert!(secret_key.is_err());

        // Test invalid characters
        let invalid_chars = "g".repeat(64); // 'g' is not a valid hex character
        let secret_key = SecretKey::from_hex(invalid_chars);
        assert!(secret_key.is_err());
    }

    #[test]
    fn test_secret_key_random() {
        let key1 = SecretKey::random();
        let key2 = SecretKey::random();

        // Keys should be different
        assert_ne!(key1.hex, key2.hex);

        // Keys should be valid hex (64 characters)
        assert_eq!(key1.hex.len(), 64);
        assert_eq!(key2.hex.len(), 64);
        assert!(key1.hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(key2.hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_send_options_with_all_fields() {
        use std::collections::HashMap;

        let memo = SendMemo {
            memo: "Test memo".to_string(),
            include_memo: true,
        };

        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), "value1".to_string());

        let options = SendOptions {
            memo: Some(memo),
            conditions: Some("{\"test\": \"condition\"}".to_string()),
            amount_split_target: SplitTarget::Value {
                amount: Amount::new(1000),
            },
            send_kind: SendKind::OnlineTolerance {
                tolerance: Amount::new(50),
            },
            include_fee: true,
            max_proofs: Some(10),
            metadata,
        };

        assert!(options.memo.is_some());
        assert!(options.conditions.is_some());
        assert!(matches!(
            options.amount_split_target,
            SplitTarget::Value { .. }
        ));
        assert!(matches!(
            options.send_kind,
            SendKind::OnlineTolerance { .. }
        ));
        assert!(options.include_fee);
        assert_eq!(options.max_proofs, Some(10));
        assert!(!options.metadata.is_empty());
    }

    #[test]
    fn test_receive_options_with_all_fields() {
        use std::collections::HashMap;

        let secret_key = SecretKey::random();
        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), "value1".to_string());

        let options = ReceiveOptions {
            amount_split_target: SplitTarget::Values {
                amounts: vec![Amount::new(100), Amount::new(200)],
            },
            p2pk_signing_keys: vec![secret_key],
            preimages: vec!["preimage1".to_string(), "preimage2".to_string()],
            metadata,
        };

        assert!(matches!(
            options.amount_split_target,
            SplitTarget::Values { .. }
        ));
        assert_eq!(options.p2pk_signing_keys.len(), 1);
        assert_eq!(options.preimages.len(), 2);
        assert!(!options.metadata.is_empty());
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
