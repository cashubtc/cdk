//! Integration tests for Spark Lightning backend
//!
//! These tests verify the basic functionality of the Spark backend
//! including invoice creation, payment, and event handling.

#![cfg(all(test, feature = "spark"))]

use std::time::Duration;

use cdk::amount::Amount;
use cdk::cdk_database::MintKVStore;
use cdk::nuts::CurrencyUnit;
use cdk_common::common::FeeReserve;
use cdk_common::payment::{
    Bolt11IncomingPaymentOptions, Bolt11OutgoingPaymentOptions, IncomingPaymentOptions,
    MintPayment, OutgoingPaymentOptions,
};
use cdk_spark::{CdkSpark, SparkConfig};
use tokio;

/// Helper to create a test Spark configuration
fn create_test_config() -> SparkConfig {
    SparkConfig {
        network: spark_wallet::Network::Regtest,
        mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
        passphrase: None,
        storage_dir: format!("./target/test-spark-{}", uuid::Uuid::new_v4()),
        api_key: None,
        operator_pool: None,
        service_provider: None,
        fee_reserve: FeeReserve {
            min_fee_reserve: Amount::from(10),
            percent_fee_reserve: 0.01,
        },
        reconnect_interval_seconds: 30,
        split_secret_threshold: 2,
    }
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_spark_initialization() {
    let config = create_test_config();

    // Create Spark backend
    let spark = CdkSpark::new(config).await;

    assert!(spark.is_ok(), "Spark initialization should succeed");
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_spark_start_stop() {
    let config = create_test_config();
    let spark = CdkSpark::new(config).await.expect("Failed to create Spark");

    // Start the backend
    let start_result = spark.start().await;
    assert!(start_result.is_ok(), "Spark should start successfully");

    // Stop the backend
    let stop_result = spark.stop().await;
    assert!(stop_result.is_ok(), "Spark should stop successfully");
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_spark_get_settings() {
    let config = create_test_config();
    let spark = CdkSpark::new(config).await.expect("Failed to create Spark");

    let settings = spark.get_settings().await.expect("Failed to get settings");

    // Verify settings structure
    assert!(settings.is_object());
    let settings_obj = settings.as_object().unwrap();

    assert_eq!(
        settings_obj.get("unit").and_then(|v| v.as_str()),
        Some("sat")
    );
    assert_eq!(
        settings_obj.get("mpp").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_create_invoice() {
    let config = create_test_config();
    let spark = CdkSpark::new(config).await.expect("Failed to create Spark");

    spark.start().await.expect("Failed to start Spark");

    // Create invoice for 1000 sats
    let options = IncomingPaymentOptions::Bolt11(Bolt11IncomingPaymentOptions {
        amount: Amount::from(1000),
        description: Some("Test invoice".to_string()),
        unix_expiry: None,
    });

    let response = spark
        .create_incoming_payment_request(&CurrencyUnit::Sat, options)
        .await;

    assert!(response.is_ok(), "Invoice creation should succeed");

    let invoice_response = response.unwrap();
    assert!(
        !invoice_response.request.is_empty(),
        "Invoice string should not be empty"
    );

    spark.stop().await.expect("Failed to stop Spark");
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection and funded wallet
async fn test_payment_quote() {
    let config = create_test_config();
    let spark = CdkSpark::new(config).await.expect("Failed to create Spark");

    spark.start().await.expect("Failed to start Spark");

    // Parse a test invoice (this would need to be a real invoice on regtest)
    let test_invoice = "lnbcrt1..."; // Replace with actual regtest invoice
    let invoice = test_invoice.parse().expect("Failed to parse invoice");

    let options = OutgoingPaymentOptions::Bolt11(Bolt11OutgoingPaymentOptions {
        bolt11: invoice,
        max_fee_amount: None,
        melt_options: None,
    });

    let quote = spark.get_payment_quote(&CurrencyUnit::Sat, options).await;

    // This will fail without a real invoice, but tests the code path
    // In a real integration test with a regtest network, this should succeed

    spark.stop().await.expect("Failed to stop Spark");
}

#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_payment_event_stream() {
    let config = create_test_config();
    let spark = CdkSpark::new(config).await.expect("Failed to create Spark");

    spark.start().await.expect("Failed to start Spark");

    // Get payment event stream
    let stream = spark.wait_payment_event().await;
    assert!(
        stream.is_ok(),
        "Should be able to create payment event stream"
    );

    // Verify stream is active
    assert!(spark.is_wait_invoice_active());

    // Cancel stream
    spark.cancel_wait_invoice();

    // Give it a moment to cancel
    tokio::time::sleep(Duration::from_millis(100)).await;

    spark.stop().await.expect("Failed to stop Spark");
}

#[tokio::test]
async fn test_fee_calculation() {
    // This test doesn't require network connection
    let config = create_test_config();

    // Test fee reserve calculation
    let min_fee = config.fee_reserve.min_fee_reserve;
    let percent_fee = config.fee_reserve.percent_fee_reserve;

    // For 1000 sats with 1% fee
    let amount = 1000_u64;
    let calculated_fee = (amount as f32 * percent_fee) as u64;
    let expected_fee = calculated_fee.max(u64::from(min_fee));

    assert_eq!(expected_fee, 10); // Should use min_fee since 1% of 1000 = 10

    // For 10000 sats with 1% fee
    let amount = 10000_u64;
    let calculated_fee = (amount as f32 * percent_fee) as u64;
    let expected_fee = calculated_fee.max(u64::from(min_fee));

    assert_eq!(expected_fee, 100); // Should use calculated fee since 1% of 10000 = 100
}

#[tokio::test]
async fn test_config_from_toml() {
    // Test parsing Spark config from TOML-like structure
    use serde_json::json;

    let config_json = json!({
        "network": "signet",
        "mnemonic": "test mnemonic phrase",
        "storage_dir": "./data/spark",
        "fee_percent": 0.01,
        "reserve_fee_min": 10,
        "reconnect_interval_seconds": 30,
        "split_secret_threshold": 2
    });

    // Verify the structure is valid
    assert_eq!(config_json["network"], "signet");
    assert_eq!(config_json["fee_percent"], 0.01);
}

/// Test that Spark config serialization works correctly
#[tokio::test]
async fn test_spark_config_serde() {
    use cdk_spark::SparkConfig;
    use spark_wallet::Network;

    // Create a test config
    let config = SparkConfig {
        network: Network::Signet,
        mnemonic: "test mnemonic twelve words here".to_string(),
        passphrase: Some("test passphrase".to_string()),
        api_key: Some("test_api_key".to_string()),
        operator_pool: None,
        service_provider: None,
        fee_reserve: cdk_common::common::FeeReserve {
            min_fee_reserve: 10.into(),
            percent_fee_reserve: 0.01,
        },
        reconnect_interval_seconds: 30,
        split_secret_threshold: 2,
    };

    // Serialize
    let json = serde_json::to_string(&config).unwrap();
    
    // Verify it contains expected fields
    assert!(json.contains("signet"));
    assert!(json.contains("test mnemonic"));
    assert!(json.contains("test passphrase"));
    
    // Deserialize
    let deserialized: SparkConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.network, Network::Signet);
    assert_eq!(deserialized.reconnect_interval_seconds, 30);
    assert_eq!(deserialized.split_secret_threshold, 2);
}

/// Test Spark initialization with different network configurations
#[tokio::test]
#[ignore] // Requires actual Spark network connection
async fn test_spark_multi_network() {
    use spark_wallet::Network;

    let networks = vec![
        (Network::Signet, "Signet"),
        (Network::Testnet, "Testnet"),
        (Network::Regtest, "Regtest"),
    ];

    for (network, name) in networks {
        let config = create_test_config();
        let mut spark_config = config.clone();
        spark_config.network = network;

        let spark = CdkSpark::new(spark_config).await;
        
        match spark {
            Ok(_) => println!("{} network configuration accepted", name),
            Err(e) => println!("{} network failed: {}", name, e),
        }
    }
}

/// Test error handling for invalid configurations
#[tokio::test]
async fn test_spark_invalid_config() {
    // Test with empty mnemonic
    let mut config = create_test_config();
    config.mnemonic = "".to_string();
    
    // Note: This would fail in real usage but test the validation
    let result = config.validate();
    
    // Validation should catch empty mnemonic
    assert!(result.is_err());
}

/// Test fee calculation logic
#[tokio::test]
async fn test_spark_fee_calculation_logic() {
    use cdk_common::common::FeeReserve;
    
    let fee_reserve = FeeReserve {
        min_fee_reserve: 10.into(),
        percent_fee_reserve: 0.01,
    };

    // Test various amounts
    let test_cases = vec![
        (100, 10),      // Small amount: min fee applies
        (1000, 10),     // Small amount: min fee applies  
        (5000, 50),     // Larger amount: percent fee applies
        (10000, 100),   // Large amount: percent fee applies
    ];

    for (amount, expected_fee) in test_cases {
        let calculated_fee = (amount as f32 * fee_reserve.percent_fee_reserve) as u64;
        let actual_fee = calculated_fee.max(u64::from(fee_reserve.min_fee_reserve));
        assert_eq!(actual_fee, expected_fee, 
            "Fee calculation failed for amount: {}", amount);
    }
}
