use std::collections::HashMap;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use cdk::types::{MeltQuote, MintQuote};
use cdk::url::UncheckedUrl;
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Redb(#[from] redb::Error),
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    #[error(transparent)]
    Table(#[from] redb::TableError),
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    #[error(transparent)]
    CDKDatabase(#[from] cdk_database::Error),
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("mint_keysets");
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const MINT_KEYS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_keys");
const PROOFS_TABLE: MultimapTableDefinition<&str, &str> = MultimapTableDefinition::new("proofs");
const PENDING_PROOFS_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("pending_proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const KEYSET_COUNTER: TableDefinition<&str, u64> = TableDefinition::new("keyset_counter");

const DATABASE_VERSION: u64 = 0;

#[derive(Debug, Clone)]
pub struct RedbWalletDatabase {
    db: Arc<Mutex<Database>>,
}

impl RedbWalletDatabase {
    pub fn new(path: &str) -> Result<Self, Error> {
        let db = Database::create(path)?;

        let write_txn = db.begin_write()?;

        // Check database version
        {
            let _ = write_txn.open_table(CONFIG_TABLE)?;
            let mut table = write_txn.open_table(CONFIG_TABLE)?;

            let db_version = table.get("db_version")?.map(|v| v.value().to_owned());

            match db_version {
                Some(db_version) => {
                    let current_file_version = u64::from_str(&db_version)?;
                    if current_file_version.ne(&DATABASE_VERSION) {
                        // Database needs to be upgraded
                        todo!()
                    }
                    let _ = write_txn.open_table(KEYSET_COUNTER)?;
                }
                None => {
                    // Open all tables to init a new db
                    let _ = write_txn.open_table(MINTS_TABLE)?;
                    let _ = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;
                    let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                    let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                    let _ = write_txn.open_table(MINT_KEYS_TABLE)?;
                    let _ = write_txn.open_multimap_table(PROOFS_TABLE)?;
                    let _ = write_txn.open_table(KEYSET_COUNTER)?;
                    table.insert("db_version", "0")?;
                }
            }
        }
        write_txn.commit()?;

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait]
impl WalletDatabase for RedbWalletDatabase {
    type Err = cdk_database::Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    mint_url.to_string().as_str(),
                    serde_json::to_string(&mint_info)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
        {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(Into::<Error>::into)
            .map_err(Error::from)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;

        let mints = table
            .iter()
            .map_err(Error::from)?
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
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(MINT_KEYSETS_TABLE)
                .map_err(Error::from)?;

            for keyset in keysets {
                table
                    .insert(
                        mint_url.to_string().as_str(),
                        serde_json::to_string(&keyset)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let keysets = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    Id::from(&keys).to_string().as_str(),
                    serde_json::to_string(&keys).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table.get(id.to_string().as_str()).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

            table.remove(id.to_string().as_str()).map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_proofs(&self, mint_url: UncheckedUrl, proofs: Proofs) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(PROOFS_TABLE)
                .map_err(Error::from)?;

            for proof in proofs {
                table
                    .insert(
                        mint_url.to_string().as_str(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_multimap_table(PROOFS_TABLE)
            .map_err(Error::from)?;

        let proofs = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(proofs)
    }

    async fn remove_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(PROOFS_TABLE)
                .map_err(Error::from)?;

            for proof in proofs {
                table
                    .remove(
                        mint_url.to_string().as_str(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;

            for proof in proofs {
                table
                    .insert(
                        mint_url.to_string().as_str(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Proofs>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_multimap_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        let proofs = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .flat_map(|k| serde_json::from_str(k.value()))
            .collect();

        Ok(proofs)
    }

    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;

            for proof in proofs {
                table
                    .remove(
                        mint_url.to_string().as_str(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let current_counter;
        {
            let read_txn = db.begin_read().map_err(Error::from)?;
            let table = read_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
            let counter = table
                .get(keyset_id.to_string().as_str())
                .map_err(Error::from)?;

            current_counter = match counter {
                Some(c) => c.value(),
                None => 0,
            };
        }

        let write_txn = db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
            let new_counter = current_counter + count;

            table
                .insert(keyset_id.to_string().as_str(), new_counter)
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;

        let counter = table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?;

        Ok(counter.map(|c| c.value()))
    }
}
