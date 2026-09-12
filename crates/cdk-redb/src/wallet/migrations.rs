//! Wallet Migrations
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use cdk_common::wallet::{self, MintQuote, ProofInfo, Transaction};
use cdk_common::{Id, MintInfo};
use redb::{
    Database, MultimapTableDefinition, ReadableDatabase, ReadableMultimapTable, ReadableTable,
    TableDefinition,
};

use super::Error;
use crate::wallet::mint_index::{MintIndex, StoredMint, MINTS_TABLE as MINTS_BY_ID_TABLE};
use crate::wallet::{
    KEYSETS_TABLE, KEYSET_COUNTER, KEYSET_U32_MAPPING, MELT_QUOTES_TABLE,
    MINT_KEYSETS_TABLE as MINT_ID_KEYSETS_TABLE, MINT_KEYS_TABLE, MINT_QUOTES_TABLE,
    P2PK_SIGNING_KEYS_TABLE, PROOFS_TABLE, SAGAS_TABLE, TRANSACTIONS_TABLE,
};

// <Mint_url, Info>
const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
// <Mint_Url, Keyset_id>
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &[u8]> =
    MultimapTableDefinition::new("mint_keysets");

pub(crate) fn migrate_02_to_03(db: Arc<Database>) -> Result<u32, Error> {
    let write_txn = db.begin_write().map_err(Error::from)?;

    let mut duplicate = false;

    {
        let table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        let ids: Vec<Id> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(id, _)| Id::from_str(id.value()))
            .collect();

        let mut table = write_txn
            .open_table(KEYSET_U32_MAPPING)
            .map_err(Error::from)?;

        // Also process existing keysets
        let keysets_table = write_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;
        let keyset_ids: Vec<Id> = keysets_table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(id_bytes, _)| Id::from_bytes(id_bytes.value()))
            .collect();

        let ids: HashSet<Id> = ids.into_iter().chain(keyset_ids).collect();

        for id in ids {
            let t = table.insert(u32::from(id), id.to_string().as_str())?;

            tracing::info!("Adding u32 {} for keyset {}", u32::from(id), id.to_string());

            if t.is_some() {
                duplicate = true;
            }
        }
    }

    if duplicate {
        write_txn.abort()?;
        return Err(Error::Duplicate);
    }

    write_txn.commit()?;

    Ok(3)
}

pub fn migrate_01_to_02(db: Arc<Database>) -> Result<u32, Error> {
    migrate_trim_mint_urls_01_to_02(db)?;
    Ok(2)
}

fn migrate_mints_table_01_to_02(db: Arc<Database>) -> Result<(), Error> {
    let mints: Vec<(String, String)>;
    {
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;

        mints = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(mint_url, mint_info)| {
                (mint_url.value().to_string(), mint_info.value().to_string())
            })
            .collect();
    }

    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
        for (mint_url_str, info) in mints {
            let mint_url = MintUrl::from_str(&mint_url_str).map_err(Error::from)?;

            table.remove(mint_url_str.as_str())?;

            table.insert(mint_url.to_string().as_str(), info.as_str())?;
        }
    }
    write_txn.commit()?;

    Ok(())
}

fn migrate_mint_keyset_table_01_to_02(db: Arc<Database>) -> Result<(), Error> {
    let mut mints: Vec<(String, Vec<Vec<u8>>)> = vec![];
    {
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let mint_keysets_range = table.iter().map_err(Error::from)?;

        for (url, keysets) in mint_keysets_range.flatten() {
            let keysets: Vec<Vec<u8>> = keysets
                .into_iter()
                .flatten()
                .map(|k| k.value().to_vec())
                .collect();

            mints.push((url.value().to_string(), keysets));
        }
    }

    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;
        for (mint_url_str, keysets) in mints {
            let mint_url = MintUrl::from_str(&mint_url_str).map_err(Error::from)?;

            table.remove_all(mint_url_str.as_str())?;
            for keyset in keysets {
                table.insert(mint_url.to_string().as_str(), keyset.deref())?;
            }
        }
    }
    write_txn.commit()?;

    Ok(())
}

fn migrate_trim_mint_urls_01_to_02(db: Arc<Database>) -> Result<(), Error> {
    migrate_mints_table_01_to_02(Arc::clone(&db))?;
    migrate_mint_keyset_table_01_to_02(Arc::clone(&db))?;
    Ok(())
}

