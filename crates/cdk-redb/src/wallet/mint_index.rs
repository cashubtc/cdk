//! Mint identity, separated from the mint URL.
//!
//! A mint URL is a mutable attribute: a mint that announces a new endpoint
//! (NUT-06 `urls`) is still the same mint. Records therefore carry an internal
//! mint id in place of `mint_url`, and moving a mint rewrites only its own row.

use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::mint_url::MintUrl;
use cdk_common::MintInfo;
use redb::{ReadableTable, TableDefinition, WriteTransaction};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;

/// <mint id, [`StoredMint`]>
pub const MINTS_TABLE: TableDefinition<u64, &str> = TableDefinition::new("mints_by_id");

const MINT_URL: &str = "mint_url";
const MINT_ID: &str = "mint_id";

/// A mint as stored: the table key is its identity, the URL is data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMint {
    /// URL the mint is currently reached at
    pub mint_url: MintUrl,
    /// Last known mint info
    pub mint_info: Option<MintInfo>,
}

/// Both directions of the mint id to URL mapping, read once per operation.
#[derive(Debug, Default)]
pub struct MintIndex {
    by_id: HashMap<u64, MintUrl>,
    by_url: HashMap<MintUrl, u64>,
}

impl MintIndex {
    /// Read the mapping from the mints table.
    pub fn read<T>(table: &T) -> Result<Self, Error>
    where
        T: ReadableTable<u64, &'static str>,
    {
        let mut index = Self::default();

        for entry in table.iter()? {
            let (id, mint) = entry?;
            let mint: StoredMint = serde_json::from_str(mint.value())?;

            index.by_id.insert(id.value(), mint.mint_url.clone());
            index.by_url.insert(mint.mint_url, id.value());
        }

        Ok(index)
    }

    /// Read the mapping, storing any of `mint_urls` that is not known yet.
    ///
    /// A wallet is built synchronously and can be handed an empty database, so
    /// it has no opportunity to register its mint before its first write; the
    /// store creates the mint on demand instead. A created mint holds only its
    /// URL, leaving metadata to [`super::WalletRedbDatabase::add_mint`], which
    /// would otherwise overwrite what is already there.
    pub fn read_ensuring<'a, I>(txn: &WriteTransaction, mint_urls: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = &'a MintUrl>,
    {
        let mut table = txn.open_table(MINTS_TABLE)?;
        let mut index = Self::read(&table)?;

        for mint_url in mint_urls {
            if index.by_url.contains_key(mint_url) {
                continue;
            }

            let mint_id = index.by_id.keys().copied().max().unwrap_or_default() + 1;
            let mint = StoredMint {
                mint_url: mint_url.clone(),
                mint_info: None,
            };

            table.insert(mint_id, serde_json::to_string(&mint)?.as_str())?;

            index.by_id.insert(mint_id, mint_url.clone());
            index.by_url.insert(mint_url.clone(), mint_id);
        }

        Ok(index)
    }

    /// Id a mint is stored under.
    pub fn id(&self, mint_url: &MintUrl) -> Result<u64, Error> {
        self.by_url
            .get(mint_url)
            .copied()
            .ok_or_else(|| Error::UnknownMint(mint_url.to_string()))
    }

    /// URL a mint is currently reached at.
    pub fn url(&self, mint_id: u64) -> Result<&MintUrl, Error> {
        self.by_id
            .get(&mint_id)
            .ok_or_else(|| Error::UnknownMint(mint_id.to_string()))
    }

    /// Serialize a record with its `mint_url` replaced by the mint's id.
    pub fn encode<T>(&self, record: &T) -> Result<String, Error>
    where
        T: Serialize,
    {
        let mut value = serde_json::to_value(record)?;
        let object = value.as_object_mut().ok_or(Error::MintReference)?;

        let mint_id = match object.remove(MINT_URL) {
            Some(Value::String(mint_url)) => {
                Value::Number(self.id(&MintUrl::from_str(&mint_url)?)?.into())
            }
            None | Some(Value::Null) => Value::Null,
            Some(_) => return Err(Error::MintReference),
        };

        object.insert(MINT_ID.to_owned(), mint_id);

        Ok(serde_json::to_string(&value)?)
    }

    /// Deserialize a record, putting the mint's current URL back in place of
    /// its id.
    pub fn decode<T>(&self, stored: &str) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let mut value: Value = serde_json::from_str(stored)?;
        let object = value.as_object_mut().ok_or(Error::MintReference)?;

        let mint_url = match object.remove(MINT_ID) {
            Some(Value::Number(mint_id)) => {
                let mint_id = mint_id.as_u64().ok_or(Error::MintReference)?;
                Value::String(self.url(mint_id)?.to_string())
            }
            None | Some(Value::Null) => Value::Null,
            Some(_) => return Err(Error::MintReference),
        };

        object.insert(MINT_URL.to_owned(), mint_url);

        Ok(serde_json::from_value(value)?)
    }
}
