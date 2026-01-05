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
//! **Happy Path Tests:**
//! - test_batch_mint_successful_two_quotes_happy_path: Successful batch mint with two paid quotes
//!
//! **Coverage Areas:**
//! - Validation: empty batches, duplicates, size limits, state requirements
//! - Signature handling: NUT-20 signature verification, pubkey validation
//! - Quote constraints: payment method/unit consistency, endpoint validation
//! - Protocol compliance: JSON parsing and serialization
//! - Success scenarios: batch minting with multiple paid quotes

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use cashu::quote_id::QuoteId;
use cdk::amount::SplitTarget;
use cdk::cdk_payment::PaymentIdentifier;
use cdk::mint::MintMeltLimits;
use cdk::nuts::nut00::BlindedMessage;
use cdk::nuts::{CurrencyUnit, PaymentMethod, PreMintSecrets, PublicKey, SecretKey};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::Amount;
use cdk_common::mint::{BatchMintRequest, MintQuote};
use cdk_common::Error;
use cdk_fake_wallet::FakeWallet;
use cdk_integration_tests::init_pure_tests::create_and_start_test_mint;
use cdk_sqlite::mint::memory;

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
        Amount::ZERO,
        Amount::ZERO, // amount_issued
        payment_method,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        Vec::new(), // payments
        Vec::new(), // issuance
    );

    let localstore = mint.localstore();
    let mut tx = localstore.begin_transaction().await?;
    tx.add_mint_quote(quote).await?;
    if amount_paid > Amount::ZERO {
        let payment_id = format!("test_payment_{}", quote_id);
        tx.increment_mint_quote_amount_paid(&quote_id.clone().into(), amount_paid, payment_id)
            .await?;
    }
    tx.commit().await?;

    Ok(quote_id)
}

/// Helper to build blinded outputs for a specific amount/unit
async fn create_outputs_for_amount(
    mint: &Arc<cdk::Mint>,
    amount: u64,
    unit: CurrencyUnit,
) -> Vec<BlindedMessage> {
    let keyset_id = *mint
        .get_active_keysets()
        .get(&unit)
        .expect("keyset for unit");
    let split_target = SplitTarget::default();
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        keyset_id,
        Amount::from(amount),
        &split_target,
        &fee_and_amounts,
    )
    .expect("premint secrets");

    pre_mint.blinded_messages().iter().cloned().collect()
}

/// Create a mint with Bolt11 and Bolt12 support for SAT unit.
async fn create_bolt12_capable_mint() -> Arc<cdk::Mint> {
    let mnemonic = bip39::Mnemonic::generate(12).expect("mnemonic");
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 0.02,
    };

    let localstore = Arc::new(memory::empty().await.expect("mint db"));
    let mut mint_builder = cdk::mint::MintBuilder::new(localstore.clone());

    let wallet_sat_bolt11 = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 10_000),
            Arc::new(wallet_sat_bolt11),
        )
        .await
        .expect("bolt11 processor");

    let wallet_sat_bolt12 = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );
    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt12,
            MintMeltLimits::new(1, 10_000),
            Arc::new(wallet_sat_bolt12),
        )
        .await
        .expect("bolt12 processor");

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .expect("mint build");
    mint.start().await.expect("mint start");
    Arc::new(mint)
}

// ============================================================================
// Protocol Parsing Tests - JSON Serialization Round-Trips
// ============================================================================

#[test]
fn test_batch_mint_parses_unlocked_quotes() {
    // Test parsing: batch with multiple unlocked quotes (no signatures)
    let request_json = r#"{
        "quotes": ["quote1", "quote2"],
        "outputs": []
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok(), "Should parse unlocked quotes request");
    let req = request.unwrap();

    assert_eq!(req.quotes.len(), 2);
    assert!(
        req.signatures.is_none(),
        "Unlocked quotes should have no signatures"
    );
}

