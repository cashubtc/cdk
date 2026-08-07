//! CDK FFI Bindings
//!
//! UniFFI bindings for the CDK Wallet and related types.

#![warn(clippy::unused_async)]
#![allow(missing_docs)]
#![allow(missing_debug_implementations)]

pub mod bip321;
pub mod database;
pub mod error;
pub mod logging;
#[cfg(feature = "npubcash")]
pub mod npubcash;
#[cfg(feature = "nwc")]
pub mod nwc;
#[cfg(feature = "postgres")]
pub mod postgres;
mod runtime;
pub mod sqlite;
#[cfg(feature = "supabase")]
pub mod supabase;
pub mod token;
pub mod types;
pub mod wallet;
pub mod wallet_repository;
mod wallet_trait;

pub use database::*;
pub use error::*;
pub use logging::*;
#[cfg(feature = "npubcash")]
pub use npubcash::*;
#[cfg(feature = "nwc")]
pub use nwc::*;
pub use types::*;
pub use wallet::*;
pub use wallet_repository::*;

uniffi::setup_scaffolding!();

#[cfg(test)]
mod tests {
    use std::convert::TryInto;

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
        assert!(options.p2pk_signing_keys.is_empty());
        assert_eq!(
            options.p2pk_locked_proof_send_mode,
            P2PKLockedProofSendMode::Swap
        );
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
            use_p2bk: false,
            p2pk_signing_keys: Vec::new(),
            p2pk_locked_proof_send_mode: P2PKLockedProofSendMode::Swap,
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
    fn test_receive_options_invalid_secret_key_returns_error() {
        let options = ReceiveOptions {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: vec![SecretKey {
                hex: "z".repeat(64),
            }],
            preimages: Vec::new(),
            metadata: Default::default(),
        };

        let result: Result<cdk::wallet::ReceiveOptions, _> = options.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_send_options_invalid_secret_key_returns_error() {
        let options = SendOptions {
            p2pk_signing_keys: vec![SecretKey {
                hex: "z".repeat(64),
            }],
            ..Default::default()
        };

        let result: Result<cdk::wallet::SendOptions, _> = options.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_send_options_invalid_conditions_returns_error() {
        let options = SendOptions {
            conditions: Some(SpendingConditions::P2PK {
                pubkey: "not_a_valid_pubkey".to_string(),
                conditions: None,
            }),
            ..Default::default()
        };

        let result: Result<cdk::wallet::SendOptions, _> = options.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_send_options_json_preserves_p2pk_signing_keys() {
        let secret_hex =
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".to_string();
        let options = SendOptions {
            p2pk_signing_keys: vec![SecretKey {
                hex: secret_hex.clone(),
            }],
            ..Default::default()
        };

        let to_json = options.to_json().unwrap();
        let encoded = crate::types::wallet::encode_send_options(options.clone()).unwrap();
        let debug = format!("{:?}", options);

        assert!(to_json.contains(&secret_hex));
        assert!(to_json.contains("p2pk_signing_keys"));
        assert!(encoded.contains(&secret_hex));
        assert!(encoded.contains("p2pk_signing_keys"));
        assert!(!debug.contains(&secret_hex));
        assert!(debug.contains("[redacted]"));

        let decoded = crate::types::wallet::decode_send_options(encoded).unwrap();

        assert_eq!(decoded.p2pk_signing_keys.len(), 1);
        assert_eq!(decoded.p2pk_signing_keys[0].hex, secret_hex);
    }

    #[test]
    fn test_send_options_json_still_decodes_p2pk_signing_keys() {
        let secret_hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let json = format!(
            r#"{{
                "memo": null,
                "conditions": null,
                "amount_split_target": "None",
                "send_kind": "OnlineExact",
                "include_fee": false,
                "use_p2bk": false,
                "max_proofs": null,
                "metadata": {{}},
                "p2pk_signing_keys": ["{}"],
                "p2pk_locked_proof_send_mode": "SignAndSend"
            }}"#,
            secret_hex
        );

        let options = crate::types::wallet::decode_send_options(json).unwrap();

        assert_eq!(options.p2pk_signing_keys.len(), 1);
        assert_eq!(options.p2pk_signing_keys[0].hex, secret_hex);
        assert_eq!(
            options.p2pk_locked_proof_send_mode,
            P2PKLockedProofSendMode::SignAndSend
        );
    }

    #[test]
    fn test_receive_options_json_preserves_p2pk_signing_keys() {
        let secret_hex =
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".to_string();
        let options = ReceiveOptions {
            p2pk_signing_keys: vec![SecretKey {
                hex: secret_hex.clone(),
            }],
            ..Default::default()
        };

        let to_json = options.to_json().unwrap();
        let encoded = crate::types::wallet::encode_receive_options(options.clone()).unwrap();
        let debug = format!("{:?}", options);

        assert!(to_json.contains(&secret_hex));
        assert!(to_json.contains("p2pk_signing_keys"));
        assert!(encoded.contains(&secret_hex));
        assert!(encoded.contains("p2pk_signing_keys"));
        assert!(!debug.contains(&secret_hex));
        assert!(debug.contains("[redacted]"));

        let decoded = crate::types::wallet::decode_receive_options(encoded).unwrap();

        assert_eq!(decoded.p2pk_signing_keys.len(), 1);
        assert_eq!(decoded.p2pk_signing_keys[0].hex, secret_hex);
    }

    #[test]
    fn test_send_options_json_defaults_new_p2pk_fields() {
        let json = r#"{
            "memo": null,
            "conditions": null,
            "amount_split_target": "None",
            "send_kind": "OnlineExact",
            "include_fee": false,
            "use_p2bk": false,
            "max_proofs": null,
            "metadata": {}
        }"#;

        let options = crate::types::wallet::decode_send_options(json.to_string()).unwrap();

        assert!(options.p2pk_signing_keys.is_empty());
        assert_eq!(
            options.p2pk_locked_proof_send_mode,
            P2PKLockedProofSendMode::Swap
        );
    }

