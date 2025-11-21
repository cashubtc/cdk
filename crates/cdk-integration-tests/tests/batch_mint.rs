//! Batch Mint Tests [NUT-XX]
//!
//! This file contains tests for the batch minting functionality per NUT-XX specification:
//! https://github.com/cashubtc/nuts/issues/XX
//!
//! ## Test Coverage
//!
//! **Handler Validation Tests:**
//! - test_batch_mint_handler_rejects_empty_quotes: Empty quote lists are rejected
//! - test_batch_mint_handler_rejects_duplicates: Duplicate quote IDs are rejected
//! - test_batch_mint_handler_rejects_over_limit: Batches over 100 quotes are rejected
//! - test_batch_mint_handler_validates_signature_count: Signature array size validation
//!
//! **NUT-20 Signature Validation:**
//! - test_batch_mint_rejects_invalid_nut20_signatures: Invalid signatures are rejected
//! - test_batch_mint_rejects_signature_without_pubkey: Signature on unlocked quote is rejected
//!
//! **Quote Validation:**
//! - test_batch_mint_rejects_unpaid_quotes: Unpaid quotes are rejected
//! - test_batch_mint_enforces_single_payment_method: All quotes must have same payment method
//! - test_batch_mint_enforces_single_currency_unit: All quotes must have same unit
//! - test_batch_mint_validates_url_path_payment_method: Quotes must match endpoint payment method
//!
//! **Protocol Parsing:**
//! - test_batch_mint_parses_unlocked_quotes: Parse unlocked quote requests
//! - test_batch_mint_parses_locked_quotes: Parse locked quote requests
//! - test_batch_mint_parses_mixed_locked_unlocked: Parse mixed requests
//! - test_batch_mint_round_trip_serialization: Serialization round-trip
//!
//! **Coverage Areas:**
//! - Validation: empty batches, duplicates, size limits, state requirements
//! - Signature handling: NUT-20 signature verification, pubkey validation
//! - Quote constraints: payment method/unit consistency, endpoint validation
//! - Protocol compliance: JSON parsing and serialization

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bip39::Mnemonic;
use cashu::quote_id::QuoteId;
use cdk::amount::SplitTarget;
use cdk::cdk_payment::PaymentIdentifier;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::BlindedMessage;
use cdk::nuts::{CurrencyUnit, PaymentMethod, PreMintSecrets, SecretKey};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::Amount;
use cdk_common::mint::{BatchMintRequest, IncomingPayment, MintQuote};
use cdk_common::Error;
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::mint::memory;

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

/// Helper to create and store a MintQuote in the database
/// Returns the QuoteId for use in requests
async fn create_and_store_mint_quote(
    mint: &Arc<cdk::Mint>,
    amount: Option<Amount>,
    payment_method: PaymentMethod,
    amount_paid: Amount,
    pubkey: Option<cdk_common::PublicKey>,
) -> Result<QuoteId, Box<dyn std::error::Error>> {
    create_and_store_mint_quote_with_unit(
        mint,
        amount,
        payment_method,
        amount_paid,
        pubkey,
        CurrencyUnit::Sat,
    )
    .await
}

async fn create_and_store_mint_quote_with_unit(
    mint: &Arc<cdk::Mint>,
    amount: Option<Amount>,
    payment_method: PaymentMethod,
    amount_paid: Amount,
    pubkey: Option<cdk_common::PublicKey>,
    unit: CurrencyUnit,
) -> Result<QuoteId, Box<dyn std::error::Error>> {
    let quote_id = QuoteId::new_uuid();
    let quote = MintQuote::new(
        Some(quote_id.clone()),
        "lnbc1000n...".to_string(),
        unit,
        amount,
        9999999999, // Far future expiry
        PaymentIdentifier::Label(format!("quote_{}", quote_id)),
        pubkey,
        amount_paid,
        Amount::ZERO, // amount_issued
        payment_method,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        if amount_paid > Amount::ZERO {
            vec![IncomingPayment::new(
                amount_paid,
                "test_payment".to_string(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs(),
            )]
        } else {
            Vec::new()
        },
        Vec::new(), // issuance
    );

    let localstore = mint.localstore();
    let mut tx = localstore.begin_transaction().await?;
    tx.add_mint_quote(quote).await?;
    tx.commit().await?;

    Ok(quote_id)
}

// ============================================================================
// Protocol Parsing Tests - JSON Serialization Round-Trips
// ============================================================================

#[test]
fn test_batch_mint_parses_unlocked_quotes() {
    // Test parsing: batch with multiple unlocked quotes (no signatures)
    let request_json = r#"{
        "quote": ["quote1", "quote2"],
        "outputs": []
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok(), "Should parse unlocked quotes request");
    let req = request.unwrap();

    assert_eq!(req.quote.len(), 2);
    assert!(
        req.signature.is_none(),
        "Unlocked quotes should have no signatures"
    );
}