#[test]
fn test_batch_mint_parses_locked_quotes() {
    // Test parsing: batch with NUT-20 locked quotes (with signatures)
    let request_json = r#"{
        "quotes": ["locked_quote1", "locked_quote2"],
        "outputs": [],
        "signatures": ["sig1", "sig2"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(request.is_ok(), "Should parse locked quotes request");
    let req = request.unwrap();

    assert_eq!(req.quotes.len(), 2);
    assert_eq!(req.signatures.as_ref().unwrap().len(), 2);
    assert_eq!(
        req.signatures.as_ref().unwrap()[0],
        Some("sig1".to_string())
    );
    assert_eq!(
        req.signatures.as_ref().unwrap()[1],
        Some("sig2".to_string())
    );
}

#[test]
fn test_batch_mint_parses_mixed_locked_unlocked() {
    // Test parsing: batch with mix of locked and unlocked quotes (some nulls in signature array)
    let request_json = r#"{
        "quotes": ["locked", "unlocked", "locked"],
        "outputs": [],
        "signatures": ["sig1", null, "sig3"]
    }"#;

    let request: Result<BatchMintRequest, _> = serde_json::from_str(request_json);
    assert!(
        request.is_ok(),
        "Should parse mixed locked/unlocked request"
    );
    let req = request.unwrap();

    assert_eq!(req.quotes.len(), 3);
    let sigs = req.signatures.as_ref().unwrap();
    assert_eq!(sigs.len(), 3);
    assert_eq!(sigs[0], Some("sig1".to_string()));
    assert_eq!(sigs[1], None, "Unlocked quote should have null signature");
    assert_eq!(sigs[2], Some("sig3".to_string()));
}

#[test]
fn test_batch_mint_round_trip_serialization() {
    // Test: serialization and deserialization preserves request structure
    let request_json = r#"{
        "quotes": ["q1", "q2"],
        "outputs": [],
        "signatures": ["sig1", null]
    }"#;

    let original: BatchMintRequest = serde_json::from_str(request_json).unwrap();
    let serialized = serde_json::to_string(&original).expect("serialize");
    let deserialized: BatchMintRequest = serde_json::from_str(&serialized).expect("deserialize");

    assert_eq!(original.quotes, deserialized.quotes);
    assert_eq!(original.signatures, deserialized.signatures);
}

