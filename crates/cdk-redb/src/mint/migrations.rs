use core::str;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::mint::MintQuote;
use cdk_common::mint_url::MintUrl;
use cdk_common::util::{hex, unix_time};
use cdk_common::{Amount, CurrencyUnit, MintQuoteState, Proof, PublicKey, State};
use lightning_invoice::Bolt11Invoice;
use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use super::{Error, MINT_QUOTES_TABLE, PROOFS_STATE_TABLE, PROOFS_TABLE, QUOTE_SIGNATURES_TABLE};

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

#[instrument(skip_all)]
pub fn migrate_05_to_06(db: Arc<Database>) -> Result<u32, Error> {
    let read_txn = db.begin_read().map_err(Error::from)?;
    let table = read_txn
        .open_table(MINT_QUOTES_TABLE)
        .map_err(Error::from)?;

    let mint_quotes: HashMap<[u8; 16], Option<V5MintQuote>>;
    {
        mint_quotes = table
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

    let mint_quotes_count = mint_quotes.capacity();

    tracing::info!("{} mint quotes before migrations.", mint_quotes_count);

    let migrated_mint_quotes: HashMap<[u8; 16], Option<MintQuote>> = mint_quotes
        .into_iter()
        .map(|(quote_id, quote)| (quote_id, quote.map(|q| q.into())))
        .collect();

    tracing::info!(
        "{} mint quotes after migrations.",
        migrated_mint_quotes.capacity()
    );

    assert_eq!(mint_quotes_count, migrated_mint_quotes.capacity());

    {
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            for (quote_id, quote) in migrated_mint_quotes {
                if let Some(quote) = quote {
                    table.insert(quote_id, serde_json::to_string(&quote)?.as_str())?;
                } else {
                    tracing::warn!("Mint quote with is {:?} failed to be migrated.", quote_id);
                }
            }
        }

        write_txn.commit()?;
    }

    Ok(6)
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct V1MintQuote {
    pub id: Uuid,
    pub mint_url: MintUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub state: V4QuoteState,
    pub expiry: u64,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum V4QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid and wallet can mint
    Paid,
    /// ecash issued for quote
    Issued,
    Pending,
}

impl From<V4QuoteState> for MintQuoteState {
    fn from(value: V4QuoteState) -> Self {
        match value {
            V4QuoteState::Unpaid => Self::Unpaid,
            V4QuoteState::Paid => Self::Paid,
            V4QuoteState::Issued => Self::Issued,
            V4QuoteState::Pending => Self::Unpaid,
        }
    }
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct V5MintQuote {
    /// Quote id
    pub id: Uuid,
    /// Amount of quote
    pub amount: Amount,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Quote state
    pub state: V4QuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
    /// Pubkey
    pub pubkey: Option<PublicKey>,
    /// Unix time quote was created
    #[serde(default)]
    pub created_time: u64,
    /// Unix time quote was paid
    pub paid_time: Option<u64>,
    /// Unix time quote was issued
    pub issued_time: Option<u64>,
}

impl From<V1MintQuote> for V5MintQuote {
    fn from(quote: V1MintQuote) -> Self {
        Self {
            id: quote.id,
            amount: quote.amount,
            unit: quote.unit,
            request: quote.request.clone(),
            state: quote.state,
            expiry: quote.expiry,
            request_lookup_id: Bolt11Invoice::from_str(&quote.request).unwrap().to_string(),
            pubkey: None,
            created_time: unix_time(),
            paid_time: None,
            issued_time: None,
        }
    }
}

impl From<V5MintQuote> for MintQuote {
    fn from(quote: V5MintQuote) -> MintQuote {
        let pending = matches!(quote.state, V4QuoteState::Pending);

        let mut payment_ids = vec![];

        let request_lookup_id = cdk_common::payment::PaymentIdentifier::PaymentHash(
            hex::decode(quote.request_lookup_id.clone())
                .expect("Valid hex")
                .try_into()
                .expect("Valid hash"),
        );

        let (amount_paid, amount_issued) = match quote.state {
            V4QuoteState::Unpaid => (Amount::ZERO, Amount::ZERO),
            V4QuoteState::Paid => {
                payment_ids.push(quote.request_lookup_id);
                (quote.amount, Amount::ZERO)
            }
            V4QuoteState::Issued => (quote.amount, quote.amount),
            V4QuoteState::Pending => (Amount::ZERO, Amount::ZERO),
        };

        let paid_time = if let Some(paid) = quote.paid_time {
            vec![paid]
        } else {
            vec![]
        };

        let issued_time = if let Some(issued) = quote.issued_time {
            vec![issued]
        } else {
            vec![]
        };

        Self::new(
            Some(quote.id),
            quote.request,
            quote.unit,
            Some(quote.amount),
            quote.expiry,
            request_lookup_id,
            quote.pubkey,
            amount_paid,
            amount_issued,
            payment_ids,
            cdk_common::PaymentMethod::Bolt11,
            pending,
            quote.created_time,
            paid_time,
            issued_time,
        )
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

    let migrated_mint_quotes: HashMap<String, Option<V5MintQuote>> = mint_quotes
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
