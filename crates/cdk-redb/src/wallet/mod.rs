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
use crate::wallet::migrations::{migrate_01_to_02, migrate_02_to_03, migrate_03_to_04};

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

const KEYSET_U32_MAPPING: TableDefinition<u32, &str> = TableDefinition::new("keyset_u32_mapping");

const DATABASE_VERSION: u32 = 4;

/// Wallet Redb Database
#[derive(Debug, Clone)]
pub struct WalletRedbDatabase {
    db: Arc<Database>,
}

/// Redb Wallet Transaction
pub struct RedbWalletTransaction {
    write_txn: Option<redb::WriteTransaction>,
}

impl RedbWalletTransaction {
    /// Create a new transaction
    fn new(write_txn: redb::WriteTransaction) -> Self {
        Self {
            write_txn: Some(write_txn),
        }
    }

    /// Get a mutable reference to the write transaction
    fn txn(&mut self) -> Result<&mut redb::WriteTransaction, Error> {
        self.write_txn.as_mut().ok_or_else(|| {
            Error::CDKDatabase(database::Error::Internal(
                "Transaction already consumed".to_owned(),
            ))
        })
    }
}

impl WalletRedbDatabase {
    /// Create new [`WalletRedbDatabase`]
    pub fn new(path: &Path) -> Result<Self, Error> {
        {
            // Check if parent directory exists before attempting to create database
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Parent directory does not exist: {parent:?}"),
                    )));
                }
            }

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

                            if current_file_version == 2 {
                                current_file_version = migrate_02_to_03(Arc::clone(&db))?;
                            }

                            if current_file_version == 3 {
                                current_file_version = migrate_03_to_04(Arc::clone(&db))?;
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
                        let _ = write_txn.open_table(KEYSET_U32_MAPPING)?;
                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        // Check parent directory again for final database creation
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Parent directory does not exist: {parent:?}"),
                )));
            }
        }

        let mut db = Database::create(path)?;

        db.upgrade()?;

        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl WalletDatabase for WalletRedbDatabase {
    type Err = database::Error;

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

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, database::Error> {
        // For redb, we still need to fetch all proofs and sum them
        // since redb doesn't have SQL aggregation
        let proofs = self.get_proofs(mint_url, unit, state, None).await?;
        Ok(proofs.iter().map(|p| u64::from(p.proof.amount)).sum())
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

    async fn begin_db_transaction<'a>(
        &'a self,
    ) -> Result<
        Box<dyn cdk_common::database::WalletDatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>,
        Self::Err,
    > {
        let write_txn = self.db.begin_write().map_err(Error::from)?;
        Ok(Box::new(RedbWalletTransaction::new(write_txn)))
    }
}