#[test]
fn test_batch_mint_parses_locked_quotes() {
    // Test parsing: batch with NUT-20 locked quotes (with signatures)
    let request_json = r#"{
        "quote": ["locked_quote1", "locked_quote2"],
        "outputs": [],
        "signature": ["sig1", "sig2"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok(), "Should parse locked quotes request");
    let req = request.unwrap();

    assert_eq!(req.quote.len(), 2);
    assert_eq!(req.signature.as_ref().unwrap().len(), 2);
    assert_eq!(req.signature.as_ref().unwrap()[0], Some("sig1".to_string()));
    assert_eq!(req.signature.as_ref().unwrap()[1], Some("sig2".to_string()));
}

#[test]
fn test_batch_mint_parses_mixed_locked_unlocked() {
    // Test parsing: batch with mix of locked and unlocked quotes (some nulls in signature array)
    let request_json = r#"{
        "quote": ["locked", "unlocked", "locked"],
        "outputs": [],
        "signature": ["sig1", null, "sig3"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(
        request.is_ok(),
        "Should parse mixed locked/unlocked request"
    );
    let req = request.unwrap();

    assert_eq!(req.quote.len(), 3);
    let sigs = req.signature.as_ref().unwrap();
    assert_eq!(sigs.len(), 3);
    assert_eq!(sigs[0], Some("sig1".to_string()));
    assert_eq!(sigs[1], None, "Unlocked quote should have null signature");
    assert_eq!(sigs[2], Some("sig3".to_string()));
}

#[test]
fn test_batch_mint_round_trip_serialization() {
    // Test: serialization and deserialization preserves request structure
    let request_json = r#"{
        "quote": ["q1", "q2"],
        "outputs": [],
        "signature": ["sig1", null]
    }"#;

    let original: BatchMintRequest = serde_json::from_str(request_json).unwrap();
    let serialized = serde_json::to_string(&original).expect("serialize");
    let deserialized: BatchMintRequest = serde_json::from_str(&serialized).expect("deserialize");

    assert_eq!(original.quote, deserialized.quote);
    assert_eq!(original.signature, deserialized.signature);
}

