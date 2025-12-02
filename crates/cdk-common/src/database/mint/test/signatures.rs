//! Blind signature tests

use std::str::FromStr;

use cashu::{Amount, BlindSignature, Id, SecretKey};

use crate::database::mint::{Database, Error, KeysDatabase, QuoteId};
use crate::database::MintSignaturesDatabase;

/// Test adding and retrieving blind signatures
pub async fn add_and_get_blind_signatures<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let quote_id = QuoteId::new_uuid();

    // Create blinded messages and signatures
    let blinded_message1 = SecretKey::generate().public_key();
    let blinded_message2 = SecretKey::generate().public_key();
    let blinded_messages = vec![blinded_message1, blinded_message2];

    let sig1 = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    let sig2 = BlindSignature {
        amount: Amount::from(200u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    let signatures = vec![sig1.clone(), sig2.clone()];

    // Add blind signatures
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(&blinded_messages, &signatures, Some(quote_id))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Retrieve blind signatures
    let retrieved = db.get_blind_signatures(&blinded_messages).await.unwrap();
    assert_eq!(retrieved.len(), 2);
    assert!(retrieved[0].is_some());
    assert!(retrieved[1].is_some());

    let retrieved_sig1 = retrieved[0].as_ref().unwrap();
    let retrieved_sig2 = retrieved[1].as_ref().unwrap();
    assert_eq!(retrieved_sig1.amount, sig1.amount);
    assert_eq!(retrieved_sig1.c, sig1.c);
    assert_eq!(retrieved_sig2.amount, sig2.amount);
    assert_eq!(retrieved_sig2.c, sig2.c);
}

/// Test getting blind signatures for a specific keyset
pub async fn get_blind_signatures_for_keyset<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id1 = Id::from_str("001711afb1de20cb").unwrap();
    let keyset_id2 = Id::from_str("002811afb1de20cb").unwrap();

    // Create signatures for keyset 1
    let blinded_message1 = SecretKey::generate().public_key();
    let sig1 = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id: keyset_id1,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Create signatures for keyset 2
    let blinded_message2 = SecretKey::generate().public_key();
    let sig2 = BlindSignature {
        amount: Amount::from(200u64),
        keyset_id: keyset_id2,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add both signatures
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(&[blinded_message1], std::slice::from_ref(&sig1), None)
        .await
        .unwrap();
    tx.add_blind_signatures(&[blinded_message2], std::slice::from_ref(&sig2), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get signatures for keyset 1
    let sigs1 = db
        .get_blind_signatures_for_keyset(&keyset_id1)
        .await
        .unwrap();
    assert!(sigs1.iter().any(|s| s.c == sig1.c));
    assert!(!sigs1.iter().any(|s| s.c == sig2.c));

    // Get signatures for keyset 2
    let sigs2 = db
        .get_blind_signatures_for_keyset(&keyset_id2)
        .await
        .unwrap();
    assert!(!sigs2.iter().any(|s| s.c == sig1.c));
    assert!(sigs2.iter().any(|s| s.c == sig2.c));
}

/// Test getting blind signatures for a specific quote
pub async fn get_blind_signatures_for_quote<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let quote_id1 = QuoteId::new_uuid();
    let quote_id2 = QuoteId::new_uuid();

    // Create signatures for quote 1
    let blinded_message1 = SecretKey::generate().public_key();
    let sig1 = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Create signatures for quote 2
    let blinded_message2 = SecretKey::generate().public_key();
    let sig2 = BlindSignature {
        amount: Amount::from(200u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add signatures with different quote ids
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(
        &[blinded_message1],
        std::slice::from_ref(&sig1),
        Some(quote_id1.clone()),
    )
    .await
    .unwrap();
    tx.add_blind_signatures(
        &[blinded_message2],
        std::slice::from_ref(&sig2),
        Some(quote_id2.clone()),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Get signatures for quote 1
    let sigs1 = db.get_blind_signatures_for_quote(&quote_id1).await.unwrap();
    assert_eq!(sigs1.len(), 1);
    assert_eq!(sigs1[0].c, sig1.c);
    assert_eq!(sigs1[0].amount, sig1.amount);

    // Get signatures for quote 2
    let sigs2 = db.get_blind_signatures_for_quote(&quote_id2).await.unwrap();
    assert_eq!(sigs2.len(), 1);
    assert_eq!(sigs2[0].c, sig2.c);
    assert_eq!(sigs2[0].amount, sig2.amount);
}

/// Test getting total issued by keyset
pub async fn get_total_issued<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create multiple signatures
    let blinded_message1 = SecretKey::generate().public_key();
    let blinded_message2 = SecretKey::generate().public_key();
    let blinded_message3 = SecretKey::generate().public_key();

    let sig1 = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    let sig2 = BlindSignature {
        amount: Amount::from(200u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    let sig3 = BlindSignature {
        amount: Amount::from(300u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add signatures
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(&[blinded_message1], &[sig1], None)
        .await
        .unwrap();
    tx.add_blind_signatures(&[blinded_message2], &[sig2], None)
        .await
        .unwrap();
    tx.add_blind_signatures(&[blinded_message3], &[sig3], None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get total issued
    let totals = db.get_total_issued().await.unwrap();
    let total = totals.get(&keyset_id).copied().unwrap_or(Amount::ZERO);

    // Should be 600 (100 + 200 + 300)
    assert!(total >= Amount::from(600));
}

/// Test retrieving non-existent blind signatures
pub async fn get_nonexistent_blind_signatures<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let blinded_message = SecretKey::generate().public_key();

    // Try to retrieve non-existent signature
    let retrieved = db.get_blind_signatures(&[blinded_message]).await.unwrap();
    assert_eq!(retrieved.len(), 1);
    assert!(retrieved[0].is_none());
}

/// Test adding duplicate blind signatures fails
pub async fn add_duplicate_blind_signatures<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let blinded_message = SecretKey::generate().public_key();

    let sig = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add signature first time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(&[blinded_message], std::slice::from_ref(&sig), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Try to add duplicate - should fail
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx
        .add_blind_signatures(&[blinded_message], std::slice::from_ref(&sig), None)
        .await;
    assert!(result.is_err());
    tx.rollback().await.unwrap();
}