pub(crate) fn migrate_03_to_04(db: Arc<Database>) -> Result<u32, Error> {
    let write_txn = db.begin_write().map_err(Error::from)?;

    // Get all existing keyset IDs from the KEYSET_COUNTER table that have a counter > 0
    let keyset_ids_to_increment: Vec<(String, u32)>;
    {
        let table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;

        keyset_ids_to_increment = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(keyset_id, counter)| {
                let counter_value = counter.value();
                // Only include keysets where counter > 0
                if counter_value > 0 {
                    Some((keyset_id.value().to_string(), counter_value))
                } else {
                    None
                }
            })
            .collect();
    }

    // Increment counter by 1 for all keysets where counter > 0
    {
        let mut table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;

        for (keyset_id, current_counter) in keyset_ids_to_increment {
            let new_counter = current_counter + 1;
            table
                .insert(keyset_id.as_str(), new_counter)
                .map_err(Error::from)?;

            tracing::info!(
                "Incremented counter for keyset {} from {} to {}",
                keyset_id,
                current_counter,
                new_counter
            );
        }
    }

    write_txn.commit()?;

    Ok(4)
}

pub(crate) fn migrate_04_to_05(db: Arc<Database>) -> Result<u32, Error> {
    tracing::info!("Starting migration from version 4 to 5: Initializing P2PK_SIGNING_KEYS_TABLE");
    let write_txn = db.begin_write().map_err(Error::from)?;

    {
        // Open the table to initialize it (redb creates tables on first open)
        let _ = write_txn
            .open_table(P2PK_SIGNING_KEYS_TABLE)
            .map_err(Error::from)?;
    }

    write_txn.commit()?;
    tracing::info!("Finished migration from version 4 to 5: P2PK_SIGNING_KEYS_TABLE initialized");

    Ok(5)
}

pub(crate) fn migrate_05_to_06(db: Arc<Database>) -> Result<u32, Error> {
    tracing::info!("Starting migration from version 5 to 6: Rekeying saga transactions");
    let write_txn = db.begin_write().map_err(Error::from)?;

    let transactions = {
        let table = write_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;
        let mut transactions = Vec::new();

        for entry in table.iter().map_err(Error::from)? {
            let (id, value) = entry.map_err(Error::from)?;
            let transaction = serde_json::from_str::<Transaction>(value.value())?;
            if transaction.saga_id.is_some() {
                transactions.push((
                    id.value().to_vec(),
                    transaction.id(),
                    value.value().to_owned(),
                ));
            }
        }

        transactions
    };

    {
        let mut table = write_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;

        for (old_id, new_id, value) in transactions {
            if old_id == new_id.as_slice() {
                continue;
            }

            table.remove(old_id.as_slice()).map_err(Error::from)?;
            table
                .insert(new_id.as_slice(), value.as_str())
                .map_err(Error::from)?;
        }
    }

    write_txn.commit()?;
    tracing::info!("Finished migration from version 5 to 6: Rekeying saga transactions");

    Ok(6)
}

