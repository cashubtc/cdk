//! Blind signature tests

use std::cmp::Reverse;
use std::str::FromStr;

use cashu::{Amount, BlindSignature, Id, SecretKey};

use cashu::nut00::KnownMethod;

use crate::database::mint::{Database, Error, KeysDatabase, QuoteId};
use crate::database::MintSignaturesDatabase;
use crate::mint::Operation;

/// Test adding and retrieving blind signatures
pub async fn add_and_get_blind_signatures<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let quote_id = QuoteId::new();

    // Create blinded messages and signatures
    // Use reverse key order to verify physical lock ordering preserves the
    // request order represented by order_index.
    let mut blinded_messages = [
        SecretKey::generate().public_key(),
        SecretKey::generate().public_key(),
    ];
    blinded_messages.sort_unstable_by_key(|message| Reverse(message.to_bytes()));

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
    let quote_id1 = QuoteId::new();
    let quote_id2 = QuoteId::new();

    // Keep request order opposite to physical lock order so order_index is exercised.
    let mut blinded_messages = [
        SecretKey::generate().public_key(),
        SecretKey::generate().public_key(),
    ];
    blinded_messages.sort_unstable_by_key(|message| Reverse(message.to_bytes()));
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

    // Create signature for quote 2
    let blinded_message3 = SecretKey::generate().public_key();
    let sig3 = BlindSignature {
        amount: Amount::from(300u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add signatures with different quote ids
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(
        &blinded_messages,
        &[sig1.clone(), sig2.clone()],
        Some(quote_id1.clone()),
    )
    .await
    .unwrap();
    tx.add_blind_signatures(
        &[blinded_message3],
        std::slice::from_ref(&sig3),
        Some(quote_id2.clone()),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Get signatures for quote 1
    let sigs1 = db.get_blind_signatures_for_quote(&quote_id1).await.unwrap();
    assert_eq!(sigs1.len(), 2);
    assert_eq!(sigs1[0].c, sig1.c);
    assert_eq!(sigs1[0].amount, sig1.amount);
    assert_eq!(sigs1[1].c, sig2.c);
    assert_eq!(sigs1[1].amount, sig2.amount);

    // Get signatures for quote 2
    let sigs2 = db.get_blind_signatures_for_quote(&quote_id2).await.unwrap();
    assert_eq!(sigs2.len(), 1);
    assert_eq!(sigs2[0].c, sig3.c);
    assert_eq!(sigs2[0].amount, sig3.amount);
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

/// A pre-registered blinded message signed under a different keyset must report
/// the keyset that actually signed it.
///
/// Melt change outputs are registered while their keyset is active but signed
/// after the payment settles, so a rotation in between makes the mint issue
/// them under another keyset. A row still naming the requested keyset would
/// hand wallets a signature its own keys cannot verify.
pub async fn fill_blinded_message_records_signing_keyset<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let requested_keyset = Id::from_str("001711afb1de20cb").unwrap();
    let signing_keyset = Id::from_str("00ad268c4d1f5826").unwrap();
    let quote_id = QuoteId::new();

    let blinded_message = cashu::BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id: requested_keyset,
        amount: Amount::ZERO,
        witness: None,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blinded_messages(
        Some(&quote_id),
        std::slice::from_ref(&blinded_message),
        &Operation::new_melt(
            Amount::ZERO,
            Amount::ZERO,
            cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        ),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let signature = BlindSignature {
        amount: Amount::from(64u64),
        keyset_id: signing_keyset,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(
        &[blinded_message.blinded_secret],
        std::slice::from_ref(&signature),
        Some(quote_id.clone()),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let for_quote = db.get_blind_signatures_for_quote(&quote_id).await.unwrap();
    assert_eq!(for_quote.len(), 1);
    assert_eq!(for_quote[0].keyset_id, signing_keyset);

    let by_message = db
        .get_blind_signatures(&[blinded_message.blinded_secret])
        .await
        .unwrap();
    assert_eq!(by_message[0].as_ref().unwrap().keyset_id, signing_keyset);

    assert!(db
        .get_blind_signatures_for_keyset(&requested_keyset)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        db.get_blind_signatures_for_keyset(&signing_keyset)
            .await
            .unwrap()
            .len(),
        1
    );

    let total_issued = db.get_total_issued().await.unwrap();
    assert_eq!(total_issued.get(&signing_keyset), Some(&signature.amount));
    assert_eq!(total_issued.get(&requested_keyset), None);
}
