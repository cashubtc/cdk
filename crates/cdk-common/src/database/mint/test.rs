//! Macro with default tests
//!
//! This set is generic and checks the default and expected behaviour for a mint database
//! implementation
use std::str::FromStr;

use cashu::secret::Secret;
use cashu::{Amount, CurrencyUnit, SecretKey};

use super::*;
use crate::database;
use crate::mint::MintKeySetInfo;

#[inline]
async fn setup_keyset<DB>(db: &DB) -> Id
where
    DB: KeysDatabase<Err = database::Error>,
{
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info = MintKeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        max_order: 32,
        input_fee_ppk: 0,
    };
    let mut writer = db.begin_transaction().await.expect("db.begin()");
    writer.add_keyset_info(keyset_info).await.unwrap();
    writer.commit().await.expect("commit()");
    keyset_id
}

/// Test update spend proofs fails
#[inline]
pub async fn test_update_spent_proofs<DB>(db: DB)
where
    DB: Database<database::Error> + KeysDatabase<Err = database::Error>,
{
    // Create a keyset and add it to the database
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info = MintKeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        max_order: 32,
        input_fee_ppk: 0,
        final_expiry: None,
    };
    let mut tx = KeysDatabase::begin_transaction(&db).await.expect("begin");
    tx.add_keyset_info(keyset_info).await.unwrap();
    tx.commit().await.expect("commit");

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
    tx.add_proofs(proofs.clone(), None).await.unwrap();

    // Mark one proof as spent
    tx.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
        .await
        .unwrap();

    // Try to update both proofs - should fail because one is spent
    let result = tx
        .update_proofs_states(&[proofs[0].y().unwrap()], State::Unspent)
        .await;

    tx.commit().await.unwrap();

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        Error::AttemptUpdateSpentProof
    ));

    // Verify states haven't changed
    let states = db
        .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
        .await
        .unwrap();

    assert_eq!(states.len(), 2);
    assert_eq!(states[0], Some(State::Spent));
    assert_eq!(states[1], Some(State::Unspent));
}

/// Test remove remove spent proof fails
pub async fn test_remove_spent_proofs<DB>(db: DB)
where
    DB: Database<database::Error> + KeysDatabase<Err = database::Error>,
{
    // Create a keyset and add it to the database
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info = MintKeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        max_order: 32,
        input_fee_ppk: 0,
        final_expiry: None,
    };
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info).await.unwrap();
    tx.commit().await.unwrap();

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
    tx.add_proofs(proofs.clone(), None).await.unwrap();

    // Mark one proof as spent
    tx.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
        .await
        .unwrap();

    tx.commit().await.unwrap();

    // Verify both proofs still exist
    let states = db
        .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
        .await
        .unwrap();

    assert_eq!(states.len(), 2);
    assert_eq!(states[0], Some(State::Spent));
    assert_eq!(states[1], Some(State::Unspent));
}

/// State transition test
pub async fn state_transition<DB>(db: DB)
where
    DB: Database<database::Error> + KeysDatabase<Err = database::Error>,
{
    let keyset_id = setup_keyset(&db).await;

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
    tx.add_proofs(proofs.clone(), None).await.unwrap();

    // Mark one proof as `pending`
    assert!(tx
        .update_proofs_states(&[proofs[0].y().unwrap()], State::Pending)
        .await
        .is_ok());

    // Attempt to select the `pending` proof, as `pending` again (which should fail)
    assert!(tx
        .update_proofs_states(&[proofs[0].y().unwrap()], State::Pending)
        .await
        .is_err());
    tx.commit().await.unwrap();
}

/// Unit test that is expected to be passed for a correct database implementation
#[macro_export]
macro_rules! mint_db_test {
    ($make_db_fn:ident) => {
        mint_db_test!(state_transition, $make_db_fn);
        mint_db_test!(test_update_spent_proofs, $make_db_fn);
        mint_db_test!(test_remove_spent_proofs, $make_db_fn);
    };
    ($name:ident, $make_db_fn:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 10)]
        async fn $name() {
            $crate::database::mint::test::$name($make_db_fn().await).await;
        }
    };
}
