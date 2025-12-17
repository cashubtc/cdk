//! Wallet Database Tests
//!
//! This module contains generic tests for wallet database implementations.
//! These tests can be used to verify any wallet database implementation
//! by using the `wallet_db_test!` macro.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cashu::secret::Secret;
use cashu::{Amount, CurrencyUnit, PaymentMethod, SecretKey};

use super::*;
use crate::common::ProofInfo;
use crate::mint_url::MintUrl;
use crate::nuts::{Id, KeySetInfo, Keys, MintInfo, Proof, State};
use crate::wallet::{MeltQuote, MintQuote, Transaction, TransactionDirection};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique test ID
fn unique_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("test_{}_{}", now, n)
}

/// Generate valid test keys and return both the keys and the matching keyset ID.
/// The keyset ID is derived from the keys using the v1 algorithm:
fn test_keys_with_id() -> (Keys, Id) {
    // Generate deterministic keys for amounts 1, 2, 4, 8
    let mut keys_map = BTreeMap::new();

    // Use deterministic secret keys for reproducibility
    let secret_bytes: [[u8; 32]; 4] = [[1u8; 32], [2u8; 32], [4u8; 32], [8u8; 32]];

    for (i, amount) in [1u64, 2, 4, 8].iter().enumerate() {
        let sk = SecretKey::from_slice(&secret_bytes[i]).expect("valid secret key");
        let pk = sk.public_key();
        keys_map.insert(Amount::from(*amount), pk);
    }

    let keys = Keys::new(keys_map);
    let id = Id::v1_from_keys(&keys);

    (keys, id)
}

/// Generate a unique test keyset ID
fn test_keyset_id() -> Id {
    Id::from_str("00916bbf7ef91a36").unwrap()
}

/// Generate a second test keyset ID
fn test_keyset_id_2() -> Id {
    Id::from_str("00916bbf7ef91a37").unwrap()
}

/// Create a test mint URL
fn test_mint_url() -> MintUrl {
    MintUrl::from_str("https://test-mint.example.com").unwrap()
}

/// Create a second test mint URL
fn test_mint_url_2() -> MintUrl {
    MintUrl::from_str("https://test-mint-2.example.com").unwrap()
}

/// Create test keyset info
fn test_keyset_info(keyset_id: Id, _mint_url: &MintUrl) -> KeySetInfo {
    KeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        input_fee_ppk: 0,
        final_expiry: None,
    }
}

/// Create a test proof
fn test_proof(keyset_id: Id, amount: u64) -> Proof {
    Proof {
        amount: Amount::from(amount),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
    }
}

/// Create test proof info
fn test_proof_info(keyset_id: Id, amount: u64, mint_url: MintUrl) -> ProofInfo {
    let proof = test_proof(keyset_id, amount);
    ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap()
}

/// Create a test mint quote
fn test_mint_quote(mint_url: MintUrl) -> MintQuote {
    MintQuote::new(
        unique_id(),
        mint_url,
        PaymentMethod::Bolt11,
        Some(Amount::from(1000)),
        CurrencyUnit::Sat,
        "lnbc1000...".to_string(),
        9999999999,
        None,
    )
}

/// Create a test melt quote
fn test_melt_quote() -> MeltQuote {
    MeltQuote {
        id: unique_id(),
        unit: CurrencyUnit::Sat,
        amount: Amount::from(1000),
        request: "lnbc1000...".to_string(),
        fee_reserve: Amount::from(10),
        state: cashu::MeltQuoteState::Unpaid,
        expiry: 9999999999,
        payment_preimage: None,
        payment_method: PaymentMethod::Bolt11,
    }
}

/// Create a test transaction
fn test_transaction(mint_url: MintUrl, direction: TransactionDirection) -> Transaction {
    let ys = vec![SecretKey::generate().public_key()];
    Transaction {
        mint_url,
        direction,
        amount: Amount::from(100),
        fee: Amount::from(1),
        unit: CurrencyUnit::Sat,
        ys,
        timestamp: 1234567890,
        memo: Some("test transaction".to_string()),
        metadata: HashMap::new(),
        quote_id: None,
        payment_request: None,
        payment_proof: None,
        payment_method: None,
    }
}

