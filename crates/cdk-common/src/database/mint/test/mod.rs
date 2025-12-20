//! Macro with default tests
//!
//! This set is generic and checks the default and expected behaviour for a mint database
//! implementation
#![allow(clippy::unwrap_used)]
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// For derivation path parsing
use bitcoin::bip32::DerivationPath;
use cashu::CurrencyUnit;

use super::*;
use crate::database::KVStoreDatabase;
use crate::mint::MintKeySetInfo;

mod keys;
mod mint;
mod proofs;
mod saga;
mod signatures;

pub use self::keys::*;
pub use self::mint::*;
pub use self::proofs::*;
pub use self::saga::*;
pub use self::signatures::*;

/// Generate standard keyset amounts as powers of 2
#[inline]
fn standard_keyset_amounts(max_order: u32) -> Vec<u64> {
    (0..max_order).map(|n| 2u64.pow(n)).collect()
}

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
        input_fee_ppk: 0,
        amounts: standard_keyset_amounts(32),
    };
    let mut writer = db.begin_transaction().await.expect("db.begin()");
    writer.add_keyset_info(keyset_info).await.unwrap();
    writer.commit().await.expect("commit()");
    keyset_id
}

/// Test KV store functionality including write, read, list, update, and remove operations
pub async fn kvstore_functionality<DB>(db: DB)
where
    DB: Database<crate::database::Error> + KVStoreDatabase<Err = crate::database::Error>,
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

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a unique, random-looking Base62 string (no external crates).
/// Not cryptographically secure, but great for ids, keys, temp names, etc.
fn unique_string() -> String {
    // 1) high-res timestamp (nanos since epoch)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // 2) per-process monotonic counter to avoid collisions in the same instant
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;

    // 3) process id to reduce collision chance across processes
    let pid = std::process::id() as u128;

    // Mix the components (simple XOR/shift mix; good enough for "random-looking")
    let mixed = now ^ (pid << 64) ^ (n << 32);

    base62_encode(mixed)
}

fn base62_encode(mut x: u128) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    if x == 0 {
        return "0".to_string();
    }
    let mut buf = [0u8; 26]; // enough for base62(u128)
    let mut i = buf.len();
    while x > 0 {
        let rem = (x % 62) as usize;
        x /= 62;
        i -= 1;
        buf[i] = ALPHABET[rem];
    }
    String::from_utf8_lossy(&buf[i..]).into_owned()
}

/// Unit test that is expected to be passed for a correct database implementation
#[macro_export]
macro_rules! mint_db_test {
    ($make_db_fn:ident) => {
        mint_db_test!(
            $make_db_fn,
            add_and_find_proofs,
            add_duplicate_proofs,
            kvstore_functionality,
            add_mint_quote,
            add_mint_quote_only_once,
            register_payments,
            read_mint_from_db_and_tx,
            get_proofs_by_keyset_id,
            reject_duplicate_payments_same_tx,
            reject_duplicate_payments_diff_tx,
            reject_over_issue_same_tx,
            reject_over_issue_different_tx,
            reject_over_issue_with_payment,
            reject_over_issue_with_payment_different_tx,
            add_melt_request_unique_blinded_messages,
            reject_melt_duplicate_blinded_signature,
            reject_duplicate_blinded_message_db_constraint,
            cleanup_melt_request_after_processing,
            add_and_get_melt_quote,
            add_melt_quote_only_once,
            update_melt_quote_state_transition,
            update_melt_quote_request_lookup_id,
            get_all_mint_quotes,
            get_all_melt_quotes,
            get_mint_quote_by_request,
            get_mint_quote_by_request_lookup_id,
            delete_blinded_messages,
            add_and_get_blind_signatures,
            get_blind_signatures_for_keyset,
            get_blind_signatures_for_quote,
            get_total_issued,
            get_nonexistent_blind_signatures,
            add_duplicate_blind_signatures,
            add_and_get_keyset_info,
            add_duplicate_keyset_info,
            get_all_keyset_infos,
            set_and_get_active_keyset,
            get_all_active_keysets,
            update_active_keyset,
            get_nonexistent_keyset_info,
            get_active_keyset_when_none_set,
            get_proofs_states,
            get_nonexistent_proof_states,
            get_proofs_by_nonexistent_ys,
            proof_transaction_isolation,
            proof_rollback,
            multiple_proofs_same_keyset,
            add_and_get_saga,
            add_duplicate_saga,
            update_saga_state,
            delete_saga,
            get_incomplete_swap_sagas,
            get_incomplete_melt_sagas,
            get_nonexistent_saga,
            update_nonexistent_saga,
            delete_nonexistent_saga,
            saga_with_quote_id,
            saga_transaction_rollback,
            multiple_sagas_different_states,
            increment_mint_quote_amount_paid,
            increment_mint_quote_amount_issued,
            get_mint_quote_in_transaction,
            get_melt_quote_in_transaction,
            get_mint_quote_by_request_in_transaction,
            get_mint_quote_by_request_lookup_id_in_transaction,
            get_blind_signatures_in_transaction,
            reject_duplicate_payment_ids,
            remove_spent_proofs_should_fail,
        );
    };
    ($make_db_fn:ident, $($name:ident),+ $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                use std::time::{SystemTime, UNIX_EPOCH};
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards");

                cdk_common::database::mint::test::$name($make_db_fn(format!("test_{}_{}", now.as_nanos(), stringify!($name))).await).await;
            }
        )+
    };
}
