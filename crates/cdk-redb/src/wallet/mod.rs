//! Redb Wallet

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::ProofInfo;
use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_common::util::unix_time;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_common::{
    database, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions,
    State,
};
use redb::{Database, MultimapTableDefinition, ReadableTable, TableDefinition};
use tracing::instrument;

use super::error::Error;
use crate::migrations::migrate_00_to_01;
use crate::wallet::migrations::migrate_01_to_02;

mod migrations;

// <Mint_url, Info>
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
// <Transaction_id, Transaction>
const TRANSACTIONS_TABLE: TableDefinition<&[u8], &str> = TableDefinition::new("transactions");

const DATABASE_VERSION: u32 = 2;

/// Wallet Redb Database
#[derive(Debug, Clone)]
pub struct WalletRedbDatabase {
    db: Arc<Database>,
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

                            if current_file_version == 1 {
                                current_file_version = migrate_01_to_02(Arc::clone(&db))?;
                            }

                            if current_file_version != DATABASE_VERSION {
                                tracing::warn!(
                                    "Database upgrade did not complete at {} current is {}",
                                    current_file_version,
                                    DATABASE_VERSION
                                );
                                return Err(Error::UnknownDatabaseVersion);
                            }

                            let write_txn = db.begin_write()?;
                            {
                                let mut table = write_txn.open_table(CONFIG_TABLE)?;

                                table
                                    .insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                            }

                            write_txn.commit()?;
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
                        let _ = write_txn.open_table(TRANSACTIONS_TABLE)?;
                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        let db = Database::create(path)?;

        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl WalletDatabase for WalletRedbDatabase {
    type Err = database::Error;

    #[instrument(skip(self))]
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
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
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINTS_TABLE).map_err(Error::from)?;
        let mints = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .map(|(mint, mint_info)| {
                (
                    MintUrl::from_str(mint.value()).unwrap(),
                    serde_json::from_str(mint_info.value()).ok(),
                )
            })
            .collect();

        Ok(mints)
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err> {
        // Update proofs table
        {
            let proofs = self
                .get_proofs(Some(old_mint_url.clone()), None, None, None)
                .await
                .map_err(Error::from)?;

            // Proofs with new url
            let updated_proofs: Vec<ProofInfo> = proofs
                .clone()
                .into_iter()
                .map(|mut p| {
                    p.mint_url = new_mint_url.clone();
                    p
                })
                .collect();

            if !updated_proofs.is_empty() {
                self.update_proofs(updated_proofs, vec![]).await?;
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
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_multimap_table(MINT_KEYSETS_TABLE)
                .map_err(Error::from)?;
            let mut keysets_table = write_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

            for keyset in keysets {
                table
                    .insert(
                        mint_url.to_string().as_str(),
                        keyset.id.to_bytes().as_slice(),
                    )
                    .map_err(Error::from)?;

                keysets_table
                    .insert(
                        keyset.id.to_bytes().as_slice(),
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

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
        let table = read_txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let keyset_ids = table
            .get(mint_url.to_string().as_str())
            .map_err(Error::from)?
            .flatten()
            .map(|k| Id::from_bytes(k.value()))
            .collect::<Result<Vec<_>, _>>()?;

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

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
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
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
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
        let read_txn = self.db.begin_read().map_err(Into::<Error>::into)?;
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
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        Ok(table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .flat_map(|(_id, quote)| serde_json::from_str(quote.value()))
            .collect())
    }

    #[instrument(skip_all)]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

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
    async fn add_keys(&self, keyset: KeySet) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        keyset.verify_id()?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    keyset.id.to_string().as_str(),
                    serde_json::to_string(&keyset.keys)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?
        {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn remove_keys(&self, keyset_id: &Id) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

            table
                .remove(keyset_id.to_string().as_str())
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self, added, deleted_ys))]
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        deleted_ys: Vec<PublicKey>,
    ) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

            for proof_info in added.iter() {
                table
                    .insert(
                        proof_info.y.to_bytes().as_slice(),
                        serde_json::to_string(&proof_info)
                            .map_err(Error::from)?
                            .as_str(),
                    )
                    .map_err(Error::from)?;
            }

            for y in deleted_ys.iter() {
                table.remove(y.to_bytes().as_slice()).map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;

        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let proofs: Vec<ProofInfo> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_k, v)| {
                let mut proof = None;

                if let Ok(proof_info) = serde_json::from_str::<ProofInfo>(v.value()) {
                    if proof_info.matches_conditions(&mint_url, &unit, &state, &spending_conditions)
                    {
                        proof = Some(proof_info)
                    }
                }

                proof
            })
            .collect();

        Ok(proofs)
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), database::Error> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        let write_txn = self.db.begin_write().map_err(Error::from)?;

        for y in ys {
            let y_slice = y.to_bytes();
            let proof = table
                .get(y_slice.as_slice())
                .map_err(Error::from)?
                .ok_or(Error::UnknownY)?;

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
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<u32, Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        let current_counter;
        {
            let table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
            let counter = table
                .get(keyset_id.to_string().as_str())
                .map_err(Error::from)?;

            current_counter = match counter {
                Some(c) => c.value(),
                None => 0,
            };
        }

        let new_counter = current_counter + count;
        {
            let mut table = write_txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
            table
                .insert(keyset_id.to_string().as_str(), new_counter)
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(new_counter)
    }

    #[instrument(skip(self))]
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    transaction.id().as_slice(),
                    serde_json::to_string(&transaction)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;

        if let Some(transaction) = table.get(transaction_id.as_slice()).map_err(Error::from)? {
            return Ok(serde_json::from_str(transaction.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Self::Err> {
        let read_txn = self.db.begin_read().map_err(Error::from)?;

        let table = read_txn
            .open_table(TRANSACTIONS_TABLE)
            .map_err(Error::from)?;

        let transactions: Vec<Transaction> = table
            .iter()
            .map_err(Error::from)?
            .flatten()
            .filter_map(|(_k, v)| {
                let mut transaction = None;

                if let Ok(tx) = serde_json::from_str::<Transaction>(v.value()) {
                    if tx.matches_conditions(&mint_url, &direction, &unit) {
                        transaction = Some(tx)
                    }
                }

                transaction
            })
            .collect();

        Ok(transactions)
    }

    #[instrument(skip(self))]
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Self::Err> {
        let write_txn = self.db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(TRANSACTIONS_TABLE)
                .map_err(Error::from)?;
            table
                .remove(transaction_id.as_slice())
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }
}
