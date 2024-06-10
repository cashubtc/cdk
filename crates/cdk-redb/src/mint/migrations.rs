use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk::mint::MintQuote;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::{Amount, UncheckedUrl};
use lightning_invoice::Bolt11Invoice;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use super::Error;

const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");

pub fn migrate_01_to_02(db: Arc<Database>) -> Result<u32, Error> {
    migrate_mint_quotes_01_to_02(db)?;

    Ok(2)
}
/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct V1MintQuote {
    pub id: String,
    pub mint_url: UncheckedUrl,
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
        .open_table(MINT_QUOTES_TABLE)
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
                .open_table(MINT_QUOTES_TABLE)
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
