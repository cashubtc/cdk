//! Keys database tests

use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use cashu::{CurrencyUnit, Id};

use crate::database::mint::{Database, Error, KeysDatabase};
use crate::mint::MintKeySetInfo;

/// Generate standard keyset amounts as powers of 2
fn standard_keyset_amounts(max_order: u32) -> Vec<u64> {
    (0..max_order).map(|n| 2u64.pow(n)).collect()
}

/// Test adding and retrieving keyset info
pub async fn add_and_get_keyset_info<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
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
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add keyset info
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Retrieve keyset info
    let retrieved = db.get_keyset_info(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, keyset_info.id);
    assert_eq!(retrieved.unit, keyset_info.unit);
    assert_eq!(retrieved.active, keyset_info.active);
    assert_eq!(retrieved.amounts, keyset_info.amounts);
}

/// Test adding duplicate keyset info is idempotent
pub async fn add_duplicate_keyset_info<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
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
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add keyset info first time
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Add the same keyset info again - this may succeed (idempotent) or fail
    // Both behaviors are acceptable
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    let result = tx.add_keyset_info(keyset_info).await;
    assert!(result.is_ok());
    tx.commit().await.unwrap();

    // Verify keyset still exists
    let retrieved = db.get_keyset_info(&keyset_id).await.unwrap();
    assert!(retrieved.is_some());
}

/// Test getting all keyset infos
pub async fn get_all_keyset_infos<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id1 = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info1 = MintKeySetInfo {
        id: keyset_id1,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    let keyset_id2 = Id::from_str("00916bbf7ef91a37").unwrap();
    let keyset_info2 = MintKeySetInfo {
        id: keyset_id2,
        unit: CurrencyUnit::Sat,
        active: false,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/1'").unwrap(),
        derivation_path_index: Some(1),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add keyset infos
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info1.clone()).await.unwrap();
    tx.add_keyset_info(keyset_info2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get all keyset infos
    let all_keysets = db.get_keyset_infos().await.unwrap();
    assert!(all_keysets.len() >= 2);
    assert!(all_keysets.iter().any(|k| k.id == keyset_id1));
    assert!(all_keysets.iter().any(|k| k.id == keyset_id2));
}

/// Test setting and getting active keyset
pub async fn set_and_get_active_keyset<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
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
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add keyset info
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get active keyset
    let active_id = db.get_active_keyset_id(&CurrencyUnit::Sat).await.unwrap();
    assert!(active_id.is_some());
    assert_eq!(active_id.unwrap(), keyset_id);
}

/// Test getting all active keysets
pub async fn get_all_active_keysets<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id_sat = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info_sat = MintKeySetInfo {
        id: keyset_id_sat,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    let keyset_id_usd = Id::from_str("00916bbf7ef91a37").unwrap();
    let keyset_info_usd = MintKeySetInfo {
        id: keyset_id_usd,
        unit: CurrencyUnit::Usd,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/1'").unwrap(),
        derivation_path_index: Some(1),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add keyset infos and set as active
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info_sat.clone()).await.unwrap();
    tx.add_keyset_info(keyset_info_usd.clone()).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id_sat)
        .await
        .unwrap();
    tx.set_active_keyset(CurrencyUnit::Usd, keyset_id_usd)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get all active keysets
    let active_keysets = db.get_active_keysets().await.unwrap();
    assert!(active_keysets.len() >= 2);
    assert_eq!(active_keysets.get(&CurrencyUnit::Sat), Some(&keyset_id_sat));
    assert_eq!(active_keysets.get(&CurrencyUnit::Usd), Some(&keyset_id_usd));
}

/// Test updating active keyset
pub async fn update_active_keyset<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id1 = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info1 = MintKeySetInfo {
        id: keyset_id1,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    let keyset_id2 = Id::from_str("00916bbf7ef91a37").unwrap();
    let keyset_info2 = MintKeySetInfo {
        id: keyset_id2,
        unit: CurrencyUnit::Sat,
        active: false,
        valid_from: 0,
        final_expiry: None,
        derivation_path: DerivationPath::from_str("m/0'/0'/1'").unwrap(),
        derivation_path_index: Some(1),
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };

    // Add both keysets and set first as active
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info1.clone()).await.unwrap();
    tx.add_keyset_info(keyset_info2.clone()).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id1)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify first keyset is active
    let active_id = db.get_active_keyset_id(&CurrencyUnit::Sat).await.unwrap();
    assert_eq!(active_id, Some(keyset_id1));

    // Update to second keyset
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id2)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify second keyset is now active
    let active_id = db.get_active_keyset_id(&CurrencyUnit::Sat).await.unwrap();
    assert_eq!(active_id, Some(keyset_id2));
}

/// Test getting non-existent keyset info
pub async fn get_nonexistent_keyset_info<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

    // Try to get non-existent keyset
    let retrieved = db.get_keyset_info(&keyset_id).await.unwrap();
    assert!(retrieved.is_none());
}

/// Test getting active keyset when none is set
pub async fn get_active_keyset_when_none_set<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    // Try to get active keyset when none is set
    let active_id = db.get_active_keyset_id(&CurrencyUnit::Sat).await.unwrap();
    assert!(active_id.is_none());
}
