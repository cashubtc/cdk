//! Redb Wallet

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::cdk_database;
use cdk::cdk_database::WalletDatabase;
use cdk::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proofs, PublicKey, SpendingConditions, State,
};
use cdk::types::{MeltQuote, MintQuote, ProofInfo};
use cdk::url::UncheckedUrl;
use cdk::util::unix_time;
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::instrument;

use super::error::Error;
use crate::migrations::migrate_00_to_01;

const MINTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mints_table");
// <Mint_Url, Keyset_id>
const MINT_KEYSETS_TABLE: MultimapTableDefinition<&str, &[u8]> =
    MultimapTableDefinition::new("mint_keysets");
// <Keyset_id, KeysetInfo>
const KEYSETS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("keysets");
// <Quote_id, quote>
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
// <Quote_id, quote>
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const MINT_KEYS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_keys");
// <Y, Proof Info>
const PROOFS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const KEYSET_COUNTER: TableDefinition<&str, u32> = TableDefinition::new("keyset_counter");
const NOSTR_LAST_CHECKED: TableDefinition<&str, u32> = TableDefinition::new("keyset_counter");

const DATABASE_VERSION: u32 = 1;

/// Wallet Redb Database
#[derive(Debug, Clone)]
pub struct WalletRedbDatabase {
    db: Arc<Mutex<Database>>,
}

impl WalletRedbDatabase {
    /// Create new [`WalletRedbDatabase`]
    pub fn new(path: &Path) -> Result<Self, Error> {
        {
            let db = Arc::new(Database::create(path)?);

            let db_version: Option<String>;
            {
                // Check database version
                let read_txn = db.begin_read()?;
                let table = read_txn.open_table(CONFIG_TABLE);

                db_version = match table {
                    Ok(table) => table.get("db_version")?.map(|v| v.value().to_string()),
                    Err(_) => None,
                };
            }

            match db_version {
                Some(db_version) => {
                    let mut current_file_version = u32::from_str(&db_version)?;
                    tracing::info!("Current file version {}", current_file_version);

                    match current_file_version.cmp(&DATABASE_VERSION) {
                        Ordering::Less => {
                            tracing::info!(
                                "Database needs to be upgraded at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            if current_file_version == 0 {
                                current_file_version = migrate_00_to_01(Arc::clone(&db))?;
                            }

                            if current_file_version != DATABASE_VERSION {
                                tracing::warn!(
                                    "Database upgrade did not complete at {} current is {}",
                                    current_file_version,
                                    DATABASE_VERSION
                                );
                                return Err(Error::UnknownDatabaseVersion);
                            }
                        }
                        Ordering::Equal => {
                            tracing::info!("Database is at current version {}", DATABASE_VERSION);
                        }
                        Ordering::Greater => {
                            tracing::warn!(
                                "Database upgrade did not complete at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            return Err(Error::UnknownDatabaseVersion);
                        }
                    }
                }
                None => {
                    let write_txn = db.begin_write()?;
                    {
                        let mut table = write_txn.open_table(CONFIG_TABLE)?;
                        // Open all tables to init a new db
                        let _ = write_txn.open_table(MINTS_TABLE)?;
                        let _ = write_txn.open_multimap_table(MINT_KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MINT_KEYS_TABLE)?;
                        let _ = write_txn.open_table(PROOFS_TABLE)?;
                        let _ = write_txn.open_table(KEYSET_COUNTER)?;
                        let _ = write_txn.open_table(NOSTR_LAST_CHECKED)?;
                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        let db = Database::create(path)?;

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait]
impl WalletDatabase for WalletRedbDatabase {
    type Err = cdk_database::Error;

    #[instrument(skip(self))]
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

    #[instrument(skip(self))]
    async fn remove_mint(&self, mint_url: UncheckedUrl) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
            table
                .remove(mint_url.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
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

    #[instrument(skip(self))]
    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
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

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: UncheckedUrl,
        new_mint_url: UncheckedUrl,
    ) -> Result<(), Self::Err> {
        // Update proofs table
        {
            let proofs = self
                .get_proofs(Some(old_mint_url.clone()), None, None, None)
                .await
                .map_err(Error::from)?;

            if let Some(proofs) = proofs {
                // Proofs with new url
                let updated_proofs: Vec<ProofInfo> = proofs
                    .clone()
                    .into_iter()
                    .map(|mut p| {
                        p.mint_url = new_mint_url.clone();
                        p
                    })
                    .collect();

                println!("{:?}", updated_proofs);

                self.add_proofs(updated_proofs).await?;
            }
        }

        // Update mint quotes
        {
            let quotes = self.get_mint_quotes().await?;

            let unix_time = unix_time();

            let quotes: Vec<MintQuote> = quotes
                .into_iter()
                .filter_map(|mut q| {
                    if q.expiry < unix_time {
                        q.mint_url = new_mint_url.clone();
                        Some(q)
                    } else {
                        None
                    }
                })
                .collect();

            for quote in quotes {
                self.add_mint_quote(quote).await?;
            }
        }

        Ok(())
    }

    #[instrument(skip(self))]
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
                        keyset.id.to_bytes().as_slice(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let keyset_ids: Vec<Id> = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .flat_map(|k| Id::from_bytes(k.value()))
            .collect();

        let mut keysets = vec![];

        let keysets_t = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        for keyset_id in keyset_ids {
            if let Some(keyset) = keysets_t
                .get(keyset_id.to_bytes().as_slice())
                .map_err(Error::from)?
            {
                let keyset = serde_json::from_str(keyset.value()).map_err(Error::from)?;

                keysets.push(keyset);
            }
        }

        match keysets.is_empty() {
            true => Ok(None),
            false => Ok(Some(keysets)),
        }
    }

    #[instrument(skip(self))]
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        match table
            .get(keyset_id.to_bytes().as_slice())
            .map_err(Error::from)?
        {
            Some(keyset) => {
                let keyset: KeySetInfo =
                    serde_json::from_str(keyset.value()).map_err(Error::from)?;

                Ok(Some(keyset))
            }
            None => Ok(None),
        }
    }

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(_id, quote)| serde_json::from_str(quote.value()))
            .collect())
    }

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
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

    #[instrument(skip_all)]
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

    #[instrument(skip(self))]
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table.get(id.to_string().as_str()).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip(self))]
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