// =============================================================================
// Mint Management Tests
// =============================================================================

/// Test adding and retrieving a mint
pub async fn add_and_get_mint<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let mint_info = MintInfo::default();

    // Add mint
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), Some(mint_info.clone()))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get mint
    let retrieved = db.get_mint(mint_url.clone()).await.unwrap();
    assert!(retrieved.is_some());

    // Get all mints
    let mints = db.get_mints().await.unwrap();
    assert!(mints.contains_key(&mint_url));
}

/// Test adding mint without info
pub async fn add_mint_without_info<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();

    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), None).await.unwrap();
    tx.commit().await.unwrap();

    // Verify mint exists in the database
    let mints = db.get_mints().await.unwrap();
    assert!(mints.contains_key(&mint_url));
}

/// Test removing a mint
pub async fn remove_mint<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();

    // Add mint
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), None).await.unwrap();
    tx.commit().await.unwrap();

    // Remove mint
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.remove_mint(mint_url.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let result = db.get_mint(mint_url).await.unwrap();
    assert!(result.is_none());
}

/// Test updating mint URL
pub async fn update_mint_url<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let old_url = test_mint_url();
    let new_url = test_mint_url_2();

    // Add mint with old URL
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(old_url.clone(), None).await.unwrap();
    tx.commit().await.unwrap();

    // Update URL
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_mint_url(old_url.clone(), new_url.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

// =============================================================================
// Keyset Management Tests
// =============================================================================

/// Test adding and retrieving keysets
pub async fn add_and_get_keysets<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let keyset_info = test_keyset_info(keyset_id, &mint_url);

    // Add mint first
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), None).await.unwrap();
    tx.add_mint_keysets(mint_url.clone(), vec![keyset_info.clone()])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get keyset by ID
    let retrieved = db.get_keyset_by_id(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, keyset_id);

    // Get keysets for mint
    let keysets = db.get_mint_keysets(mint_url).await.unwrap();
    assert!(keysets.is_some());
    assert!(!keysets.unwrap().is_empty());
}

/// Test getting keyset by ID in transaction
pub async fn get_keyset_by_id_in_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let keyset_info = test_keyset_info(keyset_id, &mint_url);

    // Add keyset
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), None).await.unwrap();
    tx.add_mint_keysets(mint_url.clone(), vec![keyset_info])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get in transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    let retrieved = tx.get_keyset_by_id(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
    tx.rollback().await.unwrap();
}

/// Test adding and retrieving keys
pub async fn add_and_get_keys<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    // Generate valid keys with matching keyset ID
    let (keys, keyset_id) = test_keys_with_id();
    let keyset = cashu::KeySet {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        keys: keys.clone(),
        final_expiry: None,
    };

    // Add keys
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_keys(keyset).await.unwrap();
    tx.commit().await.unwrap();

    // Get keys
    let retrieved = db.get_keys(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved_keys = retrieved.unwrap();
    assert_eq!(retrieved_keys.len(), keys.len());
}

/// Test getting keys in transaction
pub async fn get_keys_in_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    // Generate valid keys with matching keyset ID
    let (keys, keyset_id) = test_keys_with_id();
    let keyset = cashu::KeySet {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        keys: keys.clone(),
        final_expiry: None,
    };

    // Add keys
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_keys(keyset).await.unwrap();
    tx.commit().await.unwrap();

    // Get in transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    let retrieved = tx.get_keys(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved_keys = retrieved.unwrap();
    assert_eq!(retrieved_keys.len(), keys.len());
    tx.rollback().await.unwrap();
}

