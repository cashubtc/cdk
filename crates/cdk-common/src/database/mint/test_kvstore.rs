//! Tests for KV store validation requirements

#[cfg(test)]
mod tests {
    use crate::database::mint::{
        validate_kvstore_params, validate_kvstore_string, KVSTORE_NAMESPACE_KEY_ALPHABET,
        KVSTORE_NAMESPACE_KEY_MAX_LEN,
    };

    #[test]
    fn test_validate_kvstore_string_valid_inputs() {
        // Test valid strings
        assert!(validate_kvstore_string("").is_ok());
        assert!(validate_kvstore_string("abc").is_ok());
        assert!(validate_kvstore_string("ABC").is_ok());
        assert!(validate_kvstore_string("123").is_ok());
        assert!(validate_kvstore_string("test_key").is_ok());
        assert!(validate_kvstore_string("test-key").is_ok());
        assert!(validate_kvstore_string("test_KEY-123").is_ok());

        // Test max length string
        let max_length_str = "a".repeat(KVSTORE_NAMESPACE_KEY_MAX_LEN);
        assert!(validate_kvstore_string(&max_length_str).is_ok());
    }

    #[test]
    fn test_validate_kvstore_string_invalid_length() {
        // Test string too long
        let too_long_str = "a".repeat(KVSTORE_NAMESPACE_KEY_MAX_LEN + 1);
        let result = validate_kvstore_string(&too_long_str);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds maximum length"));
    }

    #[test]
    fn test_validate_kvstore_string_invalid_characters() {
        // Test invalid characters
        let invalid_chars = vec![
            "test@key",  // @
            "test key",  // space
            "test.key",  // .
            "test/key",  // /
            "test\\key", // \
            "test+key",  // +
            "test=key",  // =
            "test!key",  // !
            "test#key",  // #
            "test$key",  // $
            "test%key",  // %
            "test&key",  // &
            "test*key",  // *
            "test(key",  // (
            "test)key",  // )
            "test[key",  // [
            "test]key",  // ]
            "test{key",  // {
            "test}key",  // }
            "test|key",  // |
            "test;key",  // ;
            "test:key",  // :
            "test'key",  // '
            "test\"key", // "
            "test<key",  // <
            "test>key",  // >
            "test,key",  // ,
            "test?key",  // ?
            "test~key",  // ~
            "test`key",  // `
        ];

        for invalid_str in invalid_chars {
            let result = validate_kvstore_string(invalid_str);
            assert!(result.is_err(), "Expected '{}' to be invalid", invalid_str);
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("invalid characters"));
        }
    }

    #[test]
    fn test_validate_kvstore_params_valid() {
        // Test valid parameter combinations
        assert!(validate_kvstore_params("primary", "secondary", "key").is_ok());
        assert!(validate_kvstore_params("primary", "", "key").is_ok());
        assert!(validate_kvstore_params("", "", "key").is_ok());
        assert!(validate_kvstore_params("p1", "s1", "different_key").is_ok());
    }

    #[test]
    fn test_validate_kvstore_params_empty_namespace_rules() {
        // Test empty namespace rules: if primary is empty, secondary must be empty too
        let result = validate_kvstore_params("", "secondary", "key");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("If primary_namespace is empty"));
    }

    #[test]
    fn test_validate_kvstore_params_collision_prevention() {
        // Test collision prevention between keys and namespaces
        let test_cases = vec![
            ("primary", "secondary", "primary"), // key matches primary namespace
            ("primary", "secondary", "secondary"), // key matches secondary namespace
        ];

        for (primary, secondary, key) in test_cases {
            let result = validate_kvstore_params(primary, secondary, key);
            assert!(
                result.is_err(),
                "Expected collision for key '{}' with namespaces '{}'/'{}'",
                key,
                primary,
                secondary
            );
            let error_msg = result.unwrap_err().to_string();
            assert!(error_msg.contains("conflicts with namespace"));
        }

        // Test that a combined namespace string would be invalid due to the slash character
        let result = validate_kvstore_params("primary", "secondary", "primary_secondary");
        assert!(result.is_ok(), "This should be valid - no actual collision");
    }

    #[test]
    fn test_validate_kvstore_params_invalid_strings() {
        // Test invalid characters in any parameter
        let result = validate_kvstore_params("primary@", "secondary", "key");
        assert!(result.is_err());

        let result = validate_kvstore_params("primary", "secondary!", "key");
        assert!(result.is_err());

        let result = validate_kvstore_params("primary", "secondary", "key with space");
        assert!(result.is_err());
    }

    #[test]
    fn test_alphabet_constants() {
        // Verify the alphabet constant is as expected
        assert_eq!(
            KVSTORE_NAMESPACE_KEY_ALPHABET,
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
        );
        assert_eq!(KVSTORE_NAMESPACE_KEY_MAX_LEN, 120);
    }

    #[test]
    fn test_alphabet_coverage() {
        // Test that all valid characters are actually accepted
        for ch in KVSTORE_NAMESPACE_KEY_ALPHABET.chars() {
            let test_str = ch.to_string();
            assert!(
                validate_kvstore_string(&test_str).is_ok(),
                "Character '{}' should be valid",
                ch
            );
        }
    }

    #[test]
    fn test_namespace_segmentation_examples() {
        // Test realistic namespace segmentation scenarios

        // Valid segmentation examples
        let valid_examples = vec![
            ("wallets", "user123", "balance"),
            ("quotes", "mint", "quote_12345"),
            ("keysets", "", "active_keyset"),
            ("", "", "global_config"),
            ("auth", "session_456", "token"),
            ("mint_info", "", "version"),
        ];

        for (primary, secondary, key) in valid_examples {
            assert!(
                validate_kvstore_params(primary, secondary, key).is_ok(),
                "Valid example should pass: '{}'/'{}'/'{}'",
                primary,
                secondary,
                key
            );
        }
    }

    #[test]
    fn test_per_namespace_uniqueness() {
        // This test documents the requirement that implementations should ensure
        // per-namespace key uniqueness. The validation function doesn't enforce
        // database-level uniqueness (that's handled by the database schema),
        // but ensures naming conflicts don't occur between keys and namespaces.

        // These should be valid (different namespaces)
        assert!(validate_kvstore_params("ns1", "sub1", "key1").is_ok());
        assert!(validate_kvstore_params("ns2", "sub1", "key1").is_ok()); // same key, different primary namespace
        assert!(validate_kvstore_params("ns1", "sub2", "key1").is_ok()); // same key, different secondary namespace

        // These should fail (collision within namespace)
        assert!(validate_kvstore_params("ns1", "sub1", "ns1").is_err()); // key conflicts with primary namespace
        assert!(validate_kvstore_params("ns1", "sub1", "sub1").is_err()); // key conflicts with secondary namespace
    }
}
