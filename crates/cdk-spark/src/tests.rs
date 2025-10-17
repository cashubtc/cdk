//! Unit tests for cdk-spark

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CdkSpark, SparkConfig, Error};
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
            storage_dir: "./data".to_string(),
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

        // Invalid config - empty storage dir
        let mut bad_config = config.clone();
        bad_config.storage_dir = "".to_string();
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
            "./data".to_string(),
        );

        assert_eq!(config.network, spark_wallet::Network::Signet);
        assert_eq!(config.mnemonic, "test mnemonic");
        assert_eq!(config.storage_dir, "./data");
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
            let config = SparkConfig::default_for_network(
                network,
                "test".to_string(),
                "./data".to_string(),
            );
            assert_eq!(config.network, network);
        }
    }
}

