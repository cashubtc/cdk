use core::str;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk::mint::MintQuote;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MintQuoteState, Proof, State};
use cdk::Amount;
use lightning_invoice::Bolt11Invoice;
use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Error, PROOFS_STATE_TABLE, PROOFS_TABLE, QUOTE_SIGNATURES_TABLE};

const ID_STR_MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const PENDING_PROOFS_TABLE: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("pending_proofs");
const SPENT_PROOFS_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("spent_proofs");
const QUOTE_PROOFS_TABLE: MultimapTableDefinition<&str, [u8; 33]> =
    MultimapTableDefinition::new("quote_proofs");

pub fn migrate_01_to_02(db: Arc<Database>) -> Result<u32, Error> {
    migrate_mint_quotes_01_to_02(db)?;
    Ok(2)
}

pub fn migrate_02_to_03(db: Arc<Database>) -> Result<u32, Error> {
    migrate_mint_proofs_02_to_03(db)?;
    Ok(3)
}
pub fn migrate_03_to_04(db: Arc<Database>) -> Result<u32, Error> {
    let write_txn = db.begin_write()?;
    let _ = write_txn.open_multimap_table(QUOTE_PROOFS_TABLE)?;
    let _ = write_txn.open_multimap_table(QUOTE_SIGNATURES_TABLE)?;
    Ok(4)
}

pub fn migrate_04_to_05(db: Arc<Database>) -> Result<u32, Error> {
    let write_txn = db.begin_write()?;

    // Mint quotes
    {
        const MINT_QUOTE_TABLE_NAME: &str = "mint_quotes";
        const OLD_TABLE: TableDefinition<&str, &str> = TableDefinition::new(MINT_QUOTE_TABLE_NAME);
        const NEW_TABLE: TableDefinition<[u8; 16], &str> =
            TableDefinition::new(MINT_QUOTE_TABLE_NAME);

        let old_table = write_txn.open_table(OLD_TABLE)?;

        let mut tmp_hashmap = HashMap::new();

        for (k, v) in old_table.iter().map_err(Error::from)?.flatten() {
            let quote_id = Uuid::try_parse(k.value()).unwrap();
            tmp_hashmap.insert(quote_id, v.value().to_string());
        }

        write_txn.delete_table(old_table).map_err(Error::from)?;
        let mut new_table = write_txn.open_table(NEW_TABLE)?;

        for (k, v) in tmp_hashmap.into_iter() {
            new_table
                .insert(k.as_bytes(), v.as_str())
                .map_err(Error::from)?;
        }
    }
    // Melt quotes
    {
        const MELT_QUOTE_TABLE_NAME: &str = "melt_quotes";
        const OLD_TABLE: TableDefinition<&str, &str> = TableDefinition::new(MELT_QUOTE_TABLE_NAME);
        const NEW_TABLE: TableDefinition<[u8; 16], &str> =
            TableDefinition::new(MELT_QUOTE_TABLE_NAME);

        let old_table = write_txn.open_table(OLD_TABLE)?;

        let mut tmp_hashmap = HashMap::new();

        for (k, v) in old_table.iter().map_err(Error::from)?.flatten() {
            let quote_id = Uuid::try_parse(k.value()).unwrap();
            tmp_hashmap.insert(quote_id, v.value().to_string());
        }

        write_txn.delete_table(old_table).map_err(Error::from)?;
        let mut new_table = write_txn.open_table(NEW_TABLE)?;

        for (k, v) in tmp_hashmap.into_iter() {
            new_table
                .insert(k.as_bytes(), v.as_str())
                .map_err(Error::from)?;
        }
    }
    // Quote proofs
    {
        const QUOTE_PROOFS_TABLE_NAME: &str = "quote_proofs";
        const OLD_TABLE: MultimapTableDefinition<&str, [u8; 33]> =
            MultimapTableDefinition::new(QUOTE_PROOFS_TABLE_NAME);
        const NEW_TABLE: MultimapTableDefinition<[u8; 16], [u8; 33]> =
            MultimapTableDefinition::new(QUOTE_PROOFS_TABLE_NAME);

        let old_table = write_txn.open_multimap_table(OLD_TABLE)?;

        let mut tmp_hashmap = HashMap::new();

        for (k, v) in old_table.iter().map_err(Error::from)?.flatten() {
            let quote_id = Uuid::try_parse(k.value()).unwrap();
            let ys: Vec<[u8; 33]> = v.into_iter().flatten().map(|v| v.value()).collect();
            tmp_hashmap.insert(quote_id, ys);
        }

        write_txn
            .delete_multimap_table(old_table)
            .map_err(Error::from)?;
        let mut new_table = write_txn.open_multimap_table(NEW_TABLE)?;

        for (quote_id, blind_messages) in tmp_hashmap.into_iter() {
            for blind_message in blind_messages {
                new_table
                    .insert(quote_id.as_bytes(), blind_message)
                    .map_err(Error::from)?;
            }
        }
    }
    // Quote signatures
    {
        const QUOTE_SIGNATURES_TABLE_NAME: &str = "quote_signatures";
        const OLD_TABLE: MultimapTableDefinition<&str, [u8; 33]> =
            MultimapTableDefinition::new(QUOTE_SIGNATURES_TABLE_NAME);
        const NEW_TABLE: MultimapTableDefinition<[u8; 16], [u8; 33]> =
            MultimapTableDefinition::new(QUOTE_SIGNATURES_TABLE_NAME);

        let old_table = write_txn.open_multimap_table(OLD_TABLE)?;

        let mut tmp_hashmap = HashMap::new();

        for (k, v) in old_table.iter().map_err(Error::from)?.flatten() {
            let quote_id = Uuid::try_parse(k.value()).unwrap();
            let ys: Vec<[u8; 33]> = v.into_iter().flatten().map(|v| v.value()).collect();
            tmp_hashmap.insert(quote_id, ys);
        }

        write_txn
            .delete_multimap_table(old_table)
            .map_err(Error::from)?;
        let mut new_table = write_txn.open_multimap_table(NEW_TABLE)?;

        for (quote_id, signatures) in tmp_hashmap.into_iter() {
            for signature in signatures {
                new_table
                    .insert(quote_id.as_bytes(), signature)
                    .map_err(Error::from)?;
            }
        }
    }
    // Melt requests
    {
        const MELT_REQUESTS_TABLE_NAME: &str = "melt_requests";
        const OLD_TABLE: TableDefinition<&str, (&str, &str)> =
            TableDefinition::new(MELT_REQUESTS_TABLE_NAME);
        const NEW_TABLE: TableDefinition<[u8; 16], (&str, &str)> =
            TableDefinition::new(MELT_REQUESTS_TABLE_NAME);

        let old_table = write_txn.open_table(OLD_TABLE)?;

        let mut tmp_hashmap = HashMap::new();

        for (k, v) in old_table.iter().map_err(Error::from)?.flatten() {
            let quote_id = Uuid::try_parse(k.value()).unwrap();
            let value = v.value();
            tmp_hashmap.insert(quote_id, (value.0.to_string(), value.1.to_string()));
        }

        write_txn.delete_table(old_table).map_err(Error::from)?;
        let mut new_table = write_txn.open_table(NEW_TABLE)?;

        for (k, v) in tmp_hashmap.into_iter() {
            new_table
                .insert(k.as_bytes(), (v.0.as_str(), v.1.as_str()))
                .map_err(Error::from)?;
        }
    }

    write_txn.commit().map_err(Error::from)?;

    Ok(5)
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct V1MintQuote {
    pub id: Uuid,
    pub mint_url: MintUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub state: MintQuoteState,
    pub expiry: u64,
}

impl From<V1MintQuote> for MintQuote {
    fn from(quote: V1MintQuote) -> MintQuote {
        MintQuote {
            id: quote.id,
            mint_url: quote.mint_url,
            amount: quote.amount,
            unit: quote.unit,
            request: quote.request.clone(),
            state: quote.state,
            expiry: quote.expiry,
            request_lookup_id: Bolt11Invoice::from_str(&quote.request).unwrap().to_string(),
        }
    }
}

fn migrate_mint_quotes_01_to_02(db: Arc<Database>) -> Result<(), Error> {
    let read_txn = db.begin_read().map_err(Error::from)?;
    let table = read_txn
        .open_table(ID_STR_MINT_QUOTES_TABLE)
        .map_err(Error::from)?;

    let mint_quotes: HashMap<String, Option<V1MintQuote>>;
    {
        mint_quotes = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(quote_id, mint_quote)| {
                (
                    quote_id.value().to_string(),
                    serde_json::from_str(mint_quote.value()).ok(),
                )
            })
            .collect();
    }

    let migrated_mint_quotes: HashMap<String, Option<MintQuote>> = mint_quotes
        .into_iter()
        .map(|(quote_id, quote)| (quote_id, quote.map(|q| q.into())))
        .collect();

    {
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn
                .open_table(ID_STR_MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            for (quote_id, quote) in migrated_mint_quotes {
                match quote {
                    Some(quote) => {
                        let quote_str = serde_json::to_string(&quote)?;

                        table.insert(quote_id.as_str(), quote_str.as_str())?;
                    }
                    None => {
                        table.remove(quote_id.as_str())?;
                    }
                }
            }
        }

        write_txn.commit()?;
    }

    Ok(())
}