    #[instrument(skip(self, proofs_info))]
    async fn add_proofs(&self, proofs_info: Vec<ProofInfo>) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for proof_info in proofs_info.iter() {
                table
                    .insert(
                        proof_info.y.to_bytes().as_slice(),
                        serde_json::to_string(&proof_info)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;

        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let proofs: Vec<ProofInfo> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_k, v)| {
                let mut proof = None;

                if let Ok(proof_info) = serde_json::from_str::<ProofInfo>(v.value()) {
                    match proof_info.matches_conditions(
                        &mint_url,
                        &unit,
                        &state,
                        &spending_conditions,
                    ) {
                        true => proof = Some(proof_info),
                        false => (),
                    }
                }

                proof
            })
            .collect();

        if proofs.is_empty() {
            return Ok(None);
        }

        Ok(Some(proofs))
    }

    #[instrument(skip(self, proofs))]
    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for proof in proofs {
                let y_slice = proof.y().map_err(Error::from)?.to_bytes();
                table.remove(y_slice.as_slice()).map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let y_slice = y.to_bytes();
        let proof = table
            .get(y_slice.as_slice())
            .map_err(Error::from)?
            .ok_or(Error::UnknownY)?;

        let write_txn = db.begin_write().map_err(Error::from)?;

        let mut proof_info =
            serde_json::from_str::<ProofInfo>(proof.value()).map_err(Error::from)?;

        proof_info.state = state;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    y_slice.as_slice(),
                    serde_json::to_string(&proof_info)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
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

    #[instrument(skip(self))]
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;

        let counter = table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?;

        Ok(counter.map(|c| c.value()))
    }

    #[instrument(skip(self))]
    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(NOSTR_LAST_CHECKED)
            .map_err(Error::from)?;

        let last_checked = table
            .get(verifying_key.to_string().as_str())
            .map_err(Error::from)?;

        Ok(last_checked.map(|c| c.value()))
    }
    #[instrument(skip(self))]
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(NOSTR_LAST_CHECKED)
                .map_err(Error::from)?;

            table
                .insert(verifying_key.to_string().as_str(), last_checked)
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
}