/// Test removing keys
pub async fn remove_keys<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    // Generate valid keys with matching keyset ID
    let (keys, keyset_id) = test_keys_with_id();
    let keyset = cashu::KeySet {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        keys,
        final_expiry: None,
    };

    // Add keys
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_keys(keyset).await.unwrap();
    tx.commit().await.unwrap();

    // Verify keys were added
    let retrieved = db.get_keys(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());

    // Remove keys
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.remove_keys(&keyset_id).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_keys(&keyset_id).await.unwrap();
    assert!(retrieved.is_none());
}

// =============================================================================
// Mint Quote Tests
// =============================================================================

/// Test adding and retrieving mint quotes
pub async fn add_and_get_mint_quote<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let quote = test_mint_quote(mint_url);

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get quote
    let retrieved = db.get_mint_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, quote.id);

    // Get all quotes
    let quotes = db.get_mint_quotes().await.unwrap();
    assert!(!quotes.is_empty());
}

/// Test getting mint quote in transaction
pub async fn get_mint_quote_in_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let quote = test_mint_quote(mint_url);

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get in transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    let retrieved = tx.get_mint_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_some());
    tx.rollback().await.unwrap();
}

/// Test removing mint quote
pub async fn remove_mint_quote<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let quote = test_mint_quote(mint_url);

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Remove quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.remove_mint_quote(&quote.id).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_mint_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_none());
}

// =============================================================================
// Melt Quote Tests
// =============================================================================

/// Test adding and retrieving melt quotes
pub async fn add_and_get_melt_quote<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let quote = test_melt_quote();

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get quote
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, quote.id);

    // Get all quotes
    let quotes = db.get_melt_quotes().await.unwrap();
    assert!(!quotes.is_empty());
}

/// Test getting melt quote in transaction
pub async fn get_melt_quote_in_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let quote = test_melt_quote();

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get in transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    let retrieved = tx.get_melt_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_some());
    tx.rollback().await.unwrap();
}

/// Test removing melt quote
pub async fn remove_melt_quote<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let quote = test_melt_quote();

    // Add quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Remove quote
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.remove_melt_quote(&quote.id).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_quote(&quote.id).await.unwrap();
    assert!(retrieved.is_none());
}

// =============================================================================
// Proof Management Tests
// =============================================================================

/// Test adding and retrieving proofs
pub async fn add_and_get_proofs<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get proofs
    let proofs = db.get_proofs(None, None, None, None).await.unwrap();
    assert!(!proofs.is_empty());

    // Get proofs by mint URL
    let proofs = db
        .get_proofs(Some(mint_url.clone()), None, None, None)
        .await
        .unwrap();
    assert!(!proofs.is_empty());

    // Get proofs by Y
    let ys = vec![proof_info.y];
    let proofs = db.get_proofs_by_ys(ys).await.unwrap();
    assert!(!proofs.is_empty());
}

/// Test getting proofs in transaction
pub async fn get_proofs_in_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get proofs in transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    let proofs = tx.get_proofs(None, None, None, None).await.unwrap();
    assert!(!proofs.is_empty());
    tx.rollback().await.unwrap();
}

/// Test updating proofs (add and remove)
pub async fn update_proofs<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info_1 = test_proof_info(keyset_id, 100, mint_url.clone());
    let proof_info_2 = test_proof_info(keyset_id, 200, mint_url.clone());

    // Add first proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info_1.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Add second, remove first
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info_2.clone()], vec![proof_info_1.y])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify
    let proofs = db.get_proofs(None, None, None, None).await.unwrap();
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].y, proof_info_2.y);
}

/// Test updating proofs state
pub async fn update_proofs_state<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Update state
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs_state(vec![proof_info.y], State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify
    let proofs = db
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(!proofs.is_empty());
}

/// Test filtering proofs by unit
pub async fn filter_proofs_by_unit<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Filter by unit
    let proofs = db
        .get_proofs(None, Some(CurrencyUnit::Sat), None, None)
        .await
        .unwrap();
    assert!(!proofs.is_empty());

    // Filter by different unit
    let proofs = db
        .get_proofs(None, Some(CurrencyUnit::Msat), None, None)
        .await
        .unwrap();
    assert!(proofs.is_empty());
}

