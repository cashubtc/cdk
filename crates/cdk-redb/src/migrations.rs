use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState, MintQuoteState};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use super::error::Error;

const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V0MintQuote {
    pub id: String,
    pub mint_url: MintUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub paid: bool,
    pub expiry: u64,
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V1MintQuote {
    pub id: String,
    pub mint_url: MintUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub state: MintQuoteState,
    pub expiry: u64,
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V1MeltQuote {
    /// Quote id
    pub id: String,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: String,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
}

impl From<V0MintQuote> for V1MintQuote {
    fn from(quote: V0MintQuote) -> V1MintQuote {
        let state = match quote.paid {
            true => MintQuoteState::Paid,
            false => MintQuoteState::Unpaid,
        };
        V1MintQuote {
            id: quote.id,
            mint_url: quote.mint_url,
            amount: quote.amount,
            unit: quote.unit,
            request: quote.request.clone(),
            state,
            expiry: quote.expiry,
        }
    }
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V0MeltQuote {
    pub id: String,
    pub unit: CurrencyUnit,
    pub amount: Amount,
    pub request: String,
    pub fee_reserve: Amount,
    pub paid: bool,
    pub expiry: u64,
}

impl From<V0MeltQuote> for V1MeltQuote {
    fn from(quote: V0MeltQuote) -> V1MeltQuote {
        let state = match quote.paid {
            true => MeltQuoteState::Paid,
            false => MeltQuoteState::Unpaid,
        };
        V1MeltQuote {
            id: quote.id,
            amount: quote.amount,
            unit: quote.unit,
            request: quote.request,
            state,
            expiry: quote.expiry,
            fee_reserve: quote.fee_reserve,
            payment_preimage: None,
        }
    }
}

fn migrate_mint_quotes_00_to_01(db: Arc<Database>) -> Result<(), Error> {
    let read_txn = db.begin_read().map_err(Error::from)?;
    let table = read_txn
        .open_table(MINT_QUOTES_TABLE)
        .map_err(Error::from)?;

    let mint_quotes: HashMap<String, Option<V0MintQuote>>;
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

    let migrated_mint_quotes: HashMap<String, Option<V1MintQuote>> = mint_quotes
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

fn migrate_melt_quotes_00_to_01(db: Arc<Database>) -> Result<(), Error> {
    let read_txn = db.begin_read().map_err(Error::from)?;
    let table = read_txn
        .open_table(MELT_QUOTES_TABLE)
        .map_err(Error::from)?;

    let melt_quotes: HashMap<String, Option<V0MeltQuote>>;
    {
        melt_quotes = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(quote_id, melt_quote)| {
                (
                    quote_id.value().to_string(),
                    serde_json::from_str(melt_quote.value()).ok(),
                )
            })
            .collect();
    }

    let migrated_melt_quotes: HashMap<String, Option<V1MeltQuote>> = melt_quotes
        .into_iter()
        .map(|(quote_id, quote)| (quote_id, quote.map(|q| q.into())))
        .collect();

    {
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            for (quote_id, quote) in migrated_melt_quotes {
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

pub(crate) fn migrate_00_to_01(db: Arc<Database>) -> Result<u32, Error> {
    tracing::info!("Starting Migrations of mint quotes from 00 to 01");
    migrate_mint_quotes_00_to_01(Arc::clone(&db))?;
    tracing::info!("Finished Migrations of mint quotes from 00 to 01");

    tracing::info!("Starting Migrations of melt quotes from 00 to 01");
    migrate_melt_quotes_00_to_01(Arc::clone(&db))?;
    tracing::info!("Finished Migrations of melt quotes from 00 to 01");
    Ok(1)
}
