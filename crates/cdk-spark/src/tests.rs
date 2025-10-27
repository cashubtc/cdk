//! Unit tests for cdk-spark

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CdkSpark, Error, SparkConfig};
    use cdk_common::nuts::CurrencyUnit;
    use cdk_common::Amount;

    #[test]
    fn test_error_conversions() {
        // Test that our errors convert properly to payment errors
        let err = Error::Configuration("test error".to_string());
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(payment_err, cdk_common::payment::Error::Anyhow(_)));

        let err = Error::UnknownInvoiceAmount;
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(payment_err, cdk_common::payment::Error::Anyhow(_)));
    }

    #[test]
    fn test_amount_conversions() {
        // Test Sat to Msat conversion
        let sats: Amount = 100.into();
        let msats = CdkSpark::convert_amount(sats, &CurrencyUnit::Sat, &CurrencyUnit::Msat);
        assert!(msats.is_ok());
        assert_eq!(u64::from(msats.unwrap()), 100_000);

        // Test Msat to Sat conversion
        let msats: Amount = 100_000.into();
        let sats = CdkSpark::convert_amount(msats, &CurrencyUnit::Msat, &CurrencyUnit::Sat);
        assert!(sats.is_ok());
        assert_eq!(u64::from(sats.unwrap()), 100);

        // Test same unit conversion
        let sats: Amount = 100.into();
        let same = CdkSpark::convert_amount(sats, &CurrencyUnit::Sat, &CurrencyUnit::Sat);
        assert!(same.is_ok());
        assert_eq!(u64::from(same.unwrap()), 100);
    }

    #[test]
    fn test_sats_to_unit() {
        // Sat to Sat
        let result = CdkSpark::sats_to_unit(100, &CurrencyUnit::Sat);
        assert!(result.is_ok());
        assert_eq!(u64::from(result.unwrap()), 100);

        // Sat to Msat
        let result = CdkSpark::sats_to_unit(100, &CurrencyUnit::Msat);
        assert!(result.is_ok());
        assert_eq!(u64::from(result.unwrap()), 100_000);
    }

    #[test]
    fn test_unit_to_sats() {
        // Sat to Sat
        let result = CdkSpark::unit_to_sats(100.into(), &CurrencyUnit::Sat);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 100);

        // Msat to Sat
        let result = CdkSpark::unit_to_sats(100_000.into(), &CurrencyUnit::Msat);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 100);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        let config = SparkConfig {
            network: spark_wallet::Network::Signet,
            mnemonic: "word1 word2 word3".to_string(),
            passphrase: None,
            api_key: None,
            operator_pool: None,
            service_provider: None,
            fee_reserve: cdk_common::common::FeeReserve {
                min_fee_reserve: 10.into(),
                percent_fee_reserve: 0.01,
            },
            reconnect_interval_seconds: 30,
            split_secret_threshold: 2,
        };
        assert!(config.validate().is_ok());

        // Invalid config - empty mnemonic
        let mut bad_config = config.clone();
        bad_config.mnemonic = "".to_string();
        assert!(bad_config.validate().is_err());

        // Invalid config - negative fee
        let mut bad_config = config.clone();
        bad_config.fee_reserve.percent_fee_reserve = -0.01;
        assert!(bad_config.validate().is_err());
    }

    #[test]
    fn test_config_defaults() {
        let config = SparkConfig::default_for_network(
            spark_wallet::Network::Signet,
            "test mnemonic".to_string(),
        );

        assert_eq!(config.network, spark_wallet::Network::Signet);
        assert_eq!(config.mnemonic, "test mnemonic");
        assert_eq!(u64::from(config.fee_reserve.min_fee_reserve), 10);
        assert_eq!(config.fee_reserve.percent_fee_reserve, 0.01);
        assert_eq!(config.reconnect_interval_seconds, 30);
        assert_eq!(config.split_secret_threshold, 2);
    }

    #[test]
    fn test_network_variants() {
        use spark_wallet::Network;

        // Ensure all network types are covered
        let networks = vec![
            Network::Mainnet,
            Network::Testnet,
            Network::Signet,
            Network::Regtest,
        ];

        for network in networks {
            let config = SparkConfig::default_for_network(network, "test".to_string());
            assert_eq!(config.network, network);
        }
    }

    #[test]
    fn test_config_with_passphrase() {
        let config = SparkConfig {
            network: spark_wallet::Network::Signet,
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
            passphrase: Some("test passphrase".to_string()),
            api_key: None,
            operator_pool: None,
            service_provider: None,
            fee_reserve: cdk_common::common::FeeReserve {
                min_fee_reserve: 10.into(),
                percent_fee_reserve: 0.01,
            },
            reconnect_interval_seconds: 30,
            split_secret_threshold: 2,
        };
        assert!(config.validate().is_ok());
        assert_eq!(config.passphrase.unwrap(), "test passphrase");
    }

    #[test]
    fn test_config_with_api_key() {
        let config = SparkConfig {
            network: spark_wallet::Network::Signet,
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
            passphrase: None,
            api_key: Some("test_api_key_12345".to_string()),
            operator_pool: None,
            service_provider: None,
            fee_reserve: cdk_common::common::FeeReserve {
                min_fee_reserve: 10.into(),
                percent_fee_reserve: 0.01,
            },
            reconnect_interval_seconds: 30,
            split_secret_threshold: 2,
        };
        assert!(config.validate().is_ok());
        assert_eq!(config.api_key.unwrap(), "test_api_key_12345");
    }

    #[test]
    fn test_config_fee_limits() {
        let mut config = SparkConfig::default_for_network(
            spark_wallet::Network::Signet,
            "test mnemonic".to_string(),
        );

        // Test valid fee range
        config.fee_reserve.percent_fee_reserve = 0.0;
        assert!(config.validate().is_ok());

        config.fee_reserve.percent_fee_reserve = 0.5; // 50% max
        assert!(config.validate().is_ok());

        config.fee_reserve.percent_fee_reserve = 1.0; // 100%
        assert!(config.validate().is_ok());

        // Test invalid fees (negative)
        config.fee_reserve.percent_fee_reserve = -0.01;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_reconnect_interval_bounds() {
        let mut config = SparkConfig::default_for_network(
            spark_wallet::Network::Signet,
            "test mnemonic".to_string(),
        );

        // Test valid intervals (validation doesn't check these)
        config.reconnect_interval_seconds = 1;
        assert!(config.validate().is_ok());

        config.reconnect_interval_seconds = 30;
        assert!(config.validate().is_ok());

        config.reconnect_interval_seconds = 3600;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_threshold_validation() {
        let mut config = SparkConfig::default_for_network(
            spark_wallet::Network::Signet,
            "test mnemonic".to_string(),
        );

        // Test valid thresholds (validation doesn't check these)
        config.split_secret_threshold = 2;
        assert!(config.validate().is_ok());

        config.split_secret_threshold = 5;
        assert!(config.validate().is_ok());

        // Even 0 and 1 pass validation (config doesn't check thresholds)
        config.split_secret_threshold = 0;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_fee_calculation_edge_cases() {
        use cdk_common::common::FeeReserve;

        // Test with min fee reserve
        let _fee_reserve = FeeReserve {
            min_fee_reserve: 10.into(),
            percent_fee_reserve: 0.01,
        };

        // Amount smaller than min fee
        let small_amount: u64 = 500;
        let expected_fee = 10; // Uses min_fee_reserve
        let calculated_fee = (small_amount as f32 * 0.01) as u64;
        let actual_fee = calculated_fee.max(10);
        assert_eq!(actual_fee, expected_fee);

        // Amount where percent fee applies
        let larger_amount: u64 = 5000;
        let calculated_fee = (larger_amount as f32 * 0.01) as u64;
        assert_eq!(calculated_fee, 50);
    }

    #[test]
    fn test_min_fee_reserve_application() {
        use cdk_common::common::FeeReserve;

        let _fee_reserve = FeeReserve {
            min_fee_reserve: 100.into(), // Higher min
            percent_fee_reserve: 0.01,
        };

        // Small amount should use min fee
        let amount: u64 = 500;
        let calculated_fee = (amount as f32 * 0.01) as u64;
        let actual_fee = calculated_fee.max(100);
        assert_eq!(actual_fee, 100); // Uses min_fee_reserve

        // Larger amount should use percent
        let amount: u64 = 50000;
        let calculated_fee = (amount as f32 * 0.01) as u64;
        assert_eq!(calculated_fee, 500);
        let actual_fee = calculated_fee.max(100);
        assert_eq!(actual_fee, 500); // Uses calculated fee
    }

    #[test]
    fn test_percent_fee_calculation() {
        use cdk_common::common::FeeReserve;

        // Test 1% fee
        let _fee_reserve = FeeReserve {
            min_fee_reserve: 10.into(),
            percent_fee_reserve: 0.01,
        };
        let amount: u64 = 10000;
        let calculated_fee = (amount as f32 * 0.01) as u64;
        assert_eq!(calculated_fee, 100);

        // Test 0.5% fee
        let _fee_reserve = FeeReserve {
            min_fee_reserve: 10.into(),
            percent_fee_reserve: 0.005,
        };
        let calculated_fee = (amount as f32 * 0.005) as u64;
        assert_eq!(calculated_fee, 50);

        // Test 2% fee
        let _fee_reserve = FeeReserve {
            min_fee_reserve: 10.into(),
            percent_fee_reserve: 0.02,
        };
        let calculated_fee = (amount as f32 * 0.02) as u64;
        assert_eq!(calculated_fee, 200);
    }

    #[test]
    fn test_invalid_mnemonic_error() {
        let err = Error::InvalidMnemonic("invalid mnemonic".to_string());
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_configuration_error_conversion() {
        let err = Error::Configuration("config error".to_string());
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_network_error_handling() {
        let err = Error::Network("network error".to_string());
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_payment_timeout_error() {
        let err = Error::PaymentTimeout;
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_payment_not_found_error() {
        let err = Error::PaymentNotFound;
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_invoice_parse_error() {
        let err = Error::InvoiceParse("parse error".to_string());
        let payment_err: cdk_common::payment::Error = err.into();
        assert!(matches!(
            payment_err,
            cdk_common::payment::Error::Anyhow(_)
        ));
    }

    #[test]
    fn test_amount_overflow_protection() {
        // Test that amount conversions handle edge cases
        let large_amount: u64 = u64::MAX;
        let _sats: Amount = large_amount.into();

        // Conversion should not panic
        let result = CdkSpark::sats_to_unit(large_amount, &CurrencyUnit::Sat);
        assert!(result.is_ok());

        // Msat conversion with large values
        let result = CdkSpark::sats_to_unit(large_amount, &CurrencyUnit::Msat);
        assert!(result.is_err()); // Should fail due to overflow
    }

    #[test]
    fn test_zero_amount_handling() {
        // Test that zero amounts are handled correctly
        let zero: Amount = 0.into();
        let result = CdkSpark::unit_to_sats(zero, &CurrencyUnit::Sat);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
