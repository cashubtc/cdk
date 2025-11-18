//! CDK FFI Bindings
//!
//! UniFFI bindings for the CDK Wallet and related types.

#![warn(clippy::unused_async)]

pub mod database;
pub mod error;
pub mod multi_mint_wallet;
pub mod postgres;
pub mod sqlite;
pub mod token;
pub mod types;
pub mod wallet;

pub use database::*;
pub use error::*;
pub use multi_mint_wallet::*;
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
        use cdk::nuts::CurrencyUnit as CdkCurrencyUnit;

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

        let conditions = SpendingConditions::P2PK {
            pubkey: "02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc"
                .to_string(),
            conditions: None,
        };

        let options = SendOptions {
            memo: Some(memo),
            conditions: Some(conditions),
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
    fn test_wallet_config() {
        let config = WalletConfig {
            target_proof_count: None,
        };
        assert!(config.target_proof_count.is_none());

        let config_with_values = WalletConfig {
            target_proof_count: Some(5),
        };
        assert_eq!(config_with_values.target_proof_count, Some(5));
    }

    #[test]
    fn test_mnemonic_generation() {
        // Test mnemonic generation
        let mnemonic = generate_mnemonic().unwrap();
        assert!(!mnemonic.is_empty());
        assert_eq!(mnemonic.split_whitespace().count(), 12);

        // Verify it's a valid mnemonic by trying to parse it
        use bip39::Mnemonic;
        let parsed = Mnemonic::parse(&mnemonic);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_mnemonic_validation() {
        // Test with valid mnemonic
        let mnemonic = generate_mnemonic().unwrap();
        use bip39::Mnemonic;
        let parsed = Mnemonic::parse(&mnemonic);
        assert!(parsed.is_ok());

        // Test with invalid mnemonic
        let invalid_mnemonic = "invalid mnemonic phrase that should not work";
        let parsed_invalid = Mnemonic::parse(invalid_mnemonic);
        assert!(parsed_invalid.is_err());

        // Test mnemonic word count variations
        let mnemonic_12 = generate_mnemonic().unwrap();
        assert_eq!(mnemonic_12.split_whitespace().count(), 12);
    }

    #[test]
    fn test_mnemonic_to_entropy() {
        // Test with generated mnemonic
        let mnemonic = generate_mnemonic().unwrap();
        let entropy = mnemonic_to_entropy(mnemonic.clone()).unwrap();

        // For a 12-word mnemonic, entropy should be 16 bytes (128 bits)
        assert_eq!(entropy.len(), 16);

        // Test that we can recreate the mnemonic from entropy
        use bip39::Mnemonic;
        let recreated_mnemonic = Mnemonic::from_entropy(&entropy).unwrap();
        assert_eq!(recreated_mnemonic.to_string(), mnemonic);

        // Test with invalid mnemonic
        let invalid_result = mnemonic_to_entropy("invalid mnemonic".to_string());
        assert!(invalid_result.is_err());
    }
}
