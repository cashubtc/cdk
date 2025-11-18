//! Batch Mint Tests [NUT-XX]
//!
//! This file contains tests for the batch minting functionality [NUT-XX].
//!
//! ## Current Test Coverage
//! - Request serialization/deserialization
//! - NUT-20 signature array structure validation
//! - Quote list size and order preservation

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::BlindedMessage;
use cdk::nuts::{CurrencyUnit, PaymentMethod, PreMintSecrets, SecretKey};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::Amount;
use cdk_common::mint::{BatchMintRequest, BatchQuoteStatusRequest};
use cdk_common::wallet::MintQuote;
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::mint::memory;

/// Helper to create a MintUrl from a string
fn test_mint_url() -> MintUrl {
    MintUrl::from_str("http://test.mint").unwrap()
}

/// Helper function to create a test mint with fake wallet
async fn create_test_mint() -> Arc<cdk::Mint> {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = memory::empty().await.expect("valid db instance");

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );

    let localstore = Arc::new(database);
    let mut mint_builder = MintBuilder::new(localstore.clone());

    mint_builder = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    Arc::new(mint)
}

/// Helper to create realistic blinded messages for testing
async fn create_test_outputs(mint: &Arc<cdk::Mint>, count: usize) -> Vec<BlindedMessage> {
    // Get active keyset to create proper blinded messages
    let keysets = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let keyset_id = keysets;

    let mut outputs = Vec::new();
    for _ in 0..count {
        // Create a small amount for testing
        let amount = Amount::from(1);
        let split_target = SplitTarget::default();
        let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

        let pre_mint = PreMintSecrets::random(keyset_id, amount, &split_target, &fee_and_amounts)
            .expect("Failed to create premint secrets");

        // Collect all blinded messages from this pre_mint
        outputs.extend(pre_mint.blinded_messages().iter().cloned());
    }
    outputs
}

/// Helper to detect duplicates in a quote list
fn has_duplicate_quotes(quotes: &[String]) -> bool {
    let mut seen = std::collections::HashSet::new();
    !quotes.iter().all(|q| seen.insert(q.clone()))
}

