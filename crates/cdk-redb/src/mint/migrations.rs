use core::str;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk::mint::types::PaymentRequest;
use cdk::mint::MintQuote;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, PaymentMethod, Proof, State};
use cdk::Amount;
use lightning_invoice::Bolt11Invoice;
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use super::{Error, MELT_QUOTES_TABLE, PROOFS_STATE_TABLE, PROOFS_TABLE, QUOTE_SIGNATURES_TABLE};

const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
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

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V04MeltQuote {
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
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V05MeltQuote {
    /// Quote id
    pub id: String,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: PaymentRequest,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
}

impl TryFrom<V04MeltQuote> for V05MeltQuote {
    type Error = anyhow::Error;
    fn try_from(melt_quote: V04MeltQuote) -> anyhow::Result<V05MeltQuote> {
        let V04MeltQuote {
            id,
            unit,
            amount,
            request,
            fee_reserve,
            state,
            expiry,
            payment_preimage,
            request_lookup_id,
        } = melt_quote;

        let bolt11 = Bolt11Invoice::from_str(&request)?;

        let payment_request = PaymentRequest::Bolt11 { bolt11 };

        Ok(V05MeltQuote {
            id,
            unit,
            amount,
            request: payment_request,
            fee_reserve,
            state,
            expiry,
            payment_preimage,
            request_lookup_id,
        })
    }
}

pub fn migrate_04_to_05(db: Arc<Database>) -> anyhow::Result<u32> {
    let quotes: Vec<_>;
    {
        let read_txn = db.begin_write()?;
        let table = read_txn.open_table(MELT_QUOTES_TABLE)?;

        quotes = table
            .iter()?
            .flatten()
            .map(|(k, v)| (k.value().to_string(), v.value().to_string()))
            .collect();
    }

    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;

        for (quote_id, quote) in quotes {
            let melt_quote: V04MeltQuote = serde_json::from_str(&quote)?;

            let v05_melt_quote: V05MeltQuote = melt_quote.try_into()?;

            table.insert(
                quote_id.as_str(),
                serde_json::to_string(&v05_melt_quote)?.as_str(),
            )?;
        }
    }
    write_txn.commit()?;

    Ok(5)
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct V1MintQuote {
    pub id: String,
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
            amount: Some(quote.amount),
            unit: quote.unit,
            request: quote.request.clone(),
            state: quote.state,
            expiry: quote.expiry,
            request_lookup_id: Bolt11Invoice::from_str(&quote.request).unwrap().to_string(),
            // TODO: Create real migrations
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            single_use: true,
            payment_method: PaymentMethod::Bolt11,
            payment_ids: vec![],
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