// ============================================================================
// Handler Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_empty_quotes() {
    let mint = create_test_mint().await;

    let request = BatchMintRequest {
        quote: vec![],
        outputs: vec![],
        signature: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Mint should reject empty quote list");
    match result {
        Err(Error::BatchEmpty) => {} // Expected - empty batch
        Err(e) => panic!("Expected BatchEmpty error, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
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

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Mint should reject duplicate quote IDs");
    match result {
        Err(Error::DuplicateQuoteIdInBatch) => {} // Expected
        Err(e) => panic!("Expected DuplicateQuoteIdInBatch, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
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

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Mint should reject batch > 100 quotes");
    match result {
        Err(Error::BatchSizeExceeded) => {} // Expected - exceeds 100 quote limit
        Err(e) => panic!("Expected BatchSizeExceeded, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_validates_signature_count() {
    let mint = create_test_mint().await;

    // Create two unlocked quotes (no pubkey required for this test)
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create first quote");

    let quote_id_2 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create second quote");

    // Create realistic outputs for 2 quotes
    let outputs = create_test_outputs(&mint, 2).await;

    // Generate a signature
    let secret_key = SecretKey::generate();
    let mut sig_req = cdk_common::nuts::MintRequest {
        quote: quote_id_1.to_string(),
        outputs: vec![],
        signature: None,
    };
    sig_req.sign(secret_key).unwrap();

    // Provide only 1 signature for 2 quotes - this is the mismatch we're testing
    let request = BatchMintRequest {
        quote: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        outputs,
        signature: Some(vec![
            sig_req.signature.clone(), // Valid sig for q1
                                       // Missing sig for q2 - count mismatch
        ]),
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Mint should reject signature count mismatch"
    );
    match result {
        Err(Error::BatchSignatureCountMismatch) => {} // Expected
        Err(e) => panic!("Expected BatchSignatureCountMismatch, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

// ============================================================================
// NUT-20 Signature Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_invalid_nut20_signatures() {
    let mint = create_test_mint().await;

    // Create a locked quote (with pubkey) that is PAID
    let secret_key = SecretKey::generate();
    let pubkey = secret_key.public_key();
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100), // amount_paid - mark as PAID
        Some(pubkey),      // PUBKEY - locked quote
    )
    .await
    .expect("Failed to create quote");

    // Create realistic outputs for 1 quote
    let outputs = create_test_outputs(&mint, 1).await;

    let request = BatchMintRequest {
        quote: vec![quote_id.to_string()],
        outputs,
        signature: Some(vec![Some("asdf".to_string())]), // Invalid signature
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Should reject invalid signature");
    match result {
        Err(Error::BatchInvalidSignature) => {} // Expected
        Err(e) => panic!("Expected BatchInvalidSignature, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_signature_without_pubkey() {
    let mint = create_test_mint().await;

    // Create an unlocked quote (no pubkey) that is PAID
    // This tests that signature validation is properly rejected when quote has no pubkey
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100), // amount_paid - mark as PAID
        None,              // NO PUBKEY - unlocked quote
    )
    .await
    .expect("Failed to create quote");

    // Generate a signature with a random key (not the quote's key, since it has none)
    let random_key = SecretKey::generate();
    let mut mint_req = cdk_common::nuts::MintRequest {
        quote: quote_id.to_string(),
        outputs: vec![],
        signature: None,
    };
    mint_req.sign(random_key).unwrap();

    // Create realistic outputs for 1 quote
    let outputs = create_test_outputs(&mint, 1).await;

    // Try to provide signature for unlocked quote (should be rejected)
    let request = BatchMintRequest {
        quote: vec![quote_id.to_string()],
        outputs, // Realistic blinded messages
        signature: Some(vec![mint_req.signature.clone()]),
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Should reject signature when quote has no pubkey"
    );
    match result {
        Err(Error::BatchUnexpectedSignature) => {} // Expected
        Err(e) => panic!("Expected BatchUnexpectedSignature, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

// ============================================================================
// Quote Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_unpaid_quotes() {
    let mint = create_test_mint().await;

    // Create a valid quote that is NOT paid (amount_paid = 0)
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::ZERO, // NOT PAID
        None,         // pubkey
    )
    .await
    .expect("Failed to create quote");

    // Create outputs for the quote
    let outputs = create_test_outputs(&mint, 1).await;

    // Request with the valid but unpaid quote
    let request = BatchMintRequest {
        quote: vec![quote_id.to_string()],
        outputs,
        signature: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    // Should fail because quote is in Unpaid state, not PAID
    assert!(result.is_err(), "Should reject unpaid quotes in batch");
    match result {
        Err(Error::UnpaidQuote) => {} // Expected error type
        Err(e) => panic!("Expected UnpaidQuote error, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_enforces_single_payment_method() {
    let mint = create_test_mint().await;

    // Create two quotes: one Bolt11, one Bolt12 (different payment methods)
    // First quote: Bolt11
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create first quote");

    // Second quote: Bolt12 (different payment method)
    let quote_id_2 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt12,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create second quote");

    // Create realistic outputs for 2 quotes
    let outputs = create_test_outputs(&mint, 2).await;

    let request = BatchMintRequest {
        quote: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        outputs,
        signature: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Batch should fail if quotes have different payment methods"
    );
    match result {
        Err(Error::BatchPaymentMethodMismatch) => {} // Expected
        Err(e) => panic!("Expected BatchPaymentMethodMismatch, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_enforces_single_currency_unit() {
    // Create a test mint with multiple currency units
    let mnemonic = bip39::Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = cdk_sqlite::mint::memory::empty()
        .await
        .expect("valid db instance");

    let localstore = Arc::new(database);
    let mut mint_builder = cdk::mint::MintBuilder::new(localstore.clone());

    mint_builder = mint_builder
        .with_name("test mint multi-unit".to_string())
        .with_description("test mint with multiple units".to_string());

    // Add Sat with Bolt11
    let fake_wallet_sat = cdk_fake_wallet::FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet_sat),
        )
        .await
        .unwrap();

    // Add Msat with Bolt11
    let fake_wallet_msat = cdk_fake_wallet::FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Msat,
    );
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Msat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet_msat),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    let mint = Arc::new(mint);

    // Create two quotes with different units
    // First quote: SAT unit
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create first quote");

    // Second quote: Msat unit (different from SAT)
    let quote_id_2 = create_and_store_mint_quote_with_unit(
        &mint,
        Some(1000.into()),
        PaymentMethod::Bolt11,
        Amount::from(1000),
        None,
        CurrencyUnit::Msat,
    )
    .await
    .expect("Failed to create second quote");

    // Create outputs for batch
    let outputs = create_test_outputs(&mint, 2).await;

    let request = BatchMintRequest {
        quote: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        outputs,
        signature: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Batch should fail if quotes have different currency units"
    );
    match result {
        Err(Error::BatchCurrencyUnitMismatch) => {} // Expected
        Err(e) => panic!("Expected BatchCurrencyUnitMismatch, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_validates_url_path_payment_method() {
    let mint = create_test_mint().await;

    // Create a Bolt12 quote (which shouldn't be used with the Bolt11 batch endpoint)
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt12, // Bolt12 - endpoint mismatch!
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create quote");

    // Create outputs for batch
    let outputs = create_test_outputs(&mint, 1).await;

    let request = BatchMintRequest {
        quote: vec![quote_id.to_string()],
        outputs,
        signature: None,
    };

    // Call with Bolt11 as the endpoint payment method
    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Should reject Bolt12 quotes at Bolt11 batch endpoint"
    );
    match result {
        Err(Error::BatchPaymentMethodEndpointMismatch) => {} // Expected
        Err(e) => panic!("Expected BatchPaymentMethodEndpointMismatch, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}