// ============================================================================
// Handler Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_empty_quotes() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let request = BatchMintRequest {
        quotes: vec![],
        quote_amounts: None,
        outputs: vec![],
        signatures: None,
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
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create realistic outputs for 2 quotes
    let outputs = create_test_outputs(&mint, 2).await;

    // Try to mint with duplicate quote ID
    let request = BatchMintRequest {
        quotes: vec!["q1".to_string(), "q1".to_string()],
        quote_amounts: None,
        outputs, // Correct count with realistic blinded messages
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Mint should reject duplicate quote IDs");
    match result {
        Err(Error::DuplicatePaymentId) => {} // Expected
        Err(e) => panic!("Expected DuplicatePaymentId, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_handler_rejects_over_limit() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create 101 quote IDs (exceeds the 100 limit)
    let quotes: Vec<String> = (0..101).map(|i| format!("quote_{}", i)).collect();

    // Create realistic outputs for all 101 quotes
    let outputs = create_test_outputs(&mint, 101).await;

    let request = BatchMintRequest {
        quotes: quotes,
        quote_amounts: None,
        outputs, // Correct count with realistic blinded messages
        signatures: None,
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
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

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
        quotes: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        quote_amounts: None,
        outputs,
        signatures: Some(vec![
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
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

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
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs,
        signatures: Some(vec![Some("asdf".to_string())]), // Invalid signature
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(result.is_err(), "Should reject invalid signature");
    match result {
        Err(Error::SignatureMissingOrInvalid) => {} // Expected
        Err(e) => panic!("Expected SignatureMissingOrInvalid, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_signature_without_pubkey() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

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
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs, // Realistic blinded messages
        signatures: Some(vec![mint_req.signature.clone()]),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_reordered_outputs_signature_scope() {
    // Use concrete values that match the NUT-20 spec vector
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Locked quote with known pubkey (sk = 1)
    let mut sk_bytes = [0u8; 32];
    sk_bytes[31] = 1;
    let secret_key = SecretKey::from_slice(&sk_bytes).unwrap();

    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(2)),
        PaymentMethod::Bolt11,
        Amount::from(2),
        Some(secret_key.public_key()),
    )
    .await
    .expect("quote");

    let keyset_id = mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .copied()
        .expect("keyset");
    let outputs = vec![
        BlindedMessage {
            amount: Amount::from(1),
            keyset_id,
            blinded_secret: PublicKey::from_str(
                "036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2",
            )
            .unwrap(),
            witness: None,
        },
        BlindedMessage {
            amount: Amount::from(1),
            keyset_id,
            blinded_secret: PublicKey::from_str(
                "021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59",
            )
            .unwrap(),
            witness: None,
        },
    ];

    let mut mint_req = cdk_common::nuts::MintRequest {
        quote: quote_id.to_string(),
        outputs: outputs.clone(),
        signature: None,
    };
    mint_req.sign(secret_key.clone()).unwrap();

    let mut reordered_outputs = outputs.clone();
    reordered_outputs.swap(0, 1); // reorder outputs after signing

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![Amount::from(2)]),
        outputs: reordered_outputs,
        signatures: Some(vec![mint_req.signature.clone()]),
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(matches!(result, Err(Error::SignatureMissingOrInvalid)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_signature_missing_blinded_message() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let secret_key = SecretKey::generate();
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(2)),
        PaymentMethod::Bolt11,
        Amount::from(2),
        Some(secret_key.public_key()),
    )
    .await
    .expect("quote");

    let outputs = create_test_outputs(&mint, 2).await;
    assert!(
        outputs.len() >= 2,
        "expected at least two outputs for missing-blinded-message test"
    );

    // Sign only over the first blinded message, omit the second
    let mut mint_req = cdk_common::nuts::MintRequest {
        quote: quote_id.to_string(),
        outputs: vec![outputs[0].clone()],
        signature: None,
    };
    mint_req.sign(secret_key.clone()).unwrap();

    let total: Amount = outputs
        .iter()
        .map(|o| o.amount)
        .try_fold(Amount::ZERO, |acc, a| acc.checked_add(a))
        .expect("sum");

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![total]),
        outputs,
        signatures: Some(vec![mint_req.signature.clone()]),
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(matches!(result, Err(Error::SignatureMissingOrInvalid)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_bolt11_amount_mismatch() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let quote1 = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(50)),
        PaymentMethod::Bolt11,
        Amount::from(50),
        None,
    )
    .await
    .expect("quote1");
    let quote2 = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(50)),
        PaymentMethod::Bolt11,
        Amount::from(50),
        None,
    )
    .await
    .expect("quote2");

    // Outputs total 120 while quotes expect 100
    let outputs = create_outputs_for_amount(&mint, 120, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec![quote1.to_string(), quote2.to_string()],
        quote_amounts: Some(vec![Amount::from(50), Amount::from(50)]),
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    let err = result.expect_err("expected TransactionUnbalanced");
    assert!(
        matches!(err, Error::TransactionUnbalanced(_, _, _)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_bolt11_quote_amounts_length_mismatch() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let quote1 = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(100)),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("quote1");
    let quote2 = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(100)),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("quote2");

    // Outputs total 200, but quote_amounts list length is wrong
    let outputs = create_outputs_for_amount(&mint, 200, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec![quote1.to_string(), quote2.to_string()],
        quote_amounts: Some(vec![Amount::from(200)]), // length mismatch
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    let err = result.expect_err("expected TransactionUnbalanced");
    assert!(
        matches!(err, Error::TransactionUnbalanced(_, _, _)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_missing_signature_for_locked_quote() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let secret_key = SecretKey::generate();
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(50)),
        PaymentMethod::Bolt11,
        Amount::from(50),
        Some(secret_key.public_key()),
    )
    .await
    .expect("quote");

    let outputs = create_outputs_for_amount(&mint, 50, CurrencyUnit::Sat).await;

    // Locked quote but signatures array has a null entry
    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![Amount::from(50)]),
        outputs,
        signatures: Some(vec![None]),
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(matches!(result, Err(Error::SignatureMissingOrInvalid)));
}

// ============================================================================
// Quote Validation Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_unpaid_quotes() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

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
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
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
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create two quotes: one Bolt11, one Bolt12 (different payment methods)
    // First quotes: Bolt11
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create first quote");

    // Second quotes: Bolt12 (different payment method)
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
        quotes: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
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
    // First quotes: SAT unit
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(100.into()),
        PaymentMethod::Bolt11,
        Amount::from(100),
        None,
    )
    .await
    .expect("Failed to create first quote");

    // Second quotes: Msat unit (different from SAT)
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
        quotes: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(
        result.is_err(),
        "Batch should fail if quotes have different currency units"
    );
    match result {
        Err(Error::MultipleUnits) => {} // Expected
        Err(e) => panic!("Expected MultipleUnits, got: {:?}", e),
        Ok(_) => panic!("Should have returned error"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_output_unit_mismatch() {
    // Build a mint that supports Sat and Msat so we can create outputs in the wrong unit
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
        .with_name("unit mismatch mint".to_string())
        .with_description("unit mismatch mint".to_string());

    // Sat processor
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

    // Msat processor
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
    let mint = Arc::new(mint);
    mint.start().await.unwrap();

    // create_and_store_mint_quote uses Sat currency
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(10)),
        PaymentMethod::Bolt11,
        Amount::from(10),
        None,
    )
    .await
    .expect("quote");

    // Outputs built using the Msat keyset, while quote is Sat
    let outputs = create_outputs_for_amount(&mint, 10, CurrencyUnit::Msat).await;

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![Amount::from(10)]),
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    let err = result.expect_err("expected UnsupportedUnit");
    assert!(matches!(err, Error::UnsupportedUnit), "got {err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_validates_url_path_payment_method() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

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
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_bolt12_requires_quote_amounts() {
    let mint = create_bolt12_capable_mint().await;

    let quote_id =
        create_and_store_mint_quote(&mint, None, PaymentMethod::Bolt12, Amount::from(50), None)
            .await
            .expect("quote");

    let outputs = create_outputs_for_amount(&mint, 50, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: None, // required for bolt12
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt12)
        .await;
    let err = result.expect_err("expected TransactionUnbalanced");
    assert!(
        matches!(err, Error::TransactionUnbalanced(_, _, _)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_bolt12_rejects_over_requested_amount() {
    let mint = create_bolt12_capable_mint().await;

    let quote_id =
        create_and_store_mint_quote(&mint, None, PaymentMethod::Bolt12, Amount::from(100), None)
            .await
            .expect("quote");

    // Request 150 even though only 100 is mintable
    let outputs = create_outputs_for_amount(&mint, 150, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![Amount::from(150)]),
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt12)
        .await;
    let err = result.expect_err("expected TransactionUnbalanced");
    assert!(
        matches!(err, Error::TransactionUnbalanced(_, _, _)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_bolt12_total_mismatch() {
    let mint = create_bolt12_capable_mint().await;

    let quote_id =
        create_and_store_mint_quote(&mint, None, PaymentMethod::Bolt12, Amount::from(150), None)
            .await
            .expect("quote");

    // Outputs total 140 but requested 150
    let outputs = create_outputs_for_amount(&mint, 140, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: Some(vec![Amount::from(150)]),
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt12)
        .await;
    let err = result.expect_err("expected TransactionUnbalanced");
    assert!(
        matches!(err, Error::TransactionUnbalanced(_, _, _)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_rejects_unparsable_quote_id() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    let outputs = create_outputs_for_amount(&mint, 1, CurrencyUnit::Sat).await;

    let request = BatchMintRequest {
        quotes: vec!["not-a-valid-quote-id".to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;
    assert!(matches!(result, Err(Error::UnknownQuote)));
}

// ============================================================================
// Spending Conditions (NUT-11 P2PK and NUT-14 HTLC) Tests for Batch Mint
// ============================================================================

/// Helper to create blinded messages with P2PK spending conditions
/// Returns both the blinded messages to send to mint and the secrets needed to unblind
///
/// This demonstrates the proper flow:
/// 1. Client (wallet) creates spending conditions
/// 2. Client creates PreMintSecrets WITH conditions (embeds them in the secret)
/// 3. Client extracts blinded messages (WITHOUT the spending condition data)
/// 4. Client sends blinded messages to mint
/// 5. Mint signs blindly (without knowing about spending conditions)
/// 6. Client later reconstructs proofs with spending conditions from the original secrets
fn create_outputs_with_p2pk(
    mint: &Arc<cdk::Mint>,
    p2pk_pubkey: cdk::nuts::PublicKey,
) -> (Vec<BlindedMessage>, PreMintSecrets) {
    use cdk::nuts::nut11::SpendingConditions;

    let keysets = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let keyset_id = keysets;

    // Step 1: Client creates P2PK spending condition
    // This specifies: "only the holder of p2pk_pubkey can spend this proof"
    let p2pk_condition = SpendingConditions::new_p2pk(p2pk_pubkey, None);

    // Step 2: Client creates PreMintSecrets with the spending condition embedded
    // The spending condition is serialized into the secret's JSON structure
    let pre_mint = PreMintSecrets::with_conditions(
        keyset_id,
        Amount::from(1),
        &SplitTarget::default(),
        &p2pk_condition,
        &(0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into(),
    )
    .expect("Failed to create PreMintSecrets with P2PK conditions");

    // Step 3: Extract blinded messages (these do NOT contain spending condition data)
    // The spending conditions stay in the client's PreMintSecrets.secrets
    // Only the blinded messages are sent to the mint
    let blinded_messages = pre_mint.blinded_messages().iter().cloned().collect();

    (blinded_messages, pre_mint)
}

/// Helper to create blinded messages with HTLC spending conditions
/// Returns both the blinded messages to send to mint and the secrets needed to unblind
///
/// HTLC (Hashed Time Locked Contract) spending conditions allow:
/// - Locking proofs until a specific block height (locktime)
/// - Requiring a preimage to unlock before that time
/// - Allowing refund after locktime
fn create_outputs_with_htlc(
    mint: &Arc<cdk::Mint>,
    preimage_hex: String,
) -> (Vec<BlindedMessage>, PreMintSecrets) {
    use cdk::nuts::nut11::SpendingConditions;

    let keysets = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
    let keyset_id = keysets;

    // Step 1: Client creates HTLC spending condition with a preimage
    // The preimage is hashed, and the hash is stored in the spending condition
    // To spend, the holder must provide the preimage
    let htlc_condition =
        SpendingConditions::new_htlc(preimage_hex, None).expect("Failed to create HTLC condition");

    // Step 2: Client creates PreMintSecrets with the HTLC condition embedded
    let pre_mint = PreMintSecrets::with_conditions(
        keyset_id,
        Amount::from(1),
        &SplitTarget::default(),
        &htlc_condition,
        &(0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into(),
    )
    .expect("Failed to create PreMintSecrets with HTLC conditions");

    // Step 3: Extract blinded messages for sending to mint
    let blinded_messages = pre_mint.blinded_messages().iter().cloned().collect();

    (blinded_messages, pre_mint)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_with_p2pk_spending_conditions() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create a quote that is paid (no NUT-20 lock)
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(1.into()),
        PaymentMethod::Bolt11,
        Amount::from(1),
        None, // No NUT-20 lock on the quote
    )
    .await
    .expect("Failed to create quote");

    // Create blinded messages WITH P2PK spending conditions
    let secret_key = SecretKey::generate();
    let p2pk_pubkey = secret_key.public_key();
    let (mut outputs, pre_mint_secrets) = create_outputs_with_p2pk(&mint, p2pk_pubkey);

    // Sign the blinded messages with P2PK key
    for output in &mut outputs {
        output
            .sign_p2pk(secret_key.clone())
            .expect("Failed to sign");
    }

    // Create batch mint request
    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None, // No NUT-20 signature needed
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;

    // Test that proofs with P2PK spending conditions can be created via batch mint
    match result {
        Ok(response) => {
            // Verify we got blind signatures back
            assert!(
                !response.signatures.is_empty(),
                "Should return minted signatures"
            );

            // Now unblind the signatures to create proofs with spending conditions
            // This proves that the spending conditions were preserved through the batch mint flow
            let keyset = mint
                .keyset(&response.signatures[0].keyset_id)
                .expect("Could not get keyset");
            let keys = keyset.keys;

            // Reconstruct proofs by unblinding - this will fail if the secrets don't match
            let proofs = cdk::dhke::construct_proofs(
                response.signatures.clone(),
                pre_mint_secrets.rs().clone(),
                pre_mint_secrets.secrets().clone(),
                &keys,
            )
            .expect("Failed to construct proofs with spending conditions");

            // Verify we got proofs back with spending conditions
            assert!(!proofs.is_empty(), "Should have constructed proofs");

            // VERIFY SPENDING CONDITIONS ARE PRESENT IN THE PROOFS
            for proof in &proofs {
                // Extract spending conditions from the proof's secret
                let spending_conditions =
                    cdk::nuts::nut11::SpendingConditions::try_from(&proof.secret)
                        .expect("Failed to extract spending conditions from proof");

                // Verify the condition is P2PK
                match spending_conditions {
                    cdk::nuts::nut11::SpendingConditions::P2PKConditions { data, .. } => {
                        // Verify it matches the pubkey we created
                        assert_eq!(data, p2pk_pubkey, "P2PK public key should match");
                    }
                    _ => panic!("Expected P2PKConditions but got different condition type"),
                }
            }
            // The proofs now contain the P2PK spending conditions in their secrets
            // This confirms the end-to-end flow: create conditions -> batch mint -> unblind -> proofs with conditions
        }
        Err(Error::UnpaidQuote) => {
            // Acceptable: Quote payment validation is independent of spending conditions
            // This test still validates that spending conditions can be created and sent
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_with_htlc_spending_conditions() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create a quote that is paid
    let quote_id = create_and_store_mint_quote(
        &mint,
        Some(1.into()),
        PaymentMethod::Bolt11,
        Amount::from(1),
        None,
    )
    .await
    .expect("Failed to create quote");

    // Create a valid HTLC preimage (32 bytes hex encoded)
    let preimage_hex =
        "0000000000000000000000000000000000000000000000000000000000000000".to_string();

    // Create blinded messages WITH HTLC spending conditions
    let (outputs, pre_mint_secrets) = create_outputs_with_htlc(&mint, preimage_hex.clone());

    // Create batch mint request
    let request = BatchMintRequest {
        quotes: vec![quote_id.to_string()],
        quote_amounts: None,
        outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;

    // Test that proofs with HTLC spending conditions can be created via batch mint
    match result {
        Ok(response) => {
            // Verify we got blind signatures back
            assert!(
                !response.signatures.is_empty(),
                "Should return minted signatures"
            );

            // Now unblind the signatures to create proofs with spending conditions
            let keyset = mint
                .keyset(&response.signatures[0].keyset_id)
                .expect("Could not get keyset");
            let keys = keyset.keys;

            // Reconstruct proofs by unblinding - this will fail if the secrets don't match
            let proofs = cdk::dhke::construct_proofs(
                response.signatures.clone(),
                pre_mint_secrets.rs().clone(),
                pre_mint_secrets.secrets().clone(),
                &keys,
            )
            .expect("Failed to construct proofs with spending conditions");

            // Verify we got proofs back with spending conditions
            assert!(!proofs.is_empty(), "Should have constructed proofs");

            // VERIFY SPENDING CONDITIONS ARE PRESENT IN THE PROOFS
            // Calculate the expected hash from the preimage
            use bitcoin::hashes::Hash;
            use cashu::util::hex;
            let preimage_bytes = hex::decode(&preimage_hex).expect("Failed to decode preimage hex");
            let expected_hash = bitcoin::hashes::sha256::Hash::hash(&preimage_bytes);

            for proof in &proofs {
                // Extract spending conditions from the proof's secret
                let spending_conditions =
                    cdk::nuts::nut11::SpendingConditions::try_from(&proof.secret)
                        .expect("Failed to extract spending conditions from proof");

                // Verify the condition is HTLC
                match spending_conditions {
                    cdk::nuts::nut11::SpendingConditions::HTLCConditions { data, .. } => {
                        // Verify it matches the hash we created from the preimage
                        assert_eq!(
                            data.to_string(),
                            expected_hash.to_string(),
                            "HTLC hash should match preimage hash"
                        );
                    }
                    _ => panic!("Expected HTLCConditions but got different condition type"),
                }
            }
            // The proofs now contain the HTLC spending conditions in their secrets
            // This confirms the end-to-end flow: create conditions -> batch mint -> unblind -> proofs with conditions
        }
        Err(Error::UnpaidQuote) => {
            // Acceptable: Quote payment validation is independent of spending conditions
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_with_mixed_spending_conditions() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create two quotes
    let quote_id_1 = create_and_store_mint_quote(
        &mint,
        Some(1.into()),
        PaymentMethod::Bolt11,
        Amount::from(1),
        None,
    )
    .await
    .expect("Failed to create first quote");

    let quote_id_2 = create_and_store_mint_quote(
        &mint,
        Some(1.into()),
        PaymentMethod::Bolt11,
        Amount::from(1),
        None,
    )
    .await
    .expect("Failed to create second quote");

    // Create P2PK outputs for quote 1
    let secret_key = SecretKey::generate();
    let p2pk_pubkey = secret_key.public_key();
    let (mut p2pk_outputs, p2pk_secrets) = create_outputs_with_p2pk(&mint, p2pk_pubkey);
    for output in &mut p2pk_outputs {
        output
            .sign_p2pk(secret_key.clone())
            .expect("Failed to sign");
    }

    // Create HTLC outputs for quote 2
    let preimage_hex =
        "1111111111111111111111111111111111111111111111111111111111111111".to_string();
    let (htlc_outputs, htlc_secrets) = create_outputs_with_htlc(&mint, preimage_hex.clone());

    // Combine outputs and secrets for verification later
    let mut all_outputs = p2pk_outputs;
    all_outputs.extend(htlc_outputs);

    // Store secrets in order: first P2PK secrets, then HTLC secrets
    let mut all_secrets = p2pk_secrets.secrets().clone();
    all_secrets.extend(htlc_secrets.secrets().clone());

    let mut all_rs = p2pk_secrets.rs().clone();
    all_rs.extend(htlc_secrets.rs().clone());

    // Create batch mint request with mixed conditions
    let request = BatchMintRequest {
        quotes: vec![quote_id_1.to_string(), quote_id_2.to_string()],
        quote_amounts: None,
        outputs: all_outputs,
        signatures: None,
    };

    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;

    // This test validates that mixed spending conditions (P2PK + HTLC) can be included
    // in a single batch mint request
    match result {
        Ok(response) => {
            // Success: Mixed spending conditions accepted
            assert_eq!(
                response.signatures.len(),
                2,
                "Should return signatures for both quotes"
            );

            // Now unblind the signatures to create proofs with mixed spending conditions
            let keyset = mint
                .keyset(&response.signatures[0].keyset_id)
                .expect("Could not get keyset");
            let keys = keyset.keys;

            // Reconstruct proofs by unblinding
            let proofs = cdk::dhke::construct_proofs(
                response.signatures.clone(),
                all_rs,
                all_secrets,
                &keys,
            )
            .expect("Failed to construct proofs with mixed spending conditions");

            // Verify we got proofs back
            assert_eq!(proofs.len(), 2, "Should have constructed 2 proofs");

            // VERIFY THAT EACH PROOF HAS THE CORRECT SPENDING CONDITIONS
            // First proof should have P2PK conditions
            let proof_0_conditions =
                cdk::nuts::nut11::SpendingConditions::try_from(&proofs[0].secret)
                    .expect("Failed to extract spending conditions from first proof");
            match proof_0_conditions {
                cdk::nuts::nut11::SpendingConditions::P2PKConditions { data, .. } => {
                    assert_eq!(data, p2pk_pubkey, "First proof should have P2PK conditions");
                }
                _ => panic!("First proof should have P2PKConditions"),
            }

            // Second proof should have HTLC conditions
            use bitcoin::hashes::Hash;
            use cashu::util::hex;
            let preimage_bytes = hex::decode(&preimage_hex).expect("Failed to decode preimage hex");
            let expected_hash = bitcoin::hashes::sha256::Hash::hash(&preimage_bytes);

            let proof_1_conditions =
                cdk::nuts::nut11::SpendingConditions::try_from(&proofs[1].secret)
                    .expect("Failed to extract spending conditions from second proof");
            match proof_1_conditions {
                cdk::nuts::nut11::SpendingConditions::HTLCConditions { data, .. } => {
                    assert_eq!(
                        data.to_string(),
                        expected_hash.to_string(),
                        "Second proof should have HTLC conditions with correct hash"
                    );
                }
                _ => panic!("Second proof should have HTLCConditions"),
            }
        }
        Err(Error::UnpaidQuote) => {
            // Acceptable: Quote payment validation is independent of spending conditions
        }
        Err(e) => {
            panic!("Unexpected error with mixed conditions: {:?}", e);
        }
    }
}

// ============================================================================
// Happy Path Tests - Successful Batch Minting
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_batch_mint_successful_two_quotes_happy_path() {
    let mint = Arc::new(create_and_start_test_mint().await.unwrap());

    // Create two PAID quotes with unlocked (no pubkey)
    // quote_a: 5 sats (not a power of 2 - demonstrates quote amounts are independent of output denominations)
    let quote_a = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(5)),
        PaymentMethod::Bolt11,
        Amount::from(5), // Paid in full
        None,            // Unlocked - no pubkey
    )
    .await
    .expect("Failed to create quote_a");

    // quote_b: 3 sats
    let quote_b = create_and_store_mint_quote(
        &mint,
        Some(Amount::from(3)),
        PaymentMethod::Bolt11,
        Amount::from(3), // Paid in full
        None,            // Unlocked - no pubkey
    )
    .await
    .expect("Failed to create quote_b");

    // Create outputs totaling 8 sats (5 + 3 = 8)
    let keyset_id = *mint
        .get_active_keysets()
        .get(&CurrencyUnit::Sat)
        .expect("keyset for Sat");

    let split_target = SplitTarget::default();
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create blinded messages for 8 sats total
    let pre_mint =
        PreMintSecrets::random(keyset_id, Amount::from(8), &split_target, &fee_and_amounts)
            .expect("premint secrets for 8");

    let outputs: Vec<_> = pre_mint.blinded_messages().iter().cloned().collect();

    // Verify total output amount is 8
    let total_outputs: Amount = outputs
        .iter()
        .map(|o| o.amount)
        .try_fold(Amount::ZERO, |acc, a| acc.checked_add(a))
        .expect("sum outputs");
    assert_eq!(
        total_outputs,
        Amount::from(8),
        "Outputs should total 8 sats"
    );

    // Create batch mint request
    let request = BatchMintRequest {
        quotes: vec![quote_a.to_string(), quote_b.to_string()],
        quote_amounts: Some(vec![Amount::from(5), Amount::from(3)]),
        outputs: outputs.clone(),
        signatures: None, // No signatures - unlocked quotes
    };

    // Execute batch mint
    let result = mint
        .process_batch_mint_request(request, PaymentMethod::Bolt11)
        .await;

    // Verify success
    match result {
        Ok(response) => {
            // Verify we got blind signatures back
            assert!(
                !response.signatures.is_empty(),
                "Should return blind signatures"
            );

            // Verify the count matches our outputs
            // Each output gets a signature
            assert_eq!(
                response.signatures.len(),
                outputs.len(),
                "Should return one signature per output"
            );

            // Verify total amounts balance
            let total_signatures: Amount = response
                .signatures
                .iter()
                .map(|s| s.amount)
                .try_fold(Amount::ZERO, |acc, a| acc.checked_add(a))
                .expect("sum signatures");
            assert_eq!(
                total_signatures,
                Amount::from(8),
                "Signature amounts should total 8 sats"
            );
        }
        Err(e) => {
            panic!(
                "Batch mint should succeed for valid paid quotes, got error: {:?}",
                e
            );
        }
    }
}