#[async_trait]
impl<'a> cdk_common::database::WalletDatabaseTransaction<'a, database::Error>
    for RedbWalletTransaction
{
    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(
        &mut self,
        keyset_id: &Id,
    ) -> Result<Option<KeySetInfo>, database::Error> {
        let txn = self.txn().map_err(Into::<database::Error>::into)?;
        let table = txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        let result = match table
            .get(keyset_id.to_bytes().as_slice())
            .map_err(Error::from)?
        {
            Some(keyset) => {
                let keyset: KeySetInfo =
                    serde_json::from_str(keyset.value()).map_err(Error::from)?;

                Ok(Some(keyset))
            }
            None => Ok(None),
        };

        result
    }

    #[instrument(skip(self))]
    async fn add_mint(
        &mut self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MINTS_TABLE).map_err(Error::from)?;
        table
            .insert(
                mint_url.to_string().as_str(),
                serde_json::to_string(&mint_info)
                    .map_err(Error::from)?
                    .as_str(),
            )
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&mut self, mint_url: MintUrl) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MINTS_TABLE).map_err(Error::from)?;
        table
            .remove(mint_url.to_string().as_str())
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &mut self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), database::Error> {
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
            let read_txn = self.txn()?;
            let mut table = read_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            let unix_time = unix_time();

            let quotes = table
                .iter()
                .map_err(Error::from)?
                .flatten()
                .filter_map(|(_, quote)| {
                    let mut q: MintQuote = serde_json::from_str(quote.value())
                        .inspect_err(|err| {
                            tracing::warn!(
                                "Failed to deserialize {}  with error {}",
                                quote.value(),
                                err
                            )
                        })
                        .ok()?;
                    if q.expiry < unix_time {
                        q.mint_url = new_mint_url.clone();
                        Some(q)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            for quote in quotes {
                table
                    .insert(
                        quote.id.as_str(),
                        serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_mint_keysets(
        &mut self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn
            .open_multimap_table(MINT_KEYSETS_TABLE)
            .map_err(Error::from)?;
        let mut keysets_table = txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;
        let mut u32_table = txn.open_table(KEYSET_U32_MAPPING).map_err(Error::from)?;

        let mut existing_u32 = false;

        for keyset in keysets {
            // Check if keyset already exists
            let existing_keyset = {
                let existing_keyset = keysets_table
                    .get(keyset.id.to_bytes().as_slice())
                    .map_err(Error::from)?;

                existing_keyset.map(|r| r.value().to_string())
            };

            let existing = u32_table
                .insert(u32::from(keyset.id), keyset.id.to_string().as_str())
                .map_err(Error::from)?;

            match existing {
                None => existing_u32 = false,
                Some(id) => {
                    let id = Id::from_str(id.value())?;

                    if id == keyset.id {
                        existing_u32 = false;
                    } else {
                        existing_u32 = true;
                        break;
                    }
                }
            }

            let keyset = if let Some(existing_keyset) = existing_keyset {
                let mut existing_keyset: KeySetInfo = serde_json::from_str(&existing_keyset)?;

                existing_keyset.active = keyset.active;
                existing_keyset.input_fee_ppk = keyset.input_fee_ppk;

                existing_keyset
            } else {
                table
                    .insert(
                        mint_url.to_string().as_str(),
                        keyset.id.to_bytes().as_slice(),
                    )
                    .map_err(Error::from)?;

                keyset
            };

            keysets_table
                .insert(
                    keyset.id.to_bytes().as_slice(),
                    serde_json::to_string(&keyset)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        if existing_u32 {
            tracing::warn!("Keyset already exists for keyset id");
            return Err(database::Error::Duplicate);
        }

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_mint_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<MintQuote>, database::Error> {
        let txn = self.txn()?;
        let table = txn.open_table(MINT_QUOTES_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MINT_QUOTES_TABLE).map_err(Error::from)?;
        table
            .insert(
                quote.id.as_str(),
                serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
            )
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn remove_mint_quote(&mut self, quote_id: &str) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MINT_QUOTES_TABLE).map_err(Error::from)?;
        table.remove(quote_id).map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_melt_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, database::Error> {
        let txn = self.txn()?;
        let table = txn.open_table(MELT_QUOTES_TABLE).map_err(Error::from)?;

        if let Some(mint_info) = table.get(quote_id).map_err(Error::from)? {
            return Ok(serde_json::from_str(mint_info.value()).map_err(Error::from)?);
        }

        Ok(None)
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&mut self, quote: wallet::MeltQuote) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MELT_QUOTES_TABLE).map_err(Error::from)?;
        table
            .insert(
                quote.id.as_str(),
                serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
            )
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn remove_melt_quote(&mut self, quote_id: &str) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MELT_QUOTES_TABLE).map_err(Error::from)?;
        table.remove(quote_id).map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&mut self, keyset: KeySet) -> Result<(), database::Error> {
        let txn = self.txn()?;

        keyset.verify_id()?;

        let mut table = txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        let existing_keys = table
            .insert(
                keyset.id.to_string().as_str(),
                serde_json::to_string(&keyset.keys)
                    .map_err(Error::from)?
                    .as_str(),
            )
            .map_err(Error::from)?
            .is_some();

        let mut table = txn.open_table(KEYSET_U32_MAPPING).map_err(Error::from)?;

        let existing = table
            .insert(u32::from(keyset.id), keyset.id.to_string().as_str())
            .map_err(Error::from)?;

        let existing_u32 = match existing {
            None => false,
            Some(id) => {
                let id = Id::from_str(id.value())?;
                id != keyset.id
            }
        };

        if existing_keys || existing_u32 {
            tracing::warn!("Keys already exist for keyset id");
            return Err(database::Error::Duplicate);
        }

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn remove_keys(&mut self, keyset_id: &Id) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(MINT_KEYS_TABLE).map_err(Error::from)?;

        table
            .remove(keyset_id.to_string().as_str())
            .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_proofs(
        &mut self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        let txn = self.txn()?;
        let table = txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

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

    #[instrument(skip(self, added, deleted_ys))]
    async fn update_proofs(
        &mut self,
        added: Vec<ProofInfo>,
        deleted_ys: Vec<PublicKey>,
    ) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

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

        Ok(())
    }

    async fn update_proofs_state(
        &mut self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(PROOFS_TABLE).map_err(Error::from)?;

        for y in ys {
            let y_slice = y.to_bytes();
            let proof = table
                .get(y_slice.as_slice())
                .map_err(Error::from)?
                .ok_or(Error::UnknownY)?;

            let mut proof_info =
                serde_json::from_str::<ProofInfo>(proof.value()).map_err(Error::from)?;
            drop(proof);

            proof_info.state = state;

            table
                .insert(
                    y_slice.as_slice(),
                    serde_json::to_string(&proof_info)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(
        &mut self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(KEYSET_COUNTER).map_err(Error::from)?;
        let current_counter = table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?
            .map(|x| x.value())
            .unwrap_or_default();

        let new_counter = current_counter + count;

        table
            .insert(keyset_id.to_string().as_str(), new_counter)
            .map_err(Error::from)?;

        Ok(new_counter)
    }

    #[instrument(skip(self))]
    async fn add_transaction(&mut self, transaction: Transaction) -> Result<(), database::Error> {
        let id = transaction.id();
        let txn = self.txn()?;
        let mut table = txn.open_table(TRANSACTIONS_TABLE).map_err(Error::from)?;
        table
            .insert(
                id.as_slice(),
                serde_json::to_string(&transaction)
                    .map_err(Error::from)?
                    .as_str(),
            )
            .map_err(Error::from)?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_transaction(
        &mut self,
        transaction_id: TransactionId,
    ) -> Result<(), database::Error> {
        let txn = self.txn()?;
        let mut table = txn.open_table(TRANSACTIONS_TABLE).map_err(Error::from)?;
        table
            .remove(transaction_id.as_slice())
            .map_err(Error::from)?;
        Ok(())
    }
}

#[async_trait]
impl cdk_common::database::DbTransactionFinalizer for RedbWalletTransaction {
    type Err = database::Error;

    async fn commit(mut self: Box<Self>) -> Result<(), database::Error> {
        if let Some(txn) = self.write_txn.take() {
            txn.commit().map_err(Error::from)?;
        }
        Ok(())
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), database::Error> {
        if let Some(txn) = self.write_txn.take() {
            txn.abort().map_err(Error::from)?;
        }
        Ok(())
    }
}

impl Drop for RedbWalletTransaction {
    fn drop(&mut self) {
        if let Some(txn) = self.write_txn.take() {
            let _ = txn.abort();
        }
    }
}
