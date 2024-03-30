use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use cashu::types::{MeltQuote, MintQuote};
use cashu::url::UncheckedUrl;
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use tokio::sync::Mutex;

use super::{Error, LocalStore};

const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("mint_keysets");
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const MINT_KEYS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_keys");
const PROOFS_TABLE: MultimapTableDefinition<&str, &str> = MultimapTableDefinition::new("proofs");
const PENDING_PROOFS_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("pending_proofs");
#[cfg(feature = "nut13")]
const KEYSET_COUNTER: TableDefinition<&str, u64> = TableDefinition::new("keyset_counter");

#[derive(Debug, Clone)]
pub struct RedbLocalStore {
    db: Arc<Mutex<Database>>,
}

impl RedbLocalStore {
    pub fn new(path: &str) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(MINTS_TABLE)?;
            let _ = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;
            let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
            let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
            let _ = write_txn.open_table(MINT_KEYS_TABLE)?;
            let _ = write_txn.open_multimap_table(PROOFS_TABLE)?;
            #[cfg(feature = "nut13")]
            let _ = write_txn.open_table(KEYSET_COUNTER)?;
        }
        write_txn.commit()?;

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait(?Send)]
impl LocalStore for RedbLocalStore {
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINTS_TABLE)?;
            table.insert(
                mint_url.to_string().as_str(),
                serde_json::to_string(&mint_info)?.as_str(),
            )?;
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINTS_TABLE)?;

        if let Some(mint_info) = table.get(mint_url.to_string().as_str())? {
            return Ok(serde_json::from_str(mint_info.value())?);
        }

        Ok(None)
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINTS_TABLE)?;

        let mints = table
            .iter()?
            .flatten()
            .map(|(mint, mint_info)| {
                (
                    UncheckedUrl::from_str(mint.value()).unwrap(),
                    serde_json::from_str(mint_info.value()).ok(),
                )
            })
            .collect();

        Ok(mints)
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;

            for keyset in keysets {
                table.insert(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&keyset)?.as_str(),
                )?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;

        let keysets = table
            .get(mint_url.to_string().as_str())?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINT_QUOTES_TABLE)?;

        if let Some(mint_info) = table.get(quote_id)? {
            return Ok(serde_json::from_str(mint_info.value())?);
        }

        Ok(None)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.insert(quote.id.as_str(), serde_json::to_string(&quote)?.as_str())?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MELT_QUOTES_TABLE)?;

        if let Some(mint_info) = table.get(quote_id)? {
            return Ok(serde_json::from_str(mint_info.value())?);
        }

        Ok(None)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MELT_QUOTES_TABLE)?;
            table.remove(quote_id)?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE)?;
            table.insert(
                Id::from(&keys).to_string().as_str(),
                serde_json::to_string(&keys)?.as_str(),
            )?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MINT_KEYS_TABLE)?;

        if let Some(mint_info) = table.get(id.to_string().as_str())? {
            return Ok(serde_json::from_str(mint_info.value())?);
        }

        Ok(None)
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Error> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE)?;

            table.remove(id.to_string().as_str())?;
        }

        write_txn.commit()?;

        Ok(())
    }

    async fn add_proofs(&self, mint_url: UncheckedUrl, proofs: Proofs) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_multimap_table(PROOFS_TABLE)?;

            for proof in proofs {
                table.insert(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&proof)?.as_str(),
                )?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_multimap_table(PROOFS_TABLE)?;

        let proofs = table
            .get(mint_url.to_string().as_str())?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(proofs)
    }

    async fn remove_proofs(&self, mint_url: UncheckedUrl, proofs: &Proofs) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_multimap_table(PROOFS_TABLE)?;

            for proof in proofs {
                table.remove(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&proof)?.as_str(),
                )?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_multimap_table(PENDING_PROOFS_TABLE)?;

            for proof in proofs {
                table.insert(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&proof)?.as_str(),
                )?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    async fn get_pending_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_multimap_table(PENDING_PROOFS_TABLE)?;

        let proofs = table
            .get(mint_url.to_string().as_str())?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(proofs)
    }

    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Error> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_multimap_table(PENDING_PROOFS_TABLE)?;

            for proof in proofs {
                table.remove(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&proof)?.as_str(),
                )?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    #[cfg(feature = "nut13")]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Error> {
        let db = self.db.lock().await;

        let current_counter;
        {
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(KEYSET_COUNTER)?;
            let counter = table.get(keyset_id.to_string().as_str())?;
            current_counter = if let Some(counter) = counter {
                counter.value()
            } else {
                0
            };
        }

        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(KEYSET_COUNTER)?;
            let new_counter = current_counter + count;

            table.insert(keyset_id.to_string().as_str(), new_counter)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    #[cfg(feature = "nut13")]
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Error> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(KEYSET_COUNTER)?;

        let counter = table.get(keyset_id.to_string().as_str())?;

        Ok(counter.map(|c| c.value()))
    }
}
