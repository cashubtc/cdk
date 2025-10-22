//! Proofs tests

use std::str::FromStr;

use cashu::secret::Secret;
use cashu::{Amount, Id, SecretKey};

use crate::database::mint::test::setup_keyset;
use crate::database::mint::{Database, Error, KeysDatabase, Proof, QuoteId};
use crate::mint::Operation;

/// Test get proofs by keyset id
pub async fn get_proofs_by_keyset_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();
    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(proofs, Some(quote_id), &Operation::new_swap())
        .await
        .unwrap();
    assert!(tx.commit().await.is_ok());

    let (proofs, states) = db.get_proofs_by_keyset_id(&keyset_id).await.unwrap();
    assert_eq!(proofs.len(), 2);
    assert_eq!(proofs.len(), states.len());
    assert_eq!(
        states
            .into_iter()
            .map(|s| s.map(|x| x.to_string()).unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["UNSPENT".to_owned(), "UNSPENT".to_owned()]
    );

    let keyset_id = Id::from_str("00916bbf7ef91a34").unwrap();
    let (proofs, states) = db.get_proofs_by_keyset_id(&keyset_id).await.unwrap();
    assert_eq!(proofs.len(), 0);
    assert_eq!(proofs.len(), states.len());
}

/// Test the basic storing and retrieving proofs from the database. Probably the database would use
/// binary/`Vec<u8>` to store data, that's why this test would quickly identify issues before running
/// other tests
pub async fn add_and_find_proofs<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(),
    )
    .await
    .unwrap();
    assert!(tx.commit().await.is_ok());

    let proofs_from_db = db.get_proofs_by_ys(&[proofs[0].c, proofs[1].c]).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);

    let proofs_from_db = db.get_proof_ys_by_quote_id(&quote_id).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);
}

/// Test to add duplicate proofs
pub async fn add_duplicate_proofs<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(),
    )
    .await
    .unwrap();
    assert!(tx.commit().await.is_ok());

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx
        .add_proofs(
            proofs.clone(),
            Some(quote_id.clone()),
            &Operation::new_swap(),
        )
        .await;

    assert!(
        matches!(result.unwrap_err(), Error::Duplicate),
        "Duplicate entry"
    );
}