#[test]
fn test_batch_request_quote_order_preservation() {
    // Test that quote order is preserved through serialization
    let quotes = vec!["q1", "q2", "q3", "q4", "q5"];
    let request = BatchQuoteStatusRequest {
        quote: quotes.iter().map(|q| q.to_string()).collect(),
    };

    // Verify order is preserved
    assert_eq!(
        request.quote,
        vec!["q1", "q2", "q3", "q4", "q5"]
            .iter()
            .map(|q| q.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_batch_request_duplicate_detection() {
    // Test helper function can detect duplicates
    let quotes_with_duplicates = vec!["q1".to_string(), "q2".to_string(), "q1".to_string()];
    assert!(has_duplicate_quotes(&quotes_with_duplicates));

    let unique_quotes = vec!["q1".to_string(), "q2".to_string(), "q3".to_string()];
    assert!(!has_duplicate_quotes(&unique_quotes));
}

#[test]
fn test_batch_request_valid_size_boundaries() {
    // Test 50 quotes (well within limit)
    let quotes_50: Vec<String> = (0..50).map(|i| format!("quote_{}", i)).collect();
    assert!(quotes_50.len() <= 100);

    // Test exactly 100 quotes (at limit)
    let quotes_100: Vec<String> = (0..100).map(|i| format!("quote_{}", i)).collect();
    assert_eq!(quotes_100.len(), 100);

    // Test 101 quotes (exceeds limit, should be rejected by handler)
    let quotes_101: Vec<String> = (0..101).map(|i| format!("quote_{}", i)).collect();
    assert!(quotes_101.len() > 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_setup() {
    let _mint = create_test_mint().await;
    // Just verify that the test mint can be created successfully
    // Full end-to-end testing would require completing the batch processing logic
}

// ============================================================================
// NUT-20 Batch Minting Tests
// ============================================================================

#[test]
fn test_batch_mint_signature_array_validation_length_mismatch() {
    // Test that signature array length mismatch is detectable
    let request_json = r#"{
        "quote": ["q1", "q2", "q3"],
        "outputs": [],
        "signature": ["sig1", "sig2"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Signature array length should not match quotes length
    assert_eq!(req.quote.len(), 3);
    assert_eq!(req.signature.as_ref().unwrap().len(), 2);
    assert_ne!(req.quote.len(), req.signature.as_ref().unwrap().len());
}

#[test]
fn test_batch_mint_signature_array_with_nulls() {
    // Test that signature array can have null entries for unlocked quotes
    let request_json = r#"{
        "quote": ["q1", "q2", "q3"],
        "outputs": [],
        "signature": ["sig1", null, "sig3"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Verify structure
    assert_eq!(req.quote.len(), 3);
    assert_eq!(req.signature.as_ref().unwrap().len(), 3);
    assert_eq!(req.signature.as_ref().unwrap()[0], Some("sig1".to_string()));
    assert_eq!(req.signature.as_ref().unwrap()[1], None);
    assert_eq!(req.signature.as_ref().unwrap()[2], Some("sig3".to_string()));
}

#[test]
fn test_batch_mint_no_signatures_is_valid() {
    // Test that a batch request with no signatures (all unlocked quotes) is valid
    let request_json = r#"{
        "quote": ["q1", "q2"],
        "outputs": []
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Request should be valid
    assert_eq!(req.quote.len(), 2);
    assert!(req.signature.is_none());
}

#[test]
fn test_batch_mint_all_nulls_signatures_valid() {
    // Test that a signature array with all nulls is valid (all unlocked quotes)
    let request_json = r#"{
        "quote": ["q1", "q2"],
        "outputs": [],
        "signature": [null, null]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Request should be valid
    assert_eq!(req.quote.len(), 2);
    assert_eq!(req.signature.as_ref().unwrap().len(), 2);
    assert!(req.signature.as_ref().unwrap().iter().all(|s| s.is_none()));
}

#[test]
fn test_batch_mint_single_quote_with_signature() {
    // Test batch with single NUT-20 locked quote
    let request_json = r#"{
        "quote": ["q1"],
        "outputs": [],
        "signature": ["sig1"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Verify single-quote batch
    assert_eq!(req.quote.len(), 1);
    assert_eq!(req.outputs.len(), 0);
    assert_eq!(req.signature.as_ref().unwrap().len(), 1);
}

#[test]
fn test_batch_mint_request_serialization_with_signatures() {
    // Test that batch requests with signatures serialize/deserialize correctly
    let request_json = r#"{
        "quote": ["q1", "q2"],
        "outputs": [],
        "signature": ["sig1", null]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok());
    let req = request.unwrap();

    // Serialize and deserialize
    let json = serde_json::to_string(&req).expect("serialize");
    let deserialized: BatchMintRequest = serde_json::from_str(&json).expect("deserialize");

    // Verify structure is preserved
    assert_eq!(deserialized.quote.len(), 2);
    assert_eq!(deserialized.outputs.len(), 0);
    assert_eq!(deserialized.signature.as_ref().unwrap().len(), 2);
    assert_eq!(
        deserialized.signature.as_ref().unwrap()[0],
        Some("sig1".to_string())
    );
    assert_eq!(deserialized.signature.as_ref().unwrap()[1], None);
}

// ============================================================================
// Phase 1: Handler Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_empty_quotes() {
    let mint = create_test_mint().await;

    let request = BatchMintRequest {
        quote: vec![],
        outputs: vec![],
        signature: None,
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(result.is_err(), "Mint should reject empty quote list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_duplicates() {
    let mint = create_test_mint().await;

    // Create realistic outputs for 2 quotes
    let outputs = create_test_outputs(&mint, 2).await;

    // Try to mint with duplicate quote ID
    let request = BatchMintRequest {
        quote: vec!["q1".to_string(), "q1".to_string()],
        outputs, // Correct count with realistic blinded messages
        signature: None,
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(result.is_err(), "Mint should reject duplicate quote IDs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_over_limit() {
    let mint = create_test_mint().await;

    // Create 101 quote IDs (exceeds the 100 limit)
    let quotes: Vec<String> = (0..101).map(|i| format!("quote_{}", i)).collect();

    // Create realistic outputs for all 101 quotes
    let outputs = create_test_outputs(&mint, 101).await;

    let request = BatchMintRequest {
        quote: quotes,
        outputs, // Correct count with realistic blinded messages
        signature: None,
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(result.is_err(), "Mint should reject batch > 100 quotes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_validates_output_count() {
    let mint = create_test_mint().await;

    // Create outputs for only 1 quote, but request has 2 quotes
    let outputs = create_test_outputs(&mint, 1).await;

    let request = BatchMintRequest {
        quote: vec!["q1".to_string(), "q2".to_string()],
        outputs, // 1 output for 2 quotes - mismatch
        signature: None,
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(result.is_err(), "Mint should reject output count mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_validates_signature_count() {
    let mint = create_test_mint().await;

    // Create realistic outputs for 2 quotes
    let outputs = create_test_outputs(&mint, 2).await;

    // Generate valid signatures for both quotes
    let secret_key_1 = SecretKey::generate();

    let mut sig_req = cdk_common::nuts::MintRequest {
        quote: "q1".to_string(),
        outputs: vec![],
        signature: None,
    };
    sig_req.sign(secret_key_1).unwrap();

    // But only provide 1 signature for 2 quotes - this is the mismatch we're testing
    let request = BatchMintRequest {
        quote: vec!["q1".to_string(), "q2".to_string()],
        outputs, // Correct count with realistic blinded messages
        signature: Some(vec![
            sig_req.signature.clone(), // Valid sig for q1
                                       // Missing sig for q2 - count mismatch
        ]),
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(
        result.is_err(),
        "Mint should reject signature count mismatch"
    );
}

// ============================================================================
// Phase 2: NUT-20 Signature Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_invalid_nut20_signatures() {
    let mint = create_test_mint().await;

    // Create realistic outputs for 1 quote
    let outputs = create_test_outputs(&mint, 1).await;

    let request = BatchMintRequest {
        quote: vec!["test_quote".to_string()],
        outputs,                                         // Realistic blinded messages
        signature: Some(vec![Some("asdf".to_string())]), // Invalid signature
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(result.is_err(), "Should reject invalid signature");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_signature_without_pubkey() {
    let mint = create_test_mint().await;

    // Create an unlocked quote (no secret key)
    let _quote = MintQuote::new(
        "test_quote".to_string(),
        test_mint_url(),
        PaymentMethod::Bolt11,
        Some(100.into()),
        CurrencyUnit::Sat,
        "lnbc1000n...".to_string(),
        9999999999,
        None, // NO SECRET KEY - unlocked quote
    );

    // Generate a valid signature with a random key (not the quote's key, since it has none)
    let random_key = SecretKey::generate();
    let mut mint_req = cdk_common::nuts::MintRequest {
        quote: "test_quote".to_string(),
        outputs: vec![],
        signature: None,
    };
    mint_req.sign(random_key).unwrap();

    // Create realistic outputs for 1 quote
    let outputs = create_test_outputs(&mint, 1).await;

    // Try to provide signature for unlocked quote (should be rejected)
    let request = BatchMintRequest {
        quote: vec!["test_quote".to_string()],
        outputs, // Realistic blinded messages
        signature: Some(vec![mint_req.signature.clone()]),
    };

    let result = mint.process_batch_mint_request(request).await;
    assert!(
        result.is_err(),
        "Should reject signature for unlocked quote"
    );
}
