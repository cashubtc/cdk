//! Keys database tests

use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use cashu::{CurrencyUnit, Id};

use crate::common::IssuerVersion;
use crate::database::mint::{Database, Error, KeysDatabase};
use crate::mint::MintKeySetInfo;

/// Generate standard keyset amounts as powers of 2
fn standard_keyset_amounts(max_order: u32) -> Vec<u64> {
    (0..max_order).map(|n| 2u64.pow(n)).collect()
}

/// Read the active keyset id for a unit through a transaction.
///
/// Keyset reads go through a transaction now that the autocommit key reads were
/// removed from [`KeysDatabase`]; the transaction takes the global keyset lock.
async fn active_keyset_id<DB>(db: &DB, unit: &CurrencyUnit) -> Option<Id>
where
    DB: KeysDatabase<Err = Error>,
{
    let mut tx = KeysDatabase::begin_transaction(db).await.unwrap();
    let id = tx.get_active_keysets().await.unwrap().get(unit).copied();
    tx.commit().await.unwrap();
    id
}

/// Read a keyset info by id through a transaction. See [`active_keyset_id`].
async fn find_keyset_info<DB>(db: &DB, id: &Id) -> Option<MintKeySetInfo>
where
    DB: KeysDatabase<Err = Error>,
{
    let mut tx = KeysDatabase::begin_transaction(db).await.unwrap();
    let info = tx
        .get_keyset_infos()
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.id == *id);
    tx.commit().await.unwrap();
    info
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
    };

    // Add keyset info
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Retrieve keyset info
    let retrieved = find_keyset_info(&db, &keyset_id).await;
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, keyset_info.id);
    assert_eq!(retrieved.unit, keyset_info.unit);
    assert_eq!(retrieved.active, keyset_info.active);
    assert_eq!(retrieved.amounts, keyset_info.amounts);
    assert_eq!(retrieved.issuer_version, keyset_info.issuer_version);
    assert_eq!(retrieved.valid_from, keyset_info.valid_from);
    assert_eq!(retrieved.final_expiry, keyset_info.final_expiry);
    assert_eq!(retrieved.input_fee_ppk, keyset_info.input_fee_ppk);
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
    let retrieved = find_keyset_info(&db, &keyset_id).await;
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
    };

    // Add keyset infos
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info1.clone()).await.unwrap();
    tx.add_keyset_info(keyset_info2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get all keyset infos
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    let all_keysets = tx.get_keyset_infos().await.unwrap();
    tx.commit().await.unwrap();
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
    };

    // Add keyset info
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get active keyset
    let active_id = active_keyset_id(&db, &CurrencyUnit::Sat).await;
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    let active_keysets = tx.get_active_keysets().await.unwrap();
    tx.commit().await.unwrap();
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
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
    let active_id = active_keyset_id(&db, &CurrencyUnit::Sat).await;
    assert_eq!(active_id, Some(keyset_id1));

    // Update to second keyset
    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.set_active_keyset(CurrencyUnit::Sat, keyset_id2)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify second keyset is now active
    let active_id = active_keyset_id(&db, &CurrencyUnit::Sat).await;
    assert_eq!(active_id, Some(keyset_id2));
}

/// Test getting non-existent keyset info
pub async fn get_nonexistent_keyset_info<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

    // Try to get non-existent keyset
    let retrieved = find_keyset_info(&db, &keyset_id).await;
    assert!(retrieved.is_none());
}

/// Test getting active keyset when none is set
pub async fn get_active_keyset_when_none_set<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    // Try to get active keyset when none is set
    let active_id = active_keyset_id(&db, &CurrencyUnit::Sat).await;
    assert!(active_id.is_none());
}

/// The keyset `u64` fields live in signed 64-bit columns.
///
/// Everything up to `i64::MAX` must round trip, and anything past it must be
/// refused on write: it used to be narrowed silently, committing a negative
/// value that no later read could decode, which failed every keyset read in
/// the table rather than only its own row.
pub async fn keyset_u64_column_bounds<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let largest = i64::MAX as u64;
    let keyset_info = MintKeySetInfo {
        id: Id::from_str("00916bbf7ef91a36").unwrap(),
        unit: CurrencyUnit::Sat,
        active: false,
        valid_from: largest,
        final_expiry: Some(largest),
        derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        input_fee_ppk: largest,
        amounts: standard_keyset_amounts(32),
        issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
    };

    let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
    tx.add_keyset_info(keyset_info.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = find_keyset_info(&db, &keyset_info.id).await.unwrap();
    assert_eq!(retrieved.valid_from, largest);
    assert_eq!(retrieved.final_expiry, Some(largest));
    assert_eq!(retrieved.input_fee_ppk, largest);

    let oversized = largest + 1;
    let rejected_id = Id::from_str("00916bbf7ef91a37").unwrap();
    let rejected = [
        MintKeySetInfo {
            id: rejected_id,
            valid_from: oversized,
            final_expiry: None,
            input_fee_ppk: 0,
            ..keyset_info.clone()
        },
        MintKeySetInfo {
            id: rejected_id,
            valid_from: 0,
            final_expiry: Some(oversized),
            input_fee_ppk: 0,
            ..keyset_info.clone()
        },
        MintKeySetInfo {
            id: rejected_id,
            valid_from: 0,
            final_expiry: None,
            input_fee_ppk: oversized,
            ..keyset_info.clone()
        },
    ];

    for case in rejected {
        let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
        assert!(tx.add_keyset_info(case).await.is_err());
        tx.rollback().await.unwrap();

        assert!(find_keyset_info(&db, &rejected_id).await.is_none());
        assert_eq!(
            find_keyset_info(&db, &keyset_info.id).await.unwrap().id,
            keyset_info.id
        );
    }
}
