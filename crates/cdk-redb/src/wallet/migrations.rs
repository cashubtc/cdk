//! Wallet Migrations
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
};

use super::Error;

// <Mint_url, Info>
const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
// <Mint_Url, Keyset_id>
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &[u8]> =
    MultimapTableDefinition::new("mint_keysets");

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
