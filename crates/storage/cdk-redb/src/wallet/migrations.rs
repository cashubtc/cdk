//! Wallet Migrations
use std::collections::HashSet;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use cdk_common::Id;
use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
};

use super::Error;
use crate::wallet::{KEYSETS_TABLE, KEYSET_COUNTER, KEYSET_U32_MAPPING, MINT_KEYS_TABLE};

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
