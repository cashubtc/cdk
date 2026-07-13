//! Wallet Migrations
use std::collections::HashSet;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use cdk_common::wallet::Transaction;
use cdk_common::Id;
use redb::{
    Database, MultimapTableDefinition, ReadableDatabase, ReadableMultimapTable, ReadableTable,
    TableDefinition,
};

use super::Error;
use crate::wallet::{
    KEYSETS_TABLE, KEYSET_COUNTER, KEYSET_U32_MAPPING, MINT_KEYS_TABLE, P2PK_SIGNING_KEYS_TABLE,
    TRANSACTIONS_TABLE,
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
}