/// Give every mint an internal id and point records at that id.
///
/// A mint URL is a mutable attribute, so keying records on it meant a mint that
/// moved had to have every table rewritten. Records now carry a `mint_id` that
/// never changes.
///
/// Records could reference a mint that was never added; those get an identity
/// too, so nothing is stranded.
pub(crate) fn migrate_06_to_07(db: Arc<Database>) -> Result<u32, Error> {
    tracing::info!("Starting migration from version 6 to 7: Internal mint ids");
    let write_txn = db.begin_write().map_err(Error::from)?;

    {
        let mut mint_urls: HashMap<MintUrl, Option<MintInfo>> = HashMap::new();

        {
            let table = write_txn.open_table(MINTS_TABLE)?;
            for entry in table.iter()? {
                let (mint_url, mint_info) = entry?;
                let mint_url = MintUrl::from_str(mint_url.value())?;
                mint_urls.insert(mint_url, serde_json::from_str(mint_info.value())?);
            }
        }

        {
            let table = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;
            for entry in table.iter()? {
                let (mint_url, _) = entry?;
                mint_urls
                    .entry(MintUrl::from_str(mint_url.value())?)
                    .or_default();
            }
        }
        for referenced in [
            referenced_mint_urls::<ProofInfo, &[u8]>(&write_txn, PROOFS_TABLE, |p| {
                Some(p.mint_url)
            })?,
            referenced_mint_urls::<MintQuote, &str>(&write_txn, MINT_QUOTES_TABLE, |q| {
                Some(q.mint_url)
            })?,
            referenced_mint_urls::<wallet::MeltQuote, &str>(&write_txn, MELT_QUOTES_TABLE, |q| {
                q.mint_url
            })?,
            referenced_mint_urls::<Transaction, &[u8]>(&write_txn, TRANSACTIONS_TABLE, |t| {
                Some(t.mint_url)
            })?,
            referenced_mint_urls::<wallet::WalletSaga, &str>(&write_txn, SAGAS_TABLE, |s| {
                Some(s.mint_url)
            })?,
        ] {
            for mint_url in referenced {
                mint_urls.entry(mint_url).or_default();
            }
        }

        let mut mints_by_id = write_txn.open_table(MINTS_BY_ID_TABLE)?;
        let mut keyset_ids_by_mint = write_txn.open_multimap_table(MINT_ID_KEYSETS_TABLE)?;
        let old_keyset_ids = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;

        for (mint_id, (mint_url, mint_info)) in (1u64..).zip(mint_urls) {
            let old_key = mint_url.to_string();
            let mint = StoredMint {
                mint_url,
                mint_info,
            };

            mints_by_id.insert(mint_id, serde_json::to_string(&mint)?.as_str())?;

            for keyset_id in old_keyset_ids.get(old_key.as_str())?.flatten() {
                keyset_ids_by_mint.insert(mint_id, keyset_id.value())?;
            }
        }
    }

    {
        let mints = MintIndex::read(&write_txn.open_table(MINTS_BY_ID_TABLE)?)?;

        rekey_bytes_records::<ProofInfo>(&write_txn, PROOFS_TABLE, &mints)?;
        rekey_str_records::<MintQuote>(&write_txn, MINT_QUOTES_TABLE, &mints)?;
        rekey_str_records::<wallet::MeltQuote>(&write_txn, MELT_QUOTES_TABLE, &mints)?;
        rekey_bytes_records::<Transaction>(&write_txn, TRANSACTIONS_TABLE, &mints)?;
        rekey_str_records::<wallet::WalletSaga>(&write_txn, SAGAS_TABLE, &mints)?;
    }

    write_txn.delete_table(MINTS_TABLE)?;
    write_txn.delete_multimap_table(MINT_KEYSETS_TABLE)?;

    write_txn.commit()?;
    tracing::info!("Finished migration from version 6 to 7: Internal mint ids");

    Ok(7)
}

/// Mint URLs named by the records of one table.
fn referenced_mint_urls<T, K>(
    write_txn: &redb::WriteTransaction,
    table: TableDefinition<K, &str>,
    mint_url: impl Fn(T) -> Option<MintUrl>,
) -> Result<Vec<MintUrl>, Error>
where
    T: serde::de::DeserializeOwned,
    K: redb::Key + 'static,
{
    let table = write_txn.open_table(table)?;
    let mut mint_urls = Vec::new();

    for entry in table.iter()? {
        let (_, record) = entry?;
        if let Some(mint_url) = mint_url(serde_json::from_str(record.value())?) {
            mint_urls.push(mint_url);
        }
    }

    Ok(mint_urls)
}