fn migrate_mint_proofs_02_to_03(db: Arc<Database>) -> Result<(), Error> {
    let pending_proofs: Vec<([u8; 33], Option<Proof>)>;
    let spent_proofs: Vec<([u8; 33], Option<Proof>)>;

    {
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        pending_proofs = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(quote_id, mint_quote)| {
                (
                    quote_id.value(),
                    serde_json::from_str(mint_quote.value()).ok(),
                )
            })
            .collect();
    }
    {
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(SPENT_PROOFS_TABLE)
            .map_err(Error::from)?;

        spent_proofs = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(quote_id, mint_quote)| {
                (
                    quote_id.value(),
                    serde_json::from_str(mint_quote.value()).ok(),
                )
            })
            .collect();
    }

    let write_txn = db.begin_write().map_err(Error::from)?;
    {
        let mut proofs_table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
        let mut state_table = write_txn
            .open_table(PROOFS_STATE_TABLE)
            .map_err(Error::from)?;

        for (y, proof) in pending_proofs {
            if let Some(proof) = proof {
                proofs_table.insert(y, serde_json::to_string(&proof)?.as_str())?;
                state_table.insert(y, State::Pending.to_string().as_str())?;
            }
        }

        for (y, proof) in spent_proofs {
            if let Some(proof) = proof {
                proofs_table.insert(y, serde_json::to_string(&proof)?.as_str())?;
                state_table.insert(y, State::Spent.to_string().as_str())?;
            }
        }
    }

    write_txn.commit()?;
    Ok(())
}