/// Test filtering proofs by state
pub async fn filter_proofs_by_state<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Filter by state
    let proofs = db
        .get_proofs(None, None, Some(vec![State::Unspent]), None)
        .await
        .unwrap();
    assert!(!proofs.is_empty());

    // Filter by different state
    let proofs = db
        .get_proofs(None, None, Some(vec![State::Spent]), None)
        .await
        .unwrap();
    assert!(proofs.is_empty());
}

// =============================================================================
// Balance Tests
// =============================================================================

/// Test getting balance
pub async fn get_balance<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info_1 = test_proof_info(keyset_id, 100, mint_url.clone());
    let proof_info_2 = test_proof_info(keyset_id, 200, mint_url.clone());

    // Add proofs
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info_1, proof_info_2], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get total balance
    let balance = db.get_balance(None, None, None).await.unwrap();
    assert_eq!(balance, 300);

    // Get balance by mint
    let balance = db.get_balance(Some(mint_url), None, None).await.unwrap();
    assert_eq!(balance, 300);
}

/// Test getting balance by state
pub async fn get_balance_by_state<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());

    // Add proof
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info.clone()], vec![])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get balance by state
    let balance = db
        .get_balance(None, None, Some(vec![State::Unspent]))
        .await
        .unwrap();
    assert_eq!(balance, 100);

    // Get balance by different state
    let balance = db
        .get_balance(None, None, Some(vec![State::Spent]))
        .await
        .unwrap();
    assert_eq!(balance, 0);
}

// =============================================================================
// Keyset Counter Tests
// =============================================================================

/// Test incrementing keyset counter
pub async fn increment_keyset_counter<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let keyset_id = test_keyset_id();

    // Increment counter
    let mut tx = db.begin_db_transaction().await.unwrap();
    let counter1 = tx.increment_keyset_counter(&keyset_id, 5).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(counter1, 5);

    // Increment again
    let mut tx = db.begin_db_transaction().await.unwrap();
    let counter2 = tx.increment_keyset_counter(&keyset_id, 10).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(counter2, 15);
}

/// Test keyset counter isolation between keysets
pub async fn keyset_counter_isolation<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let keyset_id_1 = test_keyset_id();
    let keyset_id_2 = test_keyset_id_2();

    // Increment first keyset
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.increment_keyset_counter(&keyset_id_1, 5).await.unwrap();
    tx.commit().await.unwrap();

    // Increment second keyset
    let mut tx = db.begin_db_transaction().await.unwrap();
    let counter2 = tx.increment_keyset_counter(&keyset_id_2, 10).await.unwrap();
    tx.commit().await.unwrap();

    // Second keyset should start from 0
    assert_eq!(counter2, 10);

    // First keyset should still be at 5
    let mut tx = db.begin_db_transaction().await.unwrap();
    let counter1 = tx.increment_keyset_counter(&keyset_id_1, 0).await.unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(counter1, 5);
}

// =============================================================================
// Transaction Tests
// =============================================================================

/// Test adding and retrieving transactions
pub async fn add_and_get_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let transaction = test_transaction(mint_url.clone(), TransactionDirection::Incoming);
    let tx_id = transaction.id();

    // Add transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_transaction(transaction.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get transaction
    let retrieved = db.get_transaction(tx_id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id(), tx_id);
}

/// Test listing transactions
pub async fn list_transactions<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let tx_incoming = test_transaction(mint_url.clone(), TransactionDirection::Incoming);
    let tx_outgoing = test_transaction(mint_url.clone(), TransactionDirection::Outgoing);

    // Add transactions
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_transaction(tx_incoming).await.unwrap();
    tx.add_transaction(tx_outgoing).await.unwrap();
    tx.commit().await.unwrap();

    // List all
    let transactions = db.list_transactions(None, None, None).await.unwrap();
    assert_eq!(transactions.len(), 2);

    // List by direction
    let incoming = db
        .list_transactions(None, Some(TransactionDirection::Incoming), None)
        .await
        .unwrap();
    assert_eq!(incoming.len(), 1);

    let outgoing = db
        .list_transactions(None, Some(TransactionDirection::Outgoing), None)
        .await
        .unwrap();
    assert_eq!(outgoing.len(), 1);
}