/// Rewrite a string-keyed table's records with `mint_id` in place of `mint_url`.
fn rekey_str_records<T>(
    write_txn: &redb::WriteTransaction,
    table: TableDefinition<&str, &str>,
    mints: &MintIndex,
) -> Result<(), Error>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let mut table = write_txn.open_table(table)?;

    let rekeyed = table
        .iter()?
        .map(|entry| {
            let (key, record) = entry?;
            let record: T = serde_json::from_str(record.value())?;
            Ok((key.value().to_owned(), mints.encode(&record)?))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    for (key, record) in rekeyed {
        table.insert(key.as_str(), record.as_str())?;
    }

    Ok(())
}

/// Rewrite a byte-keyed table's records with `mint_id` in place of `mint_url`.
fn rekey_bytes_records<T>(
    write_txn: &redb::WriteTransaction,
    table: TableDefinition<&[u8], &str>,
    mints: &MintIndex,
) -> Result<(), Error>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let mut table = write_txn.open_table(table)?;

    let rekeyed = table
        .iter()?
        .map(|entry| {
            let (key, record) = entry?;
            let record: T = serde_json::from_str(record.value())?;
            Ok((key.value().to_vec(), mints.encode(&record)?))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    for (key, record) in rekeyed {
        table.insert(key.as_slice(), record.as_str())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use cdk_common::wallet::{TransactionDirection, TransactionId, TransactionStatus};
    use cdk_common::{Amount, CurrencyUnit, SecretKey};

    use super::*;

    #[test]
    fn migration_05_to_06_rekeys_saga_transactions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("wallet.redb");
        let database = Arc::new(Database::create(database_path).expect("database"));
        let saga_id = uuid::Uuid::new_v4();
        let transaction = Transaction {
            mint_url: MintUrl::from_str("https://mint.example.com").expect("valid mint URL"),
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(10),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![SecretKey::generate().public_key()],
            timestamp: 42,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status: TransactionStatus::Pending,
        };
        let legacy_id = TransactionId::new(transaction.ys.clone());
        let saga_transaction_id = transaction.id();
        assert_ne!(legacy_id, saga_transaction_id);

        let write_txn = database.begin_write().expect("write transaction");
        {
            let mut table = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .expect("transactions table");
            table
                .insert(
                    legacy_id.as_slice(),
                    serde_json::to_string(&transaction)
                        .expect("serialize transaction")
                        .as_str(),
                )
                .expect("insert transaction");
        }
        write_txn.commit().expect("commit transaction");

        assert_eq!(
            migrate_05_to_06(Arc::clone(&database)).expect("migration"),
            6
        );

        let read_txn = database.begin_read().expect("read transaction");
        let table = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .expect("transactions table");
        assert!(table
            .get(legacy_id.as_slice())
            .expect("legacy lookup")
            .is_none());
        assert!(table
            .get(saga_transaction_id.as_slice())
            .expect("saga lookup")
            .is_some());
    }

    #[test]
    fn migration_06_to_07_moves_records_onto_mint_ids() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("wallet.redb");
        let database = Arc::new(Database::create(database_path).expect("database"));

        let mint_url = MintUrl::from_str("https://mint.example.com").expect("valid mint URL");
        let orphan_url = MintUrl::from_str("https://orphan.example.com").expect("valid mint URL");
        let keyset_id = Id::from_str("00916bbf7ef91a36").expect("valid keyset id");

        let transaction = Transaction {
            mint_url: orphan_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(10),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![SecretKey::generate().public_key()],
            timestamp: 42,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: None,
            status: TransactionStatus::Completed,
        };
        let transaction_id = transaction.id();

        let write_txn = database.begin_write().expect("write transaction");
        {
            let mut mints = write_txn.open_table(MINTS_TABLE).expect("mints table");
            mints
                .insert(mint_url.to_string().as_str(), "null")
                .expect("insert mint");

            let mut keysets = write_txn
                .open_multimap_table(MINT_KEYSETS_TABLE)
                .expect("mint keysets table");
            keysets
                .insert(
                    mint_url.to_string().as_str(),
                    keyset_id.to_bytes().as_slice(),
                )
                .expect("insert keyset id");

            let mut transactions = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .expect("transactions table");
            transactions
                .insert(
                    transaction_id.as_slice(),
                    serde_json::to_string(&transaction)
                        .expect("serialize transaction")
                        .as_str(),
                )
                .expect("insert transaction");
        }
        write_txn.commit().expect("commit transaction");

        assert_eq!(
            migrate_06_to_07(Arc::clone(&database)).expect("migration"),
            7
        );

        let read_txn = database.begin_read().expect("read transaction");
        let mints = MintIndex::read(
            &read_txn
                .open_table(MINTS_BY_ID_TABLE)
                .expect("mints by id table"),
        )
        .expect("mint index");

        let mint_id = mints.id(&mint_url).expect("mint id");
        let orphan_id = mints.id(&orphan_url).expect("orphan mint id");

        let keysets = read_txn
            .open_multimap_table(MINT_ID_KEYSETS_TABLE)
            .expect("mint id keysets table");
        let stored_keysets = keysets
            .get(mint_id)
            .expect("keyset lookup")
            .flatten()
            .map(|id| id.value().to_vec())
            .collect::<Vec<_>>();
        assert_eq!(stored_keysets, vec![keyset_id.to_bytes().to_vec()]);

        let transactions = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .expect("transactions table");
        let stored = transactions
            .get(transaction_id.as_slice())
            .expect("transaction lookup")
            .expect("transaction");
        let stored_json: serde_json::Value =
            serde_json::from_str(stored.value()).expect("stored transaction");
        assert_eq!(stored_json["mint_id"], serde_json::json!(orphan_id));
        assert!(stored_json.get("mint_url").is_none());

        let rebuilt: Transaction = mints.decode(stored.value()).expect("rebuilt transaction");
        assert_eq!(rebuilt.mint_url, orphan_url);

        assert!(read_txn.open_table(MINTS_TABLE).is_err());
    }
}