    #[test]
    fn test_proof_with_invalid_dleq_returns_error() {
        let proof = Proof {
            amount: Amount::new(1),
            secret: "test-secret".to_string(),
            c: "02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc".to_string(),
            keyset_id: "009a1f293253e41e".to_string(),
            witness: None,
            dleq: Some(ProofDleq {
                e: "z".repeat(64),
                s: "a".repeat(64),
                r: "b".repeat(64),
            }),
            p2pk_e: None,
        };

        let result: Result<cdk::nuts::Proof, _> = proof.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_blind_signature_dleq_invalid_hex_returns_error() {
        let dleq = BlindSignatureDleq {
            e: "z".repeat(64),
            s: "a".repeat(64),
        };

        let result: Result<cdk::nuts::BlindSignatureDleq, _> = dleq.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_invalid_saga_id_returns_error() {
        let transaction = Transaction {
            id: TransactionId {
                hex: "a".repeat(64),
            },
            mint_url: MintUrl {
                url: "https://mint.example.com".to_string(),
            },
            direction: TransactionDirection::Outgoing,
            amount: Amount::new(100),
            fee: Amount::new(0),
            unit: CurrencyUnit::Sat,
            ys: vec![],
            timestamp: 0,
            memo: None,
            metadata: Default::default(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some("not-a-valid-uuid".to_string()),
            status: TransactionStatus::Completed,
        };

        let result: Result<cdk::wallet::types::Transaction, _> = transaction.try_into();

        assert!(result.is_err());
    }

    #[test]
    fn test_mint_quote_pending_state_does_not_inflate_mintable() {
        let ffi_quote = MintQuote {
            id: "test-quote".to_string(),
            amount: Some(Amount::new(100)),
            unit: CurrencyUnit::Sat,
            request: "lnbc1...".to_string(),
            state: QuoteState::Pending,
            expiry: u64::MAX,
            mint_url: MintUrl::new("https://mint.example.com".to_string())
                .expect("valid mint URL should convert successfully"),
            amount_issued: Amount::zero(),
            amount_paid: Amount::zero(),
            updated_at: 0,
            estimated_blocks: None,
            payment_method: PaymentMethod::Bolt11,
            secret_key: None,
            used_by_operation: None,
            version: 0,
        };

        let mintable =
            mint_quote_amount_mintable(&ffi_quote).expect("valid mint quote should convert");

        assert_eq!(mintable.value, 0);
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

    #[test]
    fn test_keyset_info_try_from_rejects_invalid_id() {
        let err = <cdk::nuts::KeySetInfo as TryFrom<KeySetInfo>>::try_from(KeySetInfo {
            id: "invalid".to_string(),
            unit: CurrencyUnit::Sat,
            active: true,
            input_fee_ppk: 0,
        })
        .expect_err("invalid keyset ID should return an error");

        assert!(err.to_string().contains("Invalid keyset ID"));
    }

    #[test]
    fn test_keyset_info_try_from_rejects_empty_id() {
        let err = <cdk::nuts::KeySetInfo as TryFrom<KeySetInfo>>::try_from(KeySetInfo {
            id: String::new(),
            unit: CurrencyUnit::Sat,
            active: true,
            input_fee_ppk: 0,
        })
        .expect_err("empty keyset ID should return an error");

        assert!(err.to_string().contains("Invalid keyset ID"));
    }

    #[test]
    fn test_id_try_from_rejects_invalid_hex() {
        let err = <cdk::nuts::Id as TryFrom<Id>>::try_from(Id {
            hex: "invalid".to_string(),
        })
        .expect_err("invalid ID hex should return an error");

        assert!(err.to_string().contains("Invalid ID hex"));
    }

    #[test]
    fn test_id_try_from_accepts_valid_hex() {
        let id: cdk::nuts::Id = Id {
            hex: "009a1f293253e41e".to_string(),
        }
        .try_into()
        .expect("valid ID hex should convert successfully");

        assert_eq!(id.to_string(), "009a1f293253e41e");
    }
}
