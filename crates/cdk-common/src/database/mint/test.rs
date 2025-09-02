//! Macro with default tests
//!
//! This set is generic and checks the default and expected behaviour for a mint database
//! implementation
use std::str::FromStr;

// For derivation path parsing
use bitcoin::bip32::DerivationPath;
use cashu::secret::Secret;
use cashu::{Amount, CurrencyUnit, SecretKey};

use super::*;
use crate::database::MintKVStoreDatabase;
use crate::mint::MintKeySetInfo;

#[inline]
async fn setup_keyset<DB>(db: &DB) -> Id
where
    DB: KeysDatabase<Err = crate::database::Error>,
{
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info = MintKeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        max_order: 32,
        input_fee_ppk: 0,
    };
    let mut writer = db.begin_transaction().await.expect("db.begin()");
    writer.add_keyset_info(keyset_info).await.unwrap();
    writer.commit().await.expect("commit()");
    keyset_id
}

/// State transition test
pub async fn state_transition<DB>(db: DB)
where
    DB: Database<crate::database::Error> + KeysDatabase<Err = crate::database::Error>,
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

/// Test the basic storing and retrieving proofs from the database. Probably the database would use
/// binary/`Vec<u8>` to store data, that's why this test would quickly identify issues before running
/// other tests
pub async fn add_and_find_proofs<DB>(db: DB)
where
    DB: Database<crate::database::Error> + KeysDatabase<Err = crate::database::Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let quote_id = Uuid::max();

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
    tx.add_proofs(proofs.clone(), Some(quote_id)).await.unwrap();
    assert!(tx.commit().await.is_ok());

    let proofs_from_db = db.get_proofs_by_ys(&[proofs[0].c, proofs[1].c]).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);

    let proofs_from_db = db.get_proof_ys_by_quote_id(&quote_id).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);
}

/// Test KV store functionality including write, read, list, update, and remove operations
pub async fn kvstore_functionality<DB>(db: DB)
where
    DB: Database<crate::database::Error> + MintKVStoreDatabase<Err = crate::database::Error>,
{
    // Test basic read/write operations in transaction
    {
        let mut tx = Database::begin_transaction(&db).await.unwrap();

        // Write some test data
        tx.kv_write("test_namespace", "sub_namespace", "key1", b"value1")
            .await
            .unwrap();
        tx.kv_write("test_namespace", "sub_namespace", "key2", b"value2")
            .await
            .unwrap();
        tx.kv_write("test_namespace", "other_sub", "key3", b"value3")
            .await
            .unwrap();

        // Read back the data in the transaction
        let value1 = tx
            .kv_read("test_namespace", "sub_namespace", "key1")
            .await
            .unwrap();
        assert_eq!(value1, Some(b"value1".to_vec()));

        // List keys in namespace
        let keys = tx.kv_list("test_namespace", "sub_namespace").await.unwrap();
        assert_eq!(keys, vec!["key1", "key2"]);

        // Commit transaction
        tx.commit().await.unwrap();
    }

    // Test read operations after commit
    {
        let value1 = db
            .kv_read("test_namespace", "sub_namespace", "key1")
            .await
            .unwrap();
        assert_eq!(value1, Some(b"value1".to_vec()));

        let keys = db.kv_list("test_namespace", "sub_namespace").await.unwrap();
        assert_eq!(keys, vec!["key1", "key2"]);

        let other_keys = db.kv_list("test_namespace", "other_sub").await.unwrap();
        assert_eq!(other_keys, vec!["key3"]);
    }

    // Test update and remove operations
    {
        let mut tx = Database::begin_transaction(&db).await.unwrap();

        // Update existing key
        tx.kv_write("test_namespace", "sub_namespace", "key1", b"updated_value1")
            .await
            .unwrap();

        // Remove a key
        tx.kv_remove("test_namespace", "sub_namespace", "key2")
            .await
            .unwrap();

        tx.commit().await.unwrap();
    }

    // Verify updates
    {
        let value1 = db
            .kv_read("test_namespace", "sub_namespace", "key1")
            .await
            .unwrap();
        assert_eq!(value1, Some(b"updated_value1".to_vec()));

        let value2 = db
            .kv_read("test_namespace", "sub_namespace", "key2")
            .await
            .unwrap();
        assert_eq!(value2, None);

        let keys = db.kv_list("test_namespace", "sub_namespace").await.unwrap();
        assert_eq!(keys, vec!["key1"]);
    }
}

/// Unit test that is expected to be passed for a correct database implementation
#[macro_export]
macro_rules! mint_db_test {
    ($make_db_fn:ident) => {
        mint_db_test!(state_transition, $make_db_fn);
        mint_db_test!(add_and_find_proofs, $make_db_fn);
        mint_db_test!(kvstore_functionality, $make_db_fn);
    };
    ($name:ident, $make_db_fn:ident) => {
        #[tokio::test]
        async fn $name() {
            cdk_common::database::mint::test::$name($make_db_fn().await).await;
        }
    };
}