/// Test filtering transactions by mint
pub async fn filter_transactions_by_mint<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url_1 = test_mint_url();
    let mint_url_2 = test_mint_url_2();
    let tx_1 = test_transaction(mint_url_1.clone(), TransactionDirection::Incoming);
    let tx_2 = test_transaction(mint_url_2.clone(), TransactionDirection::Incoming);

    // Add transactions
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_transaction(tx_1).await.unwrap();
    tx.add_transaction(tx_2).await.unwrap();
    tx.commit().await.unwrap();

    // Filter by mint
    let transactions = db
        .list_transactions(Some(mint_url_1), None, None)
        .await
        .unwrap();
    assert_eq!(transactions.len(), 1);
}

/// Test removing transaction
pub async fn remove_transaction<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let transaction = test_transaction(mint_url, TransactionDirection::Incoming);
    let tx_id = transaction.id();

    // Add transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_transaction(transaction).await.unwrap();
    tx.commit().await.unwrap();

    // Remove transaction
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.remove_transaction(tx_id).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_transaction(tx_id).await.unwrap();
    assert!(retrieved.is_none());
}

// =============================================================================
// Transaction Rollback Tests
// =============================================================================

/// Test transaction rollback
pub async fn transaction_rollback<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();

    // Add mint but rollback
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.add_mint(mint_url.clone(), None).await.unwrap();
    tx.rollback().await.unwrap();

    // Verify mint was not added
    let result = db.get_mint(mint_url).await.unwrap();
    assert!(result.is_none());
}

/// Test proof rollback
pub async fn proof_rollback<DB>(db: DB)
where
    DB: Database<crate::database::Error>,
{
    let mint_url = test_mint_url();
    let keyset_id = test_keyset_id();
    let proof_info = test_proof_info(keyset_id, 100, mint_url);

    // Add proof but rollback
    let mut tx = db.begin_db_transaction().await.unwrap();
    tx.update_proofs(vec![proof_info], vec![]).await.unwrap();
    tx.rollback().await.unwrap();

    // Verify proof was not added
    let proofs = db.get_proofs(None, None, None, None).await.unwrap();
    assert!(proofs.is_empty());
}

/// Unit test that is expected to be passed for a correct wallet database implementation
#[macro_export]
macro_rules! wallet_db_test {
    ($make_db_fn:ident) => {
        wallet_db_test!(
            $make_db_fn,
            add_and_get_mint,
            add_mint_without_info,
            remove_mint,
            update_mint_url,
            add_and_get_keysets,
            get_keyset_by_id_in_transaction,
            add_and_get_keys,
            get_keys_in_transaction,
            remove_keys,
            add_and_get_mint_quote,
            get_mint_quote_in_transaction,
            remove_mint_quote,
            add_and_get_melt_quote,
            get_melt_quote_in_transaction,
            remove_melt_quote,
            add_and_get_proofs,
            get_proofs_in_transaction,
            update_proofs,
            update_proofs_state,
            filter_proofs_by_unit,
            filter_proofs_by_state,
            get_balance,
            get_balance_by_state,
            increment_keyset_counter,
            keyset_counter_isolation,
            add_and_get_transaction,
            list_transactions,
            filter_transactions_by_mint,
            remove_transaction,
            transaction_rollback,
            proof_rollback
        );
    };
    ($make_db_fn:ident, $($name:ident),+ $(,)?) => {
        ::paste::paste! {
            $(
                #[tokio::test]
                async fn [<wallet_ $name>]() {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Time went backwards");

                    cdk_common::database::wallet::test::$name($make_db_fn(format!("test_{}_{}", now.as_nanos(), stringify!($name))).await).await;
                }
            )+
        }
    };
}
